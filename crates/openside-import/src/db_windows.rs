// Win32 handles and security descriptors have no safe standard-library equivalents.
#![allow(unsafe_code)]

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::{Component, Path, PathBuf};
use windows_sys::Win32::Foundation::{LocalFree, GENERIC_READ, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::Cryptography::{
    BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows_sys::Win32::Storage::FileSystem::*;

fn wide(path: &Path) -> io::Result<Vec<u16>> {
    let mut value: Vec<_> = path.as_os_str().encode_wide().collect();
    if value.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NUL in source path",
        ));
    }
    value.push(0);
    Ok(value)
}

fn info(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(info)
}

pub(super) fn identity(file: &File) -> io::Result<(u32, u64)> {
    let value = info(file)?;
    Ok((
        value.dwVolumeSerialNumber,
        (u64::from(value.nFileIndexHigh) << 32) | u64::from(value.nFileIndexLow),
    ))
}

fn open_component(path: &Path, directory: bool) -> io::Result<File> {
    let path = wide(path)?;
    let flags = FILE_FLAG_OPEN_REPARSE_POINT
        | if directory {
            FILE_FLAG_BACKUP_SEMANTICS
        } else {
            0
        };
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            if directory {
                FILE_READ_ATTRIBUTES
            } else {
                GENERIC_READ
            },
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            flags,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(handle) };
    let value = info(&file)?;
    if value.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (value.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
        || unsafe { GetFileType(file.as_raw_handle()) } != FILE_TYPE_DISK
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "source is not a regular non-reparse file or directory",
        ));
    }
    Ok(file)
}

pub(super) fn open_source(path: &Path) -> io::Result<File> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    if !absolute.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "drive-relative sources are not allowed",
        ));
    }
    let mut prefix = PathBuf::new();
    let mut ancestors = Vec::new();
    let mut components = absolute.components().peekable();
    while let Some(component) = components.next() {
        match component {
            Component::Prefix(value) => {
                // Device namespaces and UNC shares are not local browser profiles.
                if !matches!(
                    value.kind(),
                    std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)
                ) {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "source must be on a local drive",
                    ));
                }
                prefix.push(component.as_os_str());
                continue;
            }
            Component::RootDir => prefix.push(component.as_os_str()),
            Component::Normal(name) => {
                if name.encode_wide().any(|c| c == u16::from(b':')) {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "alternate data streams are not browser sources",
                    ));
                }
                prefix.push(name);
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "source traversal is not allowed",
                ))
            }
        }
        let directory = components.peek().is_some();
        let file = open_component(&prefix, directory)?;
        if directory {
            ancestors.push((prefix.clone(), file));
        } else {
            // CreateFileW is path-based: detect ancestor replacement across the open.
            // SQLite later reopens by name; same-user namespace races remain documented.
            for (path, ancestor) in ancestors {
                if identity(&ancestor)? != identity(&open_component(&path, true)?)? {
                    return Err(io::Error::other("source ancestor changed while opening"));
                }
            }
            return Ok(file);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "source has no filename",
    ))
}

#[derive(PartialEq, Eq)]
pub(super) struct SourceStamp {
    pub(super) size: u64,
    pub(super) header: [u8; 100],
    volume: u32,
    index: u64,
    modified: u64,
}

impl SourceStamp {
    pub(super) fn read(file: &File) -> io::Result<Self> {
        let value = info(file)?;
        let mut header = [0; 100];
        // File clones share the cursor on Windows; restore it for cold-copy reads.
        let mut reader = file.try_clone()?;
        let offset = reader.stream_position()?;
        reader.seek(SeekFrom::Start(0))?;
        let result = reader.read_exact(&mut header);
        reader.seek(SeekFrom::Start(offset))?;
        result?;
        Ok(Self {
            size: (u64::from(value.nFileSizeHigh) << 32) | u64::from(value.nFileSizeLow),
            header,
            volume: value.dwVolumeSerialNumber,
            index: (u64::from(value.nFileIndexHigh) << 32) | u64::from(value.nFileIndexLow),
            modified: (u64::from(value.ftLastWriteTime.dwHighDateTime) << 32)
                | u64::from(value.ftLastWriteTime.dwLowDateTime),
        })
    }
}

struct Descriptor(PSECURITY_DESCRIPTOR);
impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

pub(super) struct PrivateDir(PathBuf);

