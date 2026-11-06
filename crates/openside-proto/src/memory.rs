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

/// A file-shaped view of the memory store (Settings › Memory › Overview). The files do not
/// exist on disk; the daemon renders them from stored notes and parses edits back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryView {
    /// `MEMORY.md`: notes about work, decisions and context (every note not tagged `profile`).
    Memory,
    /// `USER.md`: facts about the person (notes tagged `profile`).
    User,
    /// `TAXONOMY.md`: tags and kinds with counts; read-only.
    Taxonomy,
}

impl MemoryView {
    /// Every view in display order.
    pub const ALL: [Self; 3] = [Self::Memory, Self::User, Self::Taxonomy];

    /// File name shown to the person and used in the API path.
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::Memory => "MEMORY.md",
            Self::User => "USER.md",
            Self::Taxonomy => "TAXONOMY.md",
        }
    }

    /// View for a file name, matched exactly.
    pub fn from_file_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|view| view.file_name() == name)
    }
}

/// Listing entry of `GET /v1/memory/files`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryFileInfo {
    /// `MEMORY.md`, `USER.md` or `TAXONOMY.md`.
    pub name: String,
    /// Which projection this file is.
    pub view: MemoryView,
    /// Whether `PUT /v1/memory/files/{name}` accepts edits.
    pub editable: bool,
    /// Notes rendered into the file (for `TAXONOMY.md`, distinct tags).
    pub items: u32,
    /// Latest write among the rendered notes; absent when the file is empty.
    pub updated_at: Option<DateTime<Utc>>,
}

/// Response of `GET /v1/memory/files`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryFileList {
    /// The three files in display order.
    pub files: Vec<MemoryFileInfo>,
}

/// Response of `GET /v1/memory/files/{name}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryFile {
    /// File name.
    pub name: String,
    /// Which projection this file is.
    pub view: MemoryView,
    /// Whether edits are accepted.
    pub editable: bool,
    /// Latest write among the rendered notes.
    pub updated_at: Option<DateTime<Utc>>,
    /// Change-log position this rendering reflects; send it back with the edit.
    pub revision: i64,
    /// Markdown. In editable files each note starts with a `<!-- memory id=… -->` marker
    /// line; keep markers to edit a note, delete a note's block to delete it, and add text
    /// under a bare `<!-- memory -->` marker (or before the first marker) to add notes. An
    /// optional `tags="a, b"` in a marker sets labels.
    pub markdown: String,
}

/// `PUT /v1/memory/files/{name}` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateMemoryFileRequest {
    /// `revision` of the `GET` the edit started from.
    pub revision: i64,
    /// The edited markdown.
    pub markdown: String,
}

/// Response of `PUT /v1/memory/files/{name}`: the re-rendered file and what changed. All
/// changes apply in one transaction; a note edited elsewhere since the `GET` fails the whole
/// save with `memory_conflict` (409).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryFileSaved {
    /// The file after saving.
    pub file: MemoryFile,
    /// Notes added.
    pub created: u32,
    /// Notes changed.
    pub updated: u32,
    /// Notes deleted.
    pub deleted: u32,
}

/// One entry of the global memory change log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryChange {
    /// Log position; pass as `before` to page to older entries.
    pub seq: i64,
    /// Memory the change belongs to.
    pub memory_id: String,
    /// Version this change produced.
    pub version: u32,
    /// Operation.
    pub op: MemoryOp,
    /// Who made the change.
    pub actor: String,
    /// When the change happened.
    pub at: DateTime<Utc>,
    /// Kind of the memory, while it still exists.
    pub kind: Option<MemoryKind>,
    /// Content after the change; absent once scrubbed by deletion or retention.
    pub content: Option<String>,
    /// Labels after the change; absent once scrubbed.
    pub tags: Option<Vec<String>>,
}

/// Response of `GET /v1/memory/history`, newest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryChangeList {
    /// Changes, newest first.
    pub changes: Vec<MemoryChange>,
    /// `before` value for the next page; absent on the last page.
    pub next_before: Option<i64>,
}

/// Response of `POST /v1/memory/rebuild`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryRebuild {
    /// Live memories whose full-text index entry was rebuilt.
    pub reindexed: u64,
    /// Memories that received a fresh embedding (zero without a local model).
    pub embedded: u64,
    /// Search status after the rebuild.
    pub health: MemoryHealth,
}
