# ADR-0001: GLES/EGL as required primary renderer, Vulkan optional

## Status
Accepted

## Context
RyoWM targets Intel HD 4000 class hardware where Mesa removed Ivy Bridge ANV Vulkan support in Mesa 22.3. Software Vulkan (llvmpipe) is strictly worse than hardware-accelerated GLES for a compositor's render backend.

## Decision
GLES 3.x via EGL is the primary/required renderer. Vulkan is an optional, separately-compiled backend selected at runtime only when a non-software Vulkan device is enumerated.

## Consequences
- Enables hardware-accelerated rendering on all target hardware including HD 4000.
- Vulkan backend remains available on modern GPUs that support it well.
- Renderer abstraction adds one vtable dispatch per frame-level call (noise overhead).
- No Vulkan feature or dependency is introduced until Phase 12+.

## Reference
See RyoWM-architecture.md §2.6
