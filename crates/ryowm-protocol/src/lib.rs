//! Wire protocol for the IPC/API subsystem (architecture doc §8.6-8.8).
//! Shared by ryowm-core (server side), the future ryowm-settings GUI, and
//! ryowm-ctl. NOT implemented until Phase 10 — this stub exists so the
//! crate boundary and dependency graph are correct from Phase 0.

use serde::{Deserialize, Serialize};

/// Placeholder top-level message envelope. Real request/response/event
/// variants are added in Phase 10 per architecture doc §8.6.
#[derive(Debug, Serialize, Deserialize)]
pub enum Message {
    Ping,
    Pong,
}

// Deliberately no client/server code yet — Phase 10 adds the actual
// Unix-domain-socket framing (length-prefixed bincode or JSON, decide in
// Phase 10 based on the debugging-friendliness tradeoff, not here).
