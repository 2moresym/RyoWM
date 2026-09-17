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

use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::winit::{WinitEvent, init as winit_init};
use smithay::reexports::wayland_server::Display;
use smithay::reexports::winit::platform::pump_events::PumpStatus;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::filter::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::filter::EnvFilter::new("info")),
        )
        .init();

    tracing::info!("RyoWM starting (Phase 1: minimal Wayland compositor)");

    // Phase 1 exit criteria (build brief §2.2):
    // 1. `cargo run -p ryowm-core` opens a nested winit window, no crash.
    // 2. An xdg_shell test client commit -> logged ack, no protocol error/hang.
    // 3. Clean client disconnect, no crash or leaked Wayland resources.
    // 4. `cargo xtask check` still passes.
    // 5. ADR-0004 records the anvil-wiring boundary.

    let mut reactor = reactor::Reactor::try_new()?;
    let display = Display::<wayland::RyoWmState>::new()?;
    let mut state = wayland::RyoWmState::new(display, reactor.handle());

    // Nested development window (no bare TTY needed for dev iteration).
    // Rendering is Phase 2; here the backend only provides the window and
    // the input/event pump. `_backend` is kept alive for the life of `main`
    // so the window (and its EGL context) stays valid.
    // `winit::Error` carries a non-Send/Sync source, so it cannot convert
    // into `anyhow::Error` via `?` — render it into the message instead.
    let (_backend, mut winit) = winit_init::<GlesRenderer>()
        .map_err(|e| anyhow::anyhow!("Failed to initialize winit backend: {e}"))?;
    {
        let size = _backend.window_size();
        tracing::info!(
            width = size.w,
            height = size.h,
            "Nested winit window opened"
        );
    }

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
        state.space.refresh();
        state.display_handle.flush_clients()?;
    }

    tracing::info!("RyoWM shut down cleanly");
    Ok(())
}
