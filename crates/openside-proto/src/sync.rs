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
    /// Shared directory or WebDAV URL of the configured backend; never credentials.
    #[serde(default)]
    pub location: Option<String>,
    /// WebDAV username, when the share needs one.
    #[serde(default)]
    pub webdav_username: Option<String>,
    /// Whether the live policy document is synced.
    #[serde(default)]
    pub settings: bool,
    /// Memories with any of these tags are synced.
    #[serde(default)]
    pub memory_tags: Vec<String>,
    /// Where the sync passphrase comes from.
    #[serde(default)]
    pub passphrase_source: SyncSecretSource,
    /// Where the WebDAV password comes from; absent without a WebDAV username.
    #[serde(default)]
    pub password_source: Option<SyncSecretSource>,
    /// Counts of the last run since the daemon started, conflicts included.
    #[serde(default)]
    pub last_run: Option<SyncReport>,
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

/// How a sync secret reaches the daemon.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SyncSecretSource {
    /// Read by running the command configured in `[sync]`; nothing to type.
    Command,
    /// Typed in the app and kept in daemon memory only: it must be entered again after the
    /// daemon restarts.
    #[default]
    Entered,
}

/// Secrets typed in the app for `setup`, `pair` and `unlock`. They stay in daemon memory,
/// are never written to disk or returned, and need the computer's local access.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SyncSecrets {
    /// Sync passphrase; used instead of `sync.passphrase_cmd` when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passphrase: Option<String>,
    /// WebDAV password; used instead of `sync.webdav_password_cmd` when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webdav_password: Option<String>,
}

/// `PUT /v1/sync/config`: choose where synced data is stored. Exactly one of `dir` and
/// `webdav_url`, or neither to turn sync off. The backend of a device that already joined
/// an account cannot change; `settings` and `memory_tags` always can.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateSyncConfigRequest {
    /// Shared directory synced by other means.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    /// WebDAV collection URL; https, or http on loopback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webdav_url: Option<String>,
    /// WebDAV username; its password is typed on unlock or read from `webdav_password_cmd`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webdav_username: Option<String>,
    /// Sync the live policy document.
    #[serde(default)]
    pub settings: bool,
    /// Memories with any of these tags are synced.
    #[serde(default)]
    pub memory_tags: Vec<String>,
}

/// `POST /v1/sync/pair`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PairSyncRequest {
    /// Code shown by the first device's sync setup.
    pub pairing_code: String,
    /// Sync passphrase typed in the app; needs the computer's local access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passphrase: Option<String>,
    /// WebDAV password typed in the app; needs the computer's local access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webdav_password: Option<String>,
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
