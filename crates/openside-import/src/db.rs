#[cfg(unix)]
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rusqlite::backup::{Backup, StepResult};
use rusqlite::{Connection, ErrorCode, OpenFlags};
#[cfg(not(windows))]
use tempfile::TempDir;

use crate::error::{ImportError, Result};
use crate::BrowserKind;

#[cfg(windows)]
#[path = "db_windows.rs"]
mod windows;
#[cfg(windows)]
use windows::{open_source, PrivateDir as TempDir, SourceStamp};

const MAX_SOURCE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);
const BACKUP_TIMEOUT: Duration = Duration::from_secs(30);
const BUSY_RETRIES: usize = 3;
const BACKUP_PAGES: i32 = 128;

/// Inventory origins without opening Chromium's live databases for writing.
/// Quota storage keys may include partition suffixes; only their origin is returned.
pub fn profile_storage_origins(profile: &Path) -> Result<Vec<String>> {
    let mut origins = std::collections::BTreeSet::new();
    for (relative, table, column) in [
        ("WebStorage/QuotaManager", "buckets", "storage_key"),
        ("History", "urls", "url"),
    ] {
        let item = Item {
            browser: BrowserKind::Chrome,
            what: "storage origin inventory",
            path: profile.join(relative),
        };
        let read = || -> Result<Vec<String>> {
            if !item
                .path
                .try_exists()
                .map_err(|error| item.unreadable(&error))?
            {
                return Ok(Vec::new());
            }
            let snapshot = match item.open() {
                Err(ImportError::Locked { .. }) => item.inventory_copy()?,
                result => result?,
            };
            let exists: bool = snapshot
                .conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                    [table],
                    |row| row.get(0),
                )
                .map_err(|error| item.sqlite(&error))?;
            if !exists {
                return Ok(Vec::new());
            }
            let mut query = snapshot
                .conn
                .prepare(&format!("SELECT DISTINCT {column} FROM {table}"))
                .map_err(|error| item.sqlite(&error))?;
            let rows = query
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|error| item.sqlite(&error))?;
            let mut found = Vec::new();
            for row in rows {
                let value = row.map_err(|error| item.sqlite(&error))?;
                let value = value.split('^').next().unwrap_or_default();
                if let Ok(url) = url::Url::parse(value) {
                    if matches!(url.scheme(), "http" | "https") {
                        found.push(url.origin().ascii_serialization());
                    }
                }
            }
            Ok(found)
        };
        match read() {
            Ok(found) => origins.extend(found),
            Err(_) => eprintln!("openside: a browser storage origin inventory source was unavailable; continuing with recorded origins"),
        }
    }
    Ok(origins.into_iter().collect())
}

/// SQLite's backup API includes committed WAL pages in one consistent view without
/// copying independently changing sidecars. Only the private destination is writable.
pub(crate) struct CopiedDb {
    pub(crate) conn: Connection,
    _dir: TempDir,
}

#[cfg(unix)]
#[derive(PartialEq, Eq)]
struct SourceStamp {
    size: u64,
    modified: std::time::SystemTime,
    device: u64,
    inode: u64,
    header: [u8; 100],
}

#[cfg(unix)]
impl SourceStamp {
    fn read(file: &File) -> std::io::Result<Self> {
        use std::os::unix::fs::FileExt;

        let metadata = file.metadata()?;
        let mut header = [0; 100];
        file.read_exact_at(&mut header, 0)?;
        Ok(Self {
            size: metadata.len(),
            modified: metadata.modified()?,
            device: metadata.dev(),
            inode: metadata.ino(),
            header,
        })
    }
}

pub(crate) struct Item {
    pub(crate) browser: BrowserKind,
    pub(crate) what: &'static str,
    pub(crate) path: PathBuf,
}

