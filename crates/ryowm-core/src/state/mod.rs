//! CompositorState: the single owning root for windows, workspaces,
//! outputs, and scene graph (architecture doc §8.1). Every subsystem gets
//! either &mut access during its turn on the main-thread event dispatch,
//! or a read-only snapshot for cross-thread use.

use ryowm_common::{OutputId, WindowId, WorkspaceId};
use std::collections::HashMap;

#[allow(dead_code)] // reserved counters used in later phases
pub struct CompositorState {
    pub windows: HashMap<WindowId, WindowState>,
    pub workspaces: HashMap<WorkspaceId, WorkspaceState>,
    pub outputs: HashMap<OutputId, OutputState>,
    next_window_id: u64,
    next_workspace_id: u64,
    next_output_id: u64,
}

pub struct WindowState;
pub struct WorkspaceState;
pub struct OutputState;

impl CompositorState {
    pub fn new() -> Self {
        Self {
            windows: HashMap::new(),
            workspaces: HashMap::new(),
            outputs: HashMap::new(),
            next_window_id: 0,
            next_workspace_id: 0,
            next_output_id: 0,
        }
    }

    pub fn alloc_window_id(&mut self) -> WindowId {
        let id = WindowId(self.next_window_id);
        self.next_window_id += 1;
        id
    }
}

impl Default for CompositorState {
    fn default() -> Self {
        Self::new()
    }
}
