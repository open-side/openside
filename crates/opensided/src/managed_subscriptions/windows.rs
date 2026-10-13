// Win32 security descriptors have no safe standard-library creation equivalent.
#![allow(unsafe_code)]
use super::Error;
use std::{
    fs::File,
    io::Write,
    os::windows::{ffi::OsStrExt, io::FromRawHandle},
    path::Path,
};
use windows_sys::Win32::{
    Foundation::{LocalFree, GENERIC_WRITE, INVALID_HANDLE_VALUE},
    Security::{
        Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1},
        PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
    },
    Storage::FileSystem::{
        CreateDirectoryW, CreateFileW, MoveFileExW, CREATE_NEW, FILE_ATTRIBUTE_NORMAL,
        FILE_FLAG_OPEN_REPARSE_POINT, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    },
};

fn wide(path: &Path) -> Result<Vec<u16>, Error> {
    let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
    if value.contains(&0) {
        return Err(Error::Io);
    }
    value.push(0);
    Ok(value)
}
struct Descriptor(PSECURITY_DESCRIPTOR);
impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
impl Descriptor {
    fn owner_only() -> Result<Self, Error> {
        let sddl = wide(Path::new("D:P(A;OICI;FA;;;OW)"))?;
        let mut descriptor = Self(std::ptr::null_mut());
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor.0,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(Error::Io);
        }
        Ok(descriptor)
    }
    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }
}

pub(super) fn create_dir(path: &Path) -> Result<(), Error> {
    let descriptor = Descriptor::owner_only()?;
    let encoded = wide(path)?;
    if unsafe { CreateDirectoryW(encoded.as_ptr(), &descriptor.attributes()) } == 0 {
        return Err(Error::Io);
    }
    require_owner_acl(path)
}

pub(super) fn write_file(home: &Path, name: &str, bytes: &[u8]) -> Result<(), Error> {
    let descriptor = Descriptor::owner_only()?;
    let temporary = home.join(format!(".openside-{:032x}", rand::random::<u128>()));
    let encoded = wide(&temporary)?;
    let handle = unsafe {
        CreateFileW(
            encoded.as_ptr(),
            GENERIC_WRITE,
            0,
            &descriptor.attributes(),
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(Error::Io);
    }
    let mut file = unsafe { File::from_raw_handle(handle) };
    let result = (|| {
        require_owner_acl(&temporary)?;
        file.write_all(bytes).map_err(|_| Error::Io)?;
        file.sync_all().map_err(|_| Error::Io)
    })();
    drop(file);
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    let destination = wide(&home.join(name))?;
    if unsafe {
        MoveFileExW(
            encoded.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        let _ = std::fs::remove_file(&temporary);
        return Err(Error::Io);
    }
    require_owner_acl(&home.join(name))
}

pub(super) fn require_owner_acl(path: &Path) -> Result<(), Error> {
    use windows_sys::Win32::Security::{
        Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT},
        GetAce, IsWellKnownSid, WinCreatorOwnerRightsSid, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
        DACL_SECURITY_INFORMATION, INHERIT_ONLY_ACE,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
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
    if status != 0 || acl.is_null() || unsafe { (*acl).AceCount } != 1 {
        return Err(Error::Io);
    }
    let mut ace = std::ptr::null_mut();
    if unsafe { GetAce(acl, 0, &mut ace) } == 0 {
        return Err(Error::Io);
    }
    let header = unsafe { &*(ace as *const ACE_HEADER) };
    if header.AceType != 0
        || usize::from(header.AceSize) < std::mem::size_of::<ACCESS_ALLOWED_ACE>()
        || u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0
    {
        return Err(Error::Io);
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
        return Err(Error::Io);
    }
    Ok(())
}

pub(super) fn check_directory(path: &Path) -> Result<(), Error> {
    use std::os::windows::io::AsRawHandle;
    use std::path::{Component, PathBuf, Prefix};
    use windows_sys::Win32::Storage::FileSystem::*;
    fn open(path: &Path) -> Result<(File, BY_HANDLE_FILE_INFORMATION), Error> {
        let path = wide(path)?;
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(Error::Io);
        }
        let file = unsafe { File::from_raw_handle(handle) };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0
            || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
            || unsafe { GetFileType(file.as_raw_handle()) } != FILE_TYPE_DISK
        {
            return Err(Error::Io);
        }
        Ok((file, info))
    }
    let absolute = std::path::absolute(path).map_err(|_| Error::Io)?;
    let mut prefix = PathBuf::new();
    let mut held = Vec::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(value) => {
                if !matches!(value.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)) {
                    return Err(Error::Io);
                }
                prefix.push(component.as_os_str());
                continue;
            }
            Component::RootDir => prefix.push(component.as_os_str()),
            Component::Normal(name) => {
                if name.encode_wide().any(|c| c == u16::from(b':')) {
                    return Err(Error::Io);
                }
                prefix.push(name);
            }
            _ => return Err(Error::Io),
        }
        let (file, info) = open(&prefix)?;
        held.push((prefix.clone(), file, info));
    }
    for (path, _file, before) in held {
        let (_, after) = open(&path)?;
        if (
            before.dwVolumeSerialNumber,
            before.nFileIndexHigh,
            before.nFileIndexLow,
        ) != (
            after.dwVolumeSerialNumber,
            after.nFileIndexHigh,
            after.nFileIndexLow,
        ) {
            return Err(Error::Io);
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn managed_subscription_windows_home_and_file_have_owner_only_dacl_at_creation() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("managed-home");
        super::super::storage::create(&home).unwrap();
        require_owner_acl(&home).unwrap();
        super::super::storage::write_file(&home, "auth.json", b"fake-first-credential").unwrap();
        require_owner_acl(&home.join("auth.json")).unwrap();
        super::super::storage::write_file(&home, "auth.json", b"fake-rotated-credential").unwrap();
        require_owner_acl(&home.join("auth.json")).unwrap();
        let observed = super::super::storage::read_file(&home.join("auth.json"))
            .unwrap()
            .unwrap();
        assert!(observed.private && observed.bytes.as_str() == "fake-rotated-credential");
        super::super::storage::wipe(&home).unwrap();
        assert!(!home.exists());
    }
}
