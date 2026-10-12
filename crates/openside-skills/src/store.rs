use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    error::{Error, Result},
    frontmatter::{self, Document, Frontmatter},
    install::{self, InstallOptions, Installed, StagedFile},
    skill::{checked_resource_path, Skill, MAX_RESOURCE_BYTES},
};

const MAX_SKILLS_PER_ROOT: usize = 512;
const MAX_NEW_RESOURCES: usize = 64;
const MAX_NEW_RESOURCES_BYTES: usize = 512 * 1024;
const STATE_FILE: &str = ".state.json";
/// Project skills live here, below a session's workspace.
pub const PROJECT_SKILLS_DIR: &str = ".openside/skills";

/// Where a skill comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The user's own library; editable.
    User,
    /// A workspace's `.openside/skills`; read-only because the repository is untrusted.
    Project,
}

/// A skill together with where it lives and whether it is switched on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The validated skill.
    pub skill: Skill,
    /// Origin.
    pub source: Source,
    /// `false` for skills the user switched off.
    pub enabled: bool,
}

/// A skill directory that could not be loaded; reported instead of failing the listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// Directory name.
    pub name: String,
    /// Origin.
    pub source: Source,
    /// Why it was rejected.
    pub error: String,
}

/// Result of scanning the skill directories.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scan {
    /// Valid skills, sorted by name; a user skill shadows a project skill of that name.
    pub entries: Vec<Entry>,
    /// Rejected directories.
    pub problems: Vec<Problem>,
}

/// New user skill.
#[derive(Debug, Clone)]
pub struct NewSkill {
    /// Skill name (also the directory name).
    pub name: String,
    /// Description.
    pub description: String,
    /// Markdown instructions.
    pub body: String,
    /// Optional license text.
    pub license: Option<String>,
    /// Text files stored next to `SKILL.md`; never executable.
    pub resources: Vec<NewResource>,
}

/// A text resource of a new skill.
#[derive(Debug, Clone)]
pub struct NewResource {
    /// Relative path inside the skill.
    pub path: String,
    /// UTF-8 content.
    pub content: String,
}

/// Partial update; `None` keeps the current value.
#[derive(Debug, Clone, Default)]
pub struct SkillUpdate {
    /// New description.
    pub description: Option<String>,
    /// New body.
    pub body: Option<String>,
    /// Switch on or off.
    pub enabled: Option<bool>,
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    disabled: BTreeSet<String>,
}

/// The user skill library: one directory per skill below `root`, plus a small state
/// file recording switched-off skills. All writes are atomic renames, so a daemon and
/// the CLI can use the same directory at once.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

/// Directory a workspace's project skills live in.
pub fn project_dir(workspace: &Path) -> PathBuf {
    workspace.join(PROJECT_SKILLS_DIR)
}

impl Store {
    /// A store rooted at `root`; the directory need not exist yet.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Library directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn disabled(&self) -> BTreeSet<String> {
        // A damaged state file must not hide the library; every skill reads as enabled.
        fs::read(self.root.join(STATE_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<State>(&bytes).ok())
            .map(|state| state.disabled)
            .unwrap_or_default()
    }

