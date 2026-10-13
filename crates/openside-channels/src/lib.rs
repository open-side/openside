//! Chat-platform adapters for openside (Telegram, Slack, Discord).
//!
//! Every adapter is a client of the public daemon API v1 and owns no session logic:
//! [`Core`] maps a conversation thread to a session, streams assistant output from
//! `GET /v1/sessions/{id}/events`, turns `ticket.opened` events into approval
//! buttons, and implements `/new /stop /model /sessions /usage /answer`. The
//! platform modules only translate between the platform's wire format and
//! [`Inbound`]/[`Press`]/[`Messenger`]. See `docs/specs/channels.md`.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod allow;
mod core;
mod daemon;
/// Discord Interactions endpoint adapter.
pub mod discord;
mod error;
mod http;
mod messenger;
mod model;
mod runner;
mod secret;
/// Slack Events API adapter.
pub mod slack;
mod sse;
mod store;
mod stt;
/// Telegram long-polling adapter.
pub mod telegram;
mod text;
mod watch;

pub use allow::AllowList;
pub use core::{Core, CoreOptions};
pub use daemon::{check_public, DaemonClient, EventStream};
pub use error::{ChannelError, Result};
pub use messenger::Messenger;
pub use model::{AudioRef, Button, Inbound, MsgRef, Press, ThreadKey};
pub use runner::{
    check, daemon_client, legacy_secrets, pair, process_env, start, start_account, state_dir,
    test_account, AccountSecrets, EnvFn, Running,
};
pub use secret::{resolve as resolve_secret, Secret};
pub use store::{Card, Store, ThreadState};
pub use stt::Stt;
