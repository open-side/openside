//! Projects: a named folder with optional instructions that chats can belong to
//! (`docs/specs/projects.md`). A chat in a project works in the project's folder: it is the
//! chat's workspace, so the Guard root and the file tools' working folder.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Longest project name, in characters, after trimming.
pub const MAX_PROJECT_NAME_CHARS: usize = 80;
/// Longest project instructions, in bytes.
pub const MAX_PROJECT_INSTRUCTIONS_BYTES: usize = 16 * 1024;

/// One project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Project {
    /// Stable opaque id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Canonical absolute folder the project's chats work in; fixed at creation.
    pub root: String,
    /// The daemon made the folder (Start from scratch) under `<data_dir>/projects/<id>`.
    #[serde(default)]
    pub scratch: bool,
    /// Instructions every chat in the project receives; empty for none.
    #[serde(default)]
    pub instructions: String,
    /// The folder has no recorded identity, so openside cannot tell whether it is still the
    /// folder the project was made with. A local person must confirm it
    /// (`UpdateProjectRequest::confirm_root`) before chats can use it; until then starting or
    /// moving a chat into the project fails with 409 project_root_changed.
    #[serde(default)]
    pub needs_confirmation: bool,
    /// When the project was created.
    pub created_at: DateTime<Utc>,
    /// When the name or instructions last changed.
    pub updated_at: DateTime<Utc>,
}

/// Every project, by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectList {
    /// Projects sorted by name, then id.
    pub projects: Vec<Project>,
}

/// Create a project from an existing folder, or (without `root`) from scratch in a new empty
/// folder the daemon makes. Only the computer running openside may create projects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateProjectRequest {
    /// Non-blank display name, at most [`MAX_PROJECT_NAME_CHARS`] characters after trimming.
    pub name: String,
    /// Absolute path of an existing folder; it must not be the file system root, the home
    /// folder or a folder above it, inside openside's own data, inside an operating-system
    /// folder or another person's home, or a credential folder (such as `~/.ssh`) or a folder
    /// above one. Omitted: start from scratch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// Instructions for every chat in the project, at most [`MAX_PROJECT_INSTRUCTIONS_BYTES`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

/// Rename a project, change its instructions or confirm its folder; omitted fields keep their
/// value. The folder is fixed at creation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateProjectRequest {
    /// New non-blank display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Replacement instructions; an empty string removes them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Check the project's folder again and record it as the project's folder, clearing
    /// `Project::needs_confirmation`. Only the computer running openside may confirm a folder
    /// (403 local_access_required elsewhere).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub confirm_root: bool,
}

/// The folder a person chose in the native folder picker of the computer running openside.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PickedFolder {
    /// Canonical absolute path; null when the person closed the picker without choosing.
    pub path: Option<String>,
}

/// Where a chat works: in a project's folder, in a folder of its own, or in none. At most one
/// of the two fields is set; both null leaves the project and drops the folder.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetSessionFolderRequest {
    /// Move the chat into this project; its folder becomes the chat's workspace.
    #[serde(default)]
    pub project_id: Option<String>,
    /// Attach this absolute folder to the chat outside any project. Only the computer running
    /// openside may attach a folder (403 local_access_required elsewhere).
    #[serde(default)]
    pub workspace: Option<String>,
}
