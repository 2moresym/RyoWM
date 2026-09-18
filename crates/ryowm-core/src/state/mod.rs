//! CompositorState: the single owning root for windows, workspaces,
//! outputs, and scene graph (architecture doc §8.1). Every subsystem gets
//! either &mut access during its turn on the main-thread event dispatch,
//! or a read-only snapshot for cross-thread use. No subsystem holds a
//! duplicate mutable copy of anything owned here — see engineering rule
//! §15.12 in the architecture doc.
//!
//! Phase 4 fills in the window half: lifecycle mode, focus order, and
//! placement state. Workspaces/outputs stay placeholders until Phase 7.
//! The scene itself (`Space<Window>`) lives in `RyoWmState`; this map is
//! the lifecycle/focus authority for the same windows. Invariant: every
//! mapped `Window` has exactly one `WindowState` and vice versa —
//! maintained by updating both in the same handler (map/unmap/destroy).

use std::collections::HashMap;

use ryowm_common::{OutputId, WindowId, WorkspaceId};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

use crate::window::{WindowMode, cascade_offset};

/// Lifecycle record for one managed window. Geometry lives here as the
/// authoritative placement; the scene (`Space`) mirrors it for rendering.
/// Both are written in the same handler on map/commit/move, so they cannot
/// drift — there is deliberately no second geometry cache anywhere else.
#[derive(Debug, Clone)]
pub struct WindowState {
    pub id: WindowId,
    pub surface: WlSurface,
    pub mode: WindowMode,
}

pub struct WorkspaceState;
pub struct OutputState;

#[allow(dead_code)] // workspace/output counters used in later phases
pub struct CompositorState {
    pub windows: HashMap<WindowId, WindowState>,
    pub workspaces: HashMap<WorkspaceId, WorkspaceState>,
    pub outputs: HashMap<OutputId, OutputState>,
    /// Most-recent-first focus order; the front is keyboard-focus fallback.
    pub focus_stack: Vec<WindowId>,
    /// Currently focused window, if any.
    pub focused: Option<WindowId>,
    next_window_id: u64,
    next_workspace_id: u64,
    next_output_id: u64,
    placement_seed: u64,
}

impl CompositorState {
    pub fn new() -> Self {
        Self {
            windows: HashMap::new(),
            workspaces: HashMap::new(),
            outputs: HashMap::new(),
            focus_stack: Vec::new(),
            focused: None,
            next_window_id: 0,
            next_workspace_id: 0,
            next_output_id: 0,
            placement_seed: 0,
        }
    }

    pub fn alloc_window_id(&mut self) -> WindowId {
        let id = WindowId(self.next_window_id);
        self.next_window_id += 1;
        id
    }

    /// Record keyboard focus. Pure id bookkeeping — the seat focus switch
    /// itself happens at the call site.
    pub fn focus_window(&mut self, id: WindowId) {
        self.focused = Some(id);
        self.focus_stack.retain(|known| *known != id);
        self.focus_stack.insert(0, id);
    }

    /// Drop all lifecycle records for `id`. Returns the window keyboard
    /// focus should fall back to (`None` = nothing left to focus).
    pub fn forget_window(&mut self, id: WindowId) -> Option<WindowId> {
        self.windows.remove(&id);
        self.focus_stack.retain(|known| *known != id);
        if self.focused == Some(id) {
            self.focused = self.focus_stack.first().copied();
        }
        self.focused
    }

    /// Lifecycle record lookup by client surface.
    pub fn window_id_for_surface(&self, surface: &WlSurface) -> Option<WindowId> {
        self.windows
            .iter()
            .find_map(|(id, state)| (state.surface == *surface).then_some(*id))
    }

    /// Next cascade placement offset. Monotonic (never reuses an offset
    /// until wraparound) so closed windows don't disturb the pattern.
    pub fn next_cascade_location(&mut self) -> (i32, i32) {
        let seed = self.placement_seed;
        self.placement_seed = self.placement_seed.wrapping_add(1);
        cascade_offset(seed)
    }
}

impl Default for CompositorState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_stack_is_most_recent_first() {
        let mut state = CompositorState::new();
        let (a, b, c) = (WindowId(1), WindowId(2), WindowId(3));
        state.focus_window(a);
        state.focus_window(b);
        state.focus_window(c);
        assert_eq!(state.focus_stack, vec![c, b, a]);
        // Refocusing moves to front without duplicating.
        state.focus_window(a);
        assert_eq!(state.focus_stack, vec![a, c, b]);
        assert_eq!(state.focused, Some(a));
    }

    #[test]
    fn forget_falls_back_to_most_recent() {
        let mut state = CompositorState::new();
        let (a, b) = (WindowId(1), WindowId(2));
        state.focus_window(a);
        state.focus_window(b);
        assert_eq!(state.forget_window(b), Some(a));
        assert_eq!(state.forget_window(a), None);
        assert!(state.focus_stack.is_empty());
    }

    #[test]
    fn forget_unknown_window_changes_nothing() {
        let mut state = CompositorState::new();
        let a = WindowId(1);
        state.focus_window(a);
        assert_eq!(state.forget_window(WindowId(99)), Some(a));
        assert_eq!(state.focused, Some(a));
    }

    #[test]
    fn cascade_cycles_without_reuse() {
        let mut state = CompositorState::new();
        let first: Vec<(i32, i32)> = (0..13).map(|_| state.next_cascade_location()).collect();
        assert_eq!(first[0], (0, 0));
        assert_eq!(first[1], (32, 32));
        assert_eq!(first[12], (0, 0));
    }
}
