use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Where a skill is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    /// The user's library; editable.
    User,
    /// `<workspace>/.openside/skills`; read-only.
    Project,
}

/// Name and description of one skill, as shown in lists and the `/` mention picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillSummary {
    /// Lowercase letters, digits and hyphens; also the directory name and the `/name` mention.
    pub name: String,
    /// What the skill does and when to use it.
    pub description: String,
    /// Where the skill lives.
    pub source: SkillSource,
    /// Disabled skills are neither offered to the model nor loadable by the `skill` tool.
    pub enabled: bool,
}

/// A skill directory that failed validation. The rest of the list is unaffected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillProblem {
    /// Directory name.
    pub name: String,
    /// Where the directory is.
    pub source: SkillSource,
    /// Why it was rejected; never contains file content.
    pub error: String,
}

/// Response of `GET /v1/skills`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillList {
    /// Valid skills sorted by name (or by match quality when `q` is given).
    pub skills: Vec<SkillSummary>,
    /// Rejected directories, so a settings screen can show them without failing.
    pub problems: Vec<SkillProblem>,
}

/// A bundled file next to `SKILL.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillResource {
    /// Path relative to the skill directory.
    pub path: String,
    /// Size in bytes.
    pub size: u64,
}

/// Response of `GET /v1/skills/{name}` and the write endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillDetail {
    /// Skill name.
    pub name: String,
    /// What the skill does and when to use it.
    pub description: String,
    /// Where the skill lives.
    pub source: SkillSource,
    /// Whether the skill is offered to sessions.
    pub enabled: bool,
    /// `license` frontmatter.
    pub license: Option<String>,
    /// `compatibility` frontmatter.
    pub compatibility: Option<String>,
    /// `allowed-tools` frontmatter; informational, never grants anything.
    pub allowed_tools: Vec<String>,
    /// Markdown instructions after the frontmatter.
    pub body: String,
    /// Bundled files, at most 200.
    pub resources: Vec<SkillResource>,
}

/// A text file bundled into a new skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillFile {
    /// Relative path inside the skill: no `..`, no hidden components, not `SKILL.md`.
    pub path: String,
    /// UTF-8 text of at most 24 KiB.
    pub content: String,
}

/// `POST /v1/skills` body; creates a user skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateSkillRequest {
    /// Skill name: 1-64 lowercase letters, digits and single hyphens.
    pub name: String,
    /// Required, at most 1024 characters.
    pub description: String,
    /// Markdown instructions.
    #[serde(default)]
    pub body: String,
    /// Optional license text.
    pub license: Option<String>,
    /// Optional text resources (at most 64 files, 512 KiB in total). Executable
    /// scripts cannot be uploaded; use `openside skills import --allow-scripts`.
    #[serde(default)]
    pub resources: Vec<SkillFile>,
}

/// `PATCH /v1/skills/{name}` body; absent fields are unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateSkillRequest {
    /// Replacement description.
    pub description: Option<String>,
    /// Replacement Markdown instructions.
    pub body: Option<String>,
    /// Switch the skill on or off.
    pub enabled: Option<bool>,
}
