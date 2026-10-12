//! Version 1 wire contract for openside clients and the daemon.
//!
//! [`ROUTES`] is the complete HTTP route table. [`openapi`] generates an OpenAPI
//! 3.1 document directly from the serde/JSON Schema types; schemas are also available
//! independently through [`schemas`]. IDs are opaque JSON strings. Timestamps are
//! RFC 3339 UTC, and [`Event::seq`] starts at one independently for each session.
//! [`EventKind`] is internally tagged by `kind` and flattened into the event envelope.
//!
//! SSE uses `text/event-stream`, the decimal sequence as its `id`, and serialized
//! [`Event`] JSON as its `data`. `Last-Event-ID` is the last *seen* sequence; resume
//! returns only events after it. Stream consumers must deduplicate by session and seq.
//! All event/tool/ticket content must be redacted before it reaches this contract.
//! `message.injected` records at-most-once dispatch intent before runtime I/O;
//! `message.completed` reports [`MessageOutcome`] after dispatch, including a failed
//! interrupted dispatch. It does not promise exactly-once external tool effects.
//!
//! [`Ticket`] persists human intervention for a suspended tool call. Resolution,
//! rejection, and expiry are terminal; secret-input values remain daemon-only.
//! Idle sessions with pending tickets report [`SessionStatus::WaitingHuman`].
//! `now` records a durable steering message before injection. Resolving a ticket
//! resumes the same tool call without adding a user message.
//! Capabilities are explicit per route. Unix-socket callers are implicitly `admin`
//! (the local user), allowing the first device to be paired; network callers must
//! supply a capability bearer token. Returned pairing tokens must never be logged.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod accounts;
mod api;
mod artifacts;
mod browser_tabs;
mod events;
mod memory;
mod openapi;
mod policy;
mod preview;
mod remote;
mod repl;
mod routines;
mod session_meta;
mod skills;
mod subagents;
mod sync;
mod takeover;
mod vault;

pub use accounts::*;
pub use api::*;
pub use artifacts::*;
pub use browser_tabs::*;
pub use events::*;
pub use memory::*;
pub use openapi::{openapi, schemas, HttpMethod, Route, ROUTES};
pub use openside_core::{
    ContentKind, DropReason, MessageId, MessageStatus, QueueMode, SessionId, TicketId,
};
pub use policy::*;
pub use preview::*;
pub use remote::*;
pub use repl::*;
pub use routines::*;
pub use session_meta::*;
pub use skills::*;
pub use subagents::*;
pub use sync::*;
pub use takeover::*;
pub use vault::*;

#[cfg(test)]
mod tests;
