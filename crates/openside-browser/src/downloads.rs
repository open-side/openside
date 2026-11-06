//! Downloads of script-driven tabs: attribution to an owner, waiting for a terminal state, and
//! saving into the owner's private download directory.
//!
//! Chromium writes every download as `<downloadPath>/<guid>` (`allowAndName`), so a download is
//! found by identifier even when two pages suggest the same file name. [`Browser::save_download`]
//! copies it to a name chosen by the caller, never outside the owner's directory: an isolated
//! space's own `space-downloads/<context>`, else `downloads/sessions/<owner>` of the shared
//! profile.
//!
//! The file Chromium wrote is temporary: the embedder that consumes a finished download
//! releases it ([`Browser::release_download`]), which deletes it and names the copy the
//! embedder kept, and [`Browser::sweep_stale_downloads`] deletes what a crash left behind.
//!
//! Staging uses Unix descriptors on Linux and macOS. Other platforms return `Unsupported`
//! rather than replacing confinement with path-based filesystem operations.

use tokio::sync::broadcast;

use std::time::{Duration, Instant, SystemTime};

use super::*;

const SESSION_DOWNLOADS: &str = "sessions";
const MAX_NAME_BYTES: usize = 200;
/// Longest name [`safe_file_name`] returns, in bytes.
pub const SAFE_NAME_BYTES: usize = 120;
/// Chromium names a transfer's file `<guid>.crdownload` until it completes.
const PARTIAL_SUFFIX: &str = ".crdownload";

impl Browser {
    /// A download Chromium reported, while it is among the most recent few hundred.
    pub fn download(&self, guid: &str) -> Option<DownloadInfo> {
        self.inner.page.download(guid)
    }

    /// Every recorded download of `owner`, oldest first.
    pub fn downloads_of(&self, owner: &str) -> Vec<DownloadInfo> {
        self.inner.page.downloads_of(owner)
    }

    /// Cancel any download that announces or writes more than `limit` bytes; the record of such
    /// a download is marked `over_limit` before Chromium reports it canceled.
    pub fn set_download_limit(&self, limit: Option<u64>) {
        self.inner.page.set_download_limit(limit);
    }

    /// Read a completed download of `owner` from where Chromium wrote it, through the same
    /// descriptor confinement as [`Self::save_download`]. At most `max + 1` bytes are read, so
    /// a caller can tell a file over its ceiling from one exactly at it.
    pub async fn read_download(
        &self,
        owner: &str,
        guid: &str,
        max: u64,
    ) -> Result<Vec<u8>, BrowserError> {
        let record = self.completed_download(owner, guid)?;
        let root = self.profile_path().to_owned();
        let source = self.download_source(&record)?;
        let guid = file_name(guid)?.to_owned();
        Ok(
            tokio::task::spawn_blocking(move || confined_read(&root, &source, &guid, max))
                .await
                .map_err(std::io::Error::other)??,
        )
    }

    /// Delete the file Chromium wrote for a finished download. `kept` is the caller's own copy
    /// of it, which [`Self::save_download`] copies from from now on; without one, saving the
    /// download fails. Only the guid-named entry in the download's own folder is unlinked,
    /// never through a link; a file already gone is not an error.
    pub async fn release_download(
        &self,
        guid: &str,
        kept: Option<PathBuf>,
    ) -> Result<(), BrowserError> {
        if !is_guid(guid) {
            return Err(BrowserError::InvalidAction("download id is not a guid"));
        }
        let record = self
            .inner
            .page
            .download(guid)
            .ok_or(BrowserError::InvalidAction("unknown download"))?;
        let root = self.profile_path().to_owned();
        let source = self.download_source(&record)?;
        // Recorded before the unlink, so a save that then misses the file finds the kept copy.
        if !self.inner.page.release(guid, kept) {
            return Err(BrowserError::InvalidAction("the download is still running"));
        }
        let guid = guid.to_owned();
        Ok(
            tokio::task::spawn_blocking(move || confined_unlink(&root, &source, &guid))
                .await
                .map_err(std::io::Error::other)??,
        )
    }

