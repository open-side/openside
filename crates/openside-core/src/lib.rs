//! Core domain model for openside.
//!
//! Everything here is runtime-agnostic: no async executor, no I/O. The session queue is a
//! pure state machine so its invariants can be tested exhaustively (requirements A-4,
//! PP-10, PP-24).
//!
//! IDs serialize as opaque strings. Queue modes and message dispositions have stable
//! serde/JSON Schema representations shared with `openside-proto`; the queue itself
//! is restored by replaying operations, not by exposing its internal collections.
//!
//! The runtime boundary exposes opaque run-end snapshots, deferred tool calls, and
//! a daemon-owned tool handler; credentials and human secrets never become tool results.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod content;
pub mod ids;
pub mod queue;
pub mod runtime;

pub use content::{ContentKind, ContentPart};
pub use ids::{MessageId, SessionId, TicketId};
pub use queue::{DropReason, MessageQueue, MessageStatus, QueueError, QueueMode, SubmitOutcome};
pub use runtime::{
    AgentRuntime, AgentSession, AgentState, ApprovalTier, DeferKind, DeferredCall, HostToolHandler,
    RunOutcome, RuntimeError, RuntimeEvent, SessionInfo, SessionSpec, SubagentControl,
    TicketResolution, ToolCall, ToolOutcome, ToolSpec,
};
