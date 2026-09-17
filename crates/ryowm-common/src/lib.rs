//! Shared types used across every RyoWM crate. No compositor dependencies.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WindowId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OutputId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkspaceId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self { x, y, width, height }
    }

    pub fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0
    }

    pub fn union(&self, other: &Rect) -> Rect {
        if self.is_empty() { return *other; }
        if other.is_empty() { return *self; }
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = (self.x + self.width).max(other.x + other.width);
        let y1 = (self.y + self.height).max(other.y + other.height);
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction { Up, Down, Left, Right }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_union_of_disjoint_rects_covers_both() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(20, 20, 10, 10);
        assert_eq!(a.union(&b), Rect::new(0, 0, 30, 30));
    }

    #[test]
    fn rect_union_with_empty_returns_other() {
        let a = Rect::default();
        let b = Rect::new(5, 5, 5, 5);
        assert_eq!(a.union(&b), b);
    }
}
