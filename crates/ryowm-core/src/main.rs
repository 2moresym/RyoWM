mod reactor;
mod wayland;
pub mod state;
mod scene;
pub mod layout;
mod window;
mod workspace;
mod input;
mod keybind;
mod gesture;
mod config;
mod output;
mod xwayland;
mod clipboard;
mod notifications;
mod portal;
mod audio;
mod power;
mod ipc;
mod noctilia;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    tracing::info!("RyoWM starting (Phase 0 skeleton — no compositor functionality yet)");
    Ok(())
}
