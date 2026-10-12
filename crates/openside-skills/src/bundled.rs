use std::path::PathBuf;

use crate::{error::Result, frontmatter, install::StagedFile};

/// Name of the skill that teaches other agents to delegate browsing to openside.
pub const BROWSER_SKILL: &str = "openside-browser";

const BROWSER_SKILL_MD: &str = include_str!("../bundled/openside-browser/SKILL.md");

/// Files of a bundled skill, or `None` for an unknown name.
pub fn bundled(name: &str) -> Option<Vec<StagedFile>> {
    (name == BROWSER_SKILL).then(|| {
        vec![StagedFile {
            path: PathBuf::from("SKILL.md"),
            content: BROWSER_SKILL_MD.as_bytes().to_vec(),
            executable: false,
        }]
    })
}

/// Parse the bundled `SKILL.md` (used by tests and `doctor`-style self checks).
pub fn validate_bundled() -> Result<()> {
    frontmatter::parse(BROWSER_SKILL_MD).map(|_| ())
}
