use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use openside_proto::{CapabilityScope, Event, EventKind, SessionId};
use openside_sessions::{SessionState, Store};
use rand::{rngs::OsRng, RngCore};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

pub(crate) type Error = Box<dyn std::error::Error + Send + Sync>;
pub(crate) type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Clone)]
pub(crate) struct Database(
    Arc<Mutex<Inner>>,
    #[cfg(test)] Arc<parking_lot::Mutex<std::collections::HashMap<String, Arc<CommitPause>>>>,
);
struct Inner {
    store: Store,
    devices: Connection,
}

impl Database {
    pub(crate) async fn open(directory: &Path) -> Result<Self> {
        let directory = directory.to_owned();
        tokio::task::spawn_blocking(move || {
            let store = Store::open(openside_config::data_layout::DataPath::Events.path(&directory))?;
            let devices = Connection::open(openside_config::data_layout::DataPath::Devices.path(&directory))?;
            devices.execute_batch("PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS devices (id TEXT PRIMARY KEY, hash BLOB NOT NULL UNIQUE, scopes TEXT NOT NULL); CREATE TABLE IF NOT EXISTS device_meta (id TEXT PRIMARY KEY, name TEXT NOT NULL, created_at TEXT NOT NULL);")?;
            Ok(Self(
                Arc::new(Mutex::new(Inner { store, devices })),
                #[cfg(test)]
                Arc::default(),
            ))
        }).await?
    }
    pub(crate) async fn store<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Store) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let inner = self.0.clone();
        tokio::task::spawn_blocking(move || {
            f(&mut inner.lock().map_err(|_| "database lock poisoned")?.store)
        })
        .await?
    }
    pub(crate) async fn state(&self, id: &SessionId) -> Result<SessionState> {
        let id = id.clone();
        self.store(move |s| {
            s.session(&id)
                .cloned()
                .ok_or_else(|| "unknown session".into())
        })
        .await
    }
    pub(crate) async fn append(
        &self,
        id: &SessionId,
        run: Option<String>,
        kind: EventKind,
    ) -> Result<Event> {
        #[cfg(test)]
        let pause = match &kind {
            EventKind::TicketResolved { ticket_id, .. } => self.1.lock().remove(ticket_id.as_str()),
            _ => None,
        };
        #[cfg(test)]
        if pause
            .as_ref()
            .is_some_and(|pause| pause.fail.load(std::sync::atomic::Ordering::Relaxed))
        {
            return Err("injected resolution commit failure".into());
        }
        let id = id.clone();
        let event = self.store(move |s| Ok(s.append(&id, run, kind)?)).await?;
        #[cfg(test)]
        if let Some(pause) = pause {
            tokio::task::spawn_blocking(move || pause.block()).await?;
        }
        Ok(event)
    }
    pub(crate) async fn events(&self, id: &SessionId, after: u64) -> Result<Vec<Event>> {
        let id = id.clone();
        self.store(move |s| Ok(s.events_after(&id, after, 512)?))
            .await
    }
    #[cfg(test)]
    pub(crate) fn pause_after_resolution(&self, ticket_id: &str) -> CommitPauseHandle {
        let pause = Arc::new(CommitPause::default());
        self.1.lock().insert(ticket_id.to_owned(), pause.clone());
        CommitPauseHandle(pause)
    }
    /// The next `TicketResolved` append for this ticket fails before anything is stored.
    #[cfg(test)]
    pub(crate) fn fail_resolution(&self, ticket_id: &str) {
        self.pause_after_resolution(ticket_id).fail();
    }
    #[cfg(test)]
    pub(crate) async fn pair(&self, scopes: Vec<CapabilityScope>) -> Result<(String, String)> {
        self.pair_named(String::new(), scopes).await
    }
    pub(crate) async fn pair_named(
        &self,
        name: String,
        scopes: Vec<CapabilityScope>,
    ) -> Result<(String, String)> {
        let inner = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let id = random_id();
            let token = format!("{}{}", random_id(), random_id());
            let hash = Sha256::digest(token.as_bytes());
            let mut inner = inner.lock().map_err(|_| "database lock poisoned")?;
            let transaction = inner.devices.transaction()?;
            transaction.execute(
                "INSERT INTO devices(id, hash, scopes) VALUES (?1, ?2, ?3)",
                params![id, hash.as_slice(), serde_json::to_string(&scopes)?],
            )?;
            transaction.execute(
                "INSERT INTO device_meta(id, name, created_at) VALUES (?1, ?2, ?3)",
                params![id, name, chrono::Utc::now().to_rfc3339()],
            )?;
            transaction.commit()?;
            Ok((id, token))
        })
        .await?
    }
    /// Device id and scopes of a live token.
    pub(crate) async fn device(
        &self,
        token: String,
    ) -> Result<Option<(String, Vec<CapabilityScope>)>> {
        let inner = self.0.clone();
        tokio::task::spawn_blocking(move || {
            use rusqlite::OptionalExtension;
            let hash = Sha256::digest(token.as_bytes());
            let row: Option<(String, String)> = inner
                .lock()
                .map_err(|_| "database lock poisoned")?
                .devices
                .query_row(
                    "SELECT id, scopes FROM devices WHERE hash=?1",
                    [hash.as_slice()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            row.map(|(id, text)| Ok((id, serde_json::from_str(&text)?)))
                .transpose()
        })
        .await?
    }
    pub(crate) async fn devices(&self) -> Result<Vec<openside_proto::Device>> {
        let inner = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let inner = inner.lock().map_err(|_| "database lock poisoned")?;
            let mut statement = inner.devices.prepare(
                "SELECT d.id, d.scopes, m.name, m.created_at FROM devices d LEFT JOIN device_meta m ON m.id = d.id ORDER BY m.created_at, d.rowid",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?;
            let mut devices = Vec::new();
            for row in rows {
                let (device_id, scopes, name, created_at) = row?;
                devices.push(openside_proto::Device {
                    device_id,
                    name: name.unwrap_or_default(),
                    scopes: serde_json::from_str(&scopes)?,
                    created_at: created_at
                        .and_then(|text| chrono::DateTime::parse_from_rfc3339(&text).ok())
                        .map(|time| time.with_timezone(&chrono::Utc)),
                });
            }
            Ok(devices)
        })
        .await?
    }
    pub(crate) async fn device_exists(&self, id: String) -> Result<bool> {
        let inner = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let count: i64 = inner
                .lock()
                .map_err(|_| "database lock poisoned")?
                .devices
                .query_row("SELECT COUNT(*) FROM devices WHERE id=?1", [id], |row| {
                    row.get(0)
                })?;
            Ok(count > 0)
        })
        .await?
    }
    /// Delete a device token; returns whether it existed.
    pub(crate) async fn revoke(&self, id: String) -> Result<bool> {
        let inner = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let mut inner = inner.lock().map_err(|_| "database lock poisoned")?;
            let transaction = inner.devices.transaction()?;
            let removed = transaction.execute("DELETE FROM devices WHERE id=?1", [&id])?;
            transaction.execute("DELETE FROM device_meta WHERE id=?1", [&id])?;
            transaction.commit()?;
            Ok(removed > 0)
        })
        .await?
    }
}

