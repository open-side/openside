//! Local long-term memory for openside agents.
//!
//! One SQLite database holds memories, an FTS5 index and a change-history table.
//! Search fuses FTS5 bm25 ranks with cosine ranks from an optional local
//! [`Embedder`] by reciprocal rank fusion; without an embedder it is lexical-only.
//! Every write is scanned for secrets ([`looks_secret`] plus a caller-supplied
//! known-secret probe) and rejected with [`Error::SecretDetected`] before any byte
//! reaches the database. Retention counts from the last write; expired memories are
//! hidden immediately and removed by [`MemoryStore::purge_expired`]. Delete and purge
//! scrub every stored snapshot, leaving only operation, actor and time in history.
//! This crate performs no network I/O and its dependency graph has no network client.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod embed;
mod secrets;
mod sync;

use std::{collections::HashMap, fmt, path::Path, sync::Arc};

use chrono::{DateTime, TimeZone, Utc};
use openside_proto::{
    Memory, MemoryHistory, MemoryHit, MemoryKind, MemoryMatch, MemoryOp, MemorySearchMode,
    MemorySearchResponse, MemoryVersion, Retention, UpdateMemoryRequest,
};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};

pub use embed::{Embedder, ModelError, StaticEmbedder, MODEL_FILE, TOKENIZER_FILE};
pub use secrets::looks_secret;

/// Reciprocal-rank-fusion constant (Cormack et al.); damps the head of each list.
pub const RRF_K: f64 = 60.0;
/// Vector-only candidates below this cosine are treated as unrelated.
pub const MIN_COSINE: f32 = 0.3;
/// Candidates taken from each index before fusion.
const CANDIDATES: usize = 50;
/// Upper bound for one search response.
pub const MAX_LIMIT: usize = 100;
/// Upper bound for memory content, in bytes.
pub const MAX_CONTENT: usize = 16 * 1024;
const MAX_TAGS: usize = 32;
const MAX_TAG: usize = 64;
const MAX_TITLE: usize = 512;
const MAX_URL: usize = 2048;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Time source; tests inject a fake clock to exercise retention deterministically.
pub trait Clock: Send + Sync {
    /// Current time.
    fn now(&self) -> DateTime<Utc>;
}

/// Wall clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Probe for secrets the caller knows at runtime (e.g. unlocked vault values).
pub type KnownSecret = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Store configuration.
pub struct Options {
    /// Local embedding model; none means lexical-only search.
    pub embedder: Option<Arc<dyn Embedder>>,
    /// Time source.
    pub clock: Arc<dyn Clock>,
    /// Retention for writes that do not choose one.
    pub default_retention: Retention,
    /// Additional secret probe, consulted for every written field.
    pub known_secret: Option<KnownSecret>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            embedder: None,
            clock: Arc::new(SystemClock),
            default_retention: Retention::Permanent,
            known_secret: None,
        }
    }
}

/// Memory operation failure. Messages never contain memory content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// No live memory has this identifier.
    NotFound,
    /// A written field looked like a secret; nothing was stored.
    SecretDetected,
    /// A field violated a documented limit.
    Invalid(&'static str),
    /// SQLite failure.
    Storage(String),
}

impl Error {
    /// Stable API error code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::SecretDetected => "secret_detected",
            Self::Invalid(_) => "invalid_value",
            Self::Storage(_) => "internal_error",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("memory not found"),
            Self::SecretDetected => f.write_str("content looks like a secret and was not stored"),
            Self::Invalid(reason) => f.write_str(reason),
            Self::Storage(reason) => write!(f, "memory storage failed: {reason}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(err: rusqlite::Error) -> Self {
        Self::Storage(err.to_string())
    }
}

/// Result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Fields of a new memory.
#[derive(Debug, Clone)]
pub struct NewMemory {
    /// Memory origin.
    pub kind: MemoryKind,
    /// Text to remember.
    pub content: String,
    /// Labels.
    pub tags: Vec<String>,
    /// Retention; [`Options::default_retention`] when absent.
    pub retention: Option<Retention>,
    /// Public page URL.
    pub url: Option<String>,
    /// Page title.
    pub title: Option<String>,
}

impl NewMemory {
    /// A note with default retention.
    pub fn note(content: impl Into<String>) -> Self {
        Self {
            kind: MemoryKind::Note,
            content: content.into(),
            tags: Vec::new(),
            retention: None,
            url: None,
            title: None,
        }
    }
}

/// A recall record retains its author without granting that author instruction authority.
#[derive(Debug, Clone)]
pub struct ContextMemory {
    /// Live, ownership-filtered record selected for recall.
    pub memory: Memory,
    /// Last writer, retained as data rather than instruction authority.
    pub actor: String,
}

/// Only a daemon-issued scope can authorize edits to previously shown versions.
#[derive(Debug, Clone)]
pub struct ConsolidationEdit {
    /// Identifier that must occur in the daemon-issued shown set.
    pub id: String,
    /// Exact shown version; concurrent edits invalidate the operation.
    pub version: u32,
    /// None deletes; some replaces only the text, preserving ownership and provenance.
    pub content: Option<String>,
}

/// SQLite-backed memory store; all methods are blocking and thread-safe.
pub struct MemoryStore {
    conn: Mutex<Connection>,
    embedder: Option<Arc<dyn Embedder>>,
    clock: Arc<dyn Clock>,
    default_retention: Retention,
    known_secret: Option<KnownSecret>,
}

const COLUMNS: &str = "m.id, m.kind, m.content, m.tags, m.retention, m.url, m.title, \
     m.created_at, m.updated_at, m.expires_at, m.version, m.owner";
const LIVE: &str = "(m.expires_at IS NULL OR m.expires_at > ?1)";