    /// Delete download files older than `max_age` from the shared profile's download folder
    /// and every isolated space's, except those of downloads still running: what a crash, or a
    /// download no embedder released, left behind. Saved copies in subfolders are untouched.
    /// Returns how many files were removed.
    pub async fn sweep_stale_downloads(&self, max_age: Duration) -> Result<usize, BrowserError> {
        let running = self.inner.page.running_downloads();
        let cutoff = SystemTime::now()
            .checked_sub(max_age)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let root = self.profile_path().to_owned();
        Ok(
            tokio::task::spawn_blocking(move || confined_sweep(&root, &running, cutoff))
                .await
                .map_err(std::io::Error::other)??,
        )
    }

    fn completed_download(&self, owner: &str, guid: &str) -> Result<DownloadInfo, BrowserError> {
        let record = self
            .inner
            .page
            .download(guid)
            .ok_or(BrowserError::InvalidAction("unknown download"))?;
        if record.owner.as_deref() != Some(owner) {
            return Err(BrowserError::OwnershipDenied(guid.to_owned()));
        }
        if record.status != Some(DownloadStatus::Completed) {
            return Err(BrowserError::InvalidAction("the download did not complete"));
        }
        Ok(record)
    }

    /// Profile-relative folder Chromium wrote the download into.
    fn download_source(&self, record: &DownloadInfo) -> Result<PathBuf, BrowserError> {
        let source = match &record.space {
            Some(context) => self.space_downloads(context),
            None => self.profile_path().join("downloads"),
        };
        Ok(source
            .strip_prefix(self.profile_path())
            .map_err(|_| BrowserError::InvalidAction("download source is outside the profile"))?
            .to_owned())
    }

    /// Wait until the download is completed or canceled. A download that finished before this
    /// call returns at once.
    pub async fn wait_download(
        &self,
        guid: &str,
        deadline: Instant,
    ) -> Result<DownloadStatus, BrowserError> {
        // Subscribe first: a finish between the record check and the subscription is not lost.
        let mut events = self.inner.page.subscribe();
        loop {
            match self.inner.page.download(guid) {
                Some(DownloadInfo {
                    status: Some(status),
                    ..
                }) => return Ok(status),
                Some(_) => {}
                None => return Err(BrowserError::InvalidAction("unknown download")),
            }
            let wait = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(wait, events.recv()).await {
                Err(_) => return Err(BrowserError::Timeout("download")),
                Ok(Err(broadcast::error::RecvError::Closed)) => {
                    return Err(BrowserError::Disconnected)
                }
                // Lagged: the record is re-read at the top of the loop.
                Ok(_) => {}
            }
        }
    }

    /// Directory the owner's downloads are saved into. An owner whose isolated space is gone
    /// has none: its files never move to the shared profile.
    pub async fn session_downloads(&self, owner: &str) -> Result<PathBuf, BrowserError> {
        let state = self.inner.state.lock().await;
        if let Some(context) = state.spaces.get(owner) {
            return Ok(self.space_downloads(context));
        }
        if state.isolated.contains(owner) {
            return Err(BrowserError::SpaceClosed(owner.to_owned()));
        }
        Ok(self
            .profile_path()
            .join("downloads")
            .join(SESSION_DOWNLOADS)
            .join(encode_component(owner)))
    }

