use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
};

use crate::config::Resolved;

/// External credential sources contribute path metadata, never file contents.
#[derive(Default)]
pub(crate) struct ProtectedPaths {
    paths: Vec<PathBuf>,
    directories: Vec<PathBuf>,
    managed_root: Option<PathBuf>,
    #[cfg(unix)]
    identities: parking_lot::Mutex<Vec<std::os::fd::OwnedFd>>,
    #[cfg(unix)]
    identity_error: std::sync::atomic::AtomicBool,
}

impl ProtectedPaths {
    pub(crate) fn from_config(config: &Resolved) -> Self {
        let home = source_home(|key| std::env::var_os(key));
        let codex_home = std::env::var_os("CODEX_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let mut protected = Self::build(
            &config.loaded.config.credentials,
            home.as_deref(),
            codex_home.as_deref(),
        );
        if let Some(directory) =
            std::env::var_os("CLAUDE_CONFIG_DIR").filter(|value| !value.is_empty())
        {
            let path = PathBuf::from(directory).join(".credentials.json");
            let path = if path.is_absolute() {
                path
            } else {
                std::env::current_dir().unwrap_or_default().join(path)
            };
            if !protected.paths.contains(&path) {
                protected.paths.push(path);
                #[cfg(unix)]
                let _ = protected.refresh_identities();
            }
        }
        protected.directories = [
            openside_config::data_layout::DataPath::BrowserImports,
            openside_config::data_layout::DataPath::ExtensionImportKeys,
            openside_config::data_layout::DataPath::AccountSpaces,
        ]
        .into_iter()
        .map(|path| path.path(&config.data_dir))
        .collect();
        protected.managed_root = Some(config.data_dir.join("managed-subscriptions"));
        protected
            .directories
            .extend(protected.managed_root.iter().cloned());
        protected
            .paths
            .push(openside_config::data_layout::DataPath::ConnectionsKey.path(&config.data_dir));
        protected
            .paths
            .push(openside_config::data_layout::DataPath::RestartHandoff.path(&config.data_dir));
        #[cfg(unix)]
        let _ = protected.refresh_identities();
        protected
    }

    pub(crate) fn build(
        credentials: &serde_json::Value,
        home: Option<&Path>,
        codex_home: Option<&Path>,
    ) -> Self {
        let mut paths = Vec::new();
        fn collect(value: &serde_json::Value, home: Option<&Path>, paths: &mut Vec<PathBuf>) {
            if let Some(table) = value.as_object() {
                for (key, value) in table {
                    if key == "token_file" {
                        if let Some(value) = value.as_str().filter(|v| !v.is_empty()) {
                            paths.push(expand(value, home));
                        }
                    } else {
                        collect(value, home, paths);
                    }
                }
            } else if let Some(array) = value.as_array() {
                for value in array {
                    collect(value, home, paths);
                }
            }
        }
        collect(credentials, home, &mut paths);
        if let Some(home) = home {
            paths.push(home.join(".omp").join("auth-broker.token"));
            paths.push(home.join(".claude").join(".credentials.json"));
        }
        if let Some(codex) = codex_home {
            paths.push(codex.join("auth.json"));
        } else if let Some(home) = home {
            paths.push(home.join(".codex").join("auth.json"));
        }
        let cwd = std::env::current_dir().unwrap_or_default();
        for path in &mut paths {
            if !path.is_absolute() {
                *path = cwd.join(&*path);
            }
        }
        paths.sort();
        paths.dedup();
        let protected = Self {
            paths,
            ..Self::default()
        };
        #[cfg(unix)]
        let _ = protected.refresh_identities();
        protected
    }

