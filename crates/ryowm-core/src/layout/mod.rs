//! Layout algorithms (architecture doc §2.5).
//!
//! Phase 5 exit criteria (architecture doc §14): opening 3+ windows
//! auto-tiles per the active algorithm, and runtime layout parameter
//! changes (e.g. master ratio) work.
//!
//! | Subsystem | Responsibility | Process | Threading | Failure behavior |
//! |---|---|---|---|---|
//! | Layout engines (tiling/monocle) | Compute geometry per algorithm | in-proc | main thread (pure functions) | Algorithm panic → caught at call boundary, workspace falls back to previous known-good layout, logged |
//!
//! Each workspace will hold a `Box<dyn LayoutAlgorithm>` (Phase 7); until
//! then one `LayoutEngine` covers the single output. Floating windows are
//! never routed through here — see `window::WindowMode` (architecture doc
//! §2.5). Relayout reconfigures every tiled window even when only one
//! changed: opens are rare, correctness first, and stable sizes make repeat
//! configures harmless.

mod monocle;
mod tiling_tree;

use ryowm_common::{Rect, WindowId};
use smithay::desktop::{Space, Window};
use tracing::{info, warn};

use crate::state::CompositorState;
pub use monocle::MonocleLayout;
pub use tiling_tree::MasterStackLayout;

pub trait LayoutAlgorithm: std::fmt::Debug {
    /// Pure function: given the current window set and the output's usable
    /// viewport, return the geometry each window should occupy. Must not
    /// mutate any external state and must be deterministic for a given
    /// input — this determinism is what makes runtime layout-switching
    /// safe (architecture doc §2.5) and what makes this function
    /// snapshot-testable with `insta` (architecture doc §11).
    fn compute(&self, windows: &[WindowId], viewport: Rect) -> Vec<(WindowId, Rect)>;

    /// Human-readable name for logging/IPC state reporting.
    fn name(&self) -> &'static str;
}

/// Which algorithm the engine instantiates. `Scrolling` arrives in Phase 11.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutKind {
    MasterStack,
    Monocle,
}

/// Ratio step for the temporary runtime triggers (Phase 8 replaces them).
pub const RATIO_STEP: f32 = 0.05;

/// Single-output layout state: active algorithm plus its parameters.
/// Reconstructed into a boxed algorithm per apply, mirroring the
/// per-workspace `Box<dyn LayoutAlgorithm>` Phase 7 will own.
#[derive(Debug, Clone, Copy)]
pub struct LayoutEngine {
    kind: LayoutKind,
    ratio: f32,
}

impl LayoutEngine {
    pub fn new() -> Self {
        Self {
            kind: LayoutKind::MasterStack,
            ratio: 0.55,
        }
    }

    pub fn kind(&self) -> LayoutKind {
        self.kind
    }

    pub fn ratio(&self) -> f32 {
        self.ratio
    }

    pub fn set_kind(&mut self, kind: LayoutKind) {
        self.kind = kind;
    }

    /// Nudge the master ratio, clamped to sane bounds. Returns the result.
    pub fn adjust_ratio(&mut self, delta: f32) -> f32 {
        self.ratio = (self.ratio + delta).clamp(0.1, 0.9);
        self.ratio
    }

    fn algorithm(&self) -> Box<dyn LayoutAlgorithm> {
        match self.kind {
            LayoutKind::MasterStack => Box::new(MasterStackLayout::new(self.ratio)),
            LayoutKind::Monocle => Box::new(MonocleLayout),
        }
    }

    /// Recompute geometry for every tiled window (ascending id = mapping
    /// order, stable across drags) and apply it: reposition in the space
    /// and send an xdg configure with the new size. Windows never see a
    /// partially-applied layout — positions and configures go out together
    /// per window. Marks full-output damage when anything moved; opens and
    /// resizes are rare enough that per-window damage unioning is not worth
    /// it here. Returns the number of windows (re)positioned.
    pub fn apply(
        &self,
        space: &mut Space<Window>,
        core: &mut CompositorState,
        damage_all: &mut bool,
    ) -> usize {
        let viewport = match space
            .outputs()
            .next()
            .and_then(|output| output.current_mode())
        {
            Some(mode) => Rect::new(0, 0, mode.size.w, mode.size.h),
            None => {
                warn!("Relayout skipped: no output with a current mode");
                return 0;
            }
        };
        let mut tiled: Vec<WindowId> = core
            .windows
            .iter()
            .filter(|(_, record)| matches!(record.mode, crate::window::WindowMode::Tiled))
            .map(|(id, _)| *id)
            .collect();
        tiled.sort_by_key(|id| id.0);
        if tiled.is_empty() {
            return 0;
        }

        let algorithm = self.algorithm();
        info!(
            algorithm = algorithm.name(),
            ratio = self.ratio,
            windows = tiled.len(),
            "Applying layout"
        );
        let mut applied = 0;
        for (id, rect) in algorithm.compute(&tiled, viewport) {
            if rect.is_empty() {
                warn!(?id, "Layout produced empty geometry; skipping window");
                continue;
            }
            let surface = core
                .windows
                .get(&id)
                .map(|record| record.surface.clone());
            let Some(surface) = surface else {
                continue;
            };
            let window = space
                .elements()
                .find(|window| {
                    window.toplevel().map(|toplevel| toplevel.wl_surface()) == Some(&surface)
                })
                .cloned();
            let Some(window) = window else {
                continue;
            };
            let Some(toplevel) = window.toplevel() else {
                continue;
            };
            space.map_element(window.clone(), (rect.x, rect.y), false);
            toplevel.with_pending_state(|pending| {
                pending.size = Some((rect.width, rect.height).into());
            });
            toplevel.send_configure();
            applied += 1;
        }
        if applied > 0 {
            *damage_all = true;
        }
        applied
    }
}

impl Default for LayoutEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratio_adjustment_clamps() {
        let mut engine = LayoutEngine::new();
        assert_eq!(engine.adjust_ratio(1.0), 0.9);
        assert_eq!(engine.adjust_ratio(-1.0), 0.1);
    }

    #[test]
    fn ratio_adjustment_steps() {
        let mut engine = LayoutEngine::new();
        let before = engine.ratio();
        assert_eq!(engine.adjust_ratio(RATIO_STEP), before + RATIO_STEP);
    }
}