    /// Copy a completed download of `owner` into the owner's download directory as `name`
    /// (the suggested file name, made safe, when `None`) and return the saved path. An existing
    /// file of that name is replaced. A released download is copied from the copy its
    /// releaser kept.
    pub async fn save_download(
        &self,
        owner: &str,
        guid: &str,
        name: Option<&str>,
    ) -> Result<PathBuf, BrowserError> {
        let record = self.completed_download(owner, guid)?;
        let name = match name {
            Some(name) => file_name(name)?.to_owned(),
            None => safe_file_name(&record.suggested_filename),
        };
        let root = self.profile_path().to_owned();
        let source = self.download_source(&record)?;
        let directory = self.session_downloads(owner).await?;
        let target = directory.join(&name);
        let destination = directory
            .strip_prefix(&root)
            .map_err(|_| BrowserError::InvalidAction("download directory is outside the profile"))?
            .to_owned();
        let guid = file_name(guid)?.to_owned();
        let page = self.inner.page.clone();
        tokio::task::spawn_blocking(move || {
            let file = download_file(&page, &root, &source, &guid)?;
            confined_copy(&root, file, &destination, &name)
        })
        .await
        .map_err(std::io::Error::other)??;
        Ok(target)
    }
}

/// A single path component the caller chose: no separators, no traversal, not hidden control
/// text, bounded length.
fn file_name(name: &str) -> Result<&str, BrowserError> {
    let invalid = name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > MAX_NAME_BYTES
        || name
            .chars()
            .any(|c| c == '/' || c == '\\' || c.is_control());
    if invalid {
        Err(BrowserError::InvalidAction(
            "file name must be one plain path component",
        ))
    } else {
        Ok(name)
    }
}

/// One plain path component from a page-chosen name: separators, control and format
/// characters and the characters Windows forbids become `_`, leading dots are dropped (no
/// hidden or dot-dot names), and the result is cut to [`SAFE_NAME_BYTES`] keeping the
/// extension; a Windows device name gets a `_` prefix. Never empty.
pub fn safe_file_name(suggested: &str) -> String {
    let cleaned: String = suggested
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
                || c.is_control()
                || is_format(c)
            {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned
        .trim()
        .trim_start_matches('.')
        .trim_end_matches(['.', ' ']);
    if cleaned.is_empty() {
        return "download".into();
    }
    // Before the length cut, so the prefixed name still fits the cap and keeps its extension.
    let cleaned = if is_device(cleaned) {
        format!("_{cleaned}")
    } else {
        cleaned.to_owned()
    };
    if cleaned.len() <= SAFE_NAME_BYTES {
        return cleaned;
    }
    let (stem, extension) = match cleaned.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() && extension.len() <= 16 => {
            (stem, Some(extension))
        }
        _ => (cleaned.as_str(), None),
    };
    let budget = SAFE_NAME_BYTES - extension.map_or(0, |extension| extension.len() + 1);
    let mut cut = budget.min(stem.len());
    while !stem.is_char_boundary(cut) {
        cut -= 1;
    }
    let stem = stem[..cut].trim_end_matches(['.', ' ']);
    match extension {
        Some(extension) => format!("{stem}.{extension}"),
        None if stem.is_empty() => "download".into(),
        None => stem.to_owned(),
    }
}

/// Files leave through Files onto any desktop, so a name must also be one Windows accepts as
/// written: no device name before the first dot (`CON`, `nul.txt`, `COM1.tar.gz`).
fn is_device(name: &str) -> bool {
    let device = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    matches!(
        device.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        device.strip_prefix(prefix).is_some_and(|rest| {
            matches!(rest.as_bytes(), [digit] if digit.is_ascii_digit())
                || matches!(rest, "¹" | "²" | "³")
        })
    })
}

/// Bidirectional overrides and zero-width characters can disguise an extension
/// (`report\u{202e}fdp.exe`).
fn is_format(c: char) -> bool {
    matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
}

/// Chromium's download identifier: a UUID in its 36-character text form. Nothing else in a
/// download folder is ever deleted by name.
fn is_guid(name: &str) -> bool {
    name.len() == 36
        && name.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

/// Injective, filesystem-safe encoding of an owner id: ASCII alphanumerics, `-` and `_` stay,
/// every other byte (and `~`) becomes `~HH`.
fn encode_component(owner: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(owner.len());
    for byte in owner.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "~{byte:02x}");
        }
    }
    out
}

