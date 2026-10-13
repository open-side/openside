//! Workspace path authority ends at a verified descriptor; Chromium reads a private copy.

use super::{check_owned, downloads::download_directory, Browser};
use crate::BrowserError;
use rustix::fs::{self, AtFlags, Dir, Mode, OFlags};
use std::{
    ffi::OsString,
    io,
    os::fd::OwnedFd,
    path::{Path, PathBuf},
};

const ROOT: &str = "upload-staging";
const DIRECTORY: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

fn single_component(path: &Path) -> bool {
    let mut components = path.components();
    matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none()
}

struct Staged {
    directory: OwnedFd,
    name: String,
    file: OsString,
    path: PathBuf,
    published: bool,
}
impl Drop for Staged {
    fn drop(&mut self) {
        if !self.published {
            if let Ok(child) = fs::openat(&self.directory, &self.name, DIRECTORY, Mode::empty()) {
                let _ = fs::unlinkat(&child, &self.file, AtFlags::empty());
            }
            let _ = fs::unlinkat(&self.directory, &self.name, AtFlags::REMOVEDIR);
        }
    }
}

fn source_file(path: &Path) -> io::Result<(std::fs::File, u64)> {
    let relative = path
        .strip_prefix("/")
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "upload path must be absolute"))?;
    let parent = relative
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "upload needs a file name"))?;
    let name = relative
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "upload needs a file name"))?;
    let directory = download_directory(Path::new("/"), parent, false)?;
    let file = std::fs::File::from(fs::openat(
        &directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "only regular files can be uploaded",
        ));
    }
    Ok((file, metadata.len()))
}

fn stage(root: &Path, tab: &str, source: &Path, original_name: OsString) -> io::Result<Staged> {
    if !single_component(Path::new(tab)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid upload tab",
        ));
    }
    let (source, size) = source_file(source)?;
    let mut source = std::io::Read::take(source, size);
    let staging_root = download_directory(root, Path::new(ROOT), true)?;
    if fs::fstat(&staging_root)?.st_mode & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "upload directory must be private",
        ));
    }
    let relative = Path::new(ROOT).join(tab);
    let directory = download_directory(root, &relative, true)?;
    if fs::fstat(&directory)?.st_mode & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "upload directory must be private",
        ));
    }
    let name = format!("{:032x}", rand::random::<u128>());
    fs::mkdirat(&directory, &name, Mode::from_raw_mode(0o700))?;
    let staged = Staged {
        path: root.join(relative).join(&name).join(&original_name),
        directory,
        name,
        file: original_name,
        published: false,
    };
    let child = fs::openat(&staged.directory, &staged.name, DIRECTORY, Mode::empty())?;
    let mut target = std::fs::File::from(fs::openat(
        &child,
        &staged.file,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )?);
    if std::io::copy(&mut source, &mut target)? != size {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "upload source changed while copying",
        ));
    }
    target.sync_all()?;
    fs::fchmod(&target, Mode::from_raw_mode(0o400))?;
    Ok(staged)
}

fn clear_contents(directory: &OwnedFd) -> io::Result<()> {
    for entry in Dir::read_from(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == c"." || name == c".." {
            continue;
        }
        match fs::openat(directory, name, DIRECTORY, Mode::empty()) {
            Ok(child) => {
                clear_contents(&child)?;
                fs::unlinkat(directory, name, AtFlags::REMOVEDIR)?;
            }
            Err(_) => {
                fs::unlinkat(directory, name, AtFlags::empty())?;
            }
        }
    }
    Ok(())
}

pub(super) fn remove_tab(root: &Path, tab: &str) -> io::Result<()> {
    let directory = match download_directory(root, Path::new(ROOT), false) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        result => result?,
    };
    let tab_directory = match fs::openat(&directory, tab, DIRECTORY, Mode::empty()) {
        Err(rustix::io::Errno::NOENT) => return Ok(()),
        result => result?,
    };
    clear_contents(&tab_directory)?;
    fs::unlinkat(&directory, tab, AtFlags::REMOVEDIR)?;
    Ok(())
}

