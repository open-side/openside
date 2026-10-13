use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Subagent state as projected from the parent session's event log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    /// The child is running a model turn or tool.
    Running,
    /// The child waits on a ticket listed under the parent session.
    WaitingHuman,
    /// The child finished; its output was returned to the parent's `delegate` call.
    Completed,
    /// The child failed or was interrupted; the parent received a failed result.
    Failed,
    /// The child was cancelled; the parent received a cancelled result.
    Cancelled,
}

impl SubagentStatus {
    /// Completed, failed and cancelled children never change state again.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// One child agent started by the parent session's `delegate` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Subagent {
    /// Daemon-assigned subagent identifier, also used as `ticket.origin.subagent_id`.
    pub id: String,
    /// The parent's `delegate` tool call, which stays pending until the child ends.
    pub parent_tool_call_id: String,
    /// Delegated task, truncated for display.
    pub task: String,
    /// Model selector the child runs on.
    pub model: String,
    /// Tools the child may call; a subset of the parent's tools.
    pub tools: Vec<String>,
    /// Current state.
    pub status: SubagentStatus,
    /// When the daemon admitted the child.
    pub started_at: DateTime<Utc>,
    /// When the child reached a terminal state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    /// Sanitized detail for failed or cancelled children.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Latest `subagent.progress` payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_progress: Option<Value>,
    /// A background Task started by the front agent (`create_task`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub background: bool,
    /// Final answer of a completed child, truncated for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

/// Subagent listing response, in start order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubagentList {
    /// Every subagent the session started.
    pub subagents: Vec<Subagent>,
}

/// Steering text delivered at the child's next model-request boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SteerSubagentRequest {
    /// Steering text.
    pub text: String,
}
