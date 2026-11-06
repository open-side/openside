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
mod agents;
mod api;
mod artifacts;
mod browser_prefs;
mod browser_tabs;
mod cards;
mod channels;
mod cli_defaults;
mod computer_use;
mod connections;
mod events;
mod mcp;
mod memory;
mod models;
mod oauth;
mod onboarding;
mod openapi;
mod operator;
mod person_browser;
mod person_native;
mod policy;
mod preferences;
mod preview;
mod projects;
mod remote;
mod repl;
mod routines;
mod session_meta;
mod shares;
mod skills;
mod stt;
mod subagents;
mod sync;
mod takeover;
mod tool_api;
mod vault;

pub use accounts::*;
pub use agents::*;
pub use api::*;
pub use artifacts::*;
pub use browser_prefs::*;
pub use browser_tabs::*;
pub use cards::*;
pub use channels::*;
pub use cli_defaults::*;
pub use computer_use::*;
pub use connections::*;
pub use events::*;
pub use mcp::*;
pub use memory::*;
pub use models::*;
pub use oauth::*;
pub use onboarding::*;
pub use openapi::{openapi, schemas, HttpMethod, Route, ROUTES};
pub use openside_core::{
    ContentKind, DropReason, MessageId, MessageStatus, QueueMode, SessionId, TicketId,
};
pub use operator::*;
pub use person_browser::*;
pub use person_native::*;
pub use policy::*;
pub use preferences::*;
pub use preview::*;
pub use projects::*;
pub use remote::*;
pub use repl::*;
pub use routines::*;
pub use session_meta::*;
pub use shares::*;
pub use skills::*;
pub use stt::*;
pub use subagents::*;
pub use sync::*;
pub use takeover::*;
pub use tool_api::*;
pub use vault::*;

#[cfg(test)]
mod tests;
