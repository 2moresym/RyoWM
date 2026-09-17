//! Layout algorithms (architecture doc §2.5). Each workspace holds a
//! `Box<dyn LayoutAlgorithm>`; switching modes at runtime = swapping which
//! impl is boxed and recomputing.

use ryowm_common::{Rect, WindowId};

pub trait LayoutAlgorithm: std::fmt::Debug {
    fn compute(&self, windows: &[WindowId], viewport: Rect) -> Vec<(WindowId, Rect)>;
    fn name(&self) -> &'static str;
}

// No concrete impls yet.
//   - Phase 5 adds `tiling_tree::MasterStackLayout`
//   - Phase 5 adds `monocle::MonocleLayout`
//   - Phase 11 adds `scrolling::ScrollingLayout`