    pub(crate) fn is_protected(&self, path: &Path) -> bool {
        #[cfg(unix)]
        if self.refresh_identities().is_err() {
            return true;
        }
        if self.paths.iter().any(|source| path == source) {
            return true;
        }
        let target = resolve(path);
        if self
            .directories
            .iter()
            .any(|root| target.starts_with(resolve(root)))
        {
            return true;
        }
        if self
            .paths
            .iter()
            .any(|source| target == *source || target == resolve(source))
        {
            return true;
        }
        #[cfg(unix)]
        {
            if let Ok(metadata) = std::fs::metadata(path) {
                use std::os::unix::fs::MetadataExt;
                return self.identities.lock().iter().any(|fd| {
                    rustix::fs::fstat(fd)
                        .is_ok_and(|stat| file_identity(&stat) == (metadata.dev(), metadata.ino()))
                });
            }
        }
        false
    }

    #[cfg(unix)]
    fn managed_files(&self) -> std::io::Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        let Some(root) = &self.managed_root else {
            return Ok(files);
        };
        for (source, filename) in [
            ("claude-code", ".credentials.json"),
            ("codex-cli", "auth.json"),
        ] {
            let directory = root.join(source);
            let homes = match std::fs::read_dir(&directory) {
                Ok(homes) => homes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            for home in homes {
                let home = home?;
                if home.file_type()?.is_symlink() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "Managed credential home is a symlink",
                    ));
                }
                if home.file_type()?.is_dir() {
                    files.push(home.path().join(filename));
                }
                if files.len() > 256 {
                    return Err(std::io::Error::other(
                        "Managed credential identity limit reached",
                    ));
                }
            }
        }
        Ok(files)
    }

    #[cfg(unix)]
    fn refresh_identities(&self) -> std::io::Result<()> {
        use rustix::fs::{fstat, open, Mode, OFlags};
        use std::sync::atomic::Ordering;
        if self.identity_error.load(Ordering::Relaxed) {
            return Err(std::io::Error::other(
                "Credential identity tracking failed; restart the daemon",
            ));
        }
        let mut identities = self.identities.lock();
        let managed = self.managed_files()?;
        for path in self.paths.iter().chain(&managed) {
            #[cfg(target_os = "linux")]
            let flags = OFlags::PATH | OFlags::CLOEXEC;
            #[cfg(not(target_os = "linux"))]
            let flags = OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC;
            let fd = match open(path, flags, Mode::empty()) {
                Ok(fd) => fd,
                Err(rustix::io::Errno::NOENT) => continue,
                Err(error) => {
                    self.identity_error.store(true, Ordering::Relaxed);
                    return Err(error.into());
                }
            };
            let stat = fstat(&fd)?;
            if identities.iter().any(|known| {
                fstat(known)
                    .is_ok_and(|known| (known.st_dev, known.st_ino) == (stat.st_dev, stat.st_ino))
            }) {
                continue;
            }
            if identities.len() >= 256 {
                self.identity_error.store(true, Ordering::Relaxed);
                return Err(std::io::Error::other("Credential identity limit reached"));
            }
            // Retaining the handle also prevents inode reuse after unlink. CLOEXEC keeps
            // these metadata-only handles out of every agent/sandbox subprocess.
            identities.push(fd);
        }
        Ok(())
    }

    #[cfg(unix)]
    pub(crate) fn check_fd(&self, fd: impl std::os::fd::AsFd) -> std::io::Result<()> {
        self.check_filesystem(fd.as_fd())?;
        self.refresh_identities()?;
        let stat = rustix::fs::fstat(&fd)?;
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            let path = std::fs::read_link(format!("/proc/self/fd/{}", fd.as_fd().as_raw_fd()))?;
            if self
                .directories
                .iter()
                .any(|root| resolve(&path).starts_with(resolve(root)))
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Private import data is protected",
                ));
            }
        }
        if self.identities.lock().iter().any(|known| {
            rustix::fs::fstat(known)
                .is_ok_and(|known| (known.st_dev, known.st_ino) == (stat.st_dev, stat.st_ino))
        }) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Credential source files are protected",
            ));
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn check_filesystem(&self, fd: impl std::os::fd::AsFd) -> std::io::Result<()> {
        let kind = rustix::fs::fstatfs(fd)?.f_type as u64;
        // Process/kernel introspection files can look regular and can be bind-mounted
        // under ordinary names; pathname rules and O_NOFOLLOW cannot recognize them.
        if matches!(
            kind,
            0x9fa0
                | 0x62656572
                | 0x64626720
                | 0x74726163
                | 0x73636673
                | 0xcafe4a11
                | 0x27e0eb
                | 0x63677270
                | 0x62656570
        ) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Process and kernel filesystems are protected",
            ));
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn check_filesystem(&self, fd: impl std::os::fd::AsFd) -> std::io::Result<()> {
        let stat = rustix::fs::fstatfs(fd)?;
        let name = stat.f_fstypename.map(|byte| byte as u8);
        let end = name
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(name.len());
        check_macos_filesystem_name(&name[..end])
    }

    #[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
    fn check_filesystem(&self, _: impl std::os::fd::AsFd) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Host filesystem screening is unsupported on this platform",
        ))
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn sandbox_masks(&self, workspace: &Path) -> Result<Vec<PathBuf>, &'static str> {
        use std::{io::Read, os::fd::AsRawFd};
        self.refresh_identities()
            .map_err(|_| "Credential file identities are unavailable; sandbox refused")?;
        let mut bytes = Vec::new();
        std::fs::File::open("/proc/self/mountinfo")
            .and_then(|file| file.take(1024 * 1024 + 1).read_to_end(&mut bytes))
            .map_err(|_| "Mount metadata is unavailable; sandbox refused")?;
        if bytes.len() > 1024 * 1024 {
            return Err("Mount metadata is too large; sandbox refused");
        }
        let mounts = crate::credential_mounts::parse(&bytes)
            .map_err(|_| "Invalid mount metadata; sandbox refused")?;
        let workspace = resolve(workspace);
        let mut masks = Vec::new();
        for fd in self.identities.lock().iter() {
            let stat = rustix::fs::fstat(fd)
                .map_err(|_| "Credential file identity is unavailable; sandbox refused")?;
            if stat.st_nlink > 1 {
                return Err("Credential files have hardlink aliases; remove the aliases before using sandboxed bash");
            }
            let mut path = std::fs::read_link(format!("/proc/self/fd/{}", fd.as_raw_fd()))
                .map_err(|_| "Credential file location is unavailable; sandbox refused")?;
            if stat.st_nlink == 0 {
                use std::os::unix::ffi::{OsStrExt, OsStringExt};
                let bytes = path
                    .as_os_str()
                    .as_bytes()
                    .strip_suffix(b" (deleted)")
                    .ok_or("Credential file location is unavailable; sandbox refused")?;
                path = std::ffi::OsString::from_vec(bytes.to_vec()).into();
            }
            let mut info = Vec::new();
            std::fs::File::open(format!("/proc/self/fdinfo/{}", fd.as_raw_fd()))
                .and_then(|file| file.take(4097).read_to_end(&mut info))
                .map_err(|_| "Credential mount identity is unavailable; sandbox refused")?;
            let own = std::str::from_utf8(&info)
                .ok()
                .filter(|_| info.len() <= 4096)
                .and_then(|text| {
                    text.lines().find_map(|line| {
                        line.strip_prefix("mnt_id:")
                            .and_then(|id| id.trim().parse::<u32>().ok())
                    })
                })
                .ok_or("Credential mount identity is unavailable; sandbox refused")?;
            if crate::credential_mounts::has_alias(&mounts, &path, own) {
                return Err("Credential files have bind-mount aliases; remove the aliases before using sandboxed bash");
            }
            if stat.st_nlink == 0 {
                continue;
            }
            let current = std::fs::metadata(&path)
                .map_err(|_| "Credential file location changed; sandbox refused")?;
            use std::os::unix::fs::MetadataExt;
            if (current.dev(), current.ino()) != file_identity(&stat) {
                return Err("Credential file location changed; sandbox refused");
            }
            let exposed = path.starts_with(&workspace)
                || ["/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc"]
                    .iter()
                    .any(|root| path.starts_with(root));
            if exposed {
                masks.push(path);
            }
        }
        for root in &self.directories {
            let path = resolve(root);
            if path.starts_with(&workspace)
                || ["/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc"]
                    .iter()
                    .any(|base| path.starts_with(base))
            {
                masks.push(path);
            }
        }
        masks.sort();
        masks.dedup();
        Ok(masks)
    }

    #[cfg(unix)]
    pub(crate) fn sandbox_directory(&self, path: &Path) -> bool {
        self.directories.iter().any(|root| resolve(root) == path)
    }

    #[cfg(all(unix, not(target_os = "linux")))]
    pub(crate) fn sandbox_masks(&self, _: &Path) -> Result<Vec<PathBuf>, &'static str> {
        Err("Credential-safe sandbox setup is supported on Linux")
    }
}