impl Item {
    // Inventory can tolerate an incomplete copy; credential imports must still use open().
    fn inventory_copy(&self) -> Result<CopiedDb> {
        let (dir, copy, mut output) = self.private_file()?;
        let input = open_source(&self.path).map_err(|error| self.read_error(&error))?;
        let bytes = std::io::copy(&mut input.take(MAX_SOURCE_BYTES + 1), &mut output)
            .map_err(|error| self.unreadable(&error))?;
        self.check_bytes(&self.path, bytes, MAX_SOURCE_BYTES)?;
        output.sync_all().map_err(|error| self.unreadable(&error))?;
        let source_wal = PathBuf::from(format!("{}-wal", self.path.display()));
        match open_source(&source_wal) {
            Ok(input) => {
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                options.mode(0o600);
                let mut wal = options
                    .open(PathBuf::from(format!("{}-wal", copy.display())))
                    .map_err(|error| self.unreadable(&error))?;
                let bytes = std::io::copy(&mut input.take(MAX_SOURCE_BYTES + 1), &mut wal)
                    .map_err(|error| self.unreadable(&error))?;
                self.check_bytes(&source_wal, bytes, MAX_SOURCE_BYTES)?;
                wal.sync_all().map_err(|error| self.unreadable(&error))?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(self.unreadable(&error)),
        }
        let conn = Connection::open_with_flags(
            &copy,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| self.sqlite(&error))?;
        let check: String = conn
            .query_row("PRAGMA quick_check", [], |row| row.get(0))
            .map_err(|error| self.sqlite(&error))?;
        if check != "ok" {
            return Err(self.unsupported("storage inventory snapshot failed integrity check"));
        }
        Ok(CopiedDb { conn, _dir: dir })
    }

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
        #[cfg(windows)]
        if matches!(error.raw_os_error(), Some(32 | 33)) {
            return ImportError::SourceLocked {
                browser: self.browser.family(),
                what: self.what,
                path: self.path.clone(),
            };
        }
        ImportError::Unreadable {
            what: self.what,
            path: self.path.clone(),
            reason: error.to_string(),
        }
    }

    /// Whether the source file exists. Symlinks and other non-regular files are refused:
    /// the profile directory is untrusted input and must not redirect the read elsewhere.
    pub(crate) fn probe(&self) -> Result<bool> {
        match open_source(&self.path) {
            Ok(_) => Ok(true),
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
    pub(crate) fn read_bounded(&self, limit: u64) -> Result<Vec<u8>> {
        let file = open_source(&self.path).map_err(|error| self.read_error(&error))?;
        if file
            .metadata()
            .map_err(|error| self.unreadable(&error))?
            .len()
            > limit
        {
            return Err(self.unsupported("source file exceeds the import size limit"));
        }
        let mut bytes = Vec::new();
        file.take(limit.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| self.unreadable(&error))?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
            return Err(self.unsupported("source file exceeds the import size limit"));
        }
        Ok(bytes)
    }

    fn read_error(&self, error: &std::io::Error) -> ImportError {
        if error.kind() == std::io::ErrorKind::NotFound {
            self.missing()
        } else {
            self.unreadable(error)
        }
    }

    pub(crate) fn open(&self) -> Result<CopiedDb> {
        #[cfg(any(unix, windows))]
        {
            let input = open_source(&self.path).map_err(|error| self.read_error(&error))?;
            self.check_size(&self.path, &input, MAX_SOURCE_BYTES)?;
            #[cfg(windows)]
            if self.sidecar_present("-wal")? && !self.sidecar_present("-shm")? {
                // Windows SQLite uses OPEN_ALWAYS for SHM, even with readonly_shm.
                return Err(self.locked());
            }
            if input
                .metadata()
                .map_err(|error| self.unreadable(&error))?
                .len()
                >= 100
            {
                let stamp = SourceStamp::read(&input).map_err(|error| self.unreadable(&error))?;
                // SQLite creates a missing WAL even with mode=ro. Decide this path
                // before its VFS sees the source, not after a failed normal backup.
                if &stamp.header[..16] == b"SQLite format 3\0"
                    && stamp.header[18..20] == [2, 2]
                    && !self.sidecar_present("-wal")?
                {
                    if !self.sidecars_absent()? {
                        return Err(self.locked());
                    }
                    return self.open_quiescent();
                }
            }
        }
        self.open_snapshot()
    }

    fn open_snapshot(&self) -> Result<CopiedDb> {
        let input = open_source(&self.path).map_err(|error| self.read_error(&error))?;
        self.check_size(&self.path, &input, MAX_SOURCE_BYTES)?;
        // SQLite opens these itself; refuse unsafe or oversized sidecars before that
        // boundary. Its later path-based reopen still has the documented same-user race.
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut path = self.path.clone().into_os_string();
            path.push(suffix);
            let path = PathBuf::from(path);
            match open_source(&path) {
                Ok(file) => self.check_size(&path, &file, MAX_SOURCE_BYTES)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(Item {
                        browser: self.browser,
                        what: self.what,
                        path,
                    }
                    .unreadable(&error));
                }
            }
        }