impl PrivateDir {
    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for PrivateDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(super) fn private_dir() -> io::Result<PrivateDir> {
    // Protected, inheritable OWNER RIGHTS ACL: no permissive creation-to-chmod window.
    let sddl = wide(Path::new("D:P(A;OICI;FA;;;OW)"))?;
    let mut descriptor = Descriptor(std::ptr::null_mut());
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor.0,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    for _ in 0..16 {
        let mut random = [0u8; 16];
        if unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                random.as_mut_ptr(),
                random.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        } < 0
        {
            return Err(io::Error::other("private directory randomness unavailable"));
        }
        let name = format!("openside-import-{:032x}", u128::from_ne_bytes(random));
        let path = std::env::temp_dir().join(name);
        let encoded = wide(&path)?;
        if unsafe { CreateDirectoryW(encoded.as_ptr(), &attributes) } != 0 {
            let directory = PrivateDir(path);
            require_owner_acl(directory.path())?;
            return Ok(directory);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::AlreadyExists {
            return Err(error);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "private directory collision",
    ))
}

pub(super) fn require_owner_acl(path: &Path) -> io::Result<()> {
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        GetAce, IsWellKnownSid, WinCreatorOwnerRightsSid, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
        DACL_SECURITY_INFORMATION, INHERIT_ONLY_ACE,
    };
    let path = wide(path)?;
    let mut descriptor = Descriptor(std::ptr::null_mut());
    let mut acl: *mut ACL = std::ptr::null_mut();
    let status = unsafe {
        GetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut acl,
            std::ptr::null_mut(),
            &mut descriptor.0,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let denied = || {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "private snapshot requires an owner-only DACL",
        )
    };
    if acl.is_null() || unsafe { (*acl).AceCount } != 1 {
        return Err(denied());
    }
    let mut ace = std::ptr::null_mut();
    if unsafe { GetAce(acl, 0, &mut ace) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let header = unsafe { &*(ace as *const ACE_HEADER) };
    if header.AceType != 0
        || usize::from(header.AceSize) < std::mem::size_of::<ACCESS_ALLOWED_ACE>()
        || u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0
    {
        return Err(denied());
    }
    let ace = unsafe { &*(ace as *const ACCESS_ALLOWED_ACE) };
    if ace.Mask != FILE_ALL_ACCESS
        || unsafe {
            IsWellKnownSid(
                std::ptr::addr_of!(ace.SidStart).cast_mut().cast(),
                WinCreatorOwnerRightsSid,
            )
        } == 0
    {
        return Err(denied());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Item, MAX_SOURCE_BYTES};
    use crate::{BrowserKind, ImportError};
    use rusqlite::Connection;
    use std::fs::{self, OpenOptions};
    use std::os::windows::fs::OpenOptionsExt;

    fn item(path: PathBuf) -> Item {
        Item {
            browser: BrowserKind::Chrome,
            what: "cookies",
            path,
        }
    }

    #[test]
    fn exclusive_browser_source_is_typed_and_retryable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Cookies");
        fs::write(&path, b"fixture").unwrap();
        let lock = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        let source = item(path);
        let error = source.probe().unwrap_err();
        assert!(matches!(error, ImportError::SourceLocked { .. }));
        assert_eq!(
            crate::everything::safe_note(&error),
            "Close Chrome and retry importing this browser data."
        );
        assert!(matches!(
            source.open(),
            Err(ImportError::SourceLocked { .. })
        ));
        drop(lock);
        assert!(source.probe().unwrap());
    }

