//! Per-frame damage and surface-queue state.
//!
//! Phase 2 exit criteria (architecture doc §14):
//! 1. The Phase-1 client's window is visibly rendered on screen.
//! 2. Idle-frame-suppression verified: no GPU work during no-damage idle.
//!
//! This module holds the backend-independent half of that contract: damage
//! unioning and the present/skip decision. It performs no GPU work and has
//! no Smithay dependency, so the idle-skip contract is unit-testable here
//! while `gles::GlesBackend` only executes GL after `take_submission`
//! returns `Some`.

use super::{FrameToken, SurfaceHandle};
use ryowm_common::Rect;

/// A surface queued for compositing within one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueuedSurface {
    pub handle: SurfaceHandle,
    pub geometry: Rect,
    pub z: u32,
}

/// The frame `GlesBackend` is currently assembling. Created by `begin_frame`,
/// filled via `mark_damage`/`composite_surface`, consumed by `present`.
#[derive(Debug)]
pub struct PendingFrame {
    token: FrameToken,
    damage: Option<Rect>,
    queue: Vec<QueuedSurface>,
}

/// A frame approved for GPU submission, in back-to-front order.
#[derive(Debug)]
pub struct FrameSubmission {
    pub damage: Rect,
    pub queue: Vec<QueuedSurface>,
}

impl PendingFrame {
    pub fn new(token: FrameToken) -> Self {
        Self {
            token,
            damage: None,
            queue: Vec::new(),
        }
    }

    pub fn token(&self) -> FrameToken {
        self.token
    }

    /// Union a dirty region into this frame's damage.
    pub fn mark_damage(&mut self, region: Rect) {
        self.damage = Some(match self.damage {
            Some(current) => current.union(&region),
            None => region,
        });
    }

    /// Queue a surface for compositing. No GPU work happens here.
    pub fn push(&mut self, handle: SurfaceHandle, geometry: Rect, z: u32) {
        self.queue.push(QueuedSurface {
            handle,
            geometry,
            z,
        });
    }

    /// Consume the frame. Returns `None` when no damage was ever marked —
    /// the caller must skip all GPU work (idle-frame suppression).
    /// The queue comes out front-to-back (highest `z` first), matching
    /// Smithay's `render_output` element order.
    pub fn take_submission(mut self) -> Option<FrameSubmission> {
        let damage = self.damage?;
        self.queue.sort_by_key(|entry| std::cmp::Reverse(entry.z));
        Some(FrameSubmission {
            damage,
            queue: self.queue,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_frame_submits_nothing() {
        let frame = PendingFrame::new(FrameToken(0));
        assert!(frame.take_submission().is_none());
    }

    #[test]
    fn queued_surfaces_without_damage_still_skip() {
        let mut frame = PendingFrame::new(FrameToken(1));
        frame.push(SurfaceHandle(7), Rect::new(0, 0, 10, 10), 0);
        assert!(frame.take_submission().is_none());
    }

    #[test]
    fn damage_unions_across_marks() {
        let mut frame = PendingFrame::new(FrameToken(2));
        frame.mark_damage(Rect::new(0, 0, 10, 10));
        frame.mark_damage(Rect::new(20, 20, 10, 10));
        let sub = frame.take_submission().expect("damage was marked");
        assert_eq!(sub.damage, Rect::new(0, 0, 30, 30));
    }

    #[test]
    fn queue_is_sorted_front_to_back() {
        let mut frame = PendingFrame::new(FrameToken(3));
        frame.mark_damage(Rect::new(0, 0, 100, 100));
        frame.push(SurfaceHandle(1), Rect::new(0, 0, 10, 10), 5);
        frame.push(SurfaceHandle(2), Rect::new(0, 0, 10, 10), 1);
        let sub = frame.take_submission().expect("damage was marked");
        let zs: Vec<u32> = sub.queue.iter().map(|entry| entry.z).collect();
        assert_eq!(zs, vec![5, 1]);
    }
}
