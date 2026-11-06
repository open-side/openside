//! Read-only share links: a person publishes a frozen, privacy-filtered snapshot of one chat
//! that anyone holding the link can read without a token (`docs/specs/sharing.md`).

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::SessionId;

/// Characters of a share id: 32 random bytes, unpadded base64url.
pub const SHARE_ID_LEN: usize = 43;
/// Artifacts one snapshot may carry.
pub const MAX_SHARE_ARTIFACTS: usize = 20;
/// Live (unexpired, unrevoked) links one chat may have.
pub const MAX_SHARES_PER_SESSION: usize = 50;
/// Shortest lifetime a link may be given.
pub const MIN_SHARE_LIFETIME_SECS: u64 = 60;
/// Longest lifetime a link may be given; links without one last until revoked.
pub const MAX_SHARE_LIFETIME_SECS: u64 = 365 * 24 * 60 * 60;

fn yes() -> bool {
    true
}

/// Publish a snapshot of the chat as it is now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateShareRequest {
    /// Artifact paths from `GET /v1/sessions/{id}/artifacts` to copy into the snapshot, in
    /// the order given; none by default. The transcript and working files cannot be shared.
    #[serde(default)]
    pub artifacts: Vec<String>,
    /// Include one line per step the agent took (the tool used, never its input or output).
    #[serde(default = "yes")]
    pub steps: bool,
    /// Seconds until the link stops working; omitted or null keeps it until revoked.
    #[serde(default)]
    #[schemars(range(min = 60, max = 31536000))]
    pub expires_in_secs: Option<u64>,
}

/// One published link, as its owner sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ShareLink {
    /// Unguessable id; whoever holds it can read the snapshot until it is revoked or expires.
    pub id: String,
    /// Chat the snapshot was taken from.
    pub session_id: SessionId,
    /// Chat title when the snapshot was taken.
    pub title: String,
    /// When the snapshot was taken.
    pub created_at: DateTime<Utc>,
    /// When the link stops working; null when it works until revoked.
    pub expires_at: Option<DateTime<Utc>>,
    /// Full link through remote access (Tailscale, Cloudflare or `remote.public_url`); null
    /// when none is configured, so the link opens only on this computer at `path`.
    pub url: Option<String>,
    /// App path of the read-only page, `/share#<id>`; the id stays in the fragment so it is
    /// never sent in a Referer header.
    pub path: String,
    /// Messages in the snapshot, the person's and the agent's.
    pub messages: u32,
    /// Whether step summaries were included.
    pub steps: bool,
    /// Artifacts copied into the snapshot.
    pub artifacts: Vec<SharedArtifact>,
}

/// Live links of one chat, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ShareLinkList {
    /// Unexpired, unrevoked links.
    pub shares: Vec<ShareLink>,
}

/// The public, read-only snapshot behind a link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SharedChat {
    /// Chat title when the snapshot was taken.
    pub title: String,
    /// When the snapshot was taken; later messages are not part of it.
    pub shared_at: DateTime<Utc>,
    /// When the link stops working; null when it works until revoked.
    pub expires_at: Option<DateTime<Utc>>,
    /// Conversation in order.
    pub items: Vec<SharedItem>,
    /// Files the person chose to include, fetched from
    /// `GET /v1/shares/{share_id}/artifacts/{artifact}`.
    pub artifacts: Vec<SharedArtifact>,
}

/// One entry of a shared conversation. Text has live secrets masked as `[REDACTED]`, and a
/// line that still looks like a credential is replaced whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SharedItem {
    /// A message the agent received.
    User {
        /// Message text.
        text: String,
        /// When it was delivered.
        at: DateTime<Utc>,
    },
    /// A reply of the agent.
    Assistant {
        /// Reply text (Markdown).
        text: String,
        /// When it was written.
        at: DateTime<Utc>,
    },
    /// Steps the agent took between two messages.
    Steps {
        /// In order.
        steps: Vec<SharedStep>,
    },
}

/// One step: which tool ran, never its arguments or result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SharedStep {
    /// Registered tool name.
    pub tool: String,
    /// Whether the step failed.
    pub failed: bool,
}

/// A file copied into a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SharedArtifact {
    /// Position in the snapshot; the `{artifact}` of its download route.
    pub index: u32,
    /// File name, without directories.
    pub name: String,
    /// MIME type.
    pub mime: String,
    /// Bytes.
    pub size: u64,
}
