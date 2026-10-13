//! Durable local openside API daemon with a supervised Python agent sidecar.
//!
//! [`openside_proto::ROUTES`] defines the complete HTTP surface: sessions, durable
//! message queues, SSE events, capability devices and human-step tickets. Unix
//! clients are administrators; optional TCP is loopback-only and requires bearer
//! capabilities. Socket parents must be private 0700 directories and sockets 0600.
//!
//! Flags are `--config`, `--data-dir`, `--socket` and `--listen`. All runtime,
//! model, credential-source, policy and sandbox settings come from openside-config.
//! No default model is embedded: session creation without an explicit or configured
//! model returns `model_not_configured`. The daemon stores managed provider keys only
//! in the encrypted Vault and delivers them over the private authenticated sidecar socket;
//! no public API, configuration file, event or log may expose the values.
//!
//! One warm sidecar belongs to each active session. Message injection is committed
//! before runtime I/O for at-most-once delivery. Restart or process failure closes
//! incomplete injections as `failed/interrupted` without resending them. Queued
//! messages remain durable, and lazy restart restores the latest run-end snapshot.
//! Runtime authentication failure permanently stops the session (fail closed).
//!
//! Human steps always defer through durable tickets; pending tickets hold every
//! queue lane. Resolution resumes the same tool call, including across restart.
//! Expiry is checked every second and on startup. Explicit session stop aborts the
//! run and rejects pending tickets; daemon shutdown preserves pending tickets.
//! `secret_input` is sent only to a trusted SecretSink and zeroized, never persisted
//! or delivered to the sidecar. Without a registered sink it returns HTTP 501;
//! takeover streams the ticket's tab over a WebSocket and locks agent tools out of it.
//! Browser tools lazily launch one discovered Chromium in a daemon-owned isolated
//! profile. Private endpoint, session tab ownership and document-bound ticket node
//! handles survive restart; pending browser tickets retain Chromium at shutdown.
//! Without pending browser tickets, graceful shutdown closes and reaps Chromium.
//! Secret insertion checks the exact live origin and document before filling.
//! Device approval checks URL/visible selector/visible text every two seconds;
//! manual conditions are resolved only through the ticket done endpoint.
//! Vault unlock commands use argv without a shell and zeroize output; unavailable
//! vaults hide their tools. Vault redaction covers every public event and tool result.
//! Sensitive browser targets use a conservative accessible-name/role substring
//! heuristic and always ask, even in Full mode. Typed text is logged as length only.
//! Screenshot observations reach vision models without artifact copies; only explicit
//! output captures become user files. Secret-filled tabs cannot be captured.
//!
//! Daemon-owned tools run through visibility, validation, canonical-target policy,
//! ticket approval, redaction, a 32 KiB model-output limit, events and a private
//! metadata-only audit JSONL. ReadOnly hides mutation/exec/sensitive tools. Bash uses
//! bubblewrap when available, with workspace read-write, system directories
//! read-only and network unshared; setup failure never falls back unsandboxed.
//! `sandbox.required` hides bash when bubblewrap is absent. Optional absence is
//! reported on stderr and as nonfatal `runtime.error{sandbox_unavailable}` at open.
//!
//! Routines (`openside-sched`) live in `routines.db` beside the event store. A one-second
//! tick fires due routines through the same queue path as `POST /v1/sessions/{id}/messages`;
//! start-up records occurrences missed while down instead of replaying them. See
//! `docs/specs/routines.md`. `DELETE /v1/sessions/{id}` stops a session, appends a durable
//! `session.deleted` tombstone and hides it; `session_delete::on_session_deleted` is the
//! internal hook that pauses routines targeting the session.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod access_requests;
mod accounts;
mod agents;
mod api;
mod artifacts;
mod browser;
mod browser_state;
mod browser_tabs_api;
mod card_media;
mod cards;
mod channels_api;
mod config;
mod connections;
mod crash;
#[cfg(target_os = "linux")]
mod credential_mounts;
mod credential_paths;
#[doc(hidden)]
pub mod diag;
mod doctor_api;
mod engine;
mod favicons;
mod host;
mod host_calls;
mod import_api;
mod import_delivery;
mod inhibit;
mod lifecycle;
mod managed_subscriptions;
mod memory;
mod memory_model;
mod model_accounts;
mod model_broker;
mod model_subscriptions_api;
mod models_api;
mod onboarding;
mod operator;
mod otp_ingest;
mod pairing;
mod persistence;
mod person_only;
mod policy_api;
mod preview;
mod profiles;
mod prompt;
mod push;
mod remote;
mod routines;
mod runtime;
mod session_delete;
mod session_meta;
mod settings_api;
mod skills;
mod subagents;
mod sync;
mod takeover;
mod tasks;
mod team;
mod tickets;
mod tool_output;
mod tools;
mod ui;
mod vault_api;
mod ws_stream;

#[cfg(all(test, unix))]
mod tests;

pub use browser::browser_event;
pub use config::Config;
pub use engine::Daemon;
pub use lifecycle::capture_restart_marker;

/// Parse process arguments and serve until SIGINT or SIGTERM.
pub async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use clap::Parser;
    let daemon = Daemon::open(Config::parse()).await?;
    crash::install(daemon.shared.config.data_dir.clone());
    daemon.serve().await
}