    fn save_disabled(&self, disabled: BTreeSet<String>) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(&State { disabled })
            .map_err(|e| Error::io("encoding skill state", std::io::Error::other(e)))?;
        write_atomic(&self.root, &self.root.join(STATE_FILE), &bytes)
    }

    /// List the user library and, when given, a project skills directory.
    pub fn scan(&self, project: Option<&Path>) -> Scan {
        let disabled = self.disabled();
        let mut scan = Scan::default();
        scan_root(&self.root, Source::User, None, &disabled, &mut scan);
        if let Some(project) = project.filter(|dir| plain_project_dir(dir, &mut scan)) {
            let mut project_scan = Scan::default();
            scan_root(
                project,
                Source::Project,
                Some(project),
                &disabled,
                &mut project_scan,
            );
            let user: BTreeSet<&str> = scan.entries.iter().map(|e| e.skill.name()).collect();
            project_scan
                .entries
                .retain(|e| !user.contains(e.skill.name()));
            scan.entries.extend(project_scan.entries);
            scan.problems.extend(project_scan.problems);
        }
        scan.entries
            .sort_by(|a, b| a.skill.name().cmp(b.skill.name()));
        scan
    }

    /// Look one skill up by name.
    pub fn get(&self, name: &str, project: Option<&Path>) -> Result<Entry> {
        frontmatter::validate_name(name)?;
        self.scan(project)
            .entries
            .into_iter()
            .find(|entry| entry.skill.name() == name)
            .ok_or_else(|| Error::NotFound(name.to_owned()))
    }

    /// Create a user skill.
    pub fn create(&self, new: NewSkill) -> Result<Entry> {
        let document = Document {
            frontmatter: Frontmatter {
                name: new.name.clone(),
                description: new.description,
                license: new.license,
                compatibility: None,
                allowed_tools: Vec::new(),
                metadata: Default::default(),
                extra: Default::default(),
            },
            body: new.body,
        };
        let text = frontmatter::render(&document)?;
        let mut files = vec![StagedFile {
            path: PathBuf::from("SKILL.md"),
            content: text.into_bytes(),
            executable: false,
        }];
        if new.resources.len() > MAX_NEW_RESOURCES {
            return Err(Error::invalid("resources", "too many files"));
        }
        let mut total = 0usize;
        for resource in new.resources {
            let path = checked_resource_path(&resource.path)?;
            if path == Path::new("SKILL.md") || files.iter().any(|f| f.path == path) {
                return Err(Error::invalid("resources", "duplicate or reserved path"));
            }
            if resource.content.len() > MAX_RESOURCE_BYTES {
                return Err(Error::TooLarge {
                    what: "resource",
                    max: MAX_RESOURCE_BYTES,
                });
            }
            total += resource.content.len();
            if total > MAX_NEW_RESOURCES_BYTES {
                return Err(Error::TooLarge {
                    what: "resources",
                    max: MAX_NEW_RESOURCES_BYTES,
                });
            }
            files.push(StagedFile {
                path,
                content: resource.content.into_bytes(),
                executable: false,
            });
        }
        install::write_skill(&self.root, &new.name, files, false)?;
        self.get(&new.name, None)
    }

    /// Edit or toggle a user skill. A project skill's files are read-only, but it can be
    /// switched off: that only writes the user's own state file, which is the user's way to
    /// refuse an untrusted repository's skill.
    pub fn update(&self, name: &str, update: SkillUpdate, project: Option<&Path>) -> Result<Entry> {
        let entry = self.get(name, project)?;
        let edits_files = update.description.is_some() || update.body.is_some();
        if entry.source != Source::User && edits_files {
            return Err(Error::ReadOnly(name.to_owned()));
        }
        if update.description.is_some() || update.body.is_some() {
            let skill_md = entry.skill.dir.join("SKILL.md");
            let current = fs::read_to_string(&skill_md)
                .map_err(|e| Error::io_path("reading", &skill_md, e))?;
            let text = match &update.description {
                Some(description) => {
                    let mut document = frontmatter::parse(&current)?;
                    document.frontmatter.description.clone_from(description);
                    if let Some(body) = &update.body {
                        document.body.clone_from(body);
                    }
                    frontmatter::render(&document)?
                }
                None => {
                    frontmatter::replace_body(&current, update.body.as_deref().unwrap_or_default())?
                }
            };
            write_atomic(&entry.skill.dir, &skill_md, text.as_bytes())?;
        }
        if let Some(enabled) = update.enabled {
            self.set_enabled(name, enabled)?;
        }
        self.get(name, project)
    }

    /// Switch a skill on or off without touching its files.
    pub fn set_enabled(&self, name: &str, enabled: bool) -> Result<()> {
        frontmatter::validate_name(name)?;
        let mut disabled = self.disabled();
        let changed = if enabled {
            disabled.remove(name)
        } else {
            disabled.insert(name.to_owned())
        };
        if changed {
            fs::create_dir_all(&self.root)
                .map_err(|e| Error::io_path("creating", &self.root, e))?;
            self.save_disabled(disabled)?;
        }
        Ok(())
    }

    /// Delete a user skill (a symlinked skill directory is unlinked, not followed).
    pub fn remove(&self, name: &str) -> Result<()> {
        frontmatter::validate_name(name)?;
        let path = self.root.join(name);
        if fs::symlink_metadata(&path).is_err() {
            return Err(Error::NotFound(name.to_owned()));
        }
        install::remove_entry(&path)?;
        let mut disabled = self.disabled();
        if disabled.remove(name) {
            self.save_disabled(disabled)?;
        }
        Ok(())
    }

    /// Copy a skill directory into the library.
    pub fn import(&self, src: &Path, options: InstallOptions) -> Result<Installed> {
        install::install_dir(src, &self.root, options)
    }
}

/// A repository controls `.openside` and `.openside/skills`, so neither may be a symlink:
/// confining skills to a directory that itself points outside the workspace would confine
/// nothing. Returns `false` (recording why) when the directory must not be read.
fn plain_project_dir(project: &Path, scan: &mut Scan) -> bool {
    for dir in project.parent().into_iter().chain([project]) {
        match fs::symlink_metadata(dir) {
            Ok(meta) if meta.file_type().is_symlink() => {
                scan.problems.push(Problem {
                    name: dir
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                    source: Source::Project,
                    error: "is a symbolic link; project skills are ignored".into(),
                });
                return false;
            }
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    true
}

fn scan_root(
    root: &Path,
    source: Source,
    confine: Option<&Path>,
    disabled: &BTreeSet<String>,
    scan: &mut Scan,
) {
    let Ok(read) = fs::read_dir(root) else { return };
    let mut names: Vec<String> = read
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort();
    names.truncate(MAX_SKILLS_PER_ROOT);
    for name in names {
        let path = root.join(&name);
        // A skill is a directory (or a symlink to one) with a SKILL.md; anything else is ignored.
        if !fs::metadata(&path).is_ok_and(|m| m.is_dir())
            || fs::symlink_metadata(path.join("SKILL.md")).is_err()
        {
            continue;
        }
        match Skill::load(&path, confine) {
            Ok(skill) => scan.entries.push(Entry {
                enabled: !disabled.contains(skill.name()),
                skill,
                source,
            }),
            Err(error) => scan.problems.push(Problem {
                name,
                source,
                error: error.to_string(),
            }),
        }
    }
}

/// Write `bytes` to `path` through a temporary file in `dir` plus rename.
fn write_atomic(dir: &Path, path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = dir.join(install::unique("write"));
    let result = fs::File::create(&temporary)
        .and_then(|mut file| file.write_all(bytes).and_then(|()| file.sync_all()))
        .and_then(|()| fs::rename(&temporary, path));
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(Error::io_path("writing", path, error));
    }
    Ok(())
}