        let path = if self.path.is_absolute() {
            self.path.clone()
        } else {
            std::env::current_dir()
                .map_err(|error| self.unreadable(&error))?
                .join(&self.path)
        };
        let mut uri = url::Url::from_file_path(path)
            .map_err(|()| self.unsupported("source path cannot be represented as a file URI"))?;
        // immutable=1 would ignore uncheckpointed WAL pages. readonly_shm prevents
        // SQLite from creating or writing SHM in a browser's source directory.
        uri.set_query(Some("mode=ro&immutable=0&readonly_shm=1"));
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        #[cfg(target_os = "linux")]
        let flags = flags | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let source = Connection::open_with_flags(uri.as_str(), flags)
            .map_err(|error| self.sqlite(&error))?;
        source
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(|error| self.sqlite(&error))?;
        let reopened = open_source(&self.path).map_err(|error| self.read_error(&error))?;
        #[cfg(windows)]
        if windows::identity(&input).map_err(|error| self.unreadable(&error))?
            != windows::identity(&reopened).map_err(|error| self.unreadable(&error))?
        {
            return Err(self.unreadable(&std::io::Error::other(
                "source database changed while opening it",
            )));
        }
        #[cfg(unix)]
        {
            let before = input.metadata().map_err(|error| self.unreadable(&error))?;
            let after = reopened
                .metadata()
                .map_err(|error| self.unreadable(&error))?;
            if (before.dev(), before.ino()) != (after.dev(), after.ino()) {
                return Err(self.unreadable(&std::io::Error::other(
                    "source database changed while opening it",
                )));
            }
        }
        self.check_size(&self.path, &reopened, MAX_SOURCE_BYTES)?;
        // A read transaction pins the WAL view so writes cannot endlessly restart
        // a multi-step backup. WAL writers can continue; checkpoints may wait.
        source
            .execute_batch("PRAGMA query_only=ON; BEGIN DEFERRED;")
            .map_err(|error| self.sqlite(&error))?;
        let page_count: u64 = source
            .query_row("PRAGMA page_count", [], |row| row.get(0))
            .map_err(|error| self.sqlite(&error))?;
        let page_size: u64 = source
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .map_err(|error| self.sqlite(&error))?;
        self.check_bytes(
            &self.path,
            page_count.saturating_mul(page_size),
            MAX_SOURCE_BYTES,
        )?;