#[cfg(target_os = "macos")]
fn check_macos_filesystem_name(name: &[u8]) -> std::io::Result<()> {
    match name {
        b"apfs" | b"hfs" | b"ufs" | b"nfs" | b"smbfs" | b"webdav" | b"msdos" | b"exfat"
        | b"ntfs" | b"udf" | b"cd9660" => Ok(()),
        b"devfs" | b"procfs" | b"fdesc" | b"fdescfs" | b"kernfs" => Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Process and kernel filesystems are protected",
        )),
        // Darwin names filesystems rather than exposing Linux magic values. Unknown
        // providers stay closed because they may expose process or device data.
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Host filesystem type is not supported",
        )),
    }
}

#[cfg(target_os = "linux")]
fn file_identity(stat: &rustix::fs::Stat) -> (u64, u64) {
    (stat.st_dev, stat.st_ino)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn file_identity(stat: &rustix::fs::Stat) -> (u64, u64) {
    (stat.st_dev as u64, stat.st_ino as u64)
}
// Match Python Path.home()/expanduser(), including an unset or empty HOME; otherwise
// the source could read a default credential that the daemon forgot to protect.
#[cfg(unix)]
fn source_home(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    match env("HOME") {
        Some(value) if value.is_empty() => Some(PathBuf::from("/")),
        Some(value) => Some(PathBuf::from(value)),
        None => nix::unistd::User::from_uid(nix::unistd::Uid::current())
            .ok()
            .flatten()
            .map(|user| user.dir),
    }
}

#[cfg(not(unix))]
fn source_home(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    env("USERPROFILE")
        .or_else(|| {
            let path = env("HOMEPATH")?;
            let mut drive = env("HOMEDRIVE").unwrap_or_default();
            drive.push(path);
            Some(drive)
        })
        .map(PathBuf::from)
}

fn expand(value: &str, home: Option<&Path>) -> PathBuf {
    if let Some(home) = home {
        if value == "~" {
            return home.to_owned();
        }
        if let Some(relative) = value.strip_prefix("~/") {
            return home.join(relative);
        }
    }
    #[cfg(unix)]
    if let Some(named) = value.strip_prefix('~').filter(|named| !named.is_empty()) {
        let (name, relative) = named.split_once('/').unwrap_or((named, ""));
        if let Ok(Some(user)) = nix::unistd::User::from_name(name) {
            return user.dir.join(relative);
        }
    }
    PathBuf::from(value)
}

fn lexical(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

// Missing credentials are protected against creation too. Resolve the nearest existing
// ancestor so a symlinked parent cannot bypass the boundary before a source creates its file.
fn resolve(path: &Path) -> PathBuf {
    let mut ancestor = path;
    let mut tail: Vec<&OsStr> = Vec::new();
    loop {
        if let Ok(resolved) = std::fs::canonicalize(ancestor) {
            return lexical(
                &tail
                    .iter()
                    .rev()
                    .fold(resolved, |path, name| path.join(name)),
            );
        }
        let Some(name) = ancestor.file_name() else {
            return lexical(path);
        };
        tail.push(name);
        let Some(parent) = ancestor.parent() else {
            return lexical(path);
        };
        ancestor = parent;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_and_default_sources_are_protected_without_reading_contents() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let codex = root.path().join("codex");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&codex).unwrap();
        let credentials = serde_json::json!({"plugin":{"token_file":root.path().join("source.txt")},"nested":{"source":{"token_file":"~/.custom/secret"}}});
        let protected = ProtectedPaths::build(&credentials, Some(&home), Some(&codex));
        for path in [
            root.path().join("source.txt"),
            home.join(".custom/secret"),
            home.join(".omp").join("auth-broker.token"),
            codex.join("auth.json"),
        ] {
            assert!(protected.is_protected(&path));
        }
        assert!(!protected.is_protected(&root.path().join("ordinary.txt")));
        assert!(!protected.is_protected(&home.join(".codex/auth.json")));
    }

    #[cfg(unix)]
    #[test]
    fn home_environment_matches_the_python_source_paths() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            source_home(|_| Some(std::ffi::OsString::new())),
            Some(PathBuf::from("/"))
        );
        assert_eq!(
            source_home(|_| Some(root.path().as_os_str().to_os_string())),
            Some(root.path().to_owned())
        );
    }

    #[cfg(unix)]
    #[test]
    fn configured_parent_traversal_resolves_after_symlinks() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let actual = root.path().join("actual");
        std::fs::create_dir_all(actual.join("child")).unwrap();
        std::fs::write(actual.join("source.txt"), "test-path-only").unwrap();
        symlink(actual.join("child"), root.path().join("alias")).unwrap();
        let credentials =
            serde_json::json!({"plugin":{"token_file":root.path().join("alias/../source.txt")}});
        let protected = ProtectedPaths::build(&credentials, Some(&root.path().join("home")), None);
        assert!(protected.is_protected(&actual.join("source.txt")));
        assert!(!protected.is_protected(&root.path().join("source.txt")));
    }

    #[cfg(unix)]
    #[test]
    fn aliases_and_missing_files_under_symlinked_parents_are_protected() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let actual = root.path().join("actual");
        std::fs::create_dir(&actual).unwrap();
        let source = actual.join("source.txt");
        std::fs::write(&source, "credential-path-test-canary").unwrap();
        let alias = root.path().join("alias");
        symlink(&actual, &alias).unwrap();
        let protected = ProtectedPaths {
            paths: vec![alias.join("source.txt"), alias.join("missing.txt")],
            ..ProtectedPaths::default()
        };
        assert!(protected.is_protected(&source));
        assert!(protected.is_protected(&root.path().join("actual/../actual/source.txt")));
        assert!(protected.is_protected(&actual.join("missing.txt")));
        #[cfg(target_os = "linux")]
        assert_eq!(protected.sandbox_masks(&actual).unwrap(), vec![source]);
        #[cfg(not(target_os = "linux"))]
        assert!(protected.sandbox_masks(&actual).is_err());
        assert_eq!(
            std::fs::read_to_string(actual.join("source.txt")).unwrap(),
            "credential-path-test-canary"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_filesystem_screening_accepts_storage_and_refuses_pseudo_or_unknown_types() {
        assert!(check_macos_filesystem_name(b"apfs").is_ok());
        assert!(check_macos_filesystem_name(b"hfs").is_ok());
        for name in [b"devfs".as_slice(), b"procfs", b"fdesc", b"kernfs"] {
            assert_eq!(
                check_macos_filesystem_name(name).unwrap_err().kind(),
                std::io::ErrorKind::PermissionDenied,
            );
        }
        assert_eq!(
            check_macos_filesystem_name(b"unknown-provider")
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::Unsupported,
        );
        assert!(check_macos_filesystem_name(b"").is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_descriptor_screening_allows_a_file_but_refuses_devfs() {
        let file = tempfile::tempfile().unwrap();
        let protected = ProtectedPaths::default();
        protected.check_fd(&file).unwrap();
        let device = std::fs::File::open("/dev/null").unwrap();
        assert_eq!(
            protected.check_fd(&device).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied,
        );
    }
}
