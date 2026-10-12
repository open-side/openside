#![cfg(target_os = "linux")]

use std::{
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
};

pub(crate) struct Mount {
    id: u32,
    device: (u32, u32),
    root: PathBuf,
    point: PathBuf,
}

pub(crate) fn parse(bytes: &[u8]) -> Result<Vec<Mount>, ()> {
    let mut mounts = Vec::new();
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        if mounts.len() >= 4096 {
            return Err(());
        }
        let mut fields = line.split(|byte| *byte == b' ');
        let id = number(fields.next().ok_or(())?)?;
        number(fields.next().ok_or(())?)?;
        let device = fields.next().ok_or(())?;
        let colon = device.iter().position(|byte| *byte == b':').ok_or(())?;
        let device = (number(&device[..colon])?, number(&device[colon + 1..])?);
        let root = path(fields.next().ok_or(())?)?;
        let point = path(fields.next().ok_or(())?)?;
        if !point.is_absolute() || !fields.any(|field| field == b"-") {
            return Err(());
        }
        mounts.push(Mount {
            id,
            device,
            root,
            point,
        });
    }
    if mounts.is_empty() {
        return Err(());
    }
    Ok(mounts)
}

pub(crate) fn has_alias(mounts: &[Mount], path: &Path, own_id: u32) -> bool {
    let Some(own) = mounts.iter().find(|mount| mount.id == own_id) else {
        return true;
    };
    if !own.root.is_absolute() {
        return true;
    }
    let Ok(relative) = path.strip_prefix(&own.point) else {
        return true;
    };
    let inode_path = own.root.join(relative);
    mounts.iter().any(|mount| {
        mount.id != own.id && mount.device == own.device && inode_path.starts_with(&mount.root)
    })
}

fn number(bytes: &[u8]) -> Result<u32, ()> {
    std::str::from_utf8(bytes)
        .map_err(|_| ())?
        .parse()
        .map_err(|_| ())
}

fn path(bytes: &[u8]) -> Result<PathBuf, ()> {
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            let octal = bytes.get(index + 1..index + 4).ok_or(())?;
            if !octal.iter().all(|byte| (b'0'..=b'7').contains(byte)) {
                return Err(());
            }
            let value = (u16::from(octal[0] - b'0') << 6)
                | (u16::from(octal[1] - b'0') << 3)
                | u16::from(octal[2] - b'0');
            decoded.push(u8::try_from(value).map_err(|_| ())?);
            index += 4;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    Ok(std::ffi::OsString::from_vec(decoded).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_and_file_bind_aliases_are_refused_but_unrelated_roots_are_allowed() {
        let root = b"1 0 8:1 / / rw - ext4 test rw\n";
        assert!(!has_alias(
            &parse(root).unwrap(),
            Path::new("/tmp/source.txt"),
            1
        ));
        for extra in [
            "2 1 8:1 / /alias rw - ext4 test rw\n",
            "2 1 8:1 /tmp /alias rw - ext4 test rw\n",
            "2 1 8:1 /tmp/source.txt /alias rw - ext4 test rw\n",
        ] {
            let bytes = [root.as_slice(), extra.as_bytes()].concat();
            assert!(has_alias(
                &parse(&bytes).unwrap(),
                Path::new("/tmp/source.txt"),
                1
            ));
        }
        let bytes = [
            root.as_slice(),
            b"2 1 8:1 /other /alias rw - ext4 test rw\n",
        ]
        .concat();
        assert!(!has_alias(
            &parse(&bytes).unwrap(),
            Path::new("/tmp/source.txt"),
            1
        ));
        let bytes = [root.as_slice(), b"2 1 8:1 /tmp /alias rw - ext4 test rw\n"].concat();
        assert!(has_alias(
            &parse(&bytes).unwrap(),
            Path::new("/alias/source.txt"),
            2
        ));
    }

    #[test]
    fn mount_paths_decode_kernel_octal_escapes_and_invalid_metadata_fails_closed() {
        let mounts = parse(b"1 0 8:1 / /tmp/test\\040space rw - ext4 test rw\n").unwrap();
        assert_eq!(mounts[0].point, Path::new("/tmp/test space"));
        assert!(parse(b"1 0 0:5 net:[test] /netns rw - nsfs nsfs rw\n").is_ok());
        for bytes in [
            b"".as_slice(),
            b"1 0 invalid / / rw - ext4 test rw",
            b"1 0 8:1 / /bad\\999 rw - ext4 test rw",
        ] {
            assert!(parse(bytes).is_err());
        }
    }
}
