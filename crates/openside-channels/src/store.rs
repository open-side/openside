use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};

use crate::{
    error::{ChannelError, Result},
    model::ThreadKey,
};

const VERSION: u32 = 1;
const MAX_CARDS: usize = 200;

/// An approval card the bot posted for a ticket.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Card {
    /// Platform message handle.
    pub message: String,
    /// Text the card shows (kept so a decision can append to it).
    pub text: String,
    /// Settled (resolved, rejected, expired); the card has no buttons any more.
    pub done: bool,
}

/// Everything the adapter remembers about one conversation thread.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThreadState {
    /// The thread.
    pub key: ThreadKey,
    /// Mapped openside session, if one was started.
    pub session_id: Option<String>,
    /// `/model` choice applied to the next session created in this thread.
    pub model: Option<String>,
    /// Last event sequence whose effect (reply, card) is already visible in chat.
    pub cursor: u64,
    /// Cards by ticket id.
    pub cards: BTreeMap<String, Card>,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    version: u32,
    threads: BTreeMap<String, ThreadState>,
    offsets: BTreeMap<String, i64>,
}

/// JSON thread→session map in the adapter state directory (mode 0600), rewritten
/// atomically on every change so a crash never leaves a torn file.
pub struct Store {
    path: PathBuf,
    inner: Mutex<File>,
}

impl Store {
    /// Open (creating the 0700 directory and file as needed).
    pub fn open(dir: &Path) -> Result<Self> {
        create_private_dir(dir)?;
        let path = dir.join("threads.json");
        let file = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<File>(&bytes).map_err(|_| {
                ChannelError::Config(
                    "thread map is corrupt; remove threads.json to reset it".into(),
                )
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => File {
                version: VERSION,
                ..File::default()
            },
            Err(error) => return Err(error.into()),
        };
        if file.version != VERSION {
            return Err(ChannelError::Config(
                "thread map has an unsupported version".into(),
            ));
        }
        Ok(Self {
            path,
            inner: Mutex::new(file),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, File> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Current state of a thread.
    pub fn get(&self, key: &ThreadKey) -> Option<ThreadState> {
        self.lock().threads.get(&key.id()).cloned()
    }

    /// Modify (creating if absent) and persist a thread.
    pub fn update<R>(
        &self,
        key: &ThreadKey,
        change: impl FnOnce(&mut ThreadState) -> R,
    ) -> Result<R> {
        let mut file = self.lock();
        let state = file.threads.entry(key.id()).or_insert_with(|| ThreadState {
            key: key.clone(),
            session_id: None,
            model: None,
            cursor: 0,
            cards: BTreeMap::new(),
        });
        let result = change(state);
        if state.cards.len() > MAX_CARDS {
            state.cards.retain(|_, card| !card.done);
        }
        self.persist(&file)?;
        Ok(result)
    }

    /// Every thread of one platform that has a session.
    pub fn sessions(&self, platform: &str) -> Vec<ThreadState> {
        self.lock()
            .threads
            .values()
            .filter(|state| state.key.platform == platform && state.session_id.is_some())
            .cloned()
            .collect()
    }

    /// Persisted long-poll offset of a platform.
    pub fn offset(&self, platform: &str) -> Option<i64> {
        self.lock().offsets.get(platform).copied()
    }

    /// Persist the long-poll offset of a platform.
    pub fn set_offset(&self, platform: &str, offset: i64) -> Result<()> {
        let mut file = self.lock();
        file.offsets.insert(platform.to_owned(), offset);
        self.persist(&file)
    }

    fn persist(&self, file: &File) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(file)
            .map_err(|_| ChannelError::Io("cannot encode thread map".into()))?;
        let temporary = self.path.with_extension("json.tmp");
        write_private(&temporary, &bytes)?;
        std::fs::rename(&temporary, &self.path)?;
        Ok(())
    }
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    Ok(())
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::{
        io::Write,
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
    };
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // `mode` only applies to a newly created file; a pre-existing looser file must be
    // tightened before the secret goes in.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Write a secret-bearing file readable only by the owner.
pub(crate) fn write_secret_file(path: &Path, bytes: &[u8]) -> Result<()> {
    write_private(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threads_and_offsets_survive_reopen_with_private_permissions() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let state_dir = dir.path().join("state");
        let key = ThreadKey::new("telegram", "42", "");
        {
            let store = Store::open(&state_dir).unwrap_or_else(|e| panic!("{e}"));
            store
                .update(&key, |s| {
                    s.session_id = Some("s1".into());
                    s.cursor = 9;
                })
                .unwrap_or_else(|e| panic!("{e}"));
            store
                .set_offset("telegram", 77)
                .unwrap_or_else(|e| panic!("{e}"));
        }
        let store = Store::open(&state_dir).unwrap_or_else(|e| panic!("{e}"));
        let state = store.get(&key).unwrap_or_else(|| panic!("thread lost"));
        assert_eq!(state.session_id.as_deref(), Some("s1"));
        assert_eq!(state.cursor, 9);
        assert_eq!(store.offset("telegram"), Some(77));
        assert_eq!(store.sessions("telegram").len(), 1);
        assert!(store.sessions("slack").is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).map(|m| m.permissions().mode() & 0o777);
            assert_eq!(mode(&state_dir).unwrap_or(0), 0o700);
            assert_eq!(mode(&state_dir.join("threads.json")).unwrap_or(0), 0o600);
        }
    }

    #[cfg(unix)]
    #[test]
    fn secret_file_overwriting_a_world_readable_file_is_tightened() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let path = dir.path().join("daemon.token");
        std::fs::write(&path, b"old").unwrap_or_else(|e| panic!("{e}"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .unwrap_or_else(|e| panic!("{e}"));
        write_secret_file(&path, b"new-secret").unwrap_or_else(|e| panic!("{e}"));
        let mode = std::fs::metadata(&path)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0);
        assert_eq!(mode, 0o600);
        assert_eq!(std::fs::read(&path).unwrap_or_default(), b"new-secret");
    }

    #[test]
    fn corrupt_file_is_reported_not_silently_reset() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(dir.path().join("threads.json"), b"not json")
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(Store::open(dir.path()).is_err());
    }
}
