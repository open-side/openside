use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

use crate::{
    error::{Error, Result},
    frontmatter::{self, Frontmatter, MAX_SKILL_MD_BYTES},
};

/// Largest resource file a skill can hand to the model; the tool output limit is 32 KiB.
pub const MAX_RESOURCE_BYTES: usize = 24 * 1024;
const MAX_LISTED_RESOURCES: usize = 200;
const MAX_RESOURCE_DEPTH: usize = 4;

/// A validated skill directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// Validated frontmatter; `frontmatter.name` equals the directory name.
    pub frontmatter: Frontmatter,
    /// Markdown instructions after the frontmatter.
    pub body: String,
    /// Canonical directory holding `SKILL.md` and the resources.
    pub dir: PathBuf,
}

/// A bundled file next to `SKILL.md` (references, scripts, assets).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceFile {
    /// Path relative to the skill directory, `/`-separated.
    pub path: String,
    /// Size in bytes.
    pub size: u64,
}

impl Skill {
    /// Skill name.
    pub fn name(&self) -> &str {
        &self.frontmatter.name
    }

    /// Load `<dir>/SKILL.md`. `dir` may itself be a symlink to a directory; when
    /// `confine` is set the resolved directory must stay below it.
    pub fn load(dir: &Path, confine: Option<&Path>) -> Result<Self> {
        let canonical = fs::canonicalize(dir).map_err(|e| Error::io_path("resolving", dir, e))?;
        if let Some(root) = confine {
            let root = fs::canonicalize(root).map_err(|e| Error::io_path("resolving", root, e))?;
            if !canonical.starts_with(&root) {
                return Err(Error::Unsafe(format!(
                    "skill directory {} resolves outside {}",
                    dir.display(),
                    root.display()
                )));
            }
        }
        let text = read_text_inside(
            &canonical,
            &canonical.join("SKILL.md"),
            MAX_SKILL_MD_BYTES,
            "SKILL.md",
        )?;
        let document = frontmatter::parse(&text)?;
        let dir_name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if dir_name != document.frontmatter.name {
            return Err(Error::invalid(
                "name",
                format!(
                    "`{}` must match the directory name `{dir_name}`",
                    document.frontmatter.name
                ),
            ));
        }
        Ok(Self {
            frontmatter: document.frontmatter,
            body: document.body,
            dir: canonical,
        })
    }

    /// Files bundled with the skill (excluding `SKILL.md` and dot-files), at most 200.
    pub fn resources(&self) -> Vec<ResourceFile> {
        let mut out = Vec::new();
        let mut stack = vec![(self.dir.clone(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            let mut entries: Vec<_> = entries.flatten().collect();
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                let name = entry.file_name().to_string_lossy().into_owned();
                let path = entry.path();
                if name.starts_with('.') || (depth == 0 && name == "SKILL.md") {
                    continue;
                }
                // Resolve symlinks, but only list what stays inside the skill.
                let Ok(real) = fs::canonicalize(&path) else {
                    continue;
                };
                if !real.starts_with(&self.dir) {
                    continue;
                }
                let Ok(metadata) = fs::metadata(&real) else {
                    continue;
                };
                if metadata.is_dir() {
                    if depth + 1 < MAX_RESOURCE_DEPTH
                        && !entry.file_type().is_ok_and(|t| t.is_symlink())
                    {
                        stack.push((path, depth + 1));
                    }
                } else if metadata.is_file() {
                    if let Ok(relative) = path.strip_prefix(&self.dir) {
                        out.push(ResourceFile {
                            path: relative
                                .components()
                                .map(|c| c.as_os_str().to_string_lossy())
                                .collect::<Vec<_>>()
                                .join("/"),
                            size: metadata.len(),
                        });
                    }
                }
                if out.len() >= MAX_LISTED_RESOURCES {
                    out.sort_by(|a, b| a.path.cmp(&b.path));
                    return out;
                }
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    /// Read one bundled text file. Absolute paths, `..`, dot-components and symlinks
    /// leaving the skill directory are refused. Nothing is ever executed.
    pub fn read_resource(&self, relative: &str) -> Result<String> {
        let path = self.dir.join(checked_resource_path(relative)?);
        let real = fs::canonicalize(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::NotFound(format!("{}/{relative}", self.name()))
            } else {
                Error::io_path("resolving", &path, e)
            }
        })?;
        read_text_inside(&self.dir, &real, MAX_RESOURCE_BYTES, "resource")
    }
}

/// A relative, `/`-or-platform separated path of plain, non-hidden components.
pub(crate) fn checked_resource_path(relative: &str) -> Result<PathBuf> {
    let rel = Path::new(relative);
    let clean = !relative.is_empty()
        && relative.chars().count() <= 200
        && rel.components().all(|c| match c {
            Component::Normal(part) => !part.to_string_lossy().starts_with('.'),
            _ => false,
        });
    if clean {
        Ok(rel.to_owned())
    } else {
        Err(Error::invalid(
            "resource",
            "must be a relative path inside the skill without `..` or hidden parts",
        ))
    }
}

/// Read a regular text file after proving its canonical location is below `root`.
fn read_text_inside(root: &Path, path: &Path, max: usize, what: &'static str) -> Result<String> {
    let real = fs::canonicalize(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::NotFound(path.display().to_string())
        } else {
            Error::io_path("resolving", path, e)
        }
    })?;
    if !real.starts_with(root) {
        return Err(Error::Unsafe(format!(
            "{what} {} escapes its skill directory",
            path.display()
        )));
    }
    let file = fs::File::open(&real).map_err(|e| Error::io_path("opening", &real, e))?;
    let metadata = file
        .metadata()
        .map_err(|e| Error::io_path("inspecting", &real, e))?;
    if !metadata.is_file() {
        return Err(Error::invalid(what, "is not a regular file"));
    }
    if metadata.len() > max as u64 {
        return Err(Error::TooLarge { what, max });
    }
    let mut bytes = Vec::new();
    file.take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::io_path("reading", &real, e))?;
    if bytes.len() > max {
        return Err(Error::TooLarge { what, max });
    }
    String::from_utf8(bytes).map_err(|_| Error::invalid(what, "is not valid UTF-8 text"))
}
