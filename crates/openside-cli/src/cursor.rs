use std::{collections::BTreeMap, path::PathBuf};

#[cfg(unix)]
use std::os::unix::fs::DirBuilderExt;
#[cfg(unix)]
use std::{
    fs::{DirBuilder, File},
    io::Write,
};

#[cfg(unix)]
use fs2::FileExt;
use sha2::{Digest, Sha256};

pub(crate) struct Cursor {
    directory: PathBuf,
    file: PathBuf,
    session_id: String,
}

impl Cursor {
    pub(crate) fn new(endpoint: &str, session_id: &str) -> Result<Self, String> {
        let base = if let Some(state) =
            std::env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty())
        {
            PathBuf::from(state)
        } else {
            PathBuf::from(
                std::env::var_os("HOME")
                    .filter(|value| !value.is_empty())
                    .ok_or("set XDG_STATE_HOME to persist resume cursors")?,
            )
            .join(".local/state")
        };
        if !base.is_absolute() {
            return Err("XDG_STATE_HOME must be an absolute path".into());
        }
        let directory = base.join("openside/cli");
        let mut digest = Sha256::new();
        digest.update(endpoint.as_bytes());
        digest.update([0]);
        digest.update(session_id.as_bytes());
        let file = directory.join(format!("{:x}.json", digest.finalize()));
        Ok(Self {
            directory,
            file,
            session_id: session_id.into(),
        })
    }

    pub(crate) fn load(&self) -> Result<u64, String> {
        let bytes = match std::fs::read(&self.file) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(_) => return Err("could not read resume cursor".into()),
        };
        let cursor: BTreeMap<String, u64> = serde_json::from_slice(&bytes)
            .map_err(|_| "invalid persisted resume cursor".to_owned())?;
        cursor
            .get(&self.session_id)
            .copied()
            .ok_or("resume cursor has an unexpected session ID".into())
    }

    #[cfg(not(unix))]
    pub(crate) fn save(&self, _seq: u64) -> Result<(), String> {
        Err("Private resume cursor persistence is unsupported on this platform".into())
    }

    #[cfg(unix)]
    pub(crate) fn save(&self, seq: u64) -> Result<(), String> {
        let mut directory = DirBuilder::new();
        directory.recursive(true);
        directory.mode(0o700);
        directory
            .create(&self.directory)
            .map_err(|_| "could not create resume cursor directory")?;
        // Lock the directory, not the replaced file: rename must not invalidate the lock.
        let lock =
            File::open(&self.directory).map_err(|_| "could not open resume cursor directory")?;
        lock.lock_exclusive()
            .map_err(|_| "could not lock resume cursors")?;
        let seq = seq.max(self.load()?);
        let mut file = tempfile::NamedTempFile::new_in(&self.directory)
            .map_err(|_| "could not create resume cursor file")?;
        serde_json::to_writer(&mut file, &BTreeMap::from([(&self.session_id, seq)]))
            .map_err(|_| "could not write resume cursor")?;
        file.flush().map_err(|_| "could not flush resume cursor")?;
        file.persist(&self.file)
            .map_err(|_| "could not replace resume cursor")?;
        Ok(())
    }
}

#[cfg(all(test, not(unix)))]
mod tests {
    use super::*;

    #[test]
    fn unsupported_private_persistence_creates_no_directory_or_file() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("cursor");
        let cursor = Cursor {
            file: directory.join("resume.json"),
            directory: directory.clone(),
            session_id: "session".into(),
        };
        assert_eq!(
            cursor.save(1).unwrap_err(),
            "Private resume cursor persistence is unsupported on this platform"
        );
        assert!(!directory.exists());
    }
}
