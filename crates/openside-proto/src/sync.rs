//! Pinned tabs and end-to-end encrypted device sync (`/v1/pinned-tabs`, `/v1/sync/*`) of
//! settings, pinned tabs, vault items and tagged memories.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A tab the user pinned; synced between devices when sync is set up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PinnedTab {
    /// Stable id derived from the URL, identical on every device.
    pub id: String,
    /// http(s) URL.
    pub url: String,
    /// Optional display title.
    pub title: Option<String>,
    /// When it was pinned (on the device that pinned it).
    pub pinned_at: DateTime<Utc>,
}

/// `GET /v1/pinned-tabs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PinnedTabList {
    /// Pinned tabs, oldest first.
    pub tabs: Vec<PinnedTab>,
}

/// `POST /v1/pinned-tabs`. Pinning an already pinned URL updates its title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreatePinnedTabRequest {
    /// http(s) URL.
    pub url: String,
    /// Optional display title (at most 512 characters).
    #[serde(default)]
    pub title: Option<String>,
}

/// Configured sync backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SyncBackendKind {
    /// WebDAV share (`sync.webdav_url`).
    Webdav,
    /// Shared directory (`sync.dir`).
    Dir,
}

/// `GET /v1/sync`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SyncStatus {
    /// Whether `[sync]` names a backend.
    pub configured: bool,
    /// The configured backend.
    pub backend: Option<SyncBackendKind>,
    /// Whether this device joined an account (`setup` or `pair`).
    pub set_up: bool,
    /// Whether the sync key is derived and held in memory.
    pub unlocked: bool,
    /// Account id (not secret).
    pub account_id: Option<String>,
    /// This device's id.
    pub device_id: Option<String>,
    /// Last completed sync.
    pub last_sync: Option<DateTime<Utc>>,
    /// Records known to this device, tombstones included.
    pub records: u64,
    /// Local changes not yet stored on the backend.
    pub pending: u64,
    /// The sync counter reached its ceiling: runs fail with `sync_counter_exhausted` and
    /// local changes stay pending until a new account is set up.
    #[serde(default)]
    pub exhausted: bool,
}

/// `POST /v1/sync/setup`: a new account. Give `pairing_code` (and the same passphrase)
/// to the other device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SyncSetupResponse {
    /// `osync1-…` code: account id plus a passphrase check; not a secret on its own.
    pub pairing_code: String,
    /// State after setup.
    pub status: SyncStatus,
}

/// `POST /v1/sync/pair`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PairSyncRequest {
    /// Code printed by `openside sync setup` on the first device.
    pub pairing_code: String,
}

/// `POST /v1/sync/run`: counts only, never record contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SyncReport {
    /// Records stored on the backend.
    pub pushed: u32,
    /// Remote records authenticated and decrypted.
    pub pulled: u32,
    /// Remote versions applied locally.
    pub applied: u32,
    /// Local unsynced edits replaced by a newer remote version (conflict rules).
    pub conflicts: u32,
    /// Remote records that failed authentication or validation and were not applied.
    pub rejected: u32,
    /// Remote records waiting because their kind is unavailable here (locked vault,
    /// settings sync off).
    pub deferred: u32,
    /// Whether vault items took part in this run.
    pub vault_available: bool,
}