        let (dir, copy, _output) = self.private_file()?;
        let mut conn = Connection::open_with_flags(
            &copy,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| self.sqlite(&error))?;
        conn.busy_timeout(BUSY_TIMEOUT)
            .map_err(|error| self.sqlite(&error))?;
        {
            let backup = Backup::new(&source, &mut conn).map_err(|error| self.sqlite(&error))?;
            let started = Instant::now();
            let mut retries = 0;
            loop {
                if started.elapsed() >= BACKUP_TIMEOUT {
                    return Err(self.locked());
                }
                let result = backup
                    .step(BACKUP_PAGES)
                    .map_err(|error| self.sqlite(&error))?;
                self.check_bytes(
                    &self.path,
                    u64::try_from(backup.progress().pagecount)
                        .unwrap_or(u64::MAX)
                        .saturating_mul(page_size),
                    MAX_SOURCE_BYTES,
                )?;
                match result {
                    StepResult::Done => break,
                    StepResult::More => {}
                    StepResult::Busy | StepResult::Locked if retries < BUSY_RETRIES => {
                        retries += 1;
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    _ => return Err(self.locked()),
                }
            }
        }
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|error| self.sqlite(&error))?;
        Ok(CopiedDb { conn, _dir: dir })
    }

    fn private_file(&self) -> Result<(TempDir, PathBuf, File)> {
        #[cfg(windows)]
        let dir = windows::private_dir().map_err(|error| self.unreadable(&error))?;
        #[cfg(not(windows))]
        let dir = tempfile::Builder::new()
            .prefix("openside-import-")
            .tempdir()
            .map_err(|error| self.unreadable(&error))?;
        #[cfg(unix)]
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700))
            .map_err(|error| self.unreadable(&error))?;
        let copy = dir.path().join("snapshot.sqlite");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let output = options
            .open(&copy)
            .map_err(|error| self.unreadable(&error))?;
        #[cfg(windows)]
        windows::require_owner_acl(&copy).map_err(|error| self.unreadable(&error))?;
        Ok((dir, copy, output))
    }

    #[cfg(any(unix, windows))]
    fn sidecar_present(&self, suffix: &str) -> Result<bool> {
        let mut path = self.path.clone().into_os_string();
        path.push(suffix);
        match open_source(Path::new(&path)) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(self.unreadable(&error)),
        }
    }

    #[cfg(any(unix, windows))]
    fn sidecars_absent(&self) -> Result<bool> {
        for suffix in ["-wal", "-shm", "-journal"] {
            if self.sidecar_present(suffix)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    #[cfg(any(unix, windows))]
    fn open_quiescent(&self) -> Result<CopiedDb> {
        for _ in 0..2 {
            if !self.sidecars_absent()? {
                return Err(self.locked());
            }
            let input = open_source(&self.path).map_err(|error| self.read_error(&error))?;
            self.check_size(&self.path, &input, MAX_SOURCE_BYTES)?;
            let before = SourceStamp::read(&input).map_err(|error| self.unreadable(&error))?;
            let (dir, copy, mut output) = self.private_file()?;
            let copied = std::io::copy(
                &mut (&input).take(MAX_SOURCE_BYTES.saturating_add(1)),
                &mut output,
            )
            .map_err(|error| self.unreadable(&error))?;
            self.check_bytes(&self.path, copied, MAX_SOURCE_BYTES)?;
            let after = SourceStamp::read(&input).ok();
            let current = open_source(&self.path)
                .and_then(|file| SourceStamp::read(&file))
                .ok();
            // A closed WAL database can need SHM even for read-only SQLite access.
            // Copy only a stable main file; never mix it with live WAL/journal data.
            if copied != before.size
                || after.as_ref() != Some(&before)
                || current.as_ref() != Some(&before)
                || !self.sidecars_absent()?
            {
                continue;
            }
            let conn = Connection::open_with_flags(
                &copy,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(|error| self.sqlite(&error))?;
            conn.busy_timeout(BUSY_TIMEOUT)
                .map_err(|error| self.sqlite(&error))?;
            let pages: u64 = conn
                .query_row("PRAGMA page_count", [], |row| row.get(0))
                .map_err(|error| self.sqlite(&error))?;
            let page_size: u64 = conn
                .query_row("PRAGMA page_size", [], |row| row.get(0))
                .map_err(|error| self.sqlite(&error))?;
            self.check_bytes(
                &self.path,
                pages.saturating_mul(page_size),
                MAX_SOURCE_BYTES,
            )?;
            conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(|error| self.sqlite(&error))?;
            return Ok(CopiedDb { conn, _dir: dir });
        }
        Err(self.locked())
    }

    fn locked(&self) -> ImportError {
        ImportError::Locked {
            browser: self.browser.family(),
            what: self.what,
            path: self.path.clone(),
        }
    }

    fn check_size(&self, path: &Path, file: &File, limit: u64) -> Result<()> {
        self.check_bytes(
            path,
            file.metadata()
                .map_err(|error| self.unreadable(&error))?
                .len(),
            limit,
        )
    }

    fn check_bytes(&self, path: &Path, size: u64, limit: u64) -> Result<()> {
        if size > limit {
            return Err(ImportError::SourceTooLarge {
                what: self.what,
                path: path.to_owned(),
                size,
                limit,
            });
        }
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
fn open_source(_path: &Path) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Symlink-safe profile import is unsupported on this platform",
    ))
}

#[cfg(target_os = "linux")]
fn open_source(path: &Path) -> std::io::Result<File> {
    open_source_linux(path, true)
}

#[cfg(target_os = "linux")]
fn open_source_linux(path: &Path, use_openat2: bool) -> std::io::Result<File> {
    use rustix::fs::{open, Mode, OFlags};

    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let parent = absolute
        .parent()
        .ok_or_else(|| std::io::Error::other("source has no profile directory"))?;
    let name = absolute
        .file_name()
        .ok_or_else(|| std::io::Error::other("source has no filename"))?;
    let root = open(
        "/",
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let profile = open_beneath(
        &root,
        parent.strip_prefix("/").map_err(std::io::Error::other)?,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        use_openat2,
    )?;
    // The directory handle pins the verified parent during the source open.
    // NONBLOCK must precede fstat: opening an attacker-supplied FIFO can hang.
    let file = File::from(open_beneath(
        &profile,
        Path::new(name),
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        use_openat2,
    )?);
    require_regular(file)
}

#[cfg(target_os = "linux")]
fn open_beneath(
    directory: &std::os::fd::OwnedFd,
    path: &Path,
    flags: rustix::fs::OFlags,
    use_openat2: bool,
) -> std::io::Result<std::os::fd::OwnedFd> {
    use rustix::fs::{openat2, Mode, ResolveFlags};
    use rustix::io::Errno;

    if path.as_os_str().is_empty() {
        return Ok(rustix::io::dup(directory)?);
    }
    if use_openat2 {
        match openat2(
            directory,
            path,
            flags,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
        ) {
            Ok(file) => return Ok(file),
            Err(Errno::NOSYS | Errno::INVAL) => {}
            Err(error) => return Err(source_io_error(error.into())),
        }
    }
    open_beneath_fallback(directory, path, flags)
}

#[cfg(target_os = "linux")]
fn open_beneath_fallback(
    directory: &std::os::fd::OwnedFd,
    path: &Path,
    flags: rustix::fs::OFlags,
) -> std::io::Result<std::os::fd::OwnedFd> {
    use rustix::fs::{openat, Mode, OFlags};
    use std::path::Component;

    let mut directory = rustix::io::dup(directory)?;
    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        let name = match component {
            Component::Normal(name) => name,
            Component::CurDir => continue,
            _ => {
                return Err(std::io::Error::other(
                    "source path escapes its profile directory",
                ))
            }
        };
        let component_flags = if components.peek().is_some() {
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW
        } else {
            flags | OFlags::NOFOLLOW
        };
        directory = openat(&directory, name, component_flags, Mode::empty())
            .map_err(|error| source_io_error(error.into()))?;
    }
    Ok(directory)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn open_source(path: &Path) -> std::io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(path)
        .map_err(source_io_error)?;
    require_regular(file)
}

#[cfg(unix)]
fn source_io_error(error: std::io::Error) -> std::io::Error {
    if error.raw_os_error() == Some(nix::libc::ELOOP) {
        std::io::Error::other("is a symbolic link, which is not followed")
    } else {
        error
    }
}

#[cfg(unix)]
fn require_regular(file: File) -> std::io::Result<File> {
    if !file.metadata()?.file_type().is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    Ok(file)
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

#[cfg(all(test, not(any(unix, windows))))]
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use std::os::unix::fs::symlink;
    use std::sync::{mpsc, Arc, Barrier};

    fn item(path: PathBuf) -> Item {
        Item {
            browser: BrowserKind::Chrome,
            what: "history",
            path,
        }
    }

    fn fixture_dir() -> TempDir {
        tempfile::tempdir_in(fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap()
    }

    #[test]
    fn private_snapshot_preserves_read_only_source() {
        let dir = fixture_dir();
        let path = dir.path().join("History ?#%.sqlite");
        let writer = Connection::open(&path).unwrap();
        writer
            .execute_batch("CREATE TABLE records(value); INSERT INTO records VALUES(42);")
            .unwrap();
        drop(writer);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        let before = fs::read(&path).unwrap();
        let snapshot = item(path.clone()).open().unwrap();
        assert_eq!(
            snapshot
                .conn
                .query_row("SELECT value FROM records", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            42
        );
        let copy = PathBuf::from(snapshot.conn.path().unwrap());
        assert_eq!(
            fs::metadata(&copy).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(copy.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o444
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        drop(snapshot);
        assert!(!copy.exists());
    }

    #[test]
    fn live_wal_snapshot_preserves_source_bytes_and_entries() {
        let dir = fixture_dir();
        let path = dir.path().join("History");
        let writer = Connection::open(&path).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
             CREATE TABLE records(value); INSERT INTO records VALUES(73);",
            )
            .unwrap();
        writer
            .query_row("SELECT value FROM records", [], |row| row.get::<_, i64>(0))
            .unwrap();
        let before: std::collections::BTreeMap<_, _> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), fs::read(entry.path()).unwrap())
            })
            .collect();
        assert!(before.contains_key(std::ffi::OsStr::new("History-wal")));
        assert!(before.contains_key(std::ffi::OsStr::new("History-shm")));
        let snapshot = item(path).open().unwrap();
        assert_eq!(
            snapshot
                .conn
                .query_row("SELECT value FROM records", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            73
        );
        drop(snapshot);
        let after: std::collections::BTreeMap<_, _> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), fs::read(entry.path()).unwrap())
            })
            .collect();
        assert_eq!(after, before);
    }

    #[test]
    fn closed_wal_database_imports_without_creating_source_sidecars() {
        let dir = fixture_dir();
        let path = dir.path().join("History");
        let writer = Connection::open(&path).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL;
             CREATE TABLE records(value); INSERT INTO records VALUES(91);",
            )
            .unwrap();
        drop(writer);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        let before = fs::read(&path).unwrap();
        assert_eq!(&before[18..20], &[2, 2]);
        let snapshot = item(path.clone()).open().unwrap();
        assert_eq!(
            snapshot
                .conn
                .query_row("SELECT value FROM records", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            91
        );
        assert_eq!(
            snapshot
                .conn
                .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o444
        );
    }

    #[test]
    fn missing_wal_with_existing_shm_fails_without_creating_a_wal() {
        let dir = fixture_dir();
        let path = dir.path().join("History");
        let writer = Connection::open(&path).unwrap();
        writer
            .execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE records(value);")
            .unwrap();
        drop(writer);
        let before = fs::read(&path).unwrap();
        let shm = dir.path().join("History-shm");
        fs::write(&shm, b"orphan synthetic sidecar").unwrap();
        assert!(matches!(
            item(path.clone()).open().err().unwrap(),
            ImportError::Locked { .. }
        ));
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::read(&shm).unwrap(), b"orphan synthetic sidecar");
        assert!(!dir.path().join("History-wal").exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn consistent_snapshot_with_concurrent_wal_writer() {
        let dir = fixture_dir();
        let path = dir.path().join("History");
        let writer = Connection::open(&path).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
             CREATE TABLE records(id INTEGER PRIMARY KEY, generation INTEGER, payload BLOB);
             WITH RECURSIVE ids(id) AS (SELECT 1 UNION ALL SELECT id+1 FROM ids WHERE id<1024)
             INSERT INTO records SELECT id,0,zeroblob(4096) FROM ids;",
            )
            .unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let writer_barrier = barrier.clone();
        let thread = std::thread::spawn(move || {
            writer.busy_timeout(BUSY_TIMEOUT).unwrap();
            writer_barrier.wait();
            for generation in 1..=40 {
                writer.execute_batch("BEGIN IMMEDIATE;").unwrap();
                writer
                    .execute("UPDATE records SET generation=?1", [generation])
                    .unwrap();
                writer
                    .execute_batch("COMMIT; PRAGMA wal_checkpoint(PASSIVE);")
                    .unwrap();
                std::thread::sleep(Duration::from_millis(2));
            }
            writer
        });
        barrier.wait();
        let input = item(path);
        for _ in 0..8 {
            let snapshot = input.open().unwrap();
            let (count, min, max): (i64, i64, i64) = snapshot
                .conn
                .query_row(
                    "SELECT count(*),min(generation),max(generation) FROM records",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            assert_eq!(count, 1024);
            assert_eq!(min, max);
            assert_eq!(
                snapshot
                    .conn
                    .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                    .unwrap(),
                "ok"
            );
        }
        let _writer = thread.join().unwrap();
        assert_eq!(
            input
                .open()
                .unwrap()
                .conn
                .query_row("SELECT max(generation) FROM records", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            40
        );
    }

    #[test]
    fn oversize_source_is_rejected_before_sqlite_parsing() {
        let dir = fixture_dir();
        let path = dir.path().join("History");
        File::create(&path)
            .unwrap()
            .set_len(MAX_SOURCE_BYTES + 1)
            .unwrap();
        let error = item(path.clone()).open().err().unwrap();
        assert!(matches!(error, ImportError::SourceTooLarge {
            path: rejected, size, limit, ..
        } if rejected == path && size == MAX_SOURCE_BYTES + 1 && limit == MAX_SOURCE_BYTES));
    }

    #[test]
    fn oversize_wal_is_rejected_before_sqlite_parsing() {
        let dir = fixture_dir();
        let path = dir.path().join("History");
        File::create(&path).unwrap();
        let wal = dir.path().join("History-wal");
        File::create(&wal)
            .unwrap()
            .set_len(MAX_SOURCE_BYTES + 1)
            .unwrap();
        assert!(
            matches!(item(path).open().err().unwrap(), ImportError::SourceTooLarge {
            path: rejected, ..
        } if rejected == wal)
        );
    }

    #[test]
    fn fifo_is_rejected_without_waiting_for_a_writer() {
        let dir = fixture_dir();
        let path = dir.path().join("History");
        nix::unistd::mkfifo(
            &path,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .unwrap();
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || send.send(item(path).open().err().unwrap()).unwrap());
        let error = receive
            .recv_timeout(Duration::from_secs(2))
            .expect("FIFO open blocked");
        assert!(
            matches!(error, ImportError::Unreadable { reason, .. } if reason.contains("not a regular file"))
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn symlinked_parent_is_rejected_by_openat2_and_fallback() {
        let dir = fixture_dir();
        let profile = dir.path().join("Profile");
        fs::create_dir(&profile).unwrap();
        let network = dir.path().join("Outside");
        fs::create_dir(&network).unwrap();
        fs::write(network.join("Cookies"), b"not read").unwrap();
        symlink(&network, profile.join("Network")).unwrap();
        let path = profile.join("Network/Cookies");
        assert!(matches!(
            item(path.clone()).open().err().unwrap(),
            ImportError::Unreadable { .. }
        ));
        for use_openat2 in [true, false] {
            assert!(open_source_linux(&path, use_openat2).is_err());
        }
        let alias = dir.path().join("ProfileAlias");
        symlink(&profile, &alias).unwrap();
        fs::write(profile.join("History"), b"not read").unwrap();
        for use_openat2 in [true, false] {
            assert!(open_source_linux(&alias.join("History"), use_openat2).is_err());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fallback_opens_regular_files_and_refuses_leaf_symlinks() {
        let dir = fixture_dir();
        let path = dir.path().join("History");
        fs::write(&path, b"fixture").unwrap();
        let mut file = open_source_linux(&path, false).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"fixture");
        let alias = dir.path().join("Link");
        symlink(&path, &alias).unwrap();
        assert!(open_source_linux(&alias, false).is_err());
    }
    #[test]
    fn connections_storage_inventory_reads_live_wal_origins_only() {
        let dir = fixture_dir();
        fs::create_dir(dir.path().join("WebStorage")).unwrap();
        let quota = Connection::open(dir.path().join("WebStorage/QuotaManager")).unwrap();
        quota.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE buckets(storage_key TEXT); INSERT INTO buckets VALUES('https://sub.example.test/^0https://example.test'),('https://other.test/');").unwrap();
        let history = Connection::open(dir.path().join("History")).unwrap();
        history.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE urls(url TEXT); INSERT INTO urls VALUES('https://example.test/private?token=SECRET'),('file:///private');").unwrap();
        assert_eq!(
            profile_storage_origins(dir.path()).unwrap(),
            vec![
                "https://example.test",
                "https://other.test",
                "https://sub.example.test",
            ]
        );
        assert_eq!(
            quota
                .query_row("SELECT COUNT(*) FROM buckets", [], |row| row
                    .get::<_, u64>(0))
                .unwrap(),
            2
        );
    }
    #[test]
    fn connections_storage_inventory_validates_locked_copy_and_skips_corruption() {
        let dir = fixture_dir();
        fs::create_dir(dir.path().join("WebStorage")).unwrap();
        let quota = Connection::open(dir.path().join("WebStorage/QuotaManager")).unwrap();
        quota.execute_batch("PRAGMA locking_mode=EXCLUSIVE; CREATE TABLE buckets(storage_key TEXT); INSERT INTO buckets VALUES('https://sub.example.test/');").unwrap();
        fs::write(dir.path().join("History"), b"invalid SQLite inventory").unwrap();
        assert_eq!(
            profile_storage_origins(dir.path()).unwrap(),
            vec!["https://sub.example.test"]
        );
    }
}
