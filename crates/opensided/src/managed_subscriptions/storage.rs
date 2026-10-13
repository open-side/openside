use super::Error;
use sha2::{Digest, Sha256};
#[cfg(not(windows))]
use std::io::Write;
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};
use zeroize::Zeroizing;

pub(super) struct Observed {
    pub bytes: Zeroizing<String>,
    pub digest: String,
    pub mtime: u128,
    pub private: bool,
}
pub(super) fn filename(source: &str) -> &'static str {
    if source == "claude-code" {
        ".credentials.json"
    } else {
        "auth.json"
    }
}
pub(super) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(super) fn home(root: &Path, source: &str, id: &str) -> Result<PathBuf, Error> {
    super::validate(source, id)?;
    if id == "default" {
        return Err(Error::InvalidValue);
    }
    Ok(root.join(source).join(id))
}

#[cfg(unix)]
fn directory(path: &Path, create: bool) -> Result<File, Error> {
    use rustix::fs::{mkdirat, open, openat, Mode, OFlags};
    use std::path::Component;
    let path = std::path::absolute(path).map_err(|_| Error::Io)?;
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut fd = open("/", flags, Mode::empty()).map_err(|_| Error::Io)?;
    for component in path.components() {
        let name = match component {
            Component::RootDir => continue,
            Component::Normal(name) => name,
            _ => return Err(Error::Io),
        };
        if create {
            match mkdirat(&fd, name, Mode::from_raw_mode(0o700)) {
                Ok(()) | Err(rustix::io::Errno::EXIST) => (),
                Err(_) => return Err(Error::Io),
            }
        }
        fd = openat(&fd, name, flags, Mode::empty()).map_err(|_| Error::Io)?;
    }
    Ok(File::from(fd))
}

#[cfg(windows)]
fn check_directory(path: &Path) -> Result<(), Error> {
    super::windows::check_directory(path)
}

#[cfg(not(any(unix, windows)))]
fn check_directory(path: &Path) -> Result<(), Error> {
    for ancestor in path.ancestors() {
        let metadata = fs::symlink_metadata(ancestor).map_err(|_| Error::Io)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(Error::Io);
        }
    }
    Ok(())
}

pub(super) fn create(home: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use rustix::fs::{mkdirat, openat, Mode, OFlags};
        use std::os::unix::fs::PermissionsExt;
        let parent = directory(home.parent().ok_or(Error::Io)?, true)?;
        let name = home.file_name().ok_or(Error::Io)?;
        mkdirat(&parent, name, Mode::from_raw_mode(0o700)).map_err(|_| Error::Io)?;
        let fd = openat(
            &parent,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| Error::Io)?;
        File::from(fd)
            .set_permissions(fs::Permissions::from_mode(0o700))
            .map_err(|_| Error::Io)
    }
    #[cfg(not(unix))]
    {
        if fs::symlink_metadata(home).is_ok() {
            return Err(Error::Io);
        }
        let mut missing = Vec::new();
        let mut cursor = home;
        while !cursor.exists() {
            missing.push(cursor);
            cursor = cursor.parent().ok_or(Error::Io)?;
        }
        check_directory(cursor)?;
        for path in missing.into_iter().rev() {
            #[cfg(windows)]
            super::windows::create_dir(path)?;
            #[cfg(not(windows))]
            fs::create_dir(path).map_err(|_| Error::Io)?;
        }
        check_directory(home)
    }
}