    #[test]
    fn source_handles_allow_browser_writes_and_rename() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("History");
        fs::write(&path, b"fixture").unwrap();
        let source = open_source(&path).unwrap();
        let writer = OpenOptions::new().write(true).open(&path).unwrap();
        fs::rename(&path, dir.path().join("renamed")).unwrap();
        drop(writer);
        drop(source);
    }

    #[test]
    fn rejects_directory_and_junction_ancestors() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        fs::write(real.join("History"), b"fixture").unwrap();
        assert!(open_source(&real).is_err());
        let link = dir.path().join("junction");
        let status = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(&real)
            .output()
            .unwrap();
        assert!(status.status.success(), "junction fixture creation failed");
        assert!(open_source(&link.join("History")).is_err());
        assert!(open_source(&link).is_err());
        fs::remove_dir(link).unwrap();
    }

    #[test]
    fn private_snapshot_is_owner_only_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("History #%.sqlite");
        let writer = Connection::open(&path).unwrap();
        writer
            .execute_batch("CREATE TABLE records(value); INSERT INTO records VALUES(42);")
            .unwrap();
        drop(writer);
        let before = fs::read(&path).unwrap();
        let snapshot = item(path.clone()).open().unwrap();
        assert_eq!(
            snapshot
                .conn
                .query_row("SELECT value FROM records", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            42
        );
        let copy = PathBuf::from(snapshot.conn.path().unwrap());
        require_owner_acl(&copy).unwrap();
        require_owner_acl(copy.parent().unwrap()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), before);
        drop(snapshot);
        assert!(!copy.exists());
    }

    #[test]
    fn live_wal_backup_contains_committed_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("History");
        use std::io::BufRead;
        use std::process::{Command, Stdio};

        // A browser is another process. An in-process writer would reuse SQLite's
        // writable SHM mapping instead of exercising readonly_shm.
        let mut writer = WriterProcess(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    "db::windows::tests::live_wal_writer_child",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("OPENSIDE_TEST_WAL_SOURCE", &path)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut output = std::io::BufReader::new(writer.0.stdout.take().unwrap());
        let mut ready = false;
        for line in (&mut output).lines() {
            if line.unwrap().ends_with("OPENSIDE_WAL_READY") {
                ready = true;
                break;
            }
        }
        assert!(ready, "external WAL writer exited before fixture readiness");
        let before: Vec<_> = ["History", "History-wal", "History-shm"]
            .iter()
            .map(|name| fs::read(dir.path().join(name)).unwrap())
            .collect();
        let snapshot = item(path).open().unwrap();
        assert_eq!(
            snapshot
                .conn
                .query_row("SELECT value FROM records", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            73
        );
        drop(snapshot);
        for (name, bytes) in ["History", "History-wal", "History-shm"].iter().zip(before) {
            assert_eq!(fs::read(dir.path().join(name)).unwrap(), bytes);
        }
        drop(writer.0.stdin.take());
        assert!(writer.0.wait().unwrap().success());
    }

    struct WriterProcess(std::process::Child);

    impl Drop for WriterProcess {
        fn drop(&mut self) {
            // Closing only our fixture's stdin releases its connection gracefully.
            drop(self.0.stdin.take());
            let _ = self.0.wait();
        }
    }

    #[test]
    #[ignore = "invoked as an isolated writer by live_wal_backup_contains_committed_rows"]
    fn live_wal_writer_child() {
        use std::io::Write;
        let path = std::env::var_os("OPENSIDE_TEST_WAL_SOURCE").expect("fixture source path");
        let writer = Connection::open(PathBuf::from(path)).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE records(value); INSERT INTO records VALUES(73);").unwrap();
        println!("OPENSIDE_WAL_READY");
        std::io::stdout().flush().unwrap();
        let mut release = [0u8; 1];
        std::io::stdin().read(&mut release).unwrap();
        drop(writer);
    }

    #[test]
    fn closed_wal_copy_preserves_source_and_header_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("History");
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE records(value); INSERT INTO records VALUES(19);").unwrap();
        drop(writer);
        let before = fs::read(&path).unwrap();
        let source = open_source(&path).unwrap();
        let first = SourceStamp::read(&source).unwrap();
        let snapshot = item(path.clone()).open().unwrap();
        assert_eq!(
            snapshot
                .conn
                .query_row("SELECT value FROM records", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            19
        );
        assert!(SourceStamp::read(&source).unwrap() == first);
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn oversized_source_is_refused_before_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("History");
        let file = File::create(&path).unwrap();
        file.set_len(MAX_SOURCE_BYTES + 1).unwrap();
        drop(file);
        assert!(matches!(
            item(path).open(),
            Err(ImportError::SourceTooLarge { .. })
        ));
    }

    #[test]
    fn source_stamp_detects_replacement_and_content_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("History");
        fs::write(&path, [1u8; 100]).unwrap();
        let first = SourceStamp::read(&open_source(&path).unwrap()).unwrap();
        fs::write(&path, [2u8; 100]).unwrap();
        assert!(SourceStamp::read(&open_source(&path).unwrap()).unwrap() != first);
        fs::remove_file(&path).unwrap();
        fs::write(&path, [1u8; 100]).unwrap();
        assert!(SourceStamp::read(&open_source(&path).unwrap()).unwrap() != first);
    }

    #[test]
    fn missing_shm_is_not_created_for_existing_wal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("History");
        let writer = Connection::open(&path).unwrap();
        writer
            .execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE records(value);")
            .unwrap();
        drop(writer);
        fs::write(dir.path().join("History-wal"), b"fixture").unwrap();
        assert!(matches!(item(path).open(), Err(ImportError::Locked { .. })));
        assert!(!dir.path().join("History-shm").exists());
    }

    #[test]
    fn source_rejects_file_and_directory_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("profile");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("History"), b"fixture").unwrap();
        let file_link = dir.path().join("HistoryLink");
        match std::os::windows::fs::symlink_file(real.join("History"), &file_link) {
            Ok(()) => {}
            Err(error) if error.raw_os_error() == Some(1314) => {
                eprintln!("SKIP: Windows symlink test requires Developer Mode or symlink privilege; junction rejection is tested separately");
                return;
            }
            Err(error) => panic!("symlink fixture: {error}"),
        }
        assert!(open_source(&file_link).is_err());
        let directory_link = dir.path().join("ProfileLink");
        std::os::windows::fs::symlink_dir(&real, &directory_link).unwrap();
        assert!(open_source(&directory_link.join("History")).is_err());
        std::fs::remove_dir(directory_link).unwrap();
    }
}
