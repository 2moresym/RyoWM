//! Wayland protocol handlers and state. Phase 1 wires up the minimum globals
//! (`wl_compositor`, `wl_shm`, `xdg_wm_base`) via Smithay, following anvil's
//! protocol bootstrapping structurally (see ADR-0004). No window management,
//! layout, or IPC code is carried over — only the protocol-globals plumbing.
//!
//! | Subsystem | Responsibility | Process | Threading | Failure behavior |
//! |---|---|---|---|---|
//! | Wayland protocol | Global objects, client connections, protocol state machine | in-proc | main thread (event-loop bound) | Malformed client request → protocol error to that client only, client disconnected, compositor unaffected |

use std::sync::{atomic::AtomicBool, Arc};

use ryowm_common::Rect;
use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_output, delegate_seat, delegate_shm, delegate_xdg_shell,
    desktop::{Space, Window},
    input::{keyboard::XkbConfig, pointer::PointerHandle, Seat, SeatHandler, SeatState},
    reexports::{
        calloop::{generic::Generic, Interest, Mode, PostAction},
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{wl_buffer, wl_surface},
            Client, Display, DisplayHandle, Resource,
        },
    },
    utils::{Clock, Monotonic, Physical, Size, SERIAL_COUNTER},
    wayland::{
        buffer::BufferHandler,
        compositor::{get_parent, CompositorClientState, CompositorHandler, CompositorState},
        output::OutputHandler,
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
        },
        shm::{ShmHandler, ShmState},
        socket::ListeningSocketSource,
    },
};
use tracing::{info, warn};

use crate::{
    state::{CompositorState as CoreState, WindowState},
    window::{DragState, WindowMode, window_output_rect},
};

/// Per-client state stored by wayland-server. Required by `CompositorHandler`.
#[derive(Debug, Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, client_id: ClientId, reason: DisconnectReason) {
        // Phase 1 exit criterion: clean disconnect must drop all of the
        // client's Wayland resources without affecting the compositor.
        info!(?client_id, ?reason, "Wayland client disconnected");
    }
}

/// The compositor's Wayland-facing state. Owns all Smithay protocol state
/// objects and the display handle.
///
/// Note: Smithay has its own type also named `CompositorState`; it is kept
/// behind the `smithay_compositor` field and never conflated with our
/// `ryowm-core::state::CompositorState` (the ownership root for windows,
/// workspaces, and outputs).
pub struct RyoWmState {
    pub running: Arc<AtomicBool>,
    pub display_handle: DisplayHandle,

    // Smithay protocol state objects.
    pub smithay_compositor: CompositorState,
    pub shm_state: ShmState,
    pub xdg_shell_state: XdgShellState,
    pub seat_state: SeatState<RyoWmState>,
    // Registered with clients via the wl_seat global in `new`, but not read
    // until input focus handling lands (Phase 3).
    #[allow(dead_code)]
    pub seat: Seat<RyoWmState>,
    #[allow(dead_code)]
    pub pointer: PointerHandle<RyoWmState>,

    // Desktop workspace: the live scene. Lifecycle/focus records for the
    // same windows live in `core` (see `state::CompositorState`); both are
    // updated in the same handler on map/unmap/destroy.
    pub space: Space<Window>,
    /// Window lifecycle/focus/placement authority.
    pub core: CoreState,
    /// Active pointer drag (temporary Phase 4 trigger), if any.
    pub drag: Option<DragState>,

    /// Damage accumulated since the last presented frame, consumed by the
    /// scene renderer each iteration.
    pub pending_damage: Option<Rect>,
    /// Full-output repaint request (window destroyed, output resized).
    pub damage_all: bool,
    /// Resize reported by the backend, not yet applied to the output.
    /// Applied by the main loop (which owns the renderer) after wakeup —
    /// calloop source callbacks only see `&mut RyoWmState`, never the GPU.
    pub pending_resize: Option<Size<i32, Physical>>,
    /// Frame presentation clock for client `frame` callbacks.
    pub clock: Clock<Monotonic>,
}

impl RyoWmState {
    pub fn new(
        display: Display<RyoWmState>,
        handle: calloop::LoopHandle<'static, RyoWmState>,
    ) -> Self {
        let dh = display.handle();
        let running = Arc::new(AtomicBool::new(true));

        // Client I/O: dispatch Wayland requests through the Smithay handlers
        // below. `Generic` hands the stored `Display` back as `&mut NoIoDrop`;
        // `get_mut` is unsafe only because it could drop the polled fd, which
        // we never do — same pattern as Smithay's anvil (see ADR-0004).
        handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, data: &mut RyoWmState| {
                    unsafe {
                        display.get_mut().dispatch_clients(data).unwrap();
                    }
                    Ok(PostAction::Continue)
                },
            )
            .expect("Failed to register Wayland display with the reactor");