fn prune(root: &Path, live: &std::collections::HashSet<String>) -> io::Result<Vec<String>> {
    let directory = match download_directory(root, Path::new(ROOT), false) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        result => result?,
    };
    if fs::fstat(&directory)?.st_mode & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "upload directory must be private",
        ));
    }
    let mut kept = Vec::new();
    for entry in Dir::read_from(&directory)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == c"." || name == c".." {
            continue;
        }
        let child = fs::openat(&directory, name, DIRECTORY, Mode::empty());
        if let (Ok(child), Some(tab)) = (
            child.as_ref(),
            name.to_str().ok().filter(|tab| live.contains(*tab)),
        ) {
            if fs::fstat(child)?.st_mode & 0o077 != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "upload directory must be private",
                ));
            }
            kept.push(tab.to_owned());
            continue;
        }
        match child {
            Ok(child) => {
                clear_contents(&child)?;
                fs::unlinkat(&directory, name, AtFlags::REMOVEDIR)?;
            }
            Err(_) => {
                fs::unlinkat(&directory, name, AtFlags::empty())?;
            }
        }
    }
    Ok(kept)
}

pub(super) async fn sweep_dead_profile(root: &Path) -> io::Result<()> {
    let root = root.to_owned();
    tokio::task::spawn_blocking(move || prune(&root, &std::collections::HashSet::new()))
        .await
        .map_err(io::Error::other)??;
    Ok(())
}

impl Browser {
    /// Stage an already-authorized canonical source, retaining its original file name and bytes
    /// until its tab/session ends. Every source component is opened without following links.
    pub async fn stage_upload(
        &self,
        owner: &str,
        tab: &str,
        source: &Path,
        original_name: &str,
    ) -> Result<PathBuf, BrowserError> {
        let epoch = {
            let state = self.inner.state.lock().await;
            check_owned(&state, owner, tab)?;
            if state.upload_closing.contains(tab) {
                return Err(BrowserError::TabNotReady(tab.to_owned()));
            }
            state.upload_epochs.get(tab).copied().unwrap_or_default()
        };
        if !single_component(Path::new(original_name)) {
            return Err(BrowserError::InvalidAction("invalid upload file name"));
        }
        let root = self.profile_path().to_owned();
        let (worker_tab, source, name) = (
            tab.to_owned(),
            source.to_owned(),
            OsString::from(original_name),
        );
        let mut staged =
            tokio::task::spawn_blocking(move || stage(&root, &worker_tab, &source, name))
                .await
                .map_err(io::Error::other)??;
        let mut state = self.inner.state.lock().await;
        check_owned(&state, owner, tab)?;
        if state.upload_closing.contains(tab)
            || state.upload_epochs.get(tab).copied().unwrap_or_default() != epoch
        {
            return Err(BrowserError::TabNotReady(tab.to_owned()));
        }
        if !state.uploads.contains_key(tab) {
            state
                .uploads
                .insert(tab.to_owned(), self.profile_path().to_owned());
        }
        staged.published = true;
        Ok(std::mem::take(&mut staged.path))
    }

