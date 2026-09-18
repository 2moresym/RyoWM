//! Traditional master/stack tiling (architecture doc §2.5).
//!
//! Phase 5 exit criteria (architecture doc §14): opening 3+ windows
//! auto-tiles per this algorithm, and runtime ratio changes work.
//!
//! Pure function of (window order, ratio, viewport): deterministic for a
//! given input, which is what makes runtime layout-switching safe and what
//! makes this module unit-testable instead of screen-tested.

use ryowm_common::{Rect, WindowId};

use super::LayoutAlgorithm;

/// Master/stack tiling: the first window takes a `ratio` fraction of the
/// width; the rest stack vertically beside it. A lone window takes the full
/// viewport rather than leaving an empty stack area.
#[derive(Debug, Clone, Copy)]
pub struct MasterStackLayout {
    ratio: f32,
}

impl MasterStackLayout {
    pub fn new(ratio: f32) -> Self {
        Self {
            ratio: ratio.clamp(0.1, 0.9),
        }
    }

    pub fn ratio(&self) -> f32 {
        self.ratio
    }
}

impl LayoutAlgorithm for MasterStackLayout {
    fn compute(&self, windows: &[WindowId], viewport: Rect) -> Vec<(WindowId, Rect)> {
        match windows {
            [] => Vec::new(),
            [single] => vec![(*single, viewport)],
            [master, stack @ ..] => {
                let master_width = (viewport.width as f32 * self.ratio).round() as i32;
                let mut placed = Vec::with_capacity(windows.len());
                placed.push((
                    *master,
                    Rect::new(viewport.x, viewport.y, master_width, viewport.height),
                ));
                let stack_width = viewport.width - master_width;
                let rows = stack.len() as i32;
                let (row_height, remainder) = (viewport.height / rows, viewport.height % rows);
                // Remainder goes to the last row: deterministic, no gaps.
                let mut y = viewport.y;
                for (index, id) in stack.iter().enumerate() {
                    let height = row_height + if index as i32 == rows - 1 { remainder } else { 0 };
                    placed.push((
                        *id,
                        Rect::new(viewport.x + master_width, y, stack_width, height),
                    ));
                    y += height;
                }
                placed
            }
        }
    }

    fn name(&self) -> &'static str {
        "master-stack"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewport() -> Rect {
        Rect::new(0, 0, 800, 600)
    }

    #[test]
    fn empty_input_places_nothing() {
        let layout = MasterStackLayout::new(0.5);
        assert!(layout.compute(&[], viewport()).is_empty());
    }

    #[test]
    fn lone_window_takes_full_viewport() {
        let layout = MasterStackLayout::new(0.5);
        assert_eq!(
            layout.compute(&[WindowId(1)], viewport()),
            vec![(WindowId(1), viewport())]
        );
    }

    #[test]
    fn two_windows_split_at_ratio() {
        let layout = MasterStackLayout::new(0.5);
        assert_eq!(
            layout.compute(&[WindowId(1), WindowId(2)], viewport()),
            vec![
                (WindowId(1), Rect::new(0, 0, 400, 600)),
                (WindowId(2), Rect::new(400, 0, 400, 600)),
            ]
        );
    }

    #[test]
    fn three_windows_stack_beside_master() {
        let layout = MasterStackLayout::new(0.6);
        assert_eq!(
            layout.compute(&[WindowId(1), WindowId(2), WindowId(3)], viewport()),
            vec![
                (WindowId(1), Rect::new(0, 0, 480, 600)),
                (WindowId(2), Rect::new(480, 0, 320, 300)),
                (WindowId(3), Rect::new(480, 300, 320, 300)),
            ]
        );
    }

    #[test]
    fn ratio_clamps_to_sane_bounds() {
        assert_eq!(MasterStackLayout::new(0.0).ratio(), 0.1);
        assert_eq!(MasterStackLayout::new(2.0).ratio(), 0.9);
    }

    #[test]
    fn odd_height_remainder_lands_on_last_row() {
        let layout = MasterStackLayout::new(0.5);
        let placed = layout.compute(
            &[WindowId(1), WindowId(2), WindowId(3)],
            Rect::new(0, 0, 800, 601),
        );
        assert_eq!(placed[1].1.height + placed[2].1.height, 601);
        assert_eq!(placed[2].1.height, 301);
    }
}
