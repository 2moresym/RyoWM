//! Input engine: libinput on a dedicated thread, normalized events over a
//! lock-free SPSC channel to the main thread.
//!
//! Phase 3 exit criteria (architecture doc §14):
//! 1. Moving a real mouse moves a visible cursor on screen.
//! 2. Clicking a mapped window focuses it (focus change is logged).
//! 3. Typing while a window is focused delivers key events to that client.
//! 4. Unplugging/disabling one input device mid-session neither crashes the
//!    compositor nor affects other devices.
//! 5. `cargo xtask check` still passes.
//! 6. Idle CPU stays near zero — the input thread parks on libinput's fd and
//!    the main thread only wakes on channel pings.
//!
//! | Subsystem | Responsibility | Process | Threading | Failure behavior |
//! |---|---|---|---|---|
//! | Input engine (libinput wrapper) | Raw device events → normalized input events | in-proc | dedicated input thread reading libinput's fd via epoll (thread-local calloop), forwards via lock-free SPSC channel to main | Device hotplug/error → device dropped + logged, others unaffected; full channel drops events + warns, never blocks |
//! | Keyboard/mouse engines | Interpret normalized input into focus, cursor, and client key delivery | in-proc | main thread (`dispatch_input_event`, driven by the channel's calloop source) | Unmatched/unknown input = logged no-op; routing failures never panic the loop |
//!
//! Deliberate deviations from architecture doc §8.3's `InputEvent` sketch,
//! recorded here rather than silently: no `mods` field (modifiers live in the
//! seat's xkb state main-side; copying them across threads could drift per
//! rule §15.12), no `Gesture` variant (touchpad gestures are Phase 9 —
//! those events are debug-logged on the input thread, not silently dropped),
//! and axis (scroll) events likewise stay thread-local logs until a scroll
//! consumer exists. Every variant carries the event timestamp so main-thread
//! handling uses event time, never handling time.

pub mod cursor;

