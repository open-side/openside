//! The user-selected memory subset for device sync (`openside-sync`): live memories that
//! carry one of the configured sync tags, addressed by a device-independent sync id.
//!
//! A memory created on this device is synced under its own id. A memory received from
//! another device gets a new local id; the pair is stored in `sync_ids` in the same
//! transaction that creates the memory, so a crash can never leave an unmapped copy that
//! the next run would publish as a second memory.

use rusqlite::{params, OptionalExtension, TransactionBehavior};

use crate::{fetch, read_memory, Memory, MemoryStore, NewMemory, Result, COLUMNS, LIVE};

impl MemoryStore {
    /// Live memories carrying any of `tags`, oldest first, with their sync ids. Empty `tags`
    /// selects nothing.
    pub fn tagged(&self, tags: &[String]) -> Result<Vec<(String, Memory)>> {
        if tags.is_empty() {
            return Ok(Vec::new());
        }
        let now = self.now_ms();
        let conn = self.conn.lock();
        let mut statement = conn.prepare(&format!(
            "SELECT {COLUMNS}, s.sync_id FROM memories m LEFT JOIN sync_ids s ON s.memory_id = m.id
             WHERE {LIVE} ORDER BY m.created_at, m.id"
        ))?;
        let rows = statement.query_map(params![now], |row| {
            Ok((read_memory(row)?, row.get::<_, Option<String>>("sync_id")?))
        })?;
        let mut selected = Vec::new();
        for row in rows {
            let (memory, sync_id) = row?;
            if memory.tags.iter().any(|tag| tags.contains(tag)) {
                selected.push((sync_id.unwrap_or_else(|| memory.id.clone()), memory));
            }
        }
        Ok(selected)
    }

    /// The live local memory behind `sync_id`: the one received under it, or, for a memory
    /// that originated here, the memory with that id.
    pub fn synced(&self, sync_id: &str) -> Result<Option<Memory>> {
        let now = self.now_ms();
        let conn = self.conn.lock();
        let mapped: Option<String> = conn
            .query_row(
                "SELECT memory_id FROM sync_ids WHERE sync_id = ?1",
                params![sync_id],
                |row| row.get(0),
            )
            .optional()?;
        fetch(&conn, mapped.as_deref().unwrap_or(sync_id), now)
    }

    /// Create a memory received under `sync_id` and record the mapping atomically. If the
    /// sync id already maps to a live memory, that memory is returned unchanged.
    pub fn create_synced(&self, sync_id: &str, new: NewMemory, actor: &str) -> Result<Memory> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = self.now_ms();
        crate::purge(&tx, now)?;
        let mapped: Option<String> = tx
            .query_row(
                "SELECT memory_id FROM sync_ids WHERE sync_id = ?1",
                params![sync_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(existing) = mapped
            .as_deref()
            .map(|id| fetch(&tx, id, now))
            .transpose()?
            .flatten()
        {
            return Ok(existing);
        }
        let memory = self.insert(&tx, new, actor, now)?;
        tx.execute(
            "INSERT OR REPLACE INTO sync_ids(sync_id, memory_id) VALUES (?1, ?2)",
            params![sync_id, memory.id],
        )?;
        tx.commit()?;
        Ok(memory)
    }

    /// Drop the mapping of `sync_id` (after its memory was deleted by sync).
    pub fn forget_synced(&self, sync_id: &str) -> Result<()> {
        self.conn
            .lock()
            .execute("DELETE FROM sync_ids WHERE sync_id = ?1", params![sync_id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{MemoryStore, NewMemory, Options};

    fn tagged(content: &str) -> NewMemory {
        let mut new = NewMemory::note(content);
        new.tags = vec!["sync".into(), "work".into()];
        new
    }

    #[test]
    fn tagged_selects_only_live_memories_with_a_sync_tag() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(&dir.path().join("m.db"), Options::default()).unwrap();
        let synced = store.create(tagged("synced note"), "test").unwrap();
        store
            .create(NewMemory::note("private note"), "test")
            .unwrap();
        let found = store.tagged(&["sync".into()]).unwrap();
        assert_eq!(
            found
                .iter()
                .map(|(sync, m)| (sync.clone(), m.id.clone()))
                .collect::<Vec<_>>(),
            vec![(synced.id.clone(), synced.id.clone())]
        );
        assert!(store.tagged(&[]).unwrap().is_empty());
        store.delete(&synced.id, "test").unwrap();
        assert!(store.tagged(&["sync".into()]).unwrap().is_empty());
    }

    #[test]
    fn received_memory_and_its_sync_id_survive_a_restart_without_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.db");
        let created = {
            let store = MemoryStore::open(&path, Options::default()).unwrap();
            store
                .create_synced("remote-id", tagged("received"), "sync")
                .unwrap()
        };
        // Reopened as after a crash right after the create: the mapping is already durable.
        let store = MemoryStore::open(&path, Options::default()).unwrap();
        assert_eq!(
            store.synced("remote-id").unwrap().map(|m| m.id),
            Some(created.id.clone())
        );
        let again = store
            .create_synced("remote-id", tagged("received"), "sync")
            .unwrap();
        assert_eq!(
            again.id, created.id,
            "a replayed import reuses the mapped memory"
        );
        assert_eq!(
            store
                .tagged(&["sync".into()])
                .unwrap()
                .into_iter()
                .map(|(s, _)| s)
                .collect::<Vec<_>>(),
            vec!["remote-id".to_owned()]
        );
        assert_eq!(store.count().unwrap(), 1);
        store.delete(&created.id, "sync").unwrap();
        store.forget_synced("remote-id").unwrap();
        assert_eq!(store.synced("remote-id").unwrap(), None);
    }
}
