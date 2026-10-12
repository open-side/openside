use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Agent Skills settings (`docs/specs/skills.md`).
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct SkillsConfig {
    /// The user's skill library (default `<data_dir>/skills`).
    pub dir: Option<PathBuf>,
    /// Also offer `.openside/skills/` of a session's workspace (read-only).
    pub project: bool,
    /// Skills directories of other agents by name, for `openside skills install|import`.
    /// Built-in conventions exist for `claude`, `codex` and `omp`; any other name needs an entry
    /// here or `OPENSIDE_SKILLS_<NAME>_DIR`.
    pub agents: BTreeMap<String, PathBuf>,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            dir: None,
            project: true,
            agents: BTreeMap::new(),
        }
    }
}

impl SkillsConfig {
    /// Configured library directory, else `<data_dir>/skills`.
    pub fn library_dir(&self, data_dir: &Path) -> PathBuf {
        self.dir
            .clone()
            .unwrap_or_else(|| crate::data_layout::DataPath::Skills.path(data_dir))
    }

    /// Reject empty paths and agent names that cannot be a preset name.
    pub(crate) fn validate(&self) -> Result<(), (String, &'static str)> {
        if self
            .dir
            .as_ref()
            .is_some_and(|dir| dir.as_os_str().is_empty())
        {
            return Err(("skills.dir".into(), "path must not be empty"));
        }
        for (name, path) in &self.agents {
            let valid = !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            if !valid {
                return Err((
                    format!("skills.agents.{name}"),
                    "agent names are lowercase letters, digits and hyphens",
                ));
            }
            if path.as_os_str().is_empty() {
                return Err((format!("skills.agents.{name}"), "path must not be empty"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn parse(text: &str) -> SkillsConfig {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn defaults_use_the_data_dir_and_project_skills() {
        let config = SkillsConfig::default();
        assert!(config.project && config.agents.is_empty());
        assert_eq!(
            config.library_dir(Path::new("data")),
            Path::new("data").join("skills")
        );
        assert!(config.validate().is_ok());
    }

    #[test]
    fn explicit_dir_and_agent_presets_are_read_and_validated() {
        let config = parse("dir = \"lib\"\nproject = false\n[agents]\nomp = \"x/skills\"\n");
        assert_eq!(config.library_dir(Path::new("data")), Path::new("lib"));
        assert!(!config.project);
        assert_eq!(config.agents["omp"], Path::new("x/skills"));
        assert!(config.validate().is_ok());
        for bad in [
            "dir = \"\"",
            "[agents]\nBad = \"x\"",
            "[agents]\nomp = \"\"",
        ] {
            assert!(parse(bad).validate().is_err(), "{bad}");
        }
        assert!(toml::from_str::<SkillsConfig>("unknown = 1").is_err());
    }
}
