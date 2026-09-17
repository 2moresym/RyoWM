# Dependency decisions

Recorded before adding crate manifests; versions and features follow the Phase 0/1 build brief §0. Workspace declarations for future phases are reservations, not permission to implement those phases.

| Dependency | Justification |
|---|---|
| smithay 0.7.0 | Tagged compositor primitives for protocol correctness and nested winit startup; required DRM/GBM/libinput/udev/libseat/EGL/GLES/XWayland feature declarations are retained without implementing later phases. |
| wayland-server 0.31 | Own the Wayland display and accept/dispatch client connections. |
| wayland-protocols 0.32 | Reserved standard Wayland extension bindings, compatible with Smithay. |
| calloop 0.14 | Main-thread event reactor compatible with Smithay 0.7. |
| tracing 0.1 | Structured startup, protocol lifecycle, and error logs. |
| tracing-subscriber 0.3 (env-filter) | Console logging and environment-controlled filtering. |
| tracing-journald 0.3 | Reserved journald logging integration per architecture §9. |
| zbus 5 | Reserved pure-Rust D-Bus integration; no desktop-service implementation in Phase 0/1. |
| serde 1 (derive) | Serialize shared IDs, geometry, and the placeholder IPC envelope without compositor dependencies. |
| toml 0.8 | Reserved configuration parsing per architecture §9. |
| thiserror 2 | Reserved structured subsystem error types. |
| anyhow 1 | Propagate startup/reactor and build-task failures with context. |
| libdrm, libgbm, EGL/GLES | System libraries required by the brief's Smithay backend features. |
| libinput, libudev, libseat | System device/session libraries required by the brief's Smithay backend features. |
| libwayland, libxkbcommon | Wayland/EGL transport and keyboard support used by Smithay and its nested backend. |

No Vulkan feature or extra external crate is introduced in Phase 0/1. Internal path dependencies maintain the common → protocol/render → core boundaries. `xtask` uses only the standard library.

## Phase 3 additions (`ryowm-core`)

| Dependency | Justification |
|---|---|
| rtrb 0.4 | Lock-free SPSC ring buffer for the input-thread → main-thread event channel (architecture §5: channel-based handoff, never shared-mutex state). |
| input 0.9 (default features, i.e. udev) | Direct libinput bindings with udev seat enumeration for the dedicated input thread. Smithay's re-export of `input` disables default features (no udev), so `new_with_udev` is unavailable through it — a direct dependency is required. Version pinned to the 0.9.1 already in the tree. |
| xcursor 0.3 | Parse the system cursor theme for the visible pointer image instead of hand-rolling Xcursor parsing. Version pinned to the 0.3.11 already in the tree. |

## Phase 2 additions (`ryowm-render`)

| Dependency | Justification |
|---|---|
| smithay 0.7.0 (for `ryowm-render`) | `gles::GlesBackend` drives Smithay's `GlesRenderer`/`OutputDamageTracker` directly; reusing the toolkit's GL abstraction instead of hand-rolling EGL. |
| tracing 0.1 (for `ryowm-render`) | Frame present/skip/failure logs from the render backend (failure behavior per architecture §3). |