impl MemoryStore {
    /// Open or create the database at `path` and backfill vectors for the current model.
    pub fn open(path: &Path, options: Options) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = DELETE;
             PRAGMA secure_delete = ON;
             CREATE TABLE IF NOT EXISTS memories(
                 id TEXT NOT NULL UNIQUE, kind TEXT NOT NULL, content TEXT NOT NULL,
                 tags TEXT NOT NULL, retention TEXT NOT NULL, url TEXT, title TEXT,
                 created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, expires_at INTEGER,
                 version INTEGER NOT NULL, embedding BLOB, embedding_model TEXT);
             CREATE INDEX IF NOT EXISTS memories_expiry ON memories(expires_at)
                 WHERE expires_at IS NOT NULL;
             CREATE UNIQUE INDEX IF NOT EXISTS memories_history_url ON memories(url)
                 WHERE kind = 'history';
             CREATE TABLE IF NOT EXISTS memory_versions(
                 seq INTEGER PRIMARY KEY AUTOINCREMENT, memory_id TEXT NOT NULL,
                 version INTEGER NOT NULL, op TEXT NOT NULL, actor TEXT NOT NULL,
                 at INTEGER NOT NULL, content TEXT, tags TEXT, retention TEXT);
             CREATE INDEX IF NOT EXISTS memory_versions_id ON memory_versions(memory_id, seq);
             CREATE TABLE IF NOT EXISTS sync_ids(
                 sync_id TEXT PRIMARY KEY, memory_id TEXT NOT NULL UNIQUE);",
        )?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS consolidation_scopes(
                 id TEXT PRIMARY KEY, session TEXT NOT NULL, owner TEXT, shown TEXT NOT NULL,
                 expires_at INTEGER NOT NULL, applied INTEGER NOT NULL DEFAULT 0);",
        )?;
        let has_owner = conn
            .prepare("PRAGMA table_info(memories)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter()
            .any(|column| column == "owner");
        if !has_owner {
            conn.execute_batch("ALTER TABLE memories ADD COLUMN owner TEXT;")?;
        }
        let fts_exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'memories_fts')",
            [],
            |row| row.get(0),
        )?;
        if !fts_exists {
            conn.execute_batch(
                "CREATE VIRTUAL TABLE memories_fts USING fts5(
                     title, content, tags, tokenize = 'unicode61 remove_diacritics 2');",
            )?;
        }
        // Re-asserted on every open so databases created by any build erase FTS postings.
        conn.execute_batch(
            "INSERT INTO memories_fts(memories_fts, rank) VALUES('secure-delete', 1);",
        )?;
        let store = Self {
            conn: Mutex::new(conn),
            embedder: options.embedder,
            clock: options.clock,
            default_retention: options.default_retention,
            known_secret: options.known_secret,
        };
        store.backfill()?;
        Ok(store)
    }

    /// Search strategy in effect.
    pub fn mode(&self) -> MemorySearchMode {
        if self.embedder.is_some() {
            MemorySearchMode::Hybrid
        } else {
            MemorySearchMode::LexicalOnly
        }
    }

    /// Identifier of the loaded embedding model.
    pub fn model_id(&self) -> Option<String> {
        self.embedder.as_ref().map(|e| e.id().to_owned())
    }

    /// Live memory count.
    pub fn count(&self) -> Result<u64> {
        let now = self.now_ms();
        let conn = self.conn.lock();
        let count: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM memories m WHERE {LIVE}"),
            [now],
            |row| row.get(0),
        )?;
        Ok(u64::try_from(count).unwrap_or_default())
    }

    /// Store a new memory after validation and secret screening.
    pub fn create(&self, new: NewMemory, actor: &str) -> Result<Memory> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = self.now_ms();
        purge(&tx, now)?;
        let memory = self.insert(&tx, new, actor, now)?;
        tx.commit()?;
        Ok(memory)
    }

    /// Ownership is committed with the memory, never exposed briefly as shared.
    pub fn create_owned(&self, new: NewMemory, actor: &str, owner: Option<&str>) -> Result<Memory> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = self.now_ms();
        purge(&tx, now)?;
        let mut memory = self.insert(&tx, new, actor, now)?;
        tx.execute(
            "UPDATE memories SET owner = ?2 WHERE id = ?1",
            params![memory.id, owner],
        )?;
        memory.owner = owner.map(str::to_owned);
        tx.commit()?;
        Ok(memory)
    }

    /// Compare-and-set makes cross-store saga replay safe without stealing a later transfer.
    pub fn transfer_owner(&self, id: &str, from: Option<&str>, to: Option<&str>) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Retention or an explicit deletion wins over an interrupted ownership transfer.
        let Some(memory) = fetch(&tx, id, self.now_ms())? else {
            return Ok(());
        };
        if memory.owner.as_deref() != to {
            if memory.owner.as_deref() != from {
                return Err(Error::Invalid("memory ownership changed"));
            }
            tx.execute(
                "UPDATE memories SET owner = ?2 WHERE id = ?1",
                params![id, to],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Lifecycle ownership enumeration is not a ranked search and must not truncate.
    pub fn owned_ids(&self, owner: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock();
        let mut query = conn.prepare(&format!(
            "SELECT m.id FROM memories m WHERE {LIVE} AND m.owner = ?2 ORDER BY m.id"
        ))?;
        let ids = query
            .query_map(params![self.now_ms(), owner], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(ids)
    }

    fn insert(&self, tx: &Connection, new: NewMemory, actor: &str, now: i64) -> Result<Memory> {
        let retention = new.retention.unwrap_or(self.default_retention);
        let retention = if new.tags.iter().any(|tag| tag == "scratch") {
            Retention::Hours24
        } else {
            retention
        };
        // Screen raw input first so a secret is never reported as a mere format error.
        self.screen(
            &new.content,
            &new.tags,
            new.title.as_deref(),
            new.url.as_deref(),
            actor,
        )?;
        let tags = clean_tags(new.tags)?;
        validate(
            &new.content,
            new.title.as_deref(),
            new.url.as_deref(),
            actor,
        )?;
        let memory = Memory {
            id: new_id(),
            owner: None,
            kind: new.kind,
            content: new.content,
            tags,
            retention,
            url: new.url,
            title: new.title,
            created_at: to_time(now),
            updated_at: to_time(now),
            expires_at: expiry(retention, now).map(to_time),
            version: 1,
        };
        let embedding = self.embedding(&memory);
        tx.execute(
            "INSERT INTO memories(id, kind, content, tags, retention, url, title, created_at,
                 updated_at, expires_at, version, embedding, embedding_model)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9, 1, ?10, ?11)",
            params![
                memory.id,
                memory.kind.as_str(),
                memory.content,
                tags_json(&memory.tags),
                retention.as_str(),
                memory.url,
                memory.title,
                now,
                expiry(retention, now),
                embedding.as_ref().map(|(_, blob)| blob),
                embedding.as_ref().map(|(model, _)| model),
            ],
        )?;
        let rowid = tx.last_insert_rowid();
        index(tx, rowid, &memory)?;
        record(tx, &memory, MemoryOp::Create, actor, now)?;
        Ok(memory)
    }

    /// Fetch one live memory.
    pub fn get(&self, id: &str) -> Result<Memory> {
        let now = self.now_ms();
        let conn = self.conn.lock();
        fetch(&conn, id, now)?.ok_or(Error::NotFound)
    }

    /// Apply a partial edit; retention restarts from this write. Read, patch, version
    /// increment and history append happen in one locked transaction, so concurrent edits
    /// of different fields are serialized and never lost.
    pub fn update(&self, id: &str, patch: UpdateMemoryRequest, actor: &str) -> Result<Memory> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = self.now_ms();
        purge(&tx, now)?;
        let mut memory = fetch(&tx, id, now)?.ok_or(Error::NotFound)?;
        if let Some(content) = patch.content {
            memory.content = content;
        }
        if let Some(tags) = patch.tags {
            if tags.iter().any(|tag| self.secret(tag)) {
                return Err(Error::SecretDetected);
            }
            memory.tags = clean_tags(tags)?;
        }
        if let Some(retention) = patch.retention {
            memory.retention = retention;
        }
        let memory = self.rewrite(&tx, memory, actor, now)?;
        tx.commit()?;
        Ok(memory)
    }

    fn rewrite(
        &self,
        tx: &Connection,
        mut memory: Memory,
        actor: &str,
        now: i64,
    ) -> Result<Memory> {
        self.screen(
            &memory.content,
            &memory.tags,
            memory.title.as_deref(),
            memory.url.as_deref(),
            actor,
        )?;
        validate(
            &memory.content,
            memory.title.as_deref(),
            memory.url.as_deref(),
            actor,
        )?;
        memory.updated_at = to_time(now);
        if memory.tags.iter().any(|tag| tag == "scratch") {
            memory.retention = Retention::Hours24;
        }
        memory.expires_at = expiry(memory.retention, now).map(to_time);
        memory.version += 1;
        let embedding = self.embedding(&memory);
        let rowid: i64 = tx.query_row(
            "SELECT rowid FROM memories WHERE id = ?1",
            [&memory.id],
            |row| row.get(0),
        )?;
        tx.execute(
            "UPDATE memories SET content = ?2, tags = ?3, retention = ?4, title = ?5,
                 updated_at = ?6, expires_at = ?7, version = ?8, embedding = ?9,
                 embedding_model = ?10
             WHERE rowid = ?1",
            params![
                rowid,
                memory.content,
                tags_json(&memory.tags),
                memory.retention.as_str(),
                memory.title,
                now,
                expiry(memory.retention, now),
                memory.version,
                embedding.as_ref().map(|(_, blob)| blob),
                embedding.as_ref().map(|(model, _)| model),
            ],
        )?;
        tx.execute("DELETE FROM memories_fts WHERE rowid = ?1", [rowid])?;
        index(tx, rowid, &memory)?;
        record(tx, &memory, MemoryOp::Update, actor, now)?;
        Ok(memory)
    }

    /// Delete a live memory, scrubbing its stored snapshots; returns the removed memory.
    pub fn delete(&self, id: &str, actor: &str) -> Result<Memory> {
        check_actor(actor)?;
        if self.secret(actor) {
            return Err(Error::SecretDetected);
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = self.now_ms();
        purge(&tx, now)?;
        let found = fetch(&tx, id, now)?.ok_or(Error::NotFound)?;
        remove(&tx, &found, MemoryOp::Delete, actor, now)?;
        tx.commit()?;
        Ok(found)
    }

    /// Change history, oldest first, including deleted memories. Expired memories are
    /// purged (and their snapshots scrubbed) first, so history never outlives retention.
    pub fn history(&self, id: &str) -> Result<MemoryHistory> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        purge(&tx, self.now_ms())?;
        tx.commit()?;
        let mut statement = conn.prepare(
            "SELECT version, op, actor, at, content, tags, retention FROM memory_versions
             WHERE memory_id = ?1 ORDER BY seq",
        )?;
        let versions = statement
            .query_map([id], |row| {
                Ok(MemoryVersion {
                    version: row.get(0)?,
                    op: parse_op(&row.get::<_, String>(1)?)?,
                    actor: row.get(2)?,
                    at: to_time(row.get(3)?),
                    content: row.get(4)?,
                    tags: row
                        .get::<_, Option<String>>(5)?
                        .map(|tags| serde_json::from_str(&tags).unwrap_or_default()),
                    retention: row
                        .get::<_, Option<String>>(6)?
                        .map(|value| parse_retention(&value))
                        .transpose()?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if versions.is_empty() {
            return Err(Error::NotFound);
        }
        Ok(MemoryHistory {
            id: id.to_owned(),
            versions,
        })
    }

    /// The last recorded writer is provenance, never instruction authority.
    pub fn last_actor(&self, id: &str) -> Result<String> {
        self.conn
            .lock()
            .query_row(
                "SELECT actor FROM memory_versions WHERE memory_id=?1 ORDER BY seq DESC LIMIT 1",
                [id],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    /// Select explicit context layers after ownership and expiry filtering.
    pub fn context_layer(
        &self,
        owner: Option<&str>,
        layer: &str,
        query: &str,
    ) -> Result<Vec<ContextMemory>> {
        if !matches!(layer, "profile" | "event_log" | "scratch") {
            return Err(Error::Invalid("unknown memory layer"));
        }
        let now = self.now_ms();
        let terms: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|term| !term.is_empty())
            .take(32)
            .map(str::to_lowercase)
            .collect();
        let conn = self.conn.lock();
        let mut statement = conn.prepare(&format!(
            "SELECT {COLUMNS},
             COALESCE((SELECT actor FROM memory_versions v WHERE v.memory_id = m.id
                       ORDER BY seq DESC LIMIT 1), 'unknown')
             FROM memories m WHERE {LIVE} AND (m.owner IS NULL OR m.owner IS ?2)
             AND m.kind = 'note'
             AND EXISTS(SELECT 1 FROM json_each(m.tags) WHERE value = ?3)
             AND (?3 != 'event_log' OR m.updated_at >= ?4)
             ORDER BY CASE WHEN ?3 = 'profile' THEN 0 ELSE
                 (SELECT COUNT(*) FROM json_each(?5) WHERE instr(lower(m.content), value) > 0)
                 END DESC, m.updated_at DESC, m.id LIMIT ?6"
        ))?;
        let rows = statement
            .query_map(
                params![
                    now,
                    owner,
                    layer,
                    now - 30 * DAY_MS,
                    tags_json(&terms),
                    if layer == "profile" { 100 } else { 30 }
                ],
                |row| {
                    Ok(ContextMemory {
                        memory: read_memory(row)?,
                        actor: row.get(12)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Persist a server-selected allowlist, bound to one session and owner.
    pub fn consolidation_scope(
        &self,
        session: &str,
        owner: Option<&str>,
        shown: &[(String, u32)],
    ) -> Result<String> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = self.now_ms();
        let mut bound = Vec::with_capacity(shown.len());
        for (id, version) in shown {
            let memory = fetch(&tx, id, now)?.ok_or(Error::NotFound)?;
            if memory.version != *version
                || (memory.owner.is_some() && memory.owner.as_deref() != owner)
            {
                return Err(Error::Invalid("memory scope changed"));
            }
            bound.push((id, version, memory.owner));
        }
        let scope = new_id();
        tx.execute(
            "DELETE FROM consolidation_scopes WHERE expires_at <= ?1",
            [now],
        )?;
        tx.execute(
            "INSERT INTO consolidation_scopes(id,session,owner,shown,expires_at) VALUES (?1,?2,?3,?4,?5)",
            params![scope, session, owner, serde_json::to_string(&bound)
                .map_err(|_| Error::Invalid("invalid memory scope"))?, now + DAY_MS],
        )?;
        tx.commit()?;
        Ok(scope)
    }

    /// Atomically apply a bounded pass; replay is inert and concurrent edits win.
    pub fn consolidate(
        &self,
        scope: &str,
        session: &str,
        owner: Option<&str>,
        edits: Vec<ConsolidationEdit>,
        additions: Vec<NewMemory>,
        actor: &str,
    ) -> Result<bool> {
        if edits.len() > 100 || additions.len() > 20 {
            return Err(Error::Invalid("consolidation limit exceeded"));
        }
        check_actor(actor)?;
        if self.secret(actor) {
            return Err(Error::SecretDetected);
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = self.now_ms();
        let state: Option<(String, bool)> = tx.query_row(
            "SELECT shown,applied FROM consolidation_scopes WHERE id=?1 AND session=?2 AND owner IS ?3 AND expires_at>?4",
            params![scope, session, owner, now], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        let (shown, applied) = state.ok_or(Error::Invalid("invalid consolidation scope"))?;
        if applied {
            return Ok(false);
        }
        let shown: Vec<(String, u32, Option<String>)> = serde_json::from_str(&shown)
            .map_err(|_| Error::Invalid("invalid consolidation scope"))?;
        let mut seen = std::collections::HashSet::new();
        for edit in &edits {
            if !seen.insert(&edit.id)
                || !shown
                    .iter()
                    .any(|(id, version, _)| id == &edit.id && *version == edit.version)
            {
                return Err(Error::Invalid("unshown consolidation edit"));
            }
            let memory = fetch(&tx, &edit.id, now)?.ok_or(Error::NotFound)?;
            let shown_owner = shown
                .iter()
                .find(|(id, _, _)| id == &edit.id)
                .and_then(|(_, _, owner)| owner.as_deref());
            if memory.version != edit.version || memory.owner.as_deref() != shown_owner {
                return Err(Error::Invalid("stale consolidation edit"));
            }
        }
        for edit in edits {
            let mut memory = fetch(&tx, &edit.id, now)?.ok_or(Error::NotFound)?;
            if let Some(content) = edit.content {
                memory.content = content;
                self.rewrite(&tx, memory, actor, now)?;
            } else {
                remove(&tx, &memory, MemoryOp::Delete, actor, now)?;
            }
        }
        for new in additions {
            let memory = self.insert(&tx, new, actor, now)?;
            tx.execute(
                "UPDATE memories SET owner=?2 WHERE id=?1",
                params![memory.id, owner],
            )?;
        }
        tx.execute(
            "UPDATE consolidation_scopes SET applied=1 WHERE id=?1",
            [scope],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Hybrid (or lexical-only) search; an empty query lists recent memories.
    pub fn search(
        &self,
        query: &str,
        kind: Option<MemoryKind>,
        limit: usize,
    ) -> Result<MemorySearchResponse> {
        self.search_scoped(query, kind, limit, None, true)
    }

    /// Apply ownership before ranking and limits so another owner's data never competes.
    pub fn search_scoped(
        &self,
        query: &str,
        kind: Option<MemoryKind>,
        limit: usize,
        owner: Option<Option<&str>>,
        include_shared: bool,
    ) -> Result<MemorySearchResponse> {
        let all = owner.is_none();
        let owner = owner.flatten();
        let limit = limit.clamp(1, MAX_LIMIT);
        let now = self.now_ms();
        let kind = kind.map(MemoryKind::as_str);
        let conn = self.conn.lock();
        let terms: Vec<&str> = query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|term| !term.is_empty())
            .take(32)
            .collect();
        if query.trim().is_empty() {
            let mut statement = conn.prepare(&format!(
                "SELECT {COLUMNS} FROM memories m WHERE {LIVE} AND (?2 IS NULL OR m.kind = ?2)
                 AND (?4 OR m.owner IS ?5 OR (?6 AND m.owner IS NULL))
                 ORDER BY m.updated_at DESC, m.rowid DESC LIMIT ?3"
            ))?;
            let hits = statement
                .query_map(
                    params![now, kind, limit as i64, all, owner, include_shared],
                    read_memory,
                )?
                .map(|memory| {
                    Ok(MemoryHit {
                        memory: memory?,
                        score: 0.0,
                        matched: MemoryMatch::Recent,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            return Ok(MemorySearchResponse {
                mode: self.mode(),
                hits,
            });
        }
        let mut lexical = Vec::new();
        if !terms.is_empty() {
            // Terms are alphanumeric-only, so quoting cannot inject FTS5 syntax.
            let expression = terms
                .iter()
                .map(|term| format!("\"{term}\"*"))
                .collect::<Vec<_>>()
                .join(" OR ");
            let mut statement = conn.prepare(&format!(
                "SELECT m.id FROM memories_fts JOIN memories m ON m.rowid = memories_fts.rowid
                 WHERE memories_fts MATCH ?7 AND {LIVE} AND (?2 IS NULL OR m.kind = ?2)
                 AND (?4 OR m.owner IS ?5 OR (?6 AND m.owner IS NULL))
                 ORDER BY bm25(memories_fts) LIMIT ?3"
            ))?;
            lexical = statement
                .query_map(
                    params![
                        now,
                        kind,
                        CANDIDATES as i64,
                        all,
                        owner,
                        include_shared,
                        expression
                    ],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
        }
        let mut vector = Vec::new();
        if let Some((embedder, query)) = self
            .embedder
            .as_ref()
            .and_then(|embedder| Some((embedder, embedder.embed(query)?)))
        {
            let mut statement = conn.prepare(&format!(
                "SELECT m.id, m.embedding FROM memories m
                 WHERE {LIVE} AND (?2 IS NULL OR m.kind = ?2) AND m.embedding_model = ?3
                 AND (?4 OR m.owner IS ?5 OR (?6 AND m.owner IS NULL))"
            ))?;
            let mut scored = statement
                .query_map(
                    params![now, kind, embedder.id(), all, owner, include_shared],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
                )?
                .filter_map(|row| {
                    let (id, blob) = row.ok()?;
                    let cosine = dot(&query, &blob)?;
                    (cosine >= MIN_COSINE).then_some((id, cosine))
                })
                .collect::<Vec<_>>();
            scored.sort_by(|a, b| b.1.total_cmp(&a.1));
            scored.truncate(CANDIDATES);
            vector = scored.into_iter().map(|(id, _)| id).collect();
        }
        let mut fused: HashMap<String, (f64, bool, bool)> = HashMap::new();
        for (rank, id) in lexical.into_iter().enumerate() {
            let entry = fused.entry(id).or_default();
            entry.0 += 1.0 / (RRF_K + rank as f64 + 1.0);
            entry.1 = true;
        }
        for (rank, id) in vector.into_iter().enumerate() {
            let entry = fused.entry(id).or_default();
            entry.0 += 1.0 / (RRF_K + rank as f64 + 1.0);
            entry.2 = true;
        }
        let mut hits = Vec::new();
        for (id, (score, lexical, vector)) in fused {
            if let Some(memory) = fetch(&conn, &id, now)? {
                let matched = match (lexical, vector) {
                    (true, true) => MemoryMatch::Both,
                    (true, false) => MemoryMatch::Lexical,
                    _ => MemoryMatch::Vector,
                };
                hits.push(MemoryHit {
                    memory,
                    score,
                    matched,
                });
            }
        }
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then(b.memory.updated_at.cmp(&a.memory.updated_at))
                .then_with(|| a.memory.id.cmp(&b.memory.id))
        });
        hits.truncate(limit);
        Ok(MemorySearchResponse {
            mode: self.mode(),
            hits,
        })
    }

    /// Insert or refresh the 30-day history memory for one public page URL. Expired rows
    /// are purged first, so a revisit after expiry starts a fresh memory with no old summary.
    pub fn record_visit(
        &self,
        url: &str,
        title: Option<&str>,
        text: Option<&str>,
        actor: &str,
    ) -> Result<Memory> {
        let title = title.map(str::trim).filter(|title| !title.is_empty());
        let text = text.map(str::trim).filter(|text| !text.is_empty());
        let content = text.or(title).unwrap_or(url).to_owned();
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = self.now_ms();
        purge(&tx, now)?;
        let existing = tx
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM memories m WHERE m.kind = 'history' AND m.url = ?1"
                ),
                [url],
                read_memory,
            )
            .optional()?;
        let memory = match existing {
            Some(mut memory) => {
                // A navigation without a snapshot must not replace an earlier page summary.
                let summarized = memory.url.as_deref() != Some(memory.content.as_str())
                    && memory.title.as_deref() != Some(memory.content.as_str());
                if text.is_some() || !summarized {
                    memory.content = content;
                }
                if title.is_some() {
                    memory.title = title.map(str::to_owned);
                }
                memory.retention = Retention::Days30;
                self.rewrite(&tx, memory, actor, now)?
            }
            None => self.insert(
                &tx,
                NewMemory {
                    kind: MemoryKind::History,
                    content,
                    tags: Vec::new(),
                    retention: Some(Retention::Days30),
                    url: Some(url.to_owned()),
                    title: title.map(str::to_owned),
                },
                actor,
                now,
            )?,
        };
        tx.commit()?;
        Ok(memory)
    }

    /// Remove every expired memory; returns how many were purged.
    pub fn purge_expired(&self) -> Result<usize> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let purged = purge(&tx, self.now_ms())?;
        tx.commit()?;
        Ok(purged)
    }

    fn backfill(&self) -> Result<()> {
        let Some(embedder) = &self.embedder else {
            return Ok(());
        };
        let stale = {
            let conn = self.conn.lock();
            let mut statement = conn.prepare(&format!(
                "SELECT {COLUMNS} FROM memories m
                 WHERE m.embedding_model IS NULL OR m.embedding_model != ?1"
            ))?;
            let rows = statement
                .query_map([embedder.id()], read_memory)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        let conn = self.conn.lock();
        for memory in stale {
            let embedding = self.embedding(&memory);
            conn.execute(
                "UPDATE memories SET embedding = ?2, embedding_model = ?3 WHERE id = ?1",
                params![
                    memory.id,
                    embedding.as_ref().map(|(_, blob)| blob),
                    embedder.id()
                ],
            )?;
        }
        Ok(())
    }

    fn embedding(&self, memory: &Memory) -> Option<(String, Vec<u8>)> {
        let embedder = self.embedder.as_ref()?;
        let text = [
            memory.title.as_deref().unwrap_or_default(),
            &memory.content,
            &memory.tags.join(" "),
        ]
        .join("\n");
        let vector = embedder.embed(&text)?;
        Some((
            embedder.id().to_owned(),
            vector
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect(),
        ))
    }

    /// Screen every field, the actor, and the exact aggregate text that is indexed and
    /// embedded, so a secret split across fields is caught where it is reassembled.
    fn screen(
        &self,
        content: &str,
        tags: &[String],
        title: Option<&str>,
        url: Option<&str>,
        actor: &str,
    ) -> Result<()> {
        let aggregate = [title.unwrap_or_default(), content, &tags.join(" ")].join("\n");
        let fields = std::iter::once(content)
            .chain(tags.iter().map(String::as_str))
            .chain(title)
            .chain(url)
            .chain([actor, aggregate.as_str()]);
        for field in fields {
            if self.secret(field) {
                return Err(Error::SecretDetected);
            }
        }
        Ok(())
    }

    /// Reject `text` that looks like a secret or contains a known secret value.
    pub fn check_secret(&self, text: &str) -> Result<()> {
        if self.secret(text) {
            return Err(Error::SecretDetected);
        }
        Ok(())
    }

    fn secret(&self, field: &str) -> bool {
        looks_secret(field)
            || self
                .known_secret
                .as_ref()
                .is_some_and(|probe| secrets::variants(field).iter().any(|form| probe(form)))
    }

    fn now_ms(&self) -> i64 {
        self.clock.now().timestamp_millis()
    }
}

fn validate(content: &str, title: Option<&str>, url: Option<&str>, actor: &str) -> Result<()> {
    if content.trim().is_empty() || content.len() > MAX_CONTENT {
        return Err(Error::Invalid("content must be 1-16384 bytes"));
    }
    if title.is_some_and(|title| title.len() > MAX_TITLE) {
        return Err(Error::Invalid("title must be at most 512 bytes"));
    }
    if url.is_some_and(|url| url.len() > MAX_URL) {
        return Err(Error::Invalid("url must be at most 2048 bytes"));
    }
    check_actor(actor)
}

/// Actors are identifiers (`api`, `agent:<session>`), never free text.
fn check_actor(actor: &str) -> Result<()> {
    if actor.is_empty()
        || actor.len() > 128
        || !actor.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-' | b'.' | b'@')
        })
    {
        return Err(Error::Invalid("actor must be 1-128 identifier characters"));
    }
    Ok(())
}

fn clean_tags(tags: Vec<String>) -> Result<Vec<String>> {
    if tags.len() > MAX_TAGS {
        return Err(Error::Invalid("at most 32 tags are allowed"));
    }
    let mut cleaned: Vec<String> = Vec::with_capacity(tags.len());
    for tag in tags {
        let tag = tag.trim().to_owned();
        if tag.is_empty() || tag.chars().count() > MAX_TAG || tag.chars().any(char::is_control) {
            return Err(Error::Invalid("tags must be 1-64 printable characters"));
        }
        if !cleaned.contains(&tag) {
            cleaned.push(tag);
        }
    }
    Ok(cleaned)
}

fn index(conn: &Connection, rowid: i64, memory: &Memory) -> Result<()> {
    conn.execute(
        "INSERT INTO memories_fts(rowid, title, content, tags) VALUES (?1, ?2, ?3, ?4)",
        params![
            rowid,
            memory.title.as_deref().unwrap_or_default(),
            memory.content,
            memory.tags.join(" ")
        ],
    )?;
    Ok(())
}

fn record(conn: &Connection, memory: &Memory, op: MemoryOp, actor: &str, at: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO memory_versions(memory_id, version, op, actor, at, content, tags, retention)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            memory.id,
            memory.version,
            op_name(op),
            actor,
            at,
            memory.content,
            tags_json(&memory.tags),
            memory.retention.as_str()
        ],
    )?;
    Ok(())
}

fn purge(conn: &Connection, now: i64) -> Result<usize> {
    let expired = {
        let mut statement = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM memories m WHERE m.expires_at IS NOT NULL AND m.expires_at <= ?1"
        ))?;
        let rows = statement
            .query_map([now], read_memory)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for memory in &expired {
        remove(conn, memory, MemoryOp::Purge, "retention", now)?;
    }
    Ok(expired.len())
}

fn remove(conn: &Connection, memory: &Memory, op: MemoryOp, actor: &str, at: i64) -> Result<()> {
    let rowid: i64 = conn.query_row(
        "SELECT rowid FROM memories WHERE id = ?1",
        [&memory.id],
        |row| row.get(0),
    )?;
    conn.execute("DELETE FROM memories_fts WHERE rowid = ?1", [rowid])?;
    conn.execute("DELETE FROM memories WHERE rowid = ?1", [rowid])?;
    conn.execute(
        "UPDATE memory_versions SET content = NULL, tags = NULL WHERE memory_id = ?1",
        [&memory.id],
    )?;
    conn.execute(
        "INSERT INTO memory_versions(memory_id, version, op, actor, at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![memory.id, memory.version + 1, op_name(op), actor, at],
    )?;
    Ok(())
}

fn fetch(conn: &Connection, id: &str, now: i64) -> Result<Option<Memory>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM memories m WHERE {LIVE} AND m.id = ?2"),
            params![now, id],
            read_memory,
        )
        .optional()?)
}

fn read_memory(row: &Row<'_>) -> rusqlite::Result<Memory> {
    Ok(Memory {
        id: row.get(0)?,
        kind: match row.get::<_, String>(1)?.as_str() {
            "note" => MemoryKind::Note,
            "history" => MemoryKind::History,
            _ => return Err(conversion("kind")),
        },
        content: row.get(2)?,
        tags: serde_json::from_str(&row.get::<_, String>(3)?).map_err(|_| conversion("tags"))?,
        retention: parse_retention(&row.get::<_, String>(4)?)?,
        url: row.get(5)?,
        title: row.get(6)?,
        created_at: to_time(row.get(7)?),
        updated_at: to_time(row.get(8)?),
        expires_at: row.get::<_, Option<i64>>(9)?.map(to_time),
        version: row.get(10)?,
        owner: row.get(11)?,
    })
}

fn conversion(column: &'static str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        format!("invalid stored {column}").into(),
    )
}

fn parse_retention(value: &str) -> rusqlite::Result<Retention> {
    match value {
        "permanent" => Ok(Retention::Permanent),
        "days_90" => Ok(Retention::Days90),
        "days_30" => Ok(Retention::Days30),
        "hours_24" => Ok(Retention::Hours24),
        _ => Err(conversion("retention")),
    }
}

fn op_name(op: MemoryOp) -> &'static str {
    match op {
        MemoryOp::Create => "create",
        MemoryOp::Update => "update",
        MemoryOp::Delete => "delete",
        MemoryOp::Purge => "purge",
    }
}

fn parse_op(value: &str) -> rusqlite::Result<MemoryOp> {
    match value {
        "create" => Ok(MemoryOp::Create),
        "update" => Ok(MemoryOp::Update),
        "delete" => Ok(MemoryOp::Delete),
        "purge" => Ok(MemoryOp::Purge),
        _ => Err(conversion("op")),
    }
}

fn tags_json(tags: &[String]) -> String {
    serde_json::to_string(tags).unwrap_or_else(|_| "[]".into())
}

fn expiry(retention: Retention, now: i64) -> Option<i64> {
    retention.days().map(|days| now + days * DAY_MS)
}

fn to_time(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms).single().unwrap_or_default()
}

fn new_id() -> String {
    let bytes: [u8; 12] = rand::random();
    let mut id = String::from("mem_");
    for byte in bytes {
        id.push_str(&format!("{byte:02x}"));
    }
    id
}

fn dot(query: &[f32], blob: &[u8]) -> Option<f32> {
    if blob.len() != query.len() * 4 {
        return None;
    }
    Some(
        blob.chunks_exact(4)
            .zip(query)
            .map(|(chunk, q)| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) * q)
            .sum(),
    )
}