use std::{
    os::unix::{fs::OpenOptionsExt, io::OwnedFd},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use calloop::EventLoop;
use input::{Libinput, LibinputInterface};
use rtrb::{Consumer, Producer, RingBuffer};
use smithay::{
    backend::{
        input::{
            ButtonState, Event as BackendEvent, InputEvent as BackendInputEvent,
            KeyState as BackendKeyState, Keycode, KeyboardKeyEvent, PointerButtonEvent,
            PointerMotionEvent,
        },
        libinput::LibinputInputBackend,
    },
    input::{
        keyboard::FilterResult,
        pointer::{ButtonEvent, MotionEvent},
    },
    reexports::wayland_server::Resource,
    utils::{Logical, Point, SERIAL_COUNTER},
};
use tracing::{debug, info, warn};

use crate::wayland::RyoWmState;

/// Normalized input event crossing the thread boundary. Plain data only —
/// no Smithay/libinput types, so the channel stays `Send` by construction.
#[derive(Debug, Clone, Copy)]
pub enum InputEvent {
    Key {
        code: Keycode,
        pressed: bool,
        time_ms: u32,
    },
    PointerMotion {
        dx: f64,
        dy: f64,
        time_ms: u32,
    },
    PointerButton {
        button: u32,
        pressed: bool,
        time_ms: u32,
    },
}

/// Dropped-event counter for channel-full backpressure evidence.
static DROPPED_EVENTS: AtomicU64 = AtomicU64::new(0);

/// libinput device open policy: direct open, read/write. The compositor user
/// sits in the `input` group, so no seat-master dance is needed in dev.
/// TTY seat integration (libseat) arrives with the udev backend work.
struct SessionInterface;

impl LibinputInterface for SessionInterface {
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<OwnedFd, i32> {
        // Access-mode low two bits select read/write intent (open(2)).
        const O_ACCMODE: i32 = 0o3;
        const EACCES: i32 = 13;
        let access = flags & O_ACCMODE;
        std::fs::OpenOptions::new()
            .read(access == 0 || access == 2)
            .write(access == 1 || access == 2)
            .custom_flags(flags & !O_ACCMODE)
            .open(path)
            .map(OwnedFd::from)
            .map_err(|err| err.raw_os_error().unwrap_or(EACCES))
    }

    fn close_restricted(&mut self, fd: OwnedFd) {
        drop(fd);
    }
}

/// libinput timestamps are microseconds; Smithay seat APIs take milliseconds.
fn millis(microseconds: u64) -> u32 {
    (microseconds / 1000) as u32
}

/// Translate one Smithay-normalized backend event into our plain-data enum.
/// Returns `None` for event classes with no Phase 3 consumer (axis, touch,
/// gestures) — logged here, never silently swallowed.
fn translate(event: BackendInputEvent<LibinputInputBackend>) -> Option<InputEvent> {
    match event {
        BackendInputEvent::Keyboard { event } => Some(InputEvent::Key {
            code: event.key_code(),
            pressed: event.state() == BackendKeyState::Pressed,
            time_ms: millis(event.time()),
        }),
        BackendInputEvent::PointerMotion { event } => Some(InputEvent::PointerMotion {
            dx: event.delta_x(),
            dy: event.delta_y(),
            time_ms: millis(event.time()),
        }),
        BackendInputEvent::PointerButton { event } => Some(InputEvent::PointerButton {
            button: event.button_code(),
            pressed: event.state() == ButtonState::Pressed,
            time_ms: millis(event.time()),
        }),
        BackendInputEvent::DeviceAdded { device } => {
            info!(device = device.sysname(), "Input device added");
            None
        }
        BackendInputEvent::DeviceRemoved { device } => {
            // Exit 4 evidence: removal is a log line, the loop below keeps
            // serving every other device untouched.
            info!(device = device.sysname(), "Input device removed");
            None
        }
        _ => {
            debug!("Input event class deferred to a later phase (axis/touch/gesture/switch)");
            None
        }
    }
}

/// Channel halves handed to the main thread. The ping source is registered
/// on the reactor; the consumer is drained in its callback.
pub struct InputChannel {
    pub consumer: Consumer<InputEvent>,
    pub source: calloop::ping::PingSource,
}

/// Spawn the dedicated input thread. The libinput context is created inside
/// the thread: it holds an `Rc` and cannot cross the spawn boundary.
pub fn spawn_input_thread() -> anyhow::Result<InputChannel> {
    let (producer, consumer) = RingBuffer::new(1024);
    let (ping, source) = calloop::ping::make_ping()?;
    std::thread::Builder::new()
        .name("ryowm-input".into())
        .spawn(move || run_input_loop(producer, ping))?;
    Ok(InputChannel { consumer, source })
}

fn run_input_loop(producer: Producer<InputEvent>, ping: calloop::ping::Ping) {
    // Nested-dev enumeration: /dev/input nodes here carry no ID_SEAT tags,
    // so udev_assign_seat would find nothing (verified: zero DeviceAdded).
    // Enumerate device nodes directly instead. Production TTY sessions with
    // logind-tagged devices should prefer udev_assign_seat — that path is a
    // small, explicit follow-up, not silent scope creep.
    let mut context = Libinput::new_from_path(SessionInterface);
    let mut nodes: Vec<_> = std::fs::read_dir("/dev/input")
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with("event"))
                })
                .collect()
        })
        .unwrap_or_default();
    nodes.sort();
    let mut added = 0u32;
    for node in &nodes {
        match node.to_str() {
            Some(path) if context.path_add_device(path).is_some() => added += 1,
            _ => warn!(node = ?node, "libinput rejected device node"),
        }
    }
    info!(
        added,
        nodes = nodes.len(),
        "libinput path enumeration complete"
    );
    let backend = LibinputInputBackend::new(context);

    let mut event_loop = match EventLoop::try_new() {
        Ok(event_loop) => event_loop,
        Err(err) => {
            warn!("Input thread: event loop failed: {err:?}; input disabled");
            return;
        }
    };
    if event_loop
        .handle()
        .insert_source(
            backend,
            |event, _, slots: &mut (Producer<InputEvent>, calloop::ping::Ping)| {
                let (producer, ping) = slots;
                if let Some(normalized) = translate(event) {
                    if producer.push(normalized).is_err() {
                        let dropped = DROPPED_EVENTS.fetch_add(1, Ordering::Relaxed) + 1;
                        warn!("Input channel full; dropped {dropped} events so far");
                    }
                    ping.ping();
                }
            },
        )
        .is_err()
    {
        warn!("Input thread: failed to register libinput source; input disabled");
        return;
    }

    info!("Input thread running (libinput, seat seat0)");
    let mut slots = (producer, ping);
    loop {
        // Blocking wait on libinput's fd: near-zero CPU at idle (exit 6).
        // A dispatch error is logged and retried — one bad device or fd
        // hiccup must never take the compositor's input down (exit 4).
        if event_loop.dispatch(None, &mut slots).is_err() {
            warn!("Input thread: dispatch error; retrying");
        }
    }
}

