# ADR-0003: Declarative tree-based layout model

## Status
Accepted

## Context
RyoWM supports tiling, scrolling (Niri-style), monocle, and floating layouts with runtime switching. Niri's scrolling layout uses an infinite horizontal strip coordinate model fundamentally different from dwm/i3-style recursive rectangle subdivision.

## Decision
Each workspace holds a `Box<dyn LayoutAlgorithm>` with a pure `compute` function. Floating windows are a separate per-workspace list, not routed through the trait. Layout switching at runtime = swapping the impl and recomputing. No shared geometry code between tiling-tree and scrolling — they share only the trait interface.

## Consequences
- Runtime layout switching is safe and simple (pure function swap).
- O(windows-on-output) recompute on every change is sub-microsecond at realistic window counts (<20).
- Each layout algorithm is independently testable and snapshot-testable.
- Floating coexistence is explicit per-window, never inferred.

## Reference
See RyoWM-architecture.md §2.5