        // Accept clients on an auto-allocated Wayland socket.
        let source = ListeningSocketSource::new_auto().expect("Failed to bind Wayland socket");
        let socket_name = source.socket_name().to_string_lossy().into_owned();
        info!(socket = socket_name, "Listening for Wayland clients");

        handle
            .insert_source(source, |client_stream, _, data: &mut RyoWmState| match data
                .display_handle
                .insert_client(client_stream, Arc::new(ClientState::default()))
            {
                Ok(_) => info!("Wayland client connected"),
                Err(err) => warn!("Error adding Wayland client: {}", err),
            })
            .expect("Failed to register Wayland socket with the reactor");

        // Minimum protocol globals: wl_compositor (+ subcompositor), wl_shm,
        // xdg_wm_base. Nothing else is advertised in Phase 1.
        let smithay_compositor = CompositorState::new::<RyoWmState>(&dh);
        let shm_state = ShmState::new::<RyoWmState>(&dh, vec![]);
        let xdg_shell_state = XdgShellState::new::<RyoWmState>(&dh);
        let mut seat_state = SeatState::new();

        let mut seat = seat_state.new_wl_seat(&dh, "seat0".to_string());
        let pointer = seat.add_pointer();
        seat.add_keyboard(XkbConfig::default(), 200, 25)
            .expect("Failed to initialize keyboard");

        RyoWmState {
            running,
            display_handle: dh,
            smithay_compositor,
            shm_state,
            xdg_shell_state,
            seat_state,
            seat,
            pointer,
            space: Space::default(),
            core: CoreState::new(),
            drag: None,
            pending_damage: None,
            damage_all: false,
            pending_resize: None,
            clock: Clock::new(),
        }
    }

    /// Mapped window owning `surface`, following subsurface links to the
    /// toplevel root, if any.
    fn find_window_for_surface(&self, surface: &wl_surface::WlSurface) -> Option<Window> {
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        self.space
            .elements()
            .find(|window| window.toplevel().map(|toplevel| toplevel.wl_surface()) == Some(&root))
            .cloned()
    }

    /// Mirror the committed scene geometry into the lifecycle record, so
    /// `WindowMode::Floating` always reflects reality instead of drifting.
    fn sync_stored_geometry(&mut self, window: &Window) {
        let rect = window_output_rect(&self.space, window);
        let Some(toplevel) = window.toplevel() else {
            return;
        };
        if let Some(id) = self.core.window_id_for_surface(toplevel.wl_surface()) {
            if let Some(state) = self.core.windows.get_mut(&id) {
                state.mode = WindowMode::Floating { geometry: rect };
            }
        }
    }
}

// --- Trait implementations for Smithay dispatch ---

impl CompositorHandler for RyoWmState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.smithay_compositor
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &wl_surface::WlSurface) {
        // Required first: moves the committed buffer into Smithay's renderer
        // surface state. Without this, surface trees stay empty and nothing
        // is ever composited (see `gles::GlesBackend`).
        on_commit_buffer_handler::<RyoWmState>(surface);
        info!(
            surface = surface.id().protocol_id(),
            "Acknowledged client buffer commit"
        );
        // Phase 2: feed the damage tracker. `on_commit` refreshes the
        // window's bounding box from the new buffer first — without it the
        // geometry below stays empty and damage would be lost.
        match self.find_window_for_surface(surface) {
            Some(window) => {
                window.on_commit();
                let region = window_output_rect(&self.space, &window);
                self.sync_stored_geometry(&window);
                if region.is_empty() {
                    self.damage_all = true;
                } else {
                    self.pending_damage = Some(match self.pending_damage {
                        Some(pending) => pending.union(&region),
                        None => region,
                    });
                }
            }
            // Unmapped surface (e.g. popups, which Phase 2 does not composite
            // yet): repaint everything rather than risk a stale frame.
            None => self.damage_all = true,
        }
    }
}

impl BufferHandler for RyoWmState {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {
        // Phase 1: no-op. Phase 2 ties this to scene-graph cleanup.
    }
}

