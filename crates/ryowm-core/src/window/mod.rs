//! Window lifecycle: placement, move, resize, close, focus order.
//!
//! Phase 4 exit criteria (architecture doc §14):
//! 1. Open a window (already works since Phase 2).
//! 2. Move it — verified by screenshot (visibly relocated).
//! 3. Resize it — verified by screenshot (real size change, content intact).
//! 4. Close it — verified by unmap log + screenshot (gone, no artifact).
//! 5. All of the above with 2+ windows open, independently (the actual risk
//!    case: per-window mutable state must not leak across windows).
//! 6. `cargo xtask check` passes.
//! 7. Idle CPU holds the Phase 2/3 baseline (nothing here polls).
//!
//! | Subsystem | Responsibility | Process | Threading | Failure behavior |
//! |---|---|---|---|---|
//! | Window manager | Window lifecycle, focus, state transitions | in-proc | main thread | Invalid state transition request → rejected, logged, no panic |
//!
//! Deliberate scope notes (deferred, not missing):
//! - Tiling is Phase 5. `WindowMode` is an enum (not a bool) so a `Tiled`
//!   variant extends it without a rewrite; floating windows stay a separate
//!   state, never routed through `LayoutAlgorithm` (architecture doc §2.5).
//! - Move/resize/close triggers are TEMPORARY placeholders (Alt+drag,
//!   Alt+Q): the real keybind engine is Phase 8, which replaces them —
//!   marked as such at each site, never to be extended.
//! - Alt (not Super) is used so nested testing doesn't collide with niri's
//!   default Super bindings (Super+drag would move our whole nested window
//!   on the host AND the client inside it; Super+Q would close the nested
//!   window itself).
//! - Snap/maximize/minimize/always-on-top: deferred, not exit criteria.
//! - Focus-on-open: deliberately off — focus changes only on click and on
//!   close-fallback, so focus state stays auditable until real focus policy
//!   lands with later phases.

use ryowm_common::Rect;
use smithay::desktop::{Space, Window};

/// Lifecycle mode of a managed window. Floating only for now; `Tiled`
/// arrives in Phase 5 as a new variant on this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowMode {
    /// Explicit geometry, rendered above tiled content, never routed
    /// through a layout algorithm.
    Floating { geometry: Rect },
}

/// Pointer-drag interaction in progress (temporary Phase 4 trigger; the
/// keybind engine in Phase 8 replaces mouse-chord triggers, not extends).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragMode {
    Move,
    Resize,
}

/// Active Alt+drag operation. `window` is the Smithay scene element being
/// manipulated; lifecycle/focus records live in `CompositorState`.
#[derive(Debug, Clone)]
pub struct DragState {
    pub window: Window,
    pub mode: DragMode,
    /// Pointer position (logical) when the drag started.
    pub start_pointer: smithay::utils::Point<f64, smithay::utils::Logical>,
    /// Window geometry (output space) when the drag started.
    pub start_geometry: Rect,
    /// Last size a configure was sent for (resize storm guard).
    pub last_sent_size: Option<(i32, i32)>,
}

/// Cascade step in pixels between consecutively mapped windows.
pub const CASCADE_STEP: i32 = 32;
/// Cascade pattern repeats after this many windows.
pub const CASCADE_SLOTS: u64 = 12;

/// Pure cascade offset for a placement seed. Unit-tested; wrapping is
/// intentional (placement cycles forever without growing state).
pub fn cascade_offset(seed: u64) -> (i32, i32) {
    let slot = (seed % CASCADE_SLOTS) as i32;
    (slot * CASCADE_STEP, slot * CASCADE_STEP)
}

/// Output-space rectangle of a mapped window: its space location plus its
/// committed client geometry. Single convention shared by damage marking
/// (commits, moves) and the render queue — one place, no drift.
pub fn window_output_rect(space: &Space<Window>, window: &Window) -> Rect {
    let geometry = window.geometry();
    match space.element_location(window) {
        Some(location) => Rect::new(
            location.x + geometry.loc.x,
            location.y + geometry.loc.y,
            geometry.size.w,
            geometry.size.h,
        ),
        None => Rect::new(
            geometry.loc.x,
            geometry.loc.y,
            geometry.size.w,
            geometry.size.h,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cascade_starts_at_origin() {
        assert_eq!(cascade_offset(0), (0, 0));
    }

    #[test]
    fn cascade_steps_diagonally() {
        assert_eq!(cascade_offset(1), (32, 32));
        assert_eq!(cascade_offset(2), (64, 64));
    }

    #[test]
    fn cascade_wraps_around() {
        assert_eq!(cascade_offset(CASCADE_SLOTS), (0, 0));
        assert_eq!(cascade_offset(CASCADE_SLOTS + 1), (32, 32));
    }
}