pub(super) fn read_file(path: &Path) -> Result<Option<Observed>, Error> {
    #[cfg(unix)]
    let file = {
        use rustix::fs::{openat, Mode, OFlags};
        let parent = path.parent().ok_or(Error::Io)?;
        if !parent.exists() {
            return Ok(None);
        }
        let dir = directory(parent, false)?;
        match openat(
            &dir,
            path.file_name().ok_or(Error::Io)?,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => File::from(fd),
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(_) => return Err(Error::Io),
        }
    };
    #[cfg(not(unix))]
    let file = {
        if !path.exists() {
            return Ok(None);
        }
        check_directory(path.parent().ok_or(Error::Io)?)?;
        let metadata = fs::symlink_metadata(path).map_err(|_| Error::Io)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(Error::Io);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT)
                .open(path)
                .map_err(|_| Error::Io)?
        }
        #[cfg(not(windows))]
        File::open(path).map_err(|_| Error::Io)?
    };
    let before = file.metadata().map_err(|_| Error::Io)?;
    if !before.is_file() || before.file_type().is_symlink() || before.len() > 262_144 {
        return Err(Error::Io);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.nlink() != 1 {
            return Err(Error::Io);
        }
    }
    let mut bytes = Zeroizing::new(String::new());
    (&file)
        .take(262_145)
        .read_to_string(&mut bytes)
        .map_err(|_| Error::Io)?;
    let after = file.metadata().map_err(|_| Error::Io)?;
    if bytes.len() > 262_144
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
    {
        return Err(Error::Io);
    }
    let mtime = before
        .modified()
        .map_err(|_| Error::Io)?
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::Io)?
        .as_nanos();
    #[cfg(unix)]
    let private = {
        use std::os::unix::fs::PermissionsExt;
        before.permissions().mode() & 0o777 == 0o600
    };
    #[cfg(windows)]
    let private = super::windows::require_owner_acl(path).is_ok();
    #[cfg(not(any(unix, windows)))]
    let private = true;
    Ok(Some(Observed {
        digest: digest(bytes.as_bytes()),
        bytes,
        mtime,
        private,
    }))
}

pub(super) fn write_file(home: &Path, name: &str, bytes: &[u8]) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use rustix::fs::{openat, renameat, unlinkat, AtFlags, Mode, OFlags};
        let dir = directory(home, false)?;
        let temporary = format!(".openside-{:032x}", rand::random::<u128>());
        let fd = openat(
            &dir,
            temporary.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|_| Error::Io)?;
        let mut file = File::from(fd);
        let result = (|| {
            file.write_all(bytes).map_err(|_| Error::Io)?;
            file.sync_all().map_err(|_| Error::Io)?;
            renameat(&dir, temporary.as_str(), &dir, name).map_err(|_| Error::Io)?;
            dir.sync_all().map_err(|_| Error::Io)
        })();
        if result.is_err() {
            let _ = unlinkat(&dir, temporary.as_str(), AtFlags::empty());
        }
        result
    }
    #[cfg(windows)]
    {
        check_directory(home)?;
        super::windows::write_file(home, name, bytes)
    }
    #[cfg(not(any(unix, windows)))]
    {
        check_directory(home)?;
        let mut file = tempfile::NamedTempFile::new_in(home).map_err(|_| Error::Io)?;
        file.write_all(bytes).map_err(|_| Error::Io)?;
        file.as_file().sync_all().map_err(|_| Error::Io)?;
        file.persist(home.join(name)).map_err(|_| Error::Io)?;
        Ok(())
    }
}

pub(super) fn wipe(home: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use rustix::fs::{openat, unlinkat, AtFlags, Dir, Mode, OFlags};
        fn contents(dir: &File) -> Result<(), Error> {
            for entry in Dir::read_from(dir).map_err(|_| Error::Io)? {
                let entry = entry.map_err(|_| Error::Io)?;
                let name = entry.file_name();
                if name.to_bytes() == b"." || name.to_bytes() == b".." {
                    continue;
                }
                match openat(
                    dir,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                ) {
                    Ok(fd) => {
                        contents(&File::from(fd))?;
                        unlinkat(dir, name, AtFlags::REMOVEDIR).map_err(|_| Error::Io)?;
                    }
                    Err(rustix::io::Errno::NOTDIR | rustix::io::Errno::LOOP) => {
                        unlinkat(dir, name, AtFlags::empty()).map_err(|_| Error::Io)?;
                    }
                    Err(_) => return Err(Error::Io),
                }
            }
            Ok(())
        }
        let parent = directory(home.parent().ok_or(Error::Io)?, false)?;
        let name = home.file_name().ok_or(Error::Io)?;
        let fd = match openat(
            &parent,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => return Ok(()),
            Err(_) => return Err(Error::Io),
        };
        contents(&File::from(fd))?;
        unlinkat(&parent, name, AtFlags::REMOVEDIR).map_err(|_| Error::Io)?;
        parent.sync_all().map_err(|_| Error::Io)
    }
    #[cfg(not(unix))]
    {
        if !home.exists() {
            return Ok(());
        }
        check_directory(home)?;
        fs::remove_dir_all(home).map_err(|_| Error::Io)
    }
}
pub(super) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
