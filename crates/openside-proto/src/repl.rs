use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::TicketId;

/// Run one Playwright-syntax snippet in a session-owned agent tab (`browser_repl`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplRequest {
    /// JavaScript using `page`, `expect`, and `console`; top-level `await` is allowed and the
    /// last expression statement is the return value. Each request gets a fresh context.
    pub code: String,
    /// Agent tab to drive; optional when the session owns exactly one tab or none (a blank
    /// tab is opened).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,
    /// Approved `confirm` ticket from an earlier `deferred` response; the identical `code` and
    /// `tab_id` are rerun from the start under that approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_ticket: Option<TicketId>,
}

/// Outcome category of a [`ReplRequest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReplStatus {
    /// The snippet ran; `result` holds `return_value`, `console`, optional `error`, `tab_id`.
    Done,
    /// Human approval is pending in `ticket_id`; rerun with `approval_ticket` once approved.
    Deferred,
    /// Policy or validation refused the call; see `reason` and `hint`.
    Denied,
    /// The daemon could not run the snippet.
    Failed,
}

/// Result of a [`ReplRequest`], recorded as a `browser_repl` tool call in the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReplResponse {
    /// Tool call identifier recorded in the session's `tool.started`/`tool.ended` events.
    pub tool_call_id: String,
    /// Outcome category.
    pub status: ReplStatus,
    /// Redacted, size-limited tool result when `status` is `done`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Approval ticket when `status` is `deferred`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_id: Option<TicketId>,
    /// Denial reason or failure message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// How to proceed after a denial.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}