#[cfg(unix)]
pub(super) fn download_directory(
    root: &Path,
    relative: &Path,
    create: bool,
) -> std::io::Result<std::os::fd::OwnedFd> {
    use rustix::{
        fs::{self, Mode, OFlags, CWD},
        io::Errno,
    };
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    // The configured profile is trusted; every download component below it is not.
    let mut directory = fs::openat(
        CWD,
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "download path must be normalized",
            ));
        };
        directory = match fs::openat(&directory, name, flags, Mode::empty()) {
            Err(Errno::NOENT) if create => {
                match fs::mkdirat(&directory, name, Mode::from_raw_mode(0o700)) {
                    Ok(()) | Err(Errno::EXIST) => {}
                    Err(error) => return Err(error.into()),
                }
                fs::openat(&directory, name, flags, Mode::empty())?
            }
            result => result?,
        };
    }
    Ok(directory)
}

#[cfg(unix)]
fn confined_copy(
    root: &Path,
    mut source: std::fs::File,
    destination: &Path,
    name: &str,
) -> std::io::Result<()> {
    use rustix::fs::{self, AtFlags, Mode, OFlags};
    let destination = download_directory(root, destination, true)?;
    let staging = format!(".saving-{:032x}", rand::random::<u128>());
    let staging_fd = fs::openat(
        &destination,
        &staging,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )?;
    let mut staging_file = std::fs::File::from(staging_fd);
    let result = std::io::copy(&mut source, &mut staging_file)
        .and_then(|_| staging_file.sync_all())
        .and_then(|()| {
            fs::renameat(&destination, &staging, &destination, name).map_err(std::io::Error::from)
        });
    if result.is_err() {
        let _ = fs::unlinkat(&destination, &staging, AtFlags::empty());
    }
    result
}

/// The bytes of a completed download: Chromium's file until it is released, then the copy its
/// releaser kept. A release between the two looks shows up as a missing Chromium file.
#[cfg(unix)]
fn download_file(
    page: &page_events::PageHub,
    root: &Path,
    source: &Path,
    guid: &str,
) -> std::io::Result<std::fs::File> {
    if let Some(kept) = page.released(guid) {
        return open_kept(kept);
    }
    match open_download(root, source, guid) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => match page.released(guid) {
            Some(kept) => open_kept(kept),
            None => Err(error),
        },
        result => result,
    }
}

/// The releaser's copy lives in its own storage, outside the profile; only its last component
/// is not trusted to be a regular file.
#[cfg(unix)]
fn open_kept(kept: Option<PathBuf>) -> std::io::Result<std::fs::File> {
    use rustix::fs::{self, Mode, OFlags, CWD};
    let Some(kept) = kept else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "the download is no longer available",
        ));
    };
    let file = std::fs::File::from(fs::openat(
        CWD,
        &kept,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the kept download is not a regular file",
        ));
    }
    Ok(file)
}

#[cfg(unix)]
fn open_download(root: &Path, source: &Path, guid: &str) -> std::io::Result<std::fs::File> {
    use rustix::fs::{self, Mode, OFlags};
    let source = download_directory(root, source, false)?;
    let source = fs::openat(
        &source,
        guid,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let source = std::fs::File::from(source);
    if !source.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "download source is not a regular file",
        ));
    }
    Ok(source)
}

