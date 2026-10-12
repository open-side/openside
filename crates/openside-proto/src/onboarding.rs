use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Where the first-run guide stands. `pending` until a person finishes or skips it once;
/// the flag lives on the daemon, so every device and browser agrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OnboardingState {
    /// Never finished or skipped.
    Pending,
    /// Walked through to the end.
    Completed,
    /// Dismissed before the end.
    Skipped,
}

/// `GET /v1/onboarding` and the `PUT` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OnboardingStatus {
    /// Current state.
    pub state: OnboardingState,
    /// When the state last changed; `null` while pending.
    pub updated_at: Option<DateTime<Utc>>,
}

/// `PUT /v1/onboarding`: record how the guide ended. Only `completed` and `skipped` are
/// accepted; reopening the guide from Settings does not reset the flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateOnboardingRequest {
    /// `completed` or `skipped`.
    pub state: OnboardingState,
}

/// An agent CLI openside can install its browser skill for (`openside skills install`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExternalAgent {
    /// Preset name (`claude`, `codex`, `omp`) or an entry of `[skills.agents]`.
    pub id: String,
    /// Product name for display.
    pub label: String,
    /// The agent's executable is on the daemon's `PATH` or its configuration directory exists.
    pub detected: bool,
    /// `openside-browser` is already in the agent's skills directory.
    pub skill_installed: bool,
    /// Where the skill would be (or is) installed, written `~/…` when under the daemon user's
    /// home; `null` when no directory is known.
    pub skills_dir: Option<String>,
}

/// `GET /v1/external-agents`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExternalAgentList {
    /// The three built-in presets first, then configured agents by name.
    pub agents: Vec<ExternalAgent>,
}

/// `POST /v1/external-agents/{id}/browser-skill` body.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstallAgentSkillRequest {
    /// Replace an `openside-browser` skill that is already installed.
    #[serde(default)]
    pub force: bool,
}

/// What the install wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstalledAgentSkill {
    /// Agent the skill was installed for.
    pub agent: String,
    /// Installed skill name (`openside-browser`).
    pub skill: String,
    /// Final location of the skill directory.
    pub path: String,
    /// Files written.
    pub files: u32,
}

/// Browser family of an import source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportBrowser {
    /// Google Chrome.
    Chrome,
    /// Chromium and derivatives.
    Chromium,
    /// Mozilla Firefox.
    Firefox,
}

/// A data category an import can carry over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportCategory {
    /// Page visits.
    History,
    /// Bookmarks.
    Bookmarks,
    /// Cookies.
    Cookies,
}

/// Which categories a source profile has files for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportAvailability {
    /// A history database exists.
    pub history: bool,
    /// A bookmarks file exists.
    pub bookmarks: bool,
    /// A cookie database exists.
    pub cookies: bool,
}

/// A browser profile the daemon found on its computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportProfile {
    /// Opaque id for `POST /v1/import` (`<browser>:<directory name>`).
    pub id: String,
    /// Browser family.
    pub browser: ImportBrowser,
    /// Name the browser shows for the profile.
    pub name: String,
    /// Files present in the profile.
    pub available: ImportAvailability,
}

/// `GET /v1/import/profiles`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportProfileList {
    /// Profiles of installed browsers, Chrome first, then Chromium, then Firefox.
    pub profiles: Vec<ImportProfile>,
    /// A Chromium-compatible browser is available to write imported cookies; without one
    /// only history and bookmarks can be imported.
    pub cookies_writable: bool,
}

/// `POST /v1/import` body. Always read-then-write: a dry run reads and decrypts everything
/// and writes nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    /// Profile `id` from `GET /v1/import/profiles`.
    pub profile: String,
    /// Categories to import; absent means every category the source has.
    #[serde(default)]
    pub what: Option<Vec<ImportCategory>>,
    /// Report counts without writing anything.
    #[serde(default)]
    pub dry_run: bool,
}

/// A cookie the target browser refused (host and name only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportRejectedCookie {
    /// Cookie host.
    pub host: String,
    /// Cookie name.
    pub name: String,
}

/// Outcome of one category.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportCategoryReport {
    /// Entries read from the source (cookies: decrypted).
    pub read: u64,
    /// Entries written (dry run: that would be written).
    pub imported: u64,
    /// Entries left out: non-web URLs, bookmarklets, expired or partitioned cookies.
    pub skipped: u64,
    /// Cookies the target browser refused.
    pub rejected: Vec<ImportRejectedCookie>,
    /// Why the category was skipped entirely (its source file is missing).
    pub note: Option<String>,
}

/// What an import read, or wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportReport {
    /// Nothing was written.
    pub dry_run: bool,
    /// Source browser.
    pub source: ImportBrowser,
    /// openside browser profile written to.
    pub profile: String,
    /// History outcome when requested.
    pub history: Option<ImportCategoryReport>,
    /// Bookmarks outcome when requested.
    pub bookmarks: Option<ImportCategoryReport>,
    /// Cookies outcome when requested.
    pub cookies: Option<ImportCategoryReport>,
}
