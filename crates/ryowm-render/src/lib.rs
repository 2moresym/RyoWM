//! Renderer abstraction (architecture doc §2.6, §8.2). GLES/EGL backend is
//! the required implementation (Phase 2); Vulkan backend is optional and
//! not started before Phase 12.

use ryowm_common::{OutputId, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameToken(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentResult {
    Presented,
    SkippedNoDamage,
    Failed,
}

/// Opaque handle to a client's committed buffer contents, as tracked by the
/// scene graph. Real definition lands in Phase 2 alongside the first
/// RenderBackend impl.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SurfaceHandle(pub u64);

pub trait RenderBackend {
    /// Begin composing a frame for the given output.
    fn begin_frame(&mut self, output: OutputId) -> FrameToken;

    /// Mark a region as needing redraw.
    fn mark_damage(&mut self, frame: FrameToken, region: Rect);

    /// Queue a surface for compositing at the given geometry and z-order.
    fn composite_surface(&mut self, frame: FrameToken, surface: SurfaceHandle, geometry: Rect, z: u32);

    /// Submit the frame. Must return `SkippedNoDamage` if no damage was
    /// marked for this frame token.
    fn present(&mut self, frame: FrameToken) -> PresentResult;
}

// No impl in this crate yet. Phase 2 adds `gles::GlesBackend`.
