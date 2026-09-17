//! GLES/EGL render backend (architecture doc §2.6, §8.2).
//!
//! Phase 2 exit criteria (architecture doc §14):
//! 1. The Phase-1 client's window is visibly rendered on screen.
//! 2. Idle-frame-suppression verified: no GPU work during no-damage idle.
//!
//! `GlesBackend` owns the nested window's graphics backend (window + EGL
//! context + `GlesRenderer`), the Smithay output it presents to, and an
//! `OutputDamageTracker` for partial redraws. Frames flow through the
//! `RenderBackend` trait: `begin_frame` opens a `damage::PendingFrame`,
//! `mark_damage`/`composite_surface` fill it, and `present` either returns
//! `SkippedNoDamage` without touching the GPU at all (criterion 2) or
//! submits one composited frame (criterion 1).
//!
//! Surface registry: the `RenderBackend` trait identifies surfaces by opaque
//! `SurfaceHandle`, so the backend keeps a handle ↔ `WlSurface` map fed by
//! `attach_surface`. Stale entries are reaped by `prune_dead_surfaces`,
//! which the core calls once per frame — no per-client state accumulates.
//!
//! Assumptions (documented limits, not bugs): single output, output scale
//! factor applied uniformly when translating logical geometry to physical
//! pixels. Multi-output/scale handling is Phase 7 work.

use std::collections::HashMap;

use ryowm_common::{OutputId, Rect};
use smithay::{
    backend::{
        renderer::{
            damage::OutputDamageTracker,
            element::{AsRenderElements, surface::WaylandSurfaceRenderElement},
            gles::GlesRenderer,
        },
        winit::WinitGraphicsBackend,
    },
    desktop::space::SurfaceTree,
    output::{Mode, Output},
    reexports::wayland_server::{
        Resource,
        backend::ObjectId,
        protocol::wl_surface::WlSurface,
        DisplayHandle,
    },
    utils::{IsAlive, Physical, Point, Rectangle, Scale, Size},
};
use tracing::{debug, info, warn};

use super::{
    FrameToken, PresentResult, RenderBackend, SurfaceHandle,
    damage::PendingFrame,
};

/// Entry in the per-frame compositing queue, resolved to a concrete surface.
/// Already back-to-front ordered by `PendingFrame::take_submission`.
struct QueuedEntry {
    surface: WlSurface,
    geometry: Rect,
}

pub struct GlesBackend {
    backend: WinitGraphicsBackend<GlesRenderer>,
    output: Output,
    output_id: OutputId,
    tracker: OutputDamageTracker,
    surfaces: HashMap<ObjectId, (WlSurface, SurfaceHandle)>,
    handles: HashMap<SurfaceHandle, ObjectId>,
    next_handle: u64,
    frame: Option<PendingFrame>,
    next_token: u64,
}

impl GlesBackend {
    /// Take ownership of the nested window's graphics backend and the output
    /// it presents to. EGL `wl_display` binding (dmabuf import fast path) is
    /// intentionally left out: it needs Smithay's `use_system_lib` feature
    /// and SHM buffers cover Phase 2's clients. It arrives with the udev
    /// backend work, not here.
    pub fn new(
        backend: WinitGraphicsBackend<GlesRenderer>,
        output: Output,
        _display: &DisplayHandle,
    ) -> Self {
        info!("GLES backend created for output");
        let tracker = OutputDamageTracker::from_output(&output);
        Self {
            backend,
            output,
            output_id: OutputId(0),
            tracker,
            surfaces: HashMap::new(),
            handles: HashMap::new(),
            next_handle: 0,
            frame: None,
            next_token: 0,
        }
    }

    pub fn output_id(&self) -> OutputId {
        self.output_id
    }

    pub fn output(&self) -> &Output {
        &self.output
    }

    /// Logical extent of the output, used for full-repaint damage.
    pub fn output_extent(&self) -> Rect {
        self.output
            .current_mode()
            .map(|mode| Rect::new(0, 0, mode.size.w, mode.size.h))
            .unwrap_or_default()
    }

    /// Update the output mode after a nested-window resize. Damage marking
    /// stays with the caller (it owns the fullscreen-damage policy).
    pub fn set_output_size(&mut self, size: Size<i32, Physical>) {
        let mode = Mode {
            size,
            refresh: 60_000,
        };
        self.output.change_current_state(Some(mode), None, None, None);
        self.output.set_preferred(mode);
    }

