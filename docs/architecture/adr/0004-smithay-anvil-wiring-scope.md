# ADR-0004: Smithay anvil wiring scope for Phase 1 protocol bootstrapping

## Status
Accepted

## Context
Phase 1 needs correct `wl_compositor`/`wl_shm`/`xdg_wm_base` globals plus the
calloop integration (display dispatch, auto socket). Smithay's anvil example
already solves exactly this protocol-correctness plumbing, per architecture
doc §9 (build on Smithay, don't re-solve protocol handling).

## Decision
Follow anvil's `state.rs`/`winit.rs` structurally for protocol bootstrapping
only: `Display` as a `calloop::generic::Generic` source with the
`unsafe { display.get_mut().dispatch_clients(data) }` pattern,
`ListeningSocketSource::new_auto` for the socket,
`CompositorState`/`ShmState`/`XdgShellState`/`SeatState` globals, and the
`delegate_*` macros. Do not carry over anvil's window management, layout,
rendering, input, or IPC code — all RyoWM architecture in §2–§8 stays ours.

## Consequences
- Client-visible protocol behavior matches a proven-correct reference.
- The `unsafe` block is confined to the display-dispatch closure and only
  avoids dropping the polled fd (documented at the call site).
- Reviewers can diff our `wayland/mod.rs` against anvil to confirm the
  borrowed-plumbing vs original-architecture boundary.

## Reference
See RyoWM-architecture.md §9 and Smithay anvil (`Smithay/smithay`, `anvil/`).
