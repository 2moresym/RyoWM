//! Reactor: owns the calloop event loop and its shutdown signal.
//!
//! This is the concrete home for architecture doc §4's event sources —
//! the Wayland display/socket sources (see `wayland::RyoWmState::new`), the
//! winit backend source (see `main`), and later phases' additions, all arrive
//! via calloop. The steady-state loop blocks in the kernel until a source
//! fires instead of polling: idle CPU near zero is a structural property of
//! this design, not a tuning parameter (architecture doc §6).

use std::sync::atomic::Ordering;

use calloop::{EventLoop, LoopHandle, LoopSignal};

use crate::wayland::RyoWmState;

pub struct Reactor {
    event_loop: EventLoop<'static, RyoWmState>,
    // Reserved for clean-shutdown signalling; wired to signal handling in a
    // later phase (no OS signal source is registered yet).
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

    /// Steady-state loop: block until an event source fires, dispatch it,
    /// then run per-wakeup work (damage-driven rendering lives in the
    /// caller's closure, not here — the reactor only waits and dispatches).
    pub fn run(
        mut self,
        state: &mut RyoWmState,
        mut on_wakeup: impl FnMut(&mut RyoWmState) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        while state.running.load(Ordering::SeqCst) {
            // No timeout: park in epoll until a source is ready. An idle
            // compositor sleeps here instead of spinning.
            self.event_loop.dispatch(None, state)?;
            on_wakeup(state)?;
        }
        Ok(())
    }
}