impl ShmHandler for RyoWmState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl XdgShellHandler for RyoWmState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // Send an initial configure so the client commits a buffer.
        surface.with_pending_state(|state| {
            state.size = Some((800, 600).into());
        });
        surface.send_configure();
        // Cascade placement so consecutive windows don't perfectly overlap.
        // Tiling-aware placement is Phase 5; focus is left untouched here
        // (click to focus) to keep focus changes auditable.
        let (x, y) = self.core.next_cascade_location();
        let window = Window::new_wayland_window(surface);
        let id = self.core.alloc_window_id();
        let wl_surface = window
            .toplevel()
            .map(|toplevel| toplevel.wl_surface().clone());
        self.space.map_element(window, (x, y), false);
        if let Some(wl_surface) = wl_surface {
            self.core.windows.insert(
                id,
                WindowState {
                    id,
                    surface: wl_surface,
                    mode: WindowMode::Floating {
                        geometry: Rect::default(),
                    },
                },
            );
        }
        info!(?id, x, y, "Mapped new toplevel (cascade placement)");
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let destroyed = surface.wl_surface().clone();
        // A drag in progress dies with its window — never manipulate a
        // destroyed surface on the next motion event.
        if let Some(drag) = &self.drag {
            let dragging_dead = drag
                .window
                .toplevel()
                .map(|toplevel| toplevel.wl_surface() == &destroyed)
                .unwrap_or(false);
            if dragging_dead {
                self.drag = None;
                info!("Cancelled drag: window destroyed mid-drag");
            }
        }
        // Bind first: a temporary in the `if let` scrutinee would keep the
        // immutable borrow of `space` alive across `unmap_elem` below.
        let removed = self
            .space
            .elements()
            .find(|window| {
                window.toplevel().map(|toplevel| toplevel.wl_surface()) == Some(&destroyed)
            })
            .cloned();
        if let Some(window) = removed {
            self.space.unmap_elem(&window);
            info!("Unmapped destroyed toplevel");
        }
        // Forget lifecycle records; fall keyboard focus forward to the most
        // recently used remaining window (or clear it) instead of dangling
        // focus on a destroyed surface.
        if let Some(id) = self.core.window_id_for_surface(&destroyed) {
            let fallback = self.core.forget_window(id);
            let serial = SERIAL_COUNTER.next_serial();
            match fallback.and_then(|fallback| {
                self.core
                    .windows
                    .get(&fallback)
                    .map(|state| state.surface.clone())
            }) {
                Some(surface) => {
                    if let Some(keyboard) = self.seat.get_keyboard() {
                        keyboard.set_focus(self, Some(surface.clone()), serial);
                    }
                    info!(?fallback, "Keyboard focus fell back after close");
                }
                None => {
                    if let Some(keyboard) = self.seat.get_keyboard() {
                        keyboard.set_focus(self, None, serial);
                    }
                    info!("Keyboard focus cleared: no windows left");
                }
            }
        }
        // The uncovered area needs repainting next frame.
        self.damage_all = true;
    }

    fn new_popup(&mut self, _surface: PopupSurface, _positioner: PositionerState) {
        // Phase 1: acknowledged, not managed.
    }

    fn grab(
        &mut self,
        _surface: PopupSurface,
        _seat: smithay::reexports::wayland_server::protocol::wl_seat::WlSeat,
        _serial: smithay::utils::Serial,
    ) {
        // Phase 1: no popup grab handling.
    }

    fn reposition_request(
        &mut self,
        _surface: PopupSurface,
        _positioner: PositionerState,
        _token: u32,
    ) {
        // Phase 1: no popup repositioning.
    }
}

impl SeatHandler for RyoWmState {
    // `WlSurface` already implements all three focus-target traits in
    // Smithay; dedicated focus types arrive with input handling (Phase 3).
    type KeyboardFocus = wl_surface::WlSurface;
    type PointerFocus = wl_surface::WlSurface;
    type TouchFocus = wl_surface::WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<RyoWmState> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, focused: Option<&Self::KeyboardFocus>) {
        // Exit 2 evidence: every keyboard focus switch is logged.
        // Data-device focus follows in the clipboard phase; the seat already
        // routes key events to whichever surface holds keyboard focus.
        match focused {
            Some(surface) => info!(
                surface = surface.id().protocol_id(),
                "Keyboard focus changed"
            ),
            None => info!("Keyboard focus cleared"),
        }
    }

    fn cursor_image(
        &mut self,
        _seat: &Seat<Self>,
        _image: smithay::input::pointer::CursorImageStatus,
    ) {
        // Phase 1: no cursor rendering yet.
    }
}

impl OutputHandler for RyoWmState {}

// --- Delegate macros: route protocol messages to the handlers above ---

delegate_compositor!(RyoWmState);
delegate_shm!(RyoWmState);
delegate_xdg_shell!(RyoWmState);
delegate_seat!(RyoWmState);
delegate_output!(RyoWmState);
