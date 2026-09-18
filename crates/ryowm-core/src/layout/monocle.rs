//! Monocle layout: every window takes the full viewport (architecture doc
//! §2.5). Only the topmost is visible; the rest are kept mapped so focus
//! cycling and layout-switching stay instant. Pure and unit-tested like
//! every `LayoutAlgorithm` impl.

use ryowm_common::{Rect, WindowId};

use super::LayoutAlgorithm;

/// Fullscreen-everything stacking. Shares nothing with the tiling-tree
/// geometry code by design — only the `LayoutAlgorithm` trait interface.
#[derive(Debug, Clone, Copy, Default)]
pub struct MonocleLayout;

impl LayoutAlgorithm for MonocleLayout {
    fn compute(&self, windows: &[WindowId], viewport: Rect) -> Vec<(WindowId, Rect)> {
        windows.iter().map(|id| (*id, viewport)).collect()
    }

    fn name(&self) -> &'static str {
        "monocle"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_window_gets_full_viewport() {
        let viewport = Rect::new(0, 0, 800, 600);
        let windows = [WindowId(1), WindowId(2), WindowId(3)];
        assert_eq!(
            MonocleLayout.compute(&windows, viewport),
            vec![
                (WindowId(1), viewport),
                (WindowId(2), viewport),
                (WindowId(3), viewport),
            ]
        );
    }

    #[test]
    fn empty_input_places_nothing() {
        assert!(MonocleLayout
            .compute(&[], Rect::new(0, 0, 800, 600))
            .is_empty());
    }
}
