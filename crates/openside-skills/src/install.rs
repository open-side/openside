//! Copying skills between directories (import into openside, install into another
//! agent). The source is untrusted: it is validated, symlinks may not leave it, special
//! files and executables are refused, and the result appears atomically.

use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    error::{Error, Result},
    frontmatter::{self, MAX_SKILL_MD_BYTES},
};

const MAX_FILES: usize = 512;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 8 * 1024 * 1024;
const MAX_DEPTH: usize = 8;

/// How to treat an install.
#[derive(Debug, Clone, Copy, Default)]
pub struct InstallOptions {
    /// Keep executable bits and accept files that have them. Without it any
    /// executable file in the source aborts the install.
    pub allow_scripts: bool,
    /// Replace an existing skill of the same name.
    pub force: bool,
}

/// Result of a successful install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// Installed skill name (the destination directory).
    pub name: String,
    /// Final location.
    pub path: PathBuf,
    /// Number of files written.
    pub files: usize,
    /// Total bytes written.
    pub bytes: u64,
}

/// One file to place in a skill directory.
pub struct StagedFile {
    /// Path relative to the skill directory.
    pub path: PathBuf,
    /// File content.
    pub content: Vec<u8>,
    /// Whether the file keeps an executable bit.
    pub executable: bool,
}

/// A name that is unique in this process and very likely across processes.
pub(crate) fn unique(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!(
        ".{prefix}-{}-{nanos}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// Removes a staging directory unless disarmed.
struct Staging(Option<PathBuf>);

impl Drop for Staging {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_dir_all(path);
        }
    }
}

/// Copy the skill directory `src` into `<dest_root>/<name>`, where `name` comes from
/// its validated `SKILL.md`.
pub fn install_dir(src: &Path, dest_root: &Path, options: InstallOptions) -> Result<Installed> {
    let root = fs::canonicalize(src).map_err(|e| Error::io_path("resolving", src, e))?;
    let files = collect(&root, options)?;
    let skill_md = files
        .iter()
        .find(|f| f.path == Path::new("SKILL.md"))
        .ok_or_else(|| Error::NotFound(format!("{}/SKILL.md", src.display())))?;
    let text = std::str::from_utf8(&skill_md.content)
        .map_err(|_| Error::invalid("SKILL.md", "is not valid UTF-8 text"))?;
    let name = frontmatter::parse(text)?.frontmatter.name;
    write_skill(dest_root, &name, files, options.force)
}

/// Place validated files at `<dest_root>/<name>` atomically (staging dir + rename).
/// `SKILL.md` must be among them, parse, and carry the same name.
pub fn write_skill(
    dest_root: &Path,
    name: &str,
    files: Vec<StagedFile>,
    force: bool,
) -> Result<Installed> {
    frontmatter::validate_name(name)?;
    // Callers are not trusted to have checked: a path that leaves the skill directory or a
    // duplicate that overwrites an earlier file must never reach the file system.
    let mut seen = std::collections::BTreeSet::new();
    for file in &files {
        if file.path != Path::new("SKILL.md") {
            let relative = file
                .path
                .to_str()
                .ok_or_else(|| Error::invalid("resource", "path is not valid UTF-8"))?;
            crate::skill::checked_resource_path(relative)?;
            // `Path` hides `.` and empty components; the spelling must already be canonical.
            if relative
                .split('/')
                .any(|part| part.is_empty() || part == ".")
            {
                return Err(Error::invalid("resource", "path is not normalized"));
            }
        }
        if !seen.insert(file.path.clone()) {
            return Err(Error::invalid(
                "resource",
                format!("`{}` appears twice", file.path.display()),
            ));
        }
    }
    let skill_md = files
        .iter()
        .find(|f| f.path == Path::new("SKILL.md"))
        .ok_or_else(|| Error::NotFound("SKILL.md".into()))?;
    let text = std::str::from_utf8(&skill_md.content)
        .map_err(|_| Error::invalid("SKILL.md", "is not valid UTF-8 text"))?;
    if frontmatter::parse(text)?.frontmatter.name != name {
        return Err(Error::invalid("name", "does not match the install name"));
    }
    fs::create_dir_all(dest_root).map_err(|e| Error::io_path("creating", dest_root, e))?;
    let target = dest_root.join(name);
    let exists = fs::symlink_metadata(&target).is_ok();
    if exists && !force {
        return Err(Error::Exists(name.to_owned()));
    }
    let staging_path = dest_root.join(unique("install"));
    fs::create_dir(&staging_path).map_err(|e| Error::io_path("creating", &staging_path, e))?;
    let guard = Staging(Some(staging_path.clone()));
    let mut bytes = 0u64;
    let count = files.len();
    for file in &files {
        let path = staging_path.join(&file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::io_path("creating", parent, e))?;
        }
        let mut out = fs::File::create(&path).map_err(|e| Error::io_path("creating", &path, e))?;
        out.write_all(&file.content)
            .map_err(|e| Error::io_path("writing", &path, e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if file.executable { 0o755 } else { 0o644 };
            out.set_permissions(fs::Permissions::from_mode(mode))
                .map_err(|e| Error::io_path("protecting", &path, e))?;
        }
        bytes += file.content.len() as u64;
    }
    if exists {
        remove_entry(&target)?;
    }
    fs::rename(&staging_path, &target).map_err(|e| Error::io_path("installing", &target, e))?;
    // The staging directory now is the skill; nothing is left to clean up.
    let mut guard = guard;
    guard.0 = None;
    Ok(Installed {
        name: name.to_owned(),
        path: target,
        files: count,
        bytes,
    })
}