    /// Register a client surface, returning its stable compositing handle.
    pub fn attach_surface(&mut self, surface: &WlSurface) -> SurfaceHandle {
        let key = surface.id();
        if let Some((_, handle)) = self.surfaces.get(&key) {
            return *handle;
        }
        let handle = SurfaceHandle(self.next_handle);
        self.next_handle += 1;
        self.surfaces.insert(key.clone(), (surface.clone(), handle));
        self.handles.insert(handle, key);
        handle
    }

    /// Drop registry entries whose clients are gone. Called once per frame
    /// by the core so dead surfaces never accumulate (Phase 1's no-leak bar,
    /// extended to renderer-side handles).
    pub fn prune_dead_surfaces(&mut self) {
        self.surfaces.retain(|_, (surface, handle)| {
            let alive = surface.alive();
            if !alive {
                self.handles.remove(handle);
            }
            alive
        });
    }

    fn resolve_queue(
        &self,
        queue: Vec<super::damage::QueuedSurface>,
    ) -> Vec<QueuedEntry> {
        queue
            .into_iter()
            .filter_map(|entry| {
                let key = self.handles.get(&entry.handle)?;
                let (surface, _) = self.surfaces.get(key)?;
                if !surface.alive() {
                    return None;
                }
                Some(QueuedEntry {
                    surface: surface.clone(),
                    geometry: entry.geometry,
                })
            })
            .collect()
    }
}

impl RenderBackend for GlesBackend {
    fn begin_frame(&mut self, _output: OutputId) -> FrameToken {
        if self.frame.is_some() {
            warn!("begin_frame called with a frame still open; dropping it");
        }
        let token = FrameToken(self.next_token);
        self.next_token += 1;
        self.frame = Some(PendingFrame::new(token));
        token
    }

    fn mark_damage(&mut self, frame: FrameToken, region: Rect) {
        match self.frame.as_mut() {
            Some(open) if open.token() == frame => open.mark_damage(region),
            _ => warn!("mark_damage for unknown frame token; ignoring"),
        }
    }

    fn composite_surface(
        &mut self,
        frame: FrameToken,
        surface: SurfaceHandle,
        geometry: Rect,
        z: u32,
    ) {
        match self.frame.as_mut() {
            Some(open) if open.token() == frame => open.push(surface, geometry, z),
            _ => warn!("composite_surface for unknown frame token; ignoring"),
        }
    }

    fn present(&mut self, frame: FrameToken) -> PresentResult {
        let pending = match self.frame.take() {
            Some(open) if open.token() == frame => open,
            _ => {
                warn!("present for unknown frame token");
                return PresentResult::Failed;
            }
        };
        let Some(submission) = pending.take_submission() else {
            // Idle-frame suppression: no damage was marked, so no GPU work
            // happens at all — no bind, no render, no submit.
            debug!("Skipping frame: no damage");
            return PresentResult::SkippedNoDamage;
        };

        // Resolve everything borrowing `self` before `bind()` hands out a
        // `&mut` borrow of the backend.
        let queue = self.resolve_queue(submission.queue);
        let scale_factor = self.output.current_scale().fractional_scale();
        let scale = Scale::from(scale_factor);
        // The bind/render/submit sequence borrows the backend mutably, so the
        // submitted damage is copied out as owned data first — nothing
        // referencing the backend outlives this block.
        let damage: Option<Vec<Rectangle<i32, Physical>>> = {
            let age = self.backend.buffer_age().unwrap_or(0);
            let (renderer, mut fb) = match self.backend.bind() {
                Ok(bound) => bound,
                Err(err) => {
                    warn!("Failed to bind GLES backend: {err}");
                    return PresentResult::Failed;
                }
            };

        let mut elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = Vec::new();
        for entry in &queue {
                let tree = SurfaceTree::from_surface(&entry.surface);
                let location = Point::<i32, Physical>::from((
                    (entry.geometry.x as f64 * scale_factor).round() as i32,
                    (entry.geometry.y as f64 * scale_factor).round() as i32,
                ));
            elements.extend(tree.render_elements(renderer, location, scale, 1.0));
        }

            match self.tracker.render_output(
                renderer,
                &mut fb,
                age,
                &elements,
                [0.1f32, 0.1, 0.12, 1.0],
            ) {
                Ok(rendered) => {
                    debug!(damage = ?rendered.damage, "Composited frame");
                    rendered.damage.cloned()
                }
                Err(err) => {
                    warn!("GLES render failed: {err}");
                    return PresentResult::Failed;
                }
            }
        };

        if let Err(err) = self.backend.submit(damage.as_deref()) {
            warn!("Failed to submit frame: {err}");
            return PresentResult::Failed;
        }
        PresentResult::Presented
    }
}
