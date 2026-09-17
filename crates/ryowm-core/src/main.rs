mod audio;
mod clipboard;
mod config;
mod gesture;
mod input;
mod ipc;
mod keybind;
pub mod layout;
mod noctilia;
mod notifications;
mod output;
mod portal;
mod power;
mod reactor;
mod scene;
pub mod state;
mod wayland;
mod window;
mod workspace;
mod xwayland;

use std::sync::atomic::Ordering;

use ryowm_render::gles::GlesBackend;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::winit::{init as winit_init, WinitEvent};
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::wayland_server::Display;
use smithay::utils::Transform;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::filter::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::filter::EnvFilter::new("info")),
        )
        .init();

    tracing::info!("RyoWM starting (Phase 3: input)");

    // Phase 3 exit criteria (task brief):
    // 1. Real mouse motion moves a visible cursor.
    // 2. Clicking a mapped window focuses it (focus change is logged).
    // 3. Typing with focus delivers key events to that client.
    // 4. Losing one input device neither crashes the compositor nor affects
    //    other devices.
    // 5. `cargo xtask check` still passes.
    // 6. Idle CPU stays near zero with the input thread integrated.

    let reactor = reactor::Reactor::try_new()?;
    let display = Display::<wayland::RyoWmState>::new()?;
    let mut state = wayland::RyoWmState::new(display, reactor.handle());

    // Nested development window (no bare TTY needed for dev iteration).
    let (backend, winit) = winit_init::<GlesRenderer>()
        // `winit::Error` carries a non-Send/Sync source, so it cannot convert
        // into `anyhow::Error` via `?` — render it into the message instead.
        .map_err(|e| anyhow::anyhow!("Failed to initialize winit backend: {e}"))?;
    // Single-cursor nested testing: hide the host cursor while it is over
    // our window so only the compositor's own cursor is visible. Otherwise
    // the two cursors separate by ~1 host frame (our submit waits for host
    // vsync) and read as lag/doubling.
    backend.window().set_cursor_visible(false);
    let size = backend.window_size();
    tracing::info!(
        width = size.w,
        height = size.h,
        "Nested winit window opened"
    );

    // Single hardcoded output for the nested window. Output enumeration,
    // hotplug, and multi-output mapping are Phase 7 work; Phase 2 only
    // needs one output to present client frames to.
    let mode = Mode {
        size,
        refresh: 60_000,
    };
    let output = Output::new(
        "winit-0".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "RyoWM".into(),
            model: "Winit".into(),
        },
    );
    let _output_global = output.create_global::<wayland::RyoWmState>(&state.display_handle);
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.space.map_output(&output, (0, 0));
    let mut gles = GlesBackend::new(backend, output, &state.display_handle);

    // Pointer starts at the output center so the first motion is relative to
    // a sane location rather than the (0, 0) corner.
    {
        let extent = gles.output_extent();
        let center = smithay::utils::Point::<f64, smithay::utils::Logical>::from((
            extent.width as f64 / 2.0,
            extent.height as f64 / 2.0,
        ));
        if let Some(pointer) = state.seat.get_pointer() {
            pointer.set_location(center);
        }
    }

    // Pointer image: system theme, else the built-in fallback square.
    let cursor = input::cursor::load_default_cursor();
    gles.set_cursor(cursor.buffer, cursor.hotspot);

    // Dedicated input thread (libinput) → lock-free channel → calloop ping
    // source. Main-thread handling stays inside the reactor: no polling.
    let input_channel = input::spawn_input_thread()?;
    let mut input_consumer = input_channel.consumer;
    reactor.handle().insert_source(
        input_channel.source,
        move |(), _, state: &mut wayland::RyoWmState| {
            // Coalesce adjacent motion bursts: at high report rates one
            // wakeup can carry dozens of motions, and presenting per event
            // would serialize on swap vsync and lag clients. Summed deltas
            // are indistinguishable from one larger motion.
            let mut coalesced: Option<(f64, f64, u32)> = None;
            let flush_motion = |state: &mut wayland::RyoWmState,
                                    coalesced: &mut Option<(f64, f64, u32)>| {
                if let Some((dx, dy, time_ms)) = coalesced.take() {
                    input::dispatch_input_event(
                        state,
                        input::InputEvent::PointerMotion { dx, dy, time_ms },
                    );
                }
            };
            while let Ok(event) = input_consumer.pop() {
                match event {
                    input::InputEvent::PointerMotion { dx, dy, time_ms } => {
                        let slot = coalesced.get_or_insert((0.0, 0.0, time_ms));
                        slot.0 += dx;
                        slot.1 += dy;
                        slot.2 = time_ms;
                    }
                    other => {
                        flush_motion(state, &mut coalesced);
                        input::dispatch_input_event(state, other);
                    }
                }
            }
            flush_motion(state, &mut coalesced);
        },
    )?;


    // The winit event loop is itself a calloop event source
    // (`EventSource for WinitEventLoop`): window/input events wake the parked
    // reactor instead of being polled. The callback only touches `&mut
    // RyoWmState` — GPU work stays in the per-wakeup closure below, which
    // owns the renderer.
    // Note: `InsertError<WinitEventLoop>` cannot convert into anyhow via `?`
    // (winit internals are not Send/Sync), so the error is rendered explicitly.
    reactor
        .handle()
        .insert_source(
            winit,
            |event, _, state: &mut wayland::RyoWmState| match event {
                WinitEvent::Resized { size, .. } => {
                    tracing::info!(width = size.w, height = size.h, "Nested window resized");
                    state.pending_resize = Some(size);
                }
                WinitEvent::Input(_) => {
                    tracing::debug!(
                        "Winit input event (unhandled; libinput owns input from Phase 3)"
                    );
                }
                WinitEvent::CloseRequested => {
                    tracing::info!("Nested window close requested, shutting down");
                    state.running.store(false, Ordering::SeqCst);
                }
                // Host asked for a repaint (e.g. after occlusion): full damage.
                WinitEvent::Redraw => {
                    state.damage_all = true;
                }
                WinitEvent::Focus(_) => {}
            },
        )
        .map_err(|err| anyhow::anyhow!("Failed to register winit event source: {:?}", err.error))?;

    tracing::info!("Entering main loop");
    reactor.run(&mut state, |state| {
        if let Some(size) = state.pending_resize.take() {
            gles.set_output_size(size);
            state.damage_all = true;
        }
        // Cursor follows the seat's tracked location every frame.
        if let Some(pointer) = state.seat.get_pointer() {
            gles.set_cursor_position(pointer.current_location());
        }
        scene::render_frame(state, &mut gles)?;
        state.space.refresh();
        state.display_handle.flush_clients()?;
        Ok(())
    })?;

    tracing::info!("RyoWM shut down cleanly");
    Ok(())
}
