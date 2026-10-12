//! Where an openside profile keeps its imported data.
//!
//! * Chromium user-data-dir: `<data>/browser-profiles/<name>`; the profile named `default`
//!   is the daemon's existing `<data>/browser-profile`. Cookies live here, written by
//!   Chromium itself.
//! * Imported history and bookmarks: `<data>/browser-imports/<name>/`, one file per source
//!   family (`history.<source>.jsonl`, `bookmarks.<source>.json`), replaced atomically on
//!   every import so re-importing never duplicates entries.
//!
//! History deliberately does not go into the daemon-wide memory index: that index has no
//! profile dimension (importing into profile A would surface in profile B), expires pages
//! after 30 days, and is searched by the agent. Bookmarks are not written into Chromium's
//! `Bookmarks` file because Chromium validates it with a checksum and rewrites it.

use std::fs::{self, File};
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde_json::json;
use tempfile::NamedTempFile;

use crate::error::{ImportError, Result};
use crate::model::{Bookmark, Visit};
use crate::BrowserKind;

/// Name of the profile that maps to the daemon's pre-existing browser profile directory.
pub const DEFAULT_PROFILE: &str = "default";

/// Directories of one openside browser profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileLayout {
    /// The profile name.
    pub name: String,
    /// Chromium `--user-data-dir` of the profile.
    pub browser_dir: PathBuf,
    /// Directory of imported history and bookmarks.
    pub import_dir: PathBuf,
}

impl ProfileLayout {
    /// Resolve a profile under the daemon data directory.
    pub fn new(data_dir: &Path, name: &str) -> Result<Self> {
        let valid = !name.is_empty()
            && name.len() <= 64
            && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
        if !valid {
            return Err(ImportError::InvalidProfileName(name.to_owned()));
        }
        let browser_dir = if name == DEFAULT_PROFILE {
            data_dir.join("browser-profile")
        } else {
            data_dir.join("browser-profiles").join(name)
        };
        Ok(Self {
            name: name.to_owned(),
            browser_dir,
            import_dir: data_dir.join("browser-imports").join(name),
        })
    }

    /// Imported history file for one source family.
    pub fn history_path(&self, source: BrowserKind) -> PathBuf {
        self.import_dir
            .join(format!("history.{}.jsonl", source.label()))
    }

    /// Imported bookmarks file for one source family.
    pub fn bookmarks_path(&self, source: BrowserKind) -> PathBuf {
        self.import_dir
            .join(format!("bookmarks.{}.json", source.label()))
    }
}

fn write_error(path: &Path, error: &std::io::Error) -> ImportError {
    ImportError::Write {
        path: path.to_owned(),
        reason: error.to_string(),
    }
}

/// Create `path` (and the `browser-imports` directory above it) owner-only.
fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|error| write_error(path, &error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for dir in std::iter::once(path).chain(path.parent()) {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
                .map_err(|error| write_error(dir, &error))?;
        }
    }
    Ok(())
}

/// Visits staged in a private scratch file while the source is being read, so nothing is
/// created under the target profile until every read and the cookie write have succeeded.
pub(crate) struct HistoryStage {
    out: BufWriter<NamedTempFile>,
}

impl HistoryStage {
    pub(crate) fn new() -> Result<Self> {
        let file =
            NamedTempFile::new().map_err(|error| write_error(&std::env::temp_dir(), &error))?;
        Ok(Self {
            out: BufWriter::new(file),
        })
    }

    pub(crate) fn push(&mut self, visit: &Visit) -> Result<()> {
        let path = self.out.get_ref().path().to_owned();
        serde_json::to_writer(&mut self.out, visit).map_err(|error| ImportError::Write {
            path: path.clone(),
            reason: error.to_string(),
        })?;
        self.out
            .write_all(b"\n")
            .map_err(|error| write_error(&path, &error))
    }

    /// Copy the staged visits into `dir` and rename them over `target`.
    pub(crate) fn commit(self, target: &Path, dir: &Path) -> Result<()> {
        let mut staged = self
            .out
            .into_inner()
            .map_err(|error| write_error(target, error.error()))?;
        staged
            .as_file_mut()
            .seek(SeekFrom::Start(0))
            .map_err(|error| write_error(target, &error))?;
        persist(dir, target, |out| {
            std::io::copy(staged.as_file_mut(), out).map(|_| ())
        })
    }
}

/// Write through an exclusively created 0600 temp file in `dir`, sync it, and rename it over
/// `target`, so a crash or a concurrent import never leaves a half-written or shared file.
fn persist(
    dir: &Path,
    target: &Path,
    fill: impl FnOnce(&mut File) -> std::io::Result<()>,
) -> Result<()> {
    private_dir(dir)?;
    let mut temp = tempfile::Builder::new()
        .prefix(".import-")
        .suffix(".tmp")
        .tempfile_in(dir)
        .map_err(|error| write_error(dir, &error))?;
    fill(temp.as_file_mut())
        .and_then(|()| temp.as_file().sync_all())
        .map_err(|error| write_error(target, &error))?;
    temp.persist(target)
        .map(|_| ())
        .map_err(|error| write_error(target, &error.error))
}

pub(crate) fn write_bookmarks(
    target: &Path,
    dir: &Path,
    source: BrowserKind,
    imported_at_ms: i64,
    bookmarks: &[Bookmark],
) -> Result<()> {
    let document = json!({
        "version": 1,
        "source": source.label(),
        "imported_at_ms": imported_at_ms,
        "bookmarks": bookmarks,
    });
    persist(dir, target, |file| {
        serde_json::to_writer_pretty(&mut *file, &document).map_err(std::io::Error::other)?;
        file.flush()
    })
}
