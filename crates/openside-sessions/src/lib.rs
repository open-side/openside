//! Synchronous, event-sourced SQLite persistence for openside.
//!
//! [`Store::open`] creates or reopens the database and replays all committed events.
//! [`Store::create_session`] commits `session.created` at sequence one. [`Store::append`]
//! accepts a public [`openside_proto::EventKind`] and returns the committed event;
//! timestamps and per-session, gapless sequences are assigned inside the transaction.
//! Events are immutable, protected by SQLite UPDATE/DELETE rejection triggers. The
//! payload, run id, and envelope fields are stored together in one transaction.
//!
//! Queue commands are `message.queued`, `edited`, `promoted`, `moved`, `dropped`, and `injected`
//! appends. Replay feeds these into [`openside_core::MessageQueue`]. Invalid edits,
//! removals, or out-of-order/duplicate injections fail without consuming a sequence.
//! Repeated submissions return the original queued event—even if the message is now
//! delivered or dropped—preserving MessageId idempotency across process restarts.
//! `message.injected` must name the delivery head, with steering before follow-up.
//! Committing an injection records its disposition; the caller performs runtime I/O.
//! Exactly-once external effects require runtime-side idempotency and are not promised
//! by the SQLite transaction alone.
//! `message.completed` requires a prior injection and rejects duplicate completions; only a
//! `deferred` completion (the run paused on a ticket) may later settle to a terminal outcome.
//! [`SessionState::injected`] and [`SessionState::completions`] distinguish terminal
//! results from interrupted dispatches without rescanning event payloads.
//!
//! [`Store::events_after`] supports SSE resume using the last seen sequence. [`Store::replay`]
//! rebuilds all [`SessionState`] metadata, ticket lifecycles, pending queue order,
//! edited texts, and terminal message history solely from events. Stopping a session
//! preserves its pending queue. `session` and `list_sessions` use a committed cache;
//! `replay` refreshes other-connection writes, while `append` automatically reconciles
//! its target session under an IMMEDIATE transaction. Failed replay preserves the cache.
//! `session.deleted` is a tombstone: the log is kept, but `session` and `list_sessions`
//! stop reporting the session and every later append is rejected; `events_after` still
//! serves it so live subscribers see the tombstone. [`Store::queued_messages`] returns
//! recent queued user messages of live sessions for repetition analysis.
//!
//! Ticket creation requires a unique pending ticket belonging to its session;
//! approval deferrals require a redacted invocation. Only pending tickets resolve,
//! reject, or expire. Public idle sessions with pending tickets derive `waiting_human`;
//! the underlying turn state remains idle, and queue dispatch is the daemon's concern.
//! [`Store::put_agent_state`] upserts the latest opaque run-end snapshot separately
//! from the immutable event log; [`Store::agent_state`] reads it after restart.
//! Daemon-private approval digests and consumed markers bind human authorization to
//! an exact invocation without exposing full arguments in public ticket events.
//! Terminal resolution delivery is durably marked before runtime I/O; an uncertain
//! delivery is not replayed as authorization after a crash.
//!
//! The async daemon should own a store on a dedicated thread or use `spawn_blocking`;
//! no async executor is required here. Database parent directories must exist. WAL,
//! FULL synchronization, and transaction commit provide durability without a separate
//! close method. Redact secrets before submitting events; this crate cannot infer them.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod state;
mod store;

pub use state::SessionState;
pub use store::{ApprovalClaim, QueuedMessage, Store};

/// Storage, replay, or command-validation failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// SQLite open, transaction, query, or durability failure.
    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Event JSON could not be encoded.
    #[error("event JSON encoding failed: {0}")]
    Json(#[from] serde_json::Error),
    /// The target session has no creation event.
    #[error("unknown session {0}")]
    UnknownSession(openside_core::SessionId),
    /// Creation was attempted for an existing session.
    #[error("session already exists: {0}")]
    SessionExists(openside_core::SessionId),
    /// An invalid transition or corrupt log was rejected.
    #[error("invalid event: {0}")]
    InvalidEvent(String),
    /// SQLite's signed sequence range was exhausted.
    #[error("session event sequence exhausted")]
    SequenceExhausted,
}

/// Result of a synchronous store operation.
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests;
