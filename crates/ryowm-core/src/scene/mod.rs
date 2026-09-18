//! Scene graph and frame driver.
//!
//! Phase 2 exit criteria (architecture doc §14):
//! 1. The Phase-1 client's window is visibly rendered on screen.
//! 2. Idle-frame-suppression verified: no GPU work during no-damage idle.
//!
//! | Subsystem | Responsibility | Process | Threading | Failure behavior |
//! |---|---|---|---|---|
//! | Scene graph | Tree of renderable nodes (surfaces, decorations) | in-proc | owned by main thread, read-only snapshot handed to render thread per frame | Invariant violations are programming errors → debug-assert in debug builds, defensive no-op + log in release |
//!
//! In Phase 2 the scene graph is Smithay's `Space<Window>`: mapped toplevels
//! are flattened into the render backend's frame queue each damaging frame.
//! Window lifecycle and focus stay Phase 4 work; this module only flattens
//! whatever is currently mapped. Renderer errors never propagate: a failed
//! frame is logged and skipped, the compositor keeps running (architecture
//! doc §3 failure-behavior contract).

use ryowm_render::{gles::GlesBackend, PresentResult, RenderBackend};

use crate::wayland::RyoWmState;

/// Outcome of one `render_frame` call, for logging and testing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameOutcome {
    Presented,
    Skipped,
}

/// Drive exactly one frame: consume accumulated damage, queue every mapped
/// window, and present (or skip all GPU work when clean).
pub fn render_frame(
    state: &mut RyoWmState,
    renderer: &mut GlesBackend,
) -> anyhow::Result<FrameOutcome> {
    renderer.prune_dead_surfaces();

    let token = renderer.begin_frame(renderer.output_id());

    if state.damage_all {
        renderer.mark_damage(token, renderer.output_extent());
        state.damage_all = false;
    }
    if let Some(damage) = state.pending_damage.take() {
        renderer.mark_damage(token, damage);
    }

    for (z, window) in state.space.elements().enumerate() {
        if let Some(toplevel) = window.toplevel() {
            let surface = toplevel.wl_surface().clone();
            let handle = renderer.attach_surface(&surface);
            // Output-space rect (space location + committed geometry) — the
            // shared convention in `window::window_output_rect`. Geometry
            // alone would stack every window at the origin.
            let rect = crate::window::window_output_rect(&state.space, window);
            renderer.composite_surface(token, handle, rect, z as u32);
        }
    }

    match renderer.present(token) {
        PresentResult::Presented => {
            // Unblock clients waiting for a frame callback so animated
            // clients keep producing frames. The output is reported as the
            // primary scanout target — without it Smithay withholds
            // callbacks until the 1s overdue fallback, stalling clients.
            let output = renderer.output().clone();
            for window in state.space.elements() {
                // `None` throttle: callbacks follow the primary-scanout path
                // every presented frame. (A `Some` throttle would additionally
                // drip-feed occluded surfaces on the overdue fallback; with a
                // single output and no occlusion tracking yet, every mapped
                // surface reports this output as primary, so `None` is exact.)
                window.send_frame(&output, state.clock.now(), None, |_, _| {
                    Some(output.clone())
                });
            }
            Ok(FrameOutcome::Presented)
        }
        PresentResult::SkippedNoDamage => Ok(FrameOutcome::Skipped),
        PresentResult::Failed => {
            tracing::warn!("Frame failed; continuing without presenting");
            Ok(FrameOutcome::Skipped)
        }
    }
}
