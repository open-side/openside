//! The operator surface of the web app: daemon lifecycle, diagnostics, the embedding model
//! download, skills folders of other agents and policy editing aids. Everything here has a
//! command-line twin; the web app uses these routes so nothing needs a terminal.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ApiError, PolicySettings};

/// How the daemon restarts itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RestartMethod {
    /// The daemon shuts down gracefully and executes itself in place with the same PID and
    /// arguments, including when supervised by systemd or launchd.
    ReExec,
}

/// `GET /v1/daemon`: identity of the running daemon process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DaemonStatus {
    /// Daemon version.
    pub version: String,
    /// Random id of this process; a different value after a restart.
    pub boot_id: String,
    /// When this process started.
    pub started_at: DateTime<Utc>,
    /// How `POST /v1/daemon/restart` would restart it.
    pub restart_method: RestartMethod,
}

/// `POST /v1/daemon/restart` response: the restart has been requested, not finished.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RestartDaemonResponse {
    /// How the restart is carried out.
    pub method: RestartMethod,
    /// `boot_id` of the process being replaced; the restart is done when `GET /v1/daemon`
    /// answers with another one.
    pub boot_id: String,
}

/// Outcome of one doctor check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DoctorStatus {
    /// Nothing to do.
    Ok,
    /// Works, but something is missing or degraded.
    Warn,
    /// Broken: the feature does not work.
    Fail,
}

/// Where in the web app a doctor finding can be fixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DoctorFixAction {
    /// Download the memory embedding model (Memory page).
    DownloadMemoryModel,
    /// Restart the daemon.
    RestartDaemon,
    /// Open the model and API key settings.
    OpenModels,
    /// Open the vault.
    OpenVault,
    /// Open remote access.
    OpenRemote,
    /// Open the configuration editor.
    OpenConfiguration,
    /// Open the agents list (browser skill for other agents).
    OpenAgents,
    /// Open the network and policy screen.
    OpenPolicy,
}

/// A fix the web app can offer for a finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DoctorFix {
    /// What the button does.
    pub action: DoctorFixAction,
    /// Button label.
    pub label: String,
    /// One sentence on what to expect.
    pub hint: String,
}

/// One diagnostic finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DoctorCheck {
    /// Stable check id such as `memory.embeddings`.
    pub id: String,
    /// Outcome.
    pub status: DoctorStatus,
    /// What was found, without secret values.
    pub summary: String,
    /// What to do about it in words; may mention a terminal command.
    pub next_step: Option<String>,
    /// A fix the web app offers, when one exists.
    pub fix: Option<DoctorFix>,
}

/// Findings of one area.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DoctorGroup {
    /// Area name, in display order across the report.
    pub name: String,
    /// Findings in this area; nothing is hidden, passing families are not folded.
    pub checks: Vec<DoctorCheck>,
}

/// `GET /v1/doctor`: the same checks as `openside doctor`, grouped by area, run by the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DoctorReport {
    /// No check failed (warnings allowed).
    pub ok: bool,
    /// When the checks ran.
    pub generated_at: DateTime<Utc>,
    /// Non-empty areas in display order.
    pub groups: Vec<DoctorGroup>,
}

/// Stage of the embedding model download.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryModelState {
    /// No download has run since the daemon started.
    Idle,
    /// Reading the source's file list and checksums.
    Resolving,
    /// Transferring the model files.
    Downloading,
    /// Comparing checksums and loading the staged model.
    Verifying,
    /// Swapping the verified model in.
    Installing,
    /// The last download installed a model.
    Installed,
    /// The last download failed; `error` says why.
    Failed,
    /// The last download was cancelled.
    Cancelled,
}

/// Where the embedding model comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryModelSource {
    /// Model hub base URL.
    pub endpoint: String,
    /// Repository, `owner/name`.
    pub repo: String,
    /// Branch, tag or commit.
    pub revision: String,
}

/// `GET /v1/memory/model`: the embedding model and the download job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryModelStatus {
    /// Stage of the current or last download.
    pub state: MemoryModelState,
    /// Both model files are present in `model_dir`.
    pub installed: bool,
    /// Where the model lives (`memory.embedding.model_dir`).
    pub model_dir: String,
    /// What a download without arguments fetches.
    pub source: MemoryModelSource,
    /// File being transferred.
    pub file: Option<String>,
    /// Files fully transferred.
    pub files_done: u32,
    /// Files the model needs.
    pub files_total: u32,
    /// Bytes transferred so far.
    pub bytes_done: u64,
    /// Bytes expected in total; zero until the source's file list is known.
    pub bytes_total: u64,
    /// Why the last download failed or was cancelled (`code` such as `checksum_mismatch`).
    pub error: Option<ApiError>,
    /// Id of the model the last download installed.
    pub model_id: Option<String>,
    /// A model was installed after the daemon loaded its memory store; vector search starts
    /// after a restart.
    pub restart_required: bool,
    /// When the last download ended.
    pub finished_at: Option<DateTime<Utc>>,
}

/// `POST /v1/memory/model/download`. Without fields it fetches the default model from the
/// default source; naming another source needs the computer's local access.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartMemoryModelRequest {
    /// Repository, `owner/name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Branch, tag or commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Model hub base URL; https, or http on loopback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

/// `PUT /v1/external-agents/{id}`: where another agent keeps its skills.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetExternalAgentRequest {
    /// Absolute path of the agent's skills folder, or `~/…`; must lie inside the user's home,
    /// config or data folders.
    pub skills_dir: String,
}

/// Where a list of a policy edit is wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PolicyList {
    /// `allow_origins`.
    AllowOrigins,
    /// `deny_origins`.
    DenyOrigins,
    /// `rules`.
    Rules,
    /// `network.allow` (outbound destinations).
    NetworkAllow,
}

/// One validation problem, pointing at the entry to fix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PolicyProblem {
    /// List the entry belongs to.
    pub list: PolicyList,
    /// Zero-based position within the list.
    pub index: u32,
    /// What is wrong, for the person editing.
    pub message: String,
}

/// `POST /v1/policy/validate`: check an edit without saving it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidatePolicyRequest {
    /// Live policy document to check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<PolicySettings>,
    /// Outbound network allow-list to check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_allow: Option<Vec<String>>,
}

/// Result of a [`ValidatePolicyRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PolicyValidation {
    /// No problems.
    pub valid: bool,
    /// Every problem found, in list order.
    pub problems: Vec<PolicyProblem>,
}

/// `GET`/`PUT /v1/policy/network`: where the daemon and its agents may connect, beyond this
/// computer and the configured model and channel endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NetworkPolicy {
    /// Saved `network.allow` list.
    pub allow: Vec<String>,
    /// Whether the saved list differs from the one the running daemon enforces; it applies
    /// after a restart.
    pub restart_required: bool,
}

/// `PUT /v1/policy/network`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateNetworkPolicyRequest {
    /// Replacement `network.allow` list: `host`, `*.domain`, `host:port` or
    /// `scheme://host[:port]`.
    pub allow: Vec<String>,
}