    pub(super) async fn clear_session_uploads(&self, owner: &str) -> Result<(), BrowserError> {
        let tabs = {
            let mut state = self.inner.state.lock().await;
            let ids: Vec<String> = state
                .tabs
                .values()
                .filter(|tab| tab.owner_session_id.as_deref() == Some(owner))
                .map(|tab| tab.id.clone())
                .collect();
            let mut tabs = Vec::with_capacity(ids.len());
            for tab in ids {
                let epoch = state.upload_epochs.entry(tab.clone()).or_default();
                *epoch += 1;
                let epoch = *epoch;
                state.upload_closing.insert(tab.clone());
                state.uploads.remove(&tab);
                tabs.push((tab, epoch));
            }
            tabs
        };
        let root = self.profile_path().to_owned();
        let inner = std::sync::Arc::clone(&self.inner);
        // Cleanup owns its fence even if the lifecycle caller is cancelled while files drain.
        tokio::spawn(async move {
            let (tabs, result) = tokio::task::spawn_blocking(move || {
                let result = (|| -> io::Result<()> {
                    for (tab, _) in &tabs {
                        remove_tab(&root, tab)?;
                    }
                    Ok(())
                })();
                (tabs, result)
            })
            .await
            .map_err(io::Error::other)?;
            let mut state = inner.state.lock().await;
            for (tab, epoch) in tabs {
                if state.upload_epochs.get(&tab) == Some(&epoch) {
                    state.upload_closing.remove(&tab);
                }
            }
            result
        })
        .await
        .map_err(io::Error::other)??;
        Ok(())
    }

    pub(super) async fn prune_uploads(&self) -> Result<(), BrowserError> {
        let live = self.inner.state.lock().await.tabs.keys().cloned().collect();
        let root = self.profile_path().to_owned();
        let worker_root = root.clone();
        let kept = tokio::task::spawn_blocking(move || prune(&worker_root, &live))
            .await
            .map_err(io::Error::other)??;
        let mut state = self.inner.state.lock().await;
        for tab in kept {
            if state.tabs.contains_key(&tab) {
                state.uploads.insert(tab, root.clone());
            } else {
                let root = root.clone();
                tokio::task::spawn_blocking(move || remove_tab(&root, &tab));
            }
        }
        Ok(())
    }

    pub(super) async fn wipe_uploads(&self) -> Result<(), BrowserError> {
        let root = self.profile_path().to_owned();
        tokio::task::spawn_blocking(move || {
            let directory = match download_directory(&root, Path::new(ROOT), false) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                result => result?,
            };
            clear_contents(&directory)
        })
        .await
        .map_err(io::Error::other)??;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn verified_source_descriptor_survives_an_ancestor_swap_without_reading_the_link() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let workspace_root = std::fs::canonicalize(workspace.path()).unwrap();
        let outside_root = std::fs::canonicalize(outside.path()).unwrap();
        std::fs::create_dir(workspace_root.join("parent")).unwrap();
        std::fs::write(workspace_root.join("parent/file.txt"), b"original").unwrap();
        std::fs::write(outside_root.join("file.txt"), b"substituted").unwrap();
        let source = workspace_root.join("parent/file.txt");
        let (mut opened, _) = source_file(&source).unwrap();
        std::fs::rename(workspace_root.join("parent"), workspace_root.join("old")).unwrap();
        std::os::unix::fs::symlink(&outside_root, workspace_root.join("parent")).unwrap();
        assert!(source_file(&source).is_err());
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut opened, &mut bytes).unwrap();
        assert_eq!(bytes, b"original");
    }

    #[test]
    fn startup_sweep_keeps_only_live_tab_directories_without_following_links() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        for tab in ["live", "orphan"] {
            let directory =
                download_directory(root.path(), &Path::new(ROOT).join(tab).join("copy"), true)
                    .unwrap();
            fs::openat(
                &directory,
                "file",
                OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o400),
            )
            .unwrap();
        }
        std::fs::write(outside.path().join("canary"), b"untouched").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join(ROOT).join("linked")).unwrap();
        let kept = prune(
            root.path(),
            &std::collections::HashSet::from(["live".into()]),
        )
        .unwrap();
        assert_eq!(kept, ["live"]);
        assert!(root.path().join(ROOT).join("live/copy/file").exists());
        assert!(!root.path().join(ROOT).join("orphan").exists());
        assert!(!root.path().join(ROOT).join("linked").exists());
        assert_eq!(
            std::fs::read(outside.path().join("canary")).unwrap(),
            b"untouched"
        );
    }
}
