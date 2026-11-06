//! Store primitives behind the file-shaped memory views (`MEMORY.md`, `USER.md`,
//! `TAXONOMY.md`) and the global change log. Rendering and parsing the files is the
//! daemon's job; this module only offers a consistent snapshot and an atomic batch write.

use openside_proto::{MemoryChange, MemoryChangeList, MemoryKind};
use rusqlite::{params, OptionalExtension, TransactionBehavior};

use crate::{
    check_actor, clean_tags, fetch, index, parse_op, purge, read_memory, remove, Error, Memory,
    MemoryStore, NewMemory, Result, COLUMNS, LIVE,
};

/// Upper bound of one batch: a whole edited file.
pub const MAX_EDITS: usize = 1000;

/// One change of [`MemoryStore::apply_edits`].
#[derive(Debug, Clone)]
pub enum MemoryEdit {
    /// Add a note.
    Create(NewMemory),
    /// Replace a note's text and labels, keeping retention, owner and provenance.
    Update {
        /// Note to change.
        id: String,
        /// New text.
        content: String,
        /// New labels.
        tags: Vec<String>,
    },
    /// Delete a note.
    Delete {
        /// Note to delete.
        id: String,
    },
}

/// What a batch changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EditCounts {
    /// Notes added.
    pub created: u32,
    /// Notes changed.
    pub updated: u32,
    /// Notes deleted.
    pub deleted: u32,
}

/// Live notes, the other live memories' count and the change-log position they reflect.
#[derive(Debug, Clone)]
pub struct NoteSnapshot {
    /// Highest change-log position at read time; pass it back to
    /// [`MemoryStore::apply_edits`].
    pub revision: i64,
    /// Live notes (not visited pages), oldest first.
    pub notes: Vec<Memory>,
    /// Live memories that are not notes (visited pages).
    pub history: u64,
}

