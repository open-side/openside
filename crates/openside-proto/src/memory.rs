use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Origin of a memory: agent/user notes or automatically indexed browsing history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// Written by the agent (`memory_write`) or a user (API/CLI).
    Note,
    /// Indexed page visit: public URL (no query/fragment/userinfo), title, short text.
    History,
}

impl MemoryKind {
    /// Wire and storage spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::History => "history",
        }
    }
}

/// How long a memory is kept after its last write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Retention {
    /// Never purged automatically.
    #[serde(rename = "permanent")]
    Permanent,
    /// Purged 90 days after the last write.
    #[serde(rename = "days_90")]
    Days90,
    /// Purged 30 days after the last write.
    #[serde(rename = "days_30")]
    Days30,
    /// Short-lived context, purged 24 hours after the last write.
    #[serde(rename = "hours_24")]
    Hours24,
}

impl Retention {
    /// Wire and storage spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Permanent => "permanent",
            Self::Days90 => "days_90",
            Self::Days30 => "days_30",
            Self::Hours24 => "hours_24",
        }
    }

    /// Lifetime in days, or none for permanent memories.
    pub const fn days(self) -> Option<i64> {
        match self {
            Self::Permanent => None,
            Self::Days90 => Some(90),
            Self::Days30 => Some(30),
            Self::Hours24 => Some(1),
        }
    }
}

/// One stored memory. Content never contains a value matched by secret detection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Memory {
    /// Opaque memory identifier.
    pub id: String,
    /// Null means shared; otherwise this memory belongs to one teammate.
    #[serde(default)]
    pub owner: Option<String>,
    /// Memory origin.
    pub kind: MemoryKind,
    /// Remembered text.
    pub content: String,
    /// Free-form labels.
    pub tags: Vec<String>,
    /// Retention policy.
    pub retention: Retention,
    /// Public page URL, for history memories.
    pub url: Option<String>,
    /// Page title, for history memories.
    pub title: Option<String>,
    /// First write.
    pub created_at: DateTime<Utc>,
    /// Last write; retention counts from here.
    pub updated_at: DateTime<Utc>,
    /// Purge deadline; absent for permanent memories.
    pub expires_at: Option<DateTime<Utc>>,
    /// Current version number, starting at one.
    pub version: u32,
}

/// Search strategy in effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemorySearchMode {
    /// FTS5 bm25 and local-embedding cosine, fused by reciprocal rank.
    Hybrid,
    /// FTS5 bm25 only, because no local embedding model is loaded.
    LexicalOnly,
}

/// Which index produced a search hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryMatch {
    /// Full-text match only.
    Lexical,
    /// Embedding similarity only.
    Vector,
    /// Both indexes.
    Both,
    /// No query: most recently updated memories.
    Recent,
}

/// Ranked memory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryHit {
    /// The memory.
    pub memory: Memory,
    /// Fused rank score; larger is better and only comparable within one response.
    pub score: f64,
    /// Index that matched.
    pub matched: MemoryMatch,
}

/// Response of `GET /v1/memory`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MemorySearchResponse {
    /// Strategy used for this query.
    pub mode: MemorySearchMode,
    /// Hits, best first.
    pub hits: Vec<MemoryHit>,
}

/// `POST /v1/memory` body; creates a note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateMemoryRequest {
    /// Text to remember; rejected with `secret_detected` when it looks like a secret.
    pub content: String,
    /// Optional labels.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Defaults to `memory.default_retention`.
    pub retention: Option<Retention>,
}

/// `PATCH /v1/memory/{id}` body; absent fields are unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateMemoryRequest {
    /// Replacement text.
    pub content: Option<String>,
    /// Replacement labels.
    pub tags: Option<Vec<String>>,
    /// Replacement retention; the purge deadline restarts from this write.
    pub retention: Option<Retention>,
}

/// Change recorded in memory history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryOp {
    /// First write.
    Create,
    /// Edit.
    Update,
    /// Explicit deletion.
    Delete,
    /// Removal by the retention job.
    Purge,
}

/// One history entry. After delete/purge every earlier snapshot is scrubbed and
/// only operation, actor and timestamp remain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryVersion {
    /// Version number this change produced.
    pub version: u32,
    /// Operation.
    pub op: MemoryOp,
    /// Who made the change, e.g. `api`, `agent:<session>`, `retention`.
    pub actor: String,
    /// When the change happened.
    pub at: DateTime<Utc>,
    /// Content after the change; absent once scrubbed.
    pub content: Option<String>,
    /// Labels after the change; absent once scrubbed.
    pub tags: Option<Vec<String>>,
    /// Retention after the change.
    pub retention: Option<Retention>,
}

/// Response of `GET /v1/memory/{id}/history`, oldest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryHistory {
    /// Memory identifier.
    pub id: String,
    /// Changes, oldest first.
    pub versions: Vec<MemoryVersion>,
}

/// Response of `GET /v1/memory/health`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryHealth {
    /// Search strategy in effect.
    pub mode: MemorySearchMode,
    /// Content-derived identifier of the loaded local embedding model.
    pub model_id: Option<String>,
    /// Why embeddings are unavailable, without file contents.
    pub problem: Option<String>,
    /// Action that enables hybrid search.
    pub next_step: Option<String>,
    /// Live (unexpired) memories.
    pub count: u64,
}