#[cfg(test)]
#[derive(Default)]
struct CommitPause {
    reached: tokio::sync::Notify,
    released: parking_lot::Mutex<bool>,
    wake: parking_lot::Condvar,
    fail: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
impl CommitPause {
    fn block(&self) {
        self.reached.notify_one();
        let mut released = self.released.lock();
        while !*released {
            self.wake.wait(&mut released);
        }
    }
}

#[cfg(test)]
pub(crate) struct CommitPauseHandle(Arc<CommitPause>);

#[cfg(test)]
impl CommitPauseHandle {
    pub(crate) async fn reached(&self) {
        self.0.reached.notified().await;
    }
    pub(crate) fn fail(&self) {
        self.0
            .fail
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.release();
    }
    pub(crate) fn release(&self) {
        *self.0.released.lock() = true;
        self.0.wake.notify_all();
    }
}

#[cfg(test)]
impl Drop for CommitPauseHandle {
    fn drop(&mut self) {
        // A failed assertion must never leave a blocking writer hanging at runtime shutdown.
        self.release();
    }
}

#[cfg(test)]
static BEFORE_RENAME: parking_lot::Mutex<
    Option<std::collections::HashMap<std::path::PathBuf, Arc<CommitPause>>>,
> = parking_lot::Mutex::new(None);

#[cfg(test)]
pub(crate) fn pause_before_rename(path: &Path) -> CommitPauseHandle {
    let pause = Arc::new(CommitPause::default());
    BEFORE_RENAME
        .lock()
        .get_or_insert_with(Default::default)
        .insert(path.to_owned(), pause.clone());
    CommitPauseHandle(pause)
}

/// Atomic 0600 replace (policy_api's pattern): a unique `create_new`, no-follow temporary
/// file, fsync, rename, then a best-effort directory sync. A crash leaves the old or the
/// new file, and a planted symlink or stale temp is never written through.
pub(crate) fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = parent.join(format!(".{name}.{}.tmp", random_id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(nix::libc::O_NOFOLLOW);
    }
    let written = (|| {
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        #[cfg(test)]
        {
            let pause = BEFORE_RENAME
                .lock()
                .as_mut()
                .and_then(|hooks| hooks.remove(path));
            if let Some(pause) = pause {
                pause.block();
                if pause.fail.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(std::io::Error::other("injected pre-rename failure"));
                }
            }
        }
        std::fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
        return written;
    }
    let _ = std::fs::File::open(parent).and_then(|dir| dir.sync_all());
    Ok(())
}

pub(crate) fn random_id() -> String {
    let mut bytes = [0_u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