#[cfg(unix)]
fn confined_read(root: &Path, source: &Path, guid: &str, max: u64) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    open_download(root, source, guid)?
        .take(max.saturating_add(1))
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Removes `<guid>` (and a partial `<guid>.crdownload`) from the download folder. `unlinkat`
/// never follows the last component, so a link swapped in goes, not its target.
#[cfg(unix)]
fn confined_unlink(root: &Path, source: &Path, guid: &str) -> std::io::Result<()> {
    use rustix::{
        fs::{self, AtFlags},
        io::Errno,
    };
    if !is_guid(guid) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "download id is not a guid",
        ));
    }
    let directory = match download_directory(root, source, false) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        result => result?,
    };
    for name in [guid.to_owned(), format!("{guid}{PARTIAL_SUFFIX}")] {
        match fs::unlinkat(&directory, name.as_str(), AtFlags::empty()) {
            Ok(()) | Err(Errno::NOENT) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn confined_sweep(
    root: &Path,
    running: &std::collections::HashSet<String>,
    cutoff: SystemTime,
) -> std::io::Result<usize> {
    let spaces = Path::new(spaces::SPACE_DOWNLOADS);
    let mut folders = vec![PathBuf::from("downloads")];
    match download_directory(root, spaces, false) {
        Ok(directory) => folders.extend(
            folder_entries(&directory)?
                .into_iter()
                .map(|context| spaces.join(context)),
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut removed = 0;
    for folder in folders {
        let directory = match download_directory(root, &folder, false) {
            Ok(directory) => directory,
            // Gone, or not a directory (a link or file where a space folder was): skipped.
            Err(_) => continue,
        };
        for name in folder_entries(&directory)? {
            let guid = name.strip_suffix(PARTIAL_SUFFIX).unwrap_or(&name);
            if is_guid(guid) && !running.contains(guid) && stale(&directory, &name, cutoff) {
                match rustix::fs::unlinkat(&directory, name.as_str(), rustix::fs::AtFlags::empty())
                {
                    Ok(()) => removed += 1,
                    Err(rustix::io::Errno::NOENT) => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
    }
    Ok(removed)
}

/// UTF-8 entry names of a directory, without `.` and `..`.
#[cfg(unix)]
fn folder_entries(directory: &std::os::fd::OwnedFd) -> std::io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in rustix::fs::Dir::read_from(directory)? {
        let entry = entry?;
        if let Ok(name) = entry.file_name().to_str() {
            if name != "." && name != ".." {
                names.push(name.to_owned());
            }
        }
    }
    Ok(names)
}

/// A regular file last written before `cutoff`, or a link (Chromium never writes one).
#[cfg(unix)]
fn stale(directory: &std::os::fd::OwnedFd, name: &str, cutoff: SystemTime) -> bool {
    use rustix::{
        fs::{self, Mode, OFlags},
        io::Errno,
    };
    match fs::openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(file) => std::fs::File::from(file).metadata().is_ok_and(|metadata| {
            metadata.is_file() && metadata.modified().is_ok_and(|modified| modified < cutoff)
        }),
        Err(Errno::LOOP) => true,
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn confined_read(_: &Path, _: &Path, _: &str, _: u64) -> std::io::Result<Vec<u8>> {
    Err(unsupported())
}

#[cfg(not(unix))]
fn download_file(
    _: &page_events::PageHub,
    _: &Path,
    _: &Path,
    _: &str,
) -> std::io::Result<std::fs::File> {
    Err(unsupported())
}

#[cfg(not(unix))]
fn confined_copy(_: &Path, _: std::fs::File, _: &Path, _: &str) -> std::io::Result<()> {
    Err(unsupported())
}

#[cfg(not(unix))]
fn confined_unlink(_: &Path, _: &Path, _: &str) -> std::io::Result<()> {
    Err(unsupported())
}

#[cfg(not(unix))]
fn confined_sweep(
    _: &Path,
    _: &std::collections::HashSet<String>,
    _: SystemTime,
) -> std::io::Result<usize> {
    Err(unsupported())
}

#[cfg(not(unix))]
fn unsupported() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Private downloads require Unix descriptor confinement",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUID: &str = "0f8c2a3e-5b1d-4e7a-9c6f-2d4b8a1e3f70";

    #[test]
    fn caller_names_are_single_plain_components() {
        assert_eq!(file_name("report.pdf").unwrap(), "report.pdf");
        for bad in ["", ".", "..", "a/b", "..\\x", "a\u{0}b", "line\nbreak"] {
            assert!(file_name(bad).is_err(), "{bad:?}");
        }
        assert!(file_name(&"x".repeat(MAX_NAME_BYTES + 1)).is_err());
    }

    #[test]
    fn suggested_names_are_repaired_not_refused() {
        assert_eq!(safe_file_name("../etc/passwd"), "_etc_passwd");
        assert_eq!(safe_file_name(""), "download");
        assert_eq!(safe_file_name(".."), "download");
        assert_eq!(safe_file_name("ok.csv"), "ok.csv");
    }

    #[test]
    fn names_are_one_safe_component_that_keeps_its_extension() {
        assert_eq!(safe_file_name("report.pdf"), "report.pdf");
        assert_eq!(safe_file_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(safe_file_name(".."), "download");
        assert_eq!(safe_file_name("..hidden"), "hidden");
        assert_eq!(safe_file_name(""), "download");
        assert_eq!(safe_file_name("a\\b:c\u{0}d"), "a_b_c_d");
        assert_eq!(safe_file_name("invoice\u{202e}fdp.exe"), "invoice_fdp.exe");
        assert_eq!(safe_file_name("보고서.csv"), "보고서.csv");
        let long = format!("{}.csv", "가".repeat(100));
        let cut = safe_file_name(&long);
        assert!(cut.len() <= SAFE_NAME_BYTES, "{}", cut.len());
        assert!(cut.ends_with(".csv"));
        for (device, safe) in [
            ("CON", "_CON"),
            ("nul.txt", "_nul.txt"),
            ("Com1.tar.gz", "_Com1.tar.gz"),
            ("lpt9 .log", "_lpt9 .log"),
            ("COM¹.txt", "_COM¹.txt"),
            ("aux.", "_aux"),
            ("report.pdf. . ", "report.pdf"),
            ("a<b>c\"d|e?f*g.txt", "a_b_c_d_e_f_g.txt"),
        ] {
            assert_eq!(safe_file_name(device), safe, "{device}");
        }
        for ordinary in ["console.txt", "COM10.txt", "nullable.csv", "auxiliary"] {
            assert_eq!(safe_file_name(ordinary), ordinary);
        }
        // A 120-byte device name keeps the cap and its extension after the `_` prefix,
        // including when the cut would land inside a multi-byte character.
        let exact = format!("CON.{}.pdf", "x".repeat(SAFE_NAME_BYTES - 8));
        assert_eq!(exact.len(), SAFE_NAME_BYTES);
        let safe = safe_file_name(&exact);
        assert_eq!(safe.len(), SAFE_NAME_BYTES, "{safe}");
        assert!(
            safe.starts_with("_CON.") && safe.ends_with(".pdf"),
            "{safe}"
        );
        let wide = format!("nul .{}.pdf", "가".repeat(40));
        assert_eq!(wide.len(), SAFE_NAME_BYTES + 9);
        let safe = safe_file_name(&wide);
        assert!(safe.len() <= SAFE_NAME_BYTES, "{}", safe.len());
        assert!(
            safe.starts_with("_nul .가") && safe.ends_with("가.pdf"),
            "{safe}"
        );
        let plain = format!("{}.pdf", "y".repeat(SAFE_NAME_BYTES - 4));
        assert_eq!(safe_file_name(&plain), plain);
    }

    #[test]
    fn owner_directories_never_collide() {
        assert_eq!(encode_component("abc-1_2"), "abc-1_2");
        assert_ne!(encode_component("a.b"), encode_component("a~2eb"));
        assert_eq!(encode_component("../x"), "~2e~2e~2fx");
    }

    #[test]
    fn only_uuid_text_is_a_guid() {
        assert!(is_guid(GUID));
        assert!(is_guid(&GUID.to_ascii_uppercase()));
        for bad in [
            "",
            "sessions",
            "..",
            "0f8c2a3e-5b1d-4e7a-9c6f-2d4b8a1e3f7",
            "0f8c2a3e-5b1d-4e7a-9c6f-2d4b8a1e3f701",
            "0f8c2a3e_5b1d-4e7a-9c6f-2d4b8a1e3f70",
            "0f8c2a3e-5b1d-4e7a-9c6f-2d4b8a1e3f7g",
            "../../c2a3e-5b1d-4e7a-9c6f-2d4b8a1e3f70",
        ] {
            assert!(!is_guid(bad), "{bad:?}");
        }
    }

    #[cfg(unix)]
    fn copy(
        root: &Path,
        source: &str,
        guid: &str,
        destination: &str,
        name: &str,
    ) -> std::io::Result<()> {
        let file = open_download(root, Path::new(source), guid)?;
        confined_copy(root, file, Path::new(destination), name)
    }

    #[cfg(unix)]
    #[test]
    fn copying_downloads_never_follows_session_or_staging_symlinks() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("downloads")).unwrap();
        std::fs::write(root.path().join("downloads/guid"), b"download").unwrap();
        std::fs::write(outside.path().join("canary"), b"untouched").unwrap();
        symlink(outside.path(), root.path().join("downloads/sessions")).unwrap();
        assert!(copy(
            root.path(),
            "downloads",
            "guid",
            "downloads/sessions/owner",
            "saved"
        )
        .is_err());
        assert!(!outside.path().join("owner").exists());
        std::fs::remove_file(root.path().join("downloads/sessions")).unwrap();
        std::fs::create_dir_all(root.path().join("downloads/sessions/owner")).unwrap();
        symlink(
            outside.path().join("canary"),
            root.path().join("downloads/sessions/owner/.guid.saving"),
        )
        .unwrap();
        symlink(
            outside.path().join("canary"),
            root.path().join("downloads/sessions/owner/saved"),
        )
        .unwrap();
        copy(
            root.path(),
            "downloads",
            "guid",
            "downloads/sessions/owner",
            "saved",
        )
        .unwrap();
        assert_eq!(
            std::fs::read(root.path().join("downloads/sessions/owner/saved")).unwrap(),
            b"download"
        );
        assert_eq!(
            std::fs::read(outside.path().join("canary")).unwrap(),
            b"untouched"
        );
        assert!(root
            .path()
            .join("downloads/sessions/owner/.guid.saving")
            .is_symlink());
        std::fs::remove_file(root.path().join("downloads/sessions/owner/saved")).unwrap();
        symlink(
            outside.path().join("canary"),
            root.path().join("downloads/source-link"),
        )
        .unwrap();
        assert!(copy(
            root.path(),
            "downloads",
            "source-link",
            "downloads/sessions/owner",
            "saved"
        )
        .is_err());
        assert!(!root.path().join("downloads/sessions/owner/saved").exists());
    }

    #[cfg(unix)]
    #[test]
    fn releasing_unlinks_only_the_guid_entry_never_a_link_target() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let downloads = root.path().join("downloads");
        std::fs::create_dir(&downloads).unwrap();
        std::fs::write(outside.path().join("canary"), b"untouched").unwrap();

        std::fs::write(downloads.join(GUID), b"raw").unwrap();
        std::fs::write(downloads.join(format!("{GUID}{PARTIAL_SUFFIX}")), b"part").unwrap();
        confined_unlink(root.path(), Path::new("downloads"), GUID).unwrap();
        assert!(!downloads.join(GUID).exists());
        assert!(!downloads.join(format!("{GUID}{PARTIAL_SUFFIX}")).exists());
        // Already gone is fine.
        confined_unlink(root.path(), Path::new("downloads"), GUID).unwrap();

        // A link swapped in at the guid name goes as a link; its target stays.
        symlink(outside.path().join("canary"), downloads.join(GUID)).unwrap();
        confined_unlink(root.path(), Path::new("downloads"), GUID).unwrap();
        assert!(downloads.join(GUID).symlink_metadata().is_err());
        assert_eq!(
            std::fs::read(outside.path().join("canary")).unwrap(),
            b"untouched"
        );

        // A linked download folder is never entered.
        std::fs::write(outside.path().join(GUID), b"outside").unwrap();
        std::fs::remove_dir(&downloads).unwrap();
        symlink(outside.path(), &downloads).unwrap();
        assert!(confined_unlink(root.path(), Path::new("downloads"), GUID).is_err());
        assert_eq!(
            std::fs::read(outside.path().join(GUID)).unwrap(),
            b"outside"
        );

        // Names that are not guids are refused before anything is touched.
        std::fs::remove_file(&downloads).unwrap();
        std::fs::create_dir(&downloads).unwrap();
        std::fs::create_dir(downloads.join("sessions")).unwrap();
        std::fs::write(downloads.join("notes.txt"), b"keep").unwrap();
        for name in ["sessions", "notes.txt", "..", "../downloads", ""] {
            assert!(
                confined_unlink(root.path(), Path::new("downloads"), name).is_err(),
                "{name:?}"
            );
        }
        assert!(downloads.join("sessions").is_dir());
        assert!(downloads.join("notes.txt").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn sweeping_removes_only_old_unclaimed_guid_files() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let downloads = root.path().join("downloads");
        let space = root.path().join(spaces::SPACE_DOWNLOADS).join("context-1");
        std::fs::create_dir_all(downloads.join("sessions/owner")).unwrap();
        std::fs::create_dir_all(&space).unwrap();
        std::fs::write(outside.path().join("canary"), b"untouched").unwrap();
        let guid = |n: u8| format!("00000000-0000-4000-8000-{n:012}");
        let old = SystemTime::now() - Duration::from_secs(2 * 3600);
        let write = |path: PathBuf, modified: Option<SystemTime>| {
            std::fs::write(&path, b"bytes").unwrap();
            if let Some(modified) = modified {
                std::fs::File::options()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_modified(modified)
                    .unwrap();
            }
        };
        write(downloads.join(guid(1)), Some(old));
        write(
            downloads.join(format!("{}{PARTIAL_SUFFIX}", guid(2))),
            Some(old),
        );
        write(downloads.join(guid(3)), None);
        write(downloads.join(guid(4)), Some(old));
        write(downloads.join("notes.txt"), Some(old));
        write(
            downloads.join(format!("sessions/owner/{}", guid(5))),
            Some(old),
        );
        write(space.join(guid(6)), Some(old));
        write(space.join("saved.pdf"), Some(old));
        symlink(outside.path().join("canary"), downloads.join(guid(7))).unwrap();
        // A linked space folder is not entered.
        std::fs::write(outside.path().join(guid(8)), b"outside").unwrap();
        symlink(
            outside.path(),
            root.path().join(spaces::SPACE_DOWNLOADS).join("linked"),
        )
        .unwrap();

        let running = std::collections::HashSet::from([guid(4)]);
        let cutoff = SystemTime::now() - Duration::from_secs(3600);
        assert_eq!(confined_sweep(root.path(), &running, cutoff).unwrap(), 4);
        assert!(!downloads.join(guid(1)).exists());
        assert!(!downloads
            .join(format!("{}{PARTIAL_SUFFIX}", guid(2)))
            .exists());
        assert!(downloads.join(guid(3)).exists(), "fresh file kept");
        assert!(downloads.join(guid(4)).exists(), "running download kept");
        assert!(downloads.join("notes.txt").exists());
        assert!(downloads
            .join(format!("sessions/owner/{}", guid(5)))
            .exists());
        assert!(!space.join(guid(6)).exists());
        assert!(space.join("saved.pdf").exists());
        assert!(downloads.join(guid(7)).symlink_metadata().is_err());
        assert_eq!(
            std::fs::read(outside.path().join("canary")).unwrap(),
            b"untouched"
        );
        assert_eq!(
            std::fs::read(outside.path().join(guid(8))).unwrap(),
            b"outside"
        );

        // A profile without download folders has nothing to sweep.
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(confined_sweep(empty.path(), &running, cutoff).unwrap(), 0);
    }
}
