//! Agent Skills for openside: a directory with `SKILL.md` (YAML-subset frontmatter plus
//! Markdown instructions) and optional resources.
//!
//! - [`frontmatter`] parses and renders `SKILL.md` with strict validation and size limits.
//! - [`Store`] is the user library (list, create, edit, toggle, delete) and merges a
//!   workspace's read-only project skills.
//! - [`install`] copies skills between directories: symlinks may not leave the source,
//!   executables are refused unless asked for, and the result appears atomically.
//! - [`agent_skills_dir`] and [`bundled`] support installing openside's own browser skill
//!   into other agents.
//!
//! Nothing in this crate executes skill content or reads outside a skill directory.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod agents;
mod bundled;
mod error;
pub mod frontmatter;
pub mod install;
mod skill;
mod store;

pub use agents::{agent_skills_dir, BUILTIN_AGENTS};
pub use bundled::{bundled, validate_bundled, BROWSER_SKILL};
pub use error::{Error, Result};
pub use frontmatter::{
    parse, render, validate_name, Document, Frontmatter, Value, MAX_DESCRIPTION_CHARS,
    MAX_FRONTMATTER_BYTES, MAX_NAME_CHARS, MAX_SKILL_MD_BYTES,
};
pub use install::{InstallOptions, Installed, StagedFile};
pub use skill::{ResourceFile, Skill, MAX_RESOURCE_BYTES};
pub use store::{
    project_dir, Entry, NewResource, NewSkill, Problem, Scan, SkillUpdate, Source, Store,
    PROJECT_SKILLS_DIR,
};
