use std::fs::{self, File, OpenOptions};
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, ErrorCode, OpenFlags};
use tempfile::TempDir;

use crate::error::{ImportError, Result};
use crate::BrowserKind;

/// A scratch copy of a source SQLite database. The source is only ever read, because the
/// browser may be running and holding it locked; the copy lives in a private temp
/// directory and is deleted on drop.
pub(crate) struct CopiedDb {
    pub(crate) conn: Connection,
    _dir: TempDir,
}

pub(crate) struct Item {
    pub(crate) browser: BrowserKind,
    pub(crate) what: &'static str,
    pub(crate) path: PathBuf,
}

impl Item {
    pub(crate) fn missing(&self) -> ImportError {
        ImportError::MissingFile {
            browser: self.browser.family(),
            what: self.what,
            path: self.path.clone(),
        }
    }

    pub(crate) fn unsupported(&self, detail: impl Into<String>) -> ImportError {
        ImportError::UnsupportedVersion {
            browser: self.browser.family(),
            what: self.what,
            path: self.path.clone(),
            detail: detail.into(),
        }
    }

    pub(crate) fn sqlite(&self, error: &rusqlite::Error) -> ImportError {
        match error.sqlite_error_code() {
            Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => ImportError::Locked {
                browser: self.browser.family(),
                what: self.what,
                path: self.path.clone(),
            },
            _ => ImportError::Corrupt {
                what: self.what,
                path: self.path.clone(),
                format: "SQLite",
                detail: error.to_string(),
            },
        }
    }

    pub(crate) fn unreadable(&self, error: &std::io::Error) -> ImportError {
        ImportError::Unreadable {
            what: self.what,
            path: self.path.clone(),
            reason: error.to_string(),
        }
    }

    /// Whether the source file exists. Symlinks and other non-regular files are refused:
    /// the profile directory is untrusted input and must not redirect the read elsewhere.
    pub(crate) fn probe(&self) -> Result<bool> {
        match fs::symlink_metadata(&self.path) {
            Ok(metadata) if metadata.file_type().is_file() => Ok(true),
            Ok(metadata) => Err(ImportError::Unreadable {
                what: self.what,
                path: self.path.clone(),
                reason: if metadata.file_type().is_symlink() {
                    "is a symbolic link, which is not followed".into()
                } else {
                    "is not a regular file".into()
                },
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(self.unreadable(&error)),
        }
    }

    /// Read the whole source file without following a symlink.
    pub(crate) fn read(&self) -> Result<Vec<u8>> {
        let mut file = open_source(&self.path).map_err(|error| self.read_error(&error))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|error| self.unreadable(&error))?;
        Ok(bytes)
    }

    fn read_error(&self, error: &std::io::Error) -> ImportError {
        if error.kind() == std::io::ErrorKind::NotFound {
            self.missing()
        } else {
            self.unreadable(error)
        }
    }

    /// Copy the database with its WAL/journal siblings and open the copy.
    pub(crate) fn open(&self) -> Result<CopiedDb> {
        if !self.probe()? {
            return Err(self.missing());
        }
        let dir = tempfile::Builder::new()
            .prefix("openside-import-")
            .tempdir()
            .map_err(|error| self.unreadable(&error))?;
        let name = self
            .path
            .file_name()
            .ok_or_else(|| self.missing())?
            .to_owned();
        let copy = dir.path().join(&name);
        copy_private(&self.path, &copy).map_err(|error| self.read_error(&error))?;
        // Uncheckpointed writes live in the sidecar files; without them the copy is stale.
        // A running browser can delete a sidecar between listing and copying, so a vanished
        // one is simply absent.
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut sibling = self.path.clone().into_os_string();
            sibling.push(suffix);
            let sibling = PathBuf::from(sibling);
            let mut target = copy.clone().into_os_string();
            target.push(suffix);
            match copy_private(&sibling, Path::new(&target)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(ImportError::Unreadable {
                        what: self.what,
                        path: sibling,
                        reason: error.to_string(),
                    })
                }
            }
        }
        // Read-write on the private copy lets SQLite replay a copied WAL or hot journal.
        let conn = Connection::open_with_flags(
            &copy,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| self.sqlite(&error))?;
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|error| self.sqlite(&error))?;
        Ok(CopiedDb { conn, _dir: dir })
    }
}

#[cfg(not(unix))]
fn open_source(_path: &Path) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Symlink-safe profile import is unsupported on this platform",
    ))
}

#[cfg(unix)]
fn open_source(path: &Path) -> std::io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(path)?;
    if !file.metadata()?.file_type().is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    Ok(file)
}

/// Copy with a fresh 0600 destination: the source's permission bits (a world-readable
/// plaintext cookie DB, or a read-only backup SQLite could not recover) must not carry over.
fn copy_private(source: &Path, target: &Path) -> std::io::Result<()> {
    let mut input = open_source(source)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut output = options.open(target)?;
    std::io::copy(&mut input, &mut output)?;
    Ok(())
}

impl CopiedDb {
    pub(crate) fn columns(&self, item: &Item, table: &str) -> Result<Vec<String>> {
        let mut statement = self
            .conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|error| item.sqlite(&error))?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|error| item.sqlite(&error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| item.sqlite(&error))?;
        Ok(columns)
    }

    /// Verify `table` exists with every column in `required`; return all its columns.
    pub(crate) fn require(
        &self,
        item: &Item,
        table: &str,
        required: &[&str],
    ) -> Result<Vec<String>> {
        let columns = self.columns(item, table)?;
        if columns.is_empty() {
            return Err(item.unsupported(format!("table `{table}` is missing")));
        }
        for column in required {
            if !columns.iter().any(|have| have == column) {
                return Err(item.unsupported(format!("table `{table}` has no column `{column}`")));
            }
        }
        Ok(columns)
    }
}

pub(crate) fn sqlite_text(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

#[cfg(all(test, not(unix)))]
mod tests {
    use super::*;

    #[test]
    fn unsupported_safe_open_does_not_read_the_source() {
        let error = open_source(Path::new("not-a-source")).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
        assert_eq!(
            error.to_string(),
            "Symlink-safe profile import is unsupported on this platform"
        );
    }
}
