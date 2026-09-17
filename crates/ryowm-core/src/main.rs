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
use smithay::backend::winit::{WinitEvent, init as winit_init};
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::wayland_server::Display;
use smithay::reexports::winit::platform::pump_events::PumpStatus;
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

    let mut reactor = reactor::Reactor::try_new()?;
    let display = Display::<wayland::RyoWmState>::new()?;
    let mut state = wayland::RyoWmState::new(display, reactor.handle());

    // Nested development window (no bare TTY needed for dev iteration).
    let (backend, mut winit) = winit_init::<GlesRenderer>()
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

    tracing::info!("Entering main loop");
    while state.running.load(Ordering::SeqCst) {
        // Pump the nested window first: resizes, input, close requests.
        let pump_status = winit.dispatch_new_events(|event| match event {
            WinitEvent::Resized { size, .. } => {
                tracing::info!(
                    width = size.w,
                    height = size.h,
                    "Nested window resized"
                );
                gles.set_output_size(size);
                state.damage_all = true;
            }
            WinitEvent::Input(_) => {
                tracing::debug!("Winit input event (unhandled in Phase 1)");
            }
            WinitEvent::CloseRequested => {
                tracing::info!("Nested window close requested, shutting down");
                state.running.store(false, Ordering::SeqCst);
            }
            WinitEvent::Focus(_) | WinitEvent::Redraw => {}
        });

        if let PumpStatus::Exit(_) = pump_status {
            break;
        }

        reactor.dispatch(&mut state)?;
        scene::render_frame(&mut state, &mut gles)?;
        state.space.refresh();
        state.display_handle.flush_clients()?;
    }

    tracing::info!("RyoWM shut down cleanly");
    Ok(())
}
