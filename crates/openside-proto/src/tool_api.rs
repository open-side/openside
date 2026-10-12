use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CapabilityScope, TicketDeferral, TicketId};

/// One daemon tool an API caller may run directly (`POST /v1/sessions/{id}/tools/{name}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolDescriptor {
    /// Tool name, identical to the name the in-app agent calls.
    pub name: String,
    /// The description the agent's model sees.
    pub description: String,
    /// JSON Schema of `ToolCallRequest.args`.
    pub input_schema: Value,
    /// Capability a token needs to call it.
    pub scope: CapabilityScope,
    /// Whether the call opens a person-facing ticket and its result arrives on resolution.
    pub human: bool,
}

/// `GET /v1/tools` response: every directly callable tool, in name order. A session may
/// still refuse a listed tool (its mode, an incognito session, the configured agent profile).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolCatalog {
    /// Callable tools.
    pub tools: Vec<ToolDescriptor>,
}

/// Run one daemon tool in a session through the same host pipeline as an agent call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolCallRequest {
    /// Tool arguments matching the descriptor's `input_schema`.
    #[serde(default = "empty_object")]
    pub args: Value,
    /// Approved `confirm` ticket from an earlier `deferred` response with deferral
    /// `approval`; the identical `args` run again under that approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_ticket: Option<TicketId>,
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

/// Outcome category of a [`ToolCallRequest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    /// The tool ran; `result` holds its redacted result.
    Done,
    /// A ticket in `ticket_id` is pending. With deferral `approval`, call again with
    /// `approval_ticket` once it is approved; with `external`, the ticket's resolution is
    /// the tool's result.
    Deferred,
    /// Policy or validation refused the call; see `reason` and `hint`.
    Denied,
    /// The daemon could not run the tool.
    Failed,
}

/// Result of a [`ToolCallRequest`], recorded as a tool call in the session's events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolCallResponse {
    /// Tool call identifier recorded in the session's `tool.started`/`tool.ended` events.
    pub tool_call_id: String,
    /// Outcome category.
    pub status: ToolCallStatus,
    /// Redacted tool result when `status` is `done`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Pending ticket when `status` is `deferred`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_id: Option<TicketId>,
    /// How the pending ticket resumes the call when `status` is `deferred`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferral: Option<TicketDeferral>,
    /// Artifact paths the call saved (a screenshot's PNG), readable through
    /// `GET /v1/sessions/{id}/artifacts/{path}`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<String>,
    /// Denial reason or failure message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// How to proceed after a denial.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}