#[cfg(test)]
mod context_tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, Ordering};

    struct FakeClock(AtomicI64);
    impl Clock for FakeClock {
        fn now(&self) -> DateTime<Utc> {
            to_time(self.0.load(Ordering::SeqCst))
        }
    }

    fn note(text: &str, layer: &str) -> NewMemory {
        NewMemory {
            tags: vec![layer.into()],
            ..NewMemory::note(text)
        }
    }

    #[test]
    fn layers_scope_relevance_limits_expiry_and_restart() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("memory.db");
        let clock = Arc::new(FakeClock(AtomicI64::new(100 * DAY_MS)));
        let options = || Options {
            clock: clock.clone(),
            ..Options::default()
        };
        let store = MemoryStore::open(&path, options()).unwrap();
        let shared = store.create(note("shared fact", "profile"), "api").unwrap();
        for n in 0..120 {
            store
                .create_owned(
                    note(&format!("private {n}"), "profile"),
                    "api",
                    Some("other"),
                )
                .unwrap();
            store
                .create_owned(
                    note(&format!("own 東京 {n}"), "profile"),
                    "api",
                    Some("chief"),
                )
                .unwrap();
        }
        assert_eq!(
            store
                .context_layer(Some("chief"), "profile", "")
                .unwrap()
                .len(),
            100
        );
        let only_shared = store.context_layer(None, "profile", "").unwrap();
        assert_eq!(only_shared.len(), 1);
        assert_eq!(only_shared[0].memory.id, shared.id);
        let relevant = store
            .create(note("orchid delivery confirmed", "event_log"), "api")
            .unwrap();
        clock.0.fetch_add(1000, Ordering::SeqCst);
        for n in 0..60 {
            store
                .create(note(&format!("unrelated {n}"), "event_log"), "api")
                .unwrap();
        }
        let selected = store.context_layer(None, "event_log", "orchid").unwrap();
        assert_eq!(selected.len(), 30);
        assert_eq!(selected[0].memory.id, relevant.id);
        assert_eq!(
            selected
                .iter()
                .map(|row| &row.memory.id)
                .collect::<Vec<_>>(),
            store
                .context_layer(None, "event_log", "orchid")
                .unwrap()
                .iter()
                .map(|row| &row.memory.id)
                .collect::<Vec<_>>()
        );
        let scratch = store
            .create(note("temporary detail", "scratch"), "api")
            .unwrap();
        assert_eq!(scratch.retention, Retention::Hours24);
        clock.0.fetch_add(DAY_MS, Ordering::SeqCst);
        assert!(store.context_layer(None, "scratch", "").unwrap().is_empty());
        assert!(store.get(&shared.id).is_ok());
        drop(store);
        let store = MemoryStore::open(&path, options()).unwrap();
        assert!(store.context_layer(None, "scratch", "").unwrap().is_empty());
        assert!(store.get(&shared.id).is_ok());
        clock.0.fetch_add(30 * DAY_MS, Ordering::SeqCst);
        assert!(store
            .context_layer(None, "event_log", "")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn consolidation_rejects_unshown_stale_transferred_and_secret_edits_atomically() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("memory.db");
        let store = MemoryStore::open(&path, Options::default()).unwrap();
        let shown = store.create(note("shown", "profile"), "api").unwrap();
        let unseen = store.create(note("unseen", "profile"), "api").unwrap();
        let scope = store
            .consolidation_scope("session", Some("chief"), &[(shown.id.clone(), 1)])
            .unwrap();
        let edit = |memory: &Memory| ConsolidationEdit {
            id: memory.id.clone(),
            version: memory.version,
            content: None,
        };
        assert!(store
            .consolidate(
                &scope,
                "session",
                Some("chief"),
                vec![edit(&unseen)],
                vec![],
                "agent:test"
            )
            .is_err());
        assert!(store
            .consolidate(
                &scope,
                "wrong_session",
                Some("chief"),
                vec![edit(&shown)],
                vec![],
                "agent:test"
            )
            .is_err());
        assert!(store
            .consolidate(
                &scope,
                "session",
                Some("other"),
                vec![edit(&shown)],
                vec![],
                "agent:test"
            )
            .is_err());
        assert!(store
            .consolidate(
                &scope,
                "session",
                Some("chief"),
                vec![edit(&shown)],
                vec![note("password=not-for-memory-123", "profile")],
                "agent:test"
            )
            .is_err());
        assert!(store.get(&shown.id).is_ok());
        store
            .update(
                &shown.id,
                UpdateMemoryRequest {
                    content: Some("owner correction".into()),
                    ..Default::default()
                },
                "api",
            )
            .unwrap();
        assert!(store
            .consolidate(
                &scope,
                "session",
                Some("chief"),
                vec![edit(&shown)],
                vec![],
                "agent:test"
            )
            .is_err());
        assert_eq!(store.get(&shown.id).unwrap().content, "owner correction");
        let owned = store
            .create_owned(note("owned", "profile"), "api", Some("chief"))
            .unwrap();
        let scope = store
            .consolidation_scope("session", Some("chief"), &[(owned.id.clone(), 1)])
            .unwrap();
        store
            .transfer_owner(&owned.id, Some("chief"), None)
            .unwrap();
        assert!(store
            .consolidate(
                &scope,
                "session",
                Some("chief"),
                vec![edit(&owned)],
                vec![],
                "agent:test"
            )
            .is_err());
        let fresh = store.get(&shown.id).unwrap();
        let scope = store
            .consolidation_scope(
                "session",
                Some("chief"),
                &[(fresh.id.clone(), fresh.version)],
            )
            .unwrap();
        assert!(store
            .consolidate(
                &scope,
                "session",
                Some("chief"),
                vec![edit(&fresh)],
                vec![note("new fact", "profile")],
                "agent:test"
            )
            .unwrap());
        let count = store.count().unwrap();
        drop(store);
        let store = MemoryStore::open(&path, Options::default()).unwrap();
        assert!(!store
            .consolidate(
                &scope,
                "session",
                Some("chief"),
                vec![edit(&fresh)],
                vec![note("duplicate", "profile")],
                "agent:test"
            )
            .unwrap());
        assert_eq!(store.count().unwrap(), count);
    }
}
