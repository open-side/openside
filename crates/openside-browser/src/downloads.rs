//! Downloads of script-driven tabs: attribution to an owner, waiting for a terminal state, and
//! saving into the owner's private download directory.
//!
//! Chromium writes every download as `<downloadPath>/<guid>` (`allowAndName`), so a download is
//! found by identifier even when two pages suggest the same file name. [`Browser::save_download`]
//! copies it to a name chosen by the caller, never outside the owner's directory: an isolated
//! space's own `space-downloads/<context>`, else `downloads/sessions/<owner>` of the shared
//! profile.
//!
//! Staging uses Unix descriptors on Linux and macOS. Other platforms return `Unsupported`
//! rather than replacing confinement with path-based filesystem operations.

use tokio::sync::broadcast;

use std::time::Instant;

use super::*;

const SESSION_DOWNLOADS: &str = "sessions";
const MAX_NAME_BYTES: usize = 200;

impl Browser {
    /// A download Chromium reported, while it is among the most recent few hundred.
    pub fn download(&self, guid: &str) -> Option<DownloadInfo> {
        self.inner.page.download(guid)
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
    /// file of that name is replaced.
    pub async fn save_download(
        &self,
        owner: &str,
        guid: &str,
        name: Option<&str>,
    ) -> Result<PathBuf, BrowserError> {
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
        let name = match name {
            Some(name) => file_name(name)?.to_owned(),
            None => sanitize_name(&record.suggested_filename),
        };
        let root = self.profile_path().to_owned();
        let source = match &record.space {
            Some(context) => self.space_downloads(context),
            None => root.join("downloads"),
        };
        let directory = self.session_downloads(owner).await?;
        let target = directory.join(&name);
        let destination = directory
            .strip_prefix(&root)
            .map_err(|_| BrowserError::InvalidAction("download directory is outside the profile"))?
            .to_owned();
        let source = source
            .strip_prefix(&root)
            .map_err(|_| BrowserError::InvalidAction("download source is outside the profile"))?
            .to_owned();
        let guid = file_name(guid)?.to_owned();
        tokio::task::spawn_blocking(move || {
            confined_copy(&root, &source, &guid, &destination, &name)
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

/// The page-suggested name with everything `file_name` refuses replaced; never empty.
fn sanitize_name(suggested: &str) -> String {
    let mut name: String = suggested
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    while name.len() > MAX_NAME_BYTES {
        name.pop();
    }
    if matches!(name.as_str(), "" | "." | "..") {
        "download".into()
    } else {
        name
    }
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
    source: &Path,
    guid: &str,
    destination: &Path,
    name: &str,
) -> std::io::Result<()> {
    use rustix::fs::{self, AtFlags, Mode, OFlags};
    let source = download_directory(root, source, false)?;
    let destination = download_directory(root, destination, true)?;
    let source = fs::openat(
        &source,
        guid,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let mut source = std::fs::File::from(source);
    if !source.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "download source is not a regular file",
        ));
    }
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

#[cfg(not(unix))]
fn confined_copy(_: &Path, _: &Path, _: &str, _: &Path, _: &str) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Private download staging requires Unix descriptor confinement",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(sanitize_name("../etc/passwd"), ".._etc_passwd");
        assert_eq!(sanitize_name(""), "download");
        assert_eq!(sanitize_name(".."), "download");
        assert_eq!(sanitize_name("ok.csv"), "ok.csv");
    }

    #[test]
    fn owner_directories_never_collide() {
        assert_eq!(encode_component("abc-1_2"), "abc-1_2");
        assert_ne!(encode_component("a.b"), encode_component("a~2eb"));
        assert_eq!(encode_component("../x"), "~2e~2e~2fx");
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
        assert!(confined_copy(
            root.path(),
            Path::new("downloads"),
            "guid",
            Path::new("downloads/sessions/owner"),
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
        confined_copy(
            root.path(),
            Path::new("downloads"),
            "guid",
            Path::new("downloads/sessions/owner"),
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
        assert!(confined_copy(
            root.path(),
            Path::new("downloads"),
            "source-link",
            Path::new("downloads/sessions/owner"),
            "saved"
        )
        .is_err());
        assert!(!root.path().join("downloads/sessions/owner/saved").exists());
    }
}