/// Clamp a pointer position into the usable area. Pure helper, unit-tested.
fn clamp_position(pos: Point<f64, Logical>, extent: (f64, f64)) -> Point<f64, Logical> {
    Point::<f64, Logical>::from((
        pos.x.clamp(0.0, extent.0),
        pos.y.clamp(0.0, extent.1),
    ))
}

fn output_extent(state: &RyoWmState) -> (f64, f64) {
    state
        .space
        .outputs()
        .next()
        .and_then(|output| output.current_mode())
        .map(|mode| (mode.size.w as f64, mode.size.h as f64))
        .unwrap_or((f64::MAX, f64::MAX))
}

/// Main-thread interpretation of one normalized event: cursor motion, click
/// focus, and key delivery to the focused client.
pub fn dispatch_input_event(state: &mut RyoWmState, event: InputEvent) {
    match event {
        InputEvent::Key {
            code,
            pressed,
            time_ms,
        } => {
            let serial = SERIAL_COUNTER.next_serial();
            let key_state = if pressed {
                BackendKeyState::Pressed
            } else {
                BackendKeyState::Released
            };
            // Raw evdev code: Smithay applies the +8 XKB offset internally.
            // No keybind interception yet (Phase 8) — forward everything.
            if let Some(keyboard) = state.seat.get_keyboard() {
                keyboard.input(state, code, key_state, serial, time_ms, |_, _, _| {
                    FilterResult::<()>::Forward
                });
            }
        }
        InputEvent::PointerMotion { dx, dy, time_ms } => {
            let Some(pointer) = state.seat.get_pointer() else {
                return;
            };
            let serial = SERIAL_COUNTER.next_serial();
            let position = clamp_position(
                pointer.current_location() + Point::from((dx, dy)),
                output_extent(state),
            );
            let focus = state.space.element_under(position).and_then(|(window, _)| {
                window
                    .toplevel()
                    .map(|toplevel| (toplevel.wl_surface().clone(), position))
            });
            pointer.motion(
                state,
                focus,
                &MotionEvent {
                    location: position,
                    serial,
                    time: time_ms,
                },
            );
            // No damage marking here: the renderer detects cursor movement
            // itself in `present` and forces a submission only then. Marking
            // full-output damage per motion event serialized presents on swap
            // vsync and visibly lagged clients under a moving cursor.
        }
        InputEvent::PointerButton {
            button,
            pressed,
            time_ms,
        } => {
            let serial = SERIAL_COUNTER.next_serial();
            if pressed {
                // Focus-before-press: the click lands in the newly focused
                // window, matching every major compositor's behavior.
                let position = state
                    .seat
                    .get_pointer()
                    .map(|pointer| pointer.current_location())
                    .unwrap_or_default();
                let target = state.space.element_under(position).and_then(|(window, _)| {
                    window.toplevel().map(|toplevel| toplevel.wl_surface().clone())
                });
                if let Some(surface) = target {
                    if let Some(keyboard) = state.seat.get_keyboard() {
                        keyboard.set_focus(state, Some(surface.clone()), serial);
                    }
                    info!(
                        surface = surface.id().protocol_id(),
                        "Click focused window"
                    );
                }
            }
            if let Some(pointer) = state.seat.get_pointer() {
                pointer.button(
                    state,
                    &ButtonEvent {
                        button,
                        state: if pressed {
                            ButtonState::Pressed
                        } else {
                            ButtonState::Released
                        },
                        serial,
                        time: time_ms,
                    },
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_keeps_interior_positions() {
        let pos = Point::<f64, Logical>::from((100.0, 200.0));
        assert_eq!(clamp_position(pos, (800.0, 600.0)), pos);
    }

    #[test]
    fn clamp_pins_negative_to_zero() {
        let pos = Point::<f64, Logical>::from((-50.0, -1.0));
        assert_eq!(
            clamp_position(pos, (800.0, 600.0)),
            Point::<f64, Logical>::from((0.0, 0.0))
        );
    }

    #[test]
    fn clamp_pins_overflow_to_extent() {
        let pos = Point::<f64, Logical>::from((900.0, 700.0));
        assert_eq!(
            clamp_position(pos, (800.0, 600.0)),
            Point::<f64, Logical>::from((800.0, 600.0))
        );
    }
}
