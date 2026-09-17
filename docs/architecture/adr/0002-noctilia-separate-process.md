# ADR-0002: Noctilia as a separate process over persistent IPC

## Status
Accepted

## Context
Noctilia is a shell/bar UI with its own rendering and event loop. Embedding it in the compositor process violates the blast-radius rule: any Noctilia bug becomes a compositor crash bringing down every window.

## Decision
Noctilia runs as a separate process communicating over a persistent Unix domain socket using the general IPC/API subsystem. It is not embedded, dynamically linked, or loaded as a plugin.

## Consequences
- Crash isolation: Noctilia dying does not affect compositor operation.
- Independent release cadence for Noctilia and the compositor.
- IPC cost is irrelevant at human-interaction rates (bar updates, clock ticks).
- Matches how every mature Wayland bar ecosystem works (waybar, etc.).

## Reference
See RyoWM-architecture.md §2.4