/// Remove a skill entry: a symlink is unlinked, never followed.
pub(crate) fn remove_entry(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|e| Error::io_path("inspecting", path, e))?;
    let result = if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    result.map_err(|e| Error::io_path("removing", path, e))
}

fn collect(root: &Path, options: InstallOptions) -> Result<Vec<StagedFile>> {
    let mut files = Vec::new();
    let mut total = 0u64;
    let mut visited = HashSet::from([root.to_path_buf()]);
    let mut stack = vec![(root.to_path_buf(), PathBuf::new(), 0usize)];
    while let Some((dir, relative, depth)) = stack.pop() {
        if depth >= MAX_DEPTH {
            return Err(Error::TooLarge {
                what: "skill directory depth",
                max: MAX_DEPTH,
            });
        }
        let entries = fs::read_dir(&dir).map_err(|e| Error::io_path("listing", &dir, e))?;
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name();
            // Hidden entries (.git, editor state) are never part of a skill.
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            let rel = relative.join(&name);
            let real =
                fs::canonicalize(&path).map_err(|e| Error::io_path("resolving", &path, e))?;
            if !real.starts_with(root) {
                return Err(Error::Unsafe(format!(
                    "{} is a symlink that leaves the skill directory",
                    rel.display()
                )));
            }
            let metadata =
                fs::metadata(&real).map_err(|e| Error::io_path("inspecting", &real, e))?;
            if metadata.is_dir() {
                if !visited.insert(real.clone()) {
                    return Err(Error::Unsafe(format!(
                        "{} loops back into the skill",
                        rel.display()
                    )));
                }
                stack.push((real, rel, depth + 1));
                continue;
            }
            if !metadata.is_file() {
                return Err(Error::Unsafe(format!(
                    "{} is not a regular file",
                    rel.display()
                )));
            }
            let executable = is_executable(&metadata);
            if executable && !options.allow_scripts {
                return Err(Error::Unsafe(format!(
                    "{} is executable; pass allow-scripts to install it",
                    rel.display()
                )));
            }
            if metadata.len() > MAX_FILE_BYTES {
                return Err(Error::TooLarge {
                    what: "skill file",
                    max: MAX_FILE_BYTES as usize,
                });
            }
            total += metadata.len();
            if files.len() >= MAX_FILES || total > MAX_TOTAL_BYTES {
                return Err(Error::TooLarge {
                    what: "skill directory",
                    max: MAX_TOTAL_BYTES as usize,
                });
            }
            if rel == Path::new("SKILL.md") && metadata.len() > MAX_SKILL_MD_BYTES as u64 {
                return Err(Error::TooLarge {
                    what: "SKILL.md",
                    max: MAX_SKILL_MD_BYTES,
                });
            }
            let content = fs::read(&real).map_err(|e| Error::io_path("reading", &real, e))?;
            files.push(StagedFile {
                path: rel,
                content,
                executable,
            });
        }
    }
    Ok(files)
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_: &fs::Metadata) -> bool {
    false
}
