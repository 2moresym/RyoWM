//! Reactor: owns the calloop event loop and its shutdown signal.
//!
//! This is the concrete home for architecture doc §4's event sources —
//! Phase 1 registers the Wayland display and socket sources here (see
//! `wayland::RyoWmState::new`); later phases add the rest.

use std::time::Duration;

use calloop::{EventLoop, LoopHandle, LoopSignal};

use crate::wayland::RyoWmState;

pub struct Reactor {
    event_loop: EventLoop<'static, RyoWmState>,
    // Reserved for clean-shutdown signalling; wired to signal handling in a
    // later phase (no OS signal source is registered in Phase 1).
    #[allow(dead_code)]
    signal: LoopSignal,
}

impl Reactor {
    pub fn try_new() -> anyhow::Result<Self> {
        let event_loop = EventLoop::try_new()?;
        let signal = event_loop.get_signal();
        Ok(Self { event_loop, signal })
    }

    pub fn handle(&self) -> LoopHandle<'static, RyoWmState> {
        self.event_loop.handle()
    }

    /// Pump one round of registered event sources. Phase 1's main loop uses
    /// this (instead of `run`) so winit backend events — which are pumped
    /// manually via `dispatch_new_events`, not as a calloop source — can be
    /// interleaved with calloop dispatch each iteration.
    pub fn dispatch(&mut self, state: &mut RyoWmState) -> anyhow::Result<()> {
        self.event_loop
            .dispatch(Some(Duration::from_millis(1)), state)?;
        Ok(())
    }

    /// Steady-state loop for phases where every backend is a calloop source.
    /// Unused in Phase 1 (winit is pumped manually in `main`); kept as the
    /// standing event-loop entry point per the build brief §2.1 step 1.
    #[allow(dead_code)]
    pub fn run(mut self, state: &mut RyoWmState) -> anyhow::Result<()> {
        loop {
            self.dispatch(state)?;
        }
    }
}
