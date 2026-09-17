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

    tracing::info!("RyoWM starting (Phase 2: GLES rendering)");

    // Phase 2 exit criteria (architecture doc §14):
    // 1. The Phase-1 client's window is visibly rendered on screen.
    // 2. Idle-frame-suppression verified: no GPU work during no-damage idle.
    // (Phase 1 criteria — nested window, commit ack, clean disconnect,
    // xtask green, ADR-0004 — keep holding while rendering is added.)

    let reactor = reactor::Reactor::try_new()?;
    let display = Display::<wayland::RyoWmState>::new()?;
    let mut state = wayland::RyoWmState::new(display, reactor.handle());

    // Nested development window (no bare TTY needed for dev iteration).
    let (backend, winit) = winit_init::<GlesRenderer>()
        // `winit::Error` carries a non-Send/Sync source, so it cannot convert
        // into `anyhow::Error` via `?` — render it into the message instead.
        .map_err(|e| anyhow::anyhow!("Failed to initialize winit backend: {e}"))?;
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
        scene::render_frame(state, &mut gles)?;
        state.space.refresh();
        state.display_handle.flush_clients()?;
        Ok(())
    })?;

    tracing::info!("RyoWM shut down cleanly");
    Ok(())
}