impl MemoryStore {
    /// Every live note, the count of everything else and the change-log position they
    /// reflect, read under one lock so the counts match the notes.
    pub fn notes(&self) -> Result<NoteSnapshot> {
        let now = self.now_ms();
        let conn = self.conn.lock();
        let revision: i64 = conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM memory_versions",
            [],
            |row| row.get(0),
        )?;
        let mut statement = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM memories m WHERE {LIVE} AND m.kind = 'note'
             ORDER BY m.created_at, m.rowid"
        ))?;
        let notes = statement
            .query_map([now], read_memory)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let history: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM memories m WHERE {LIVE} AND m.kind <> 'note'"),
            [now],
            |row| row.get(0),
        )?;
        Ok(NoteSnapshot {
            revision,
            notes,
            history: u64::try_from(history).unwrap_or_default(),
        })
    }

    /// Apply a whole batch in one transaction or nothing. An update or delete of a note that
    /// changed (or appeared) after `revision` fails the batch with [`Error::Conflict`], so a
    /// stale file never overwrites or drops someone else's edit.
    pub fn apply_edits(
        &self,
        revision: i64,
        edits: Vec<MemoryEdit>,
        actor: &str,
    ) -> Result<EditCounts> {
        if edits.len() > MAX_EDITS {
            return Err(Error::Invalid("at most 1000 changes per save"));
        }
        check_actor(actor)?;
        if self.secret(actor) {
            return Err(Error::SecretDetected);
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let latest: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM memory_versions",
            [],
            |row| row.get(0),
        )?;
        // Only a revision this store issued is a snapshot; a made-up later one would skip
        // every conflict check.
        if !(0..=latest).contains(&revision) {
            return Err(Error::Conflict);
        }
        let now = self.now_ms();
        purge(&tx, now)?;
        let mut counts = EditCounts::default();
        for edit in edits {
            match edit {
                MemoryEdit::Create(new) => {
                    if new.kind != MemoryKind::Note {
                        return Err(Error::Invalid("only notes can be added here"));
                    }
                    self.insert(&tx, new, actor, now)?;
                    counts.created += 1;
                }
                MemoryEdit::Update { id, content, tags } => {
                    let mut memory = current(&tx, &id, revision, now)?;
                    if tags.iter().any(|tag| self.secret(tag)) {
                        return Err(Error::SecretDetected);
                    }
                    memory.content = content;
                    memory.tags = clean_tags(tags)?;
                    self.rewrite(&tx, memory, actor, now)?;
                    counts.updated += 1;
                }
                MemoryEdit::Delete { id } => {
                    let memory = current(&tx, &id, revision, now)?;
                    remove(&tx, &memory, openside_proto::MemoryOp::Delete, actor, now)?;
                    counts.deleted += 1;
                }
            }
        }
        tx.commit()?;
        Ok(counts)
    }

    /// The change log across all memories, newest first; `before` pages to older entries.
    pub fn changes(&self, before: Option<i64>, limit: usize) -> Result<MemoryChangeList> {
        let limit = limit.clamp(1, 200);
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        purge(&tx, self.now_ms())?;
        tx.commit()?;
        let mut statement = conn.prepare(
            "SELECT v.seq, v.memory_id, v.version, v.op, v.actor, v.at, v.content, v.tags, m.kind
             FROM memory_versions v LEFT JOIN memories m ON m.id = v.memory_id
             WHERE ?1 IS NULL OR v.seq < ?1
             ORDER BY v.seq DESC LIMIT ?2",
        )?;
        let mut changes = statement
            .query_map(params![before, limit as i64 + 1], |row| {
                Ok(MemoryChange {
                    seq: row.get(0)?,
                    memory_id: row.get(1)?,
                    version: row.get(2)?,
                    op: parse_op(&row.get::<_, String>(3)?)?,
                    actor: row.get(4)?,
                    at: crate::to_time(row.get(5)?),
                    content: row.get(6)?,
                    tags: row
                        .get::<_, Option<String>>(7)?
                        .map(|tags| serde_json::from_str(&tags).unwrap_or_default()),
                    kind: row
                        .get::<_, Option<String>>(8)?
                        .and_then(|kind| match kind.as_str() {
                            "note" => Some(MemoryKind::Note),
                            "history" => Some(MemoryKind::History),
                            _ => None,
                        }),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let next_before = (changes.len() > limit).then(|| {
            changes.truncate(limit);
            changes.last().map(|change| change.seq).unwrap_or_default()
        });
        Ok(MemoryChangeList {
            changes,
            next_before,
        })
    }

    /// Purge expired memories, rebuild the full-text index from the live rows and recompute
    /// embeddings with the loaded model. Returns `(reindexed, embedded)`.
    pub fn rebuild(&self) -> Result<(u64, u64)> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = self.now_ms();
        purge(&tx, now)?;
        let rows = {
            let mut statement = tx.prepare(&format!(
                "SELECT m.rowid, m.id FROM memories m WHERE {LIVE}"
            ))?;
            let rows = statement
                .query_map([now], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        tx.execute("DELETE FROM memories_fts", [])?;
        let (mut reindexed, mut embedded) = (0u64, 0u64);
        for (rowid, id) in rows {
            let Some(memory) = fetch(&tx, &id, now)? else {
                continue;
            };
            index(&tx, rowid, &memory)?;
            reindexed += 1;
            if let Some((model, blob)) = self.embedding(&memory) {
                tx.execute(
                    "UPDATE memories SET embedding = ?2, embedding_model = ?3 WHERE rowid = ?1",
                    params![rowid, blob, model],
                )?;
                embedded += 1;
            }
        }
        tx.commit()?;
        Ok((reindexed, embedded))
    }
}

fn current(tx: &rusqlite::Connection, id: &str, revision: i64, now: i64) -> Result<Memory> {
    let memory = fetch(tx, id, now)?.ok_or(Error::Conflict)?;
    if memory.kind != MemoryKind::Note {
        return Err(Error::Invalid("only notes can be edited here"));
    }
    let latest: Option<i64> = tx
        .query_row(
            "SELECT MAX(seq) FROM memory_versions WHERE memory_id = ?1",
            [id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    if latest.unwrap_or_default() > revision {
        return Err(Error::Conflict);
    }
    Ok(memory)
}
