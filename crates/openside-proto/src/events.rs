use chrono::{DateTime, Utc};
use openside_core::{DropReason, MessageId, QueueMode, SessionId, TicketId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{BrowserSpace, PolicyMode, SubagentStatus, Ticket};

/// Terminal runtime disposition, distinct from dispatch to the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageOutcome {
    /// The run completed successfully.
    Completed,
    /// The user or runtime cancelled the message.
    Aborted,
    /// Runtime failure or an interrupted, ambiguous dispatch.
    Failed,
    /// The run paused on tickets; resolution resumes without a new message.
    Deferred,
}

/// Immutable durable record, with a gapless per-session sequence starting at one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Event {
    /// Session whose event log owns the record.
    pub session_id: SessionId,
    /// Assigned transactionally by the store, never by clients.
    #[schemars(range(min = 1))]
    pub seq: u64,
    /// Recording timestamp in RFC 3339 UTC.
    pub ts: DateTime<Utc>,
    /// Runtime turn/run correlation, if applicable.
    pub run_id: Option<String>,
    /// Internally tagged payload, flattened alongside the envelope fields.
    #[serde(flatten)]
    pub kind: EventKind,
}

/// Public event payloads with stable `kind` identifiers, dotted except for `usage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum EventKind {
    /// Message accepted durably into the queue.
    #[serde(rename = "message.queued")]
    MessageQueued {
        /// Durable idempotency key.
        message_id: MessageId,
        /// Original text.
        text: String,
        /// Queue lane; `now` is normalized to `steer`.
        mode: QueueMode,
        /// Stored attachments, referenced by artifact path; bytes never enter the log.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<crate::Attachment>,
    },
    /// Pending message text replaced.
    #[serde(rename = "message.edited")]
    MessageEdited {
        /// Pending message identifier.
        message_id: MessageId,
        /// Replacement text.
        text: String,
    },
    /// Pending follow-up promoted to the steering lane.
    #[serde(rename = "message.promoted")]
    MessagePromoted {
        /// Pending message identifier.
        message_id: MessageId,
    },
    /// Pending message moved to another position inside its lane.
    #[serde(rename = "message.moved")]
    MessageMoved {
        /// Pending message identifier.
        message_id: MessageId,
        /// Zero-based index within the message's own lane, clamped to the lane length.
        position: u32,
    },
    /// Queue head reserved for at-most-once runtime dispatch.
    #[serde(rename = "message.injected")]
    MessageInjected {
        /// Message durably marked dispatched before runtime I/O.
        message_id: MessageId,
    },
    /// Terminal result for a previously injected message.
    #[serde(rename = "message.completed")]
    MessageCompleted {
        /// Previously injected message identifier.
        message_id: MessageId,
        /// Successful, aborted, failed, or deferred run completion.
        outcome: MessageOutcome,
        /// Sanitized failure or cancellation detail, when available.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Pending message explicitly dropped with a terminal reason.
    #[serde(rename = "message.dropped")]
    MessageDropped {
        /// Pending message identifier.
        message_id: MessageId,
        /// Why delivery will not occur.
        reason: DropReason,
    },
    /// Agent turn began.
    #[serde(rename = "turn.started")]
    TurnStarted {},
    /// Agent turn ended.
    #[serde(rename = "turn.ended")]
    TurnEnded {},
    /// Incremental assistant output.
    #[serde(rename = "assistant.delta")]
    AssistantDelta {
        /// Text fragment, appended in sequence order.
        text: String,
    },
    /// Complete assistant response.
    #[serde(rename = "assistant.final")]
    AssistantFinal {
        /// Full final text.
        text: String,
    },
    /// Incremental model thinking output.
    #[serde(rename = "thinking.delta")]
    ThinkingDelta {
        /// Text fragment, appended in sequence order.
        text: String,
    },
    /// Token usage for a model request.
    #[serde(rename = "usage")]
    Usage {
        /// Provider-qualified model selector.
        model: String,
        /// Input token count.
        input_tokens: u64,
        /// Output token count.
        output_tokens: u64,
    },
    /// Tool call began.
    #[serde(rename = "tool.started")]
    ToolStarted {
        /// Runtime call identifier.
        tool_call_id: String,
        /// Registered tool name.
        name: String,
        /// Redacted invocation arguments.
        args: Value,
    },
    /// Tool produced intermediate progress.
    #[serde(rename = "tool.updated")]
    ToolUpdated {
        /// Runtime call identifier.
        tool_call_id: String,
        /// Redacted progress payload.
        update: Value,
    },
    /// Tool completed or failed.
    #[serde(rename = "tool.ended")]
    ToolEnded {
        /// Runtime call identifier.
        tool_call_id: String,
        /// Redacted final result.
        result: Value,
        /// Whether the invocation failed.
        is_error: bool,
    },
    /// A tool created or modified a file now kept in the session's artifact directory.
    #[serde(rename = "artifact.saved")]
    ArtifactSaved {
        /// Path relative to the artifact directory.
        path: String,
        /// Byte size of the stored copy.
        size: u64,
        /// MIME type inferred from the extension and content.
        mime: String,
        /// Tool call that wrote it.
        tool_call_id: String,
        /// Tool that wrote it.
        tool: String,
    },
    /// A durable pending human-intervention ticket was created.
    #[serde(rename = "ticket.opened")]
    TicketOpened {
        /// Full redacted ticket needed to rebuild pending state.
        ticket: Box<Ticket>,
    },
    /// Pending ticket resolved.
    #[serde(rename = "ticket.resolved")]
    TicketResolved {
        /// Ticket identifier.
        ticket_id: TicketId,
        /// Redacted resolution; never a secret-input value.
        resolution: Value,
    },
    /// Pending ticket rejected.
    #[serde(rename = "ticket.rejected")]
    TicketRejected {
        /// Ticket identifier.
        ticket_id: TicketId,
        /// Sanitized rejection detail.
        reason: String,
    },
    /// Pending ticket expired.
    #[serde(rename = "ticket.expired")]
    TicketExpired {
        /// Ticket identifier.
        ticket_id: TicketId,
    },
    /// The parent's `delegate` call was admitted and a child agent started.
    #[serde(rename = "subagent.started")]
    SubagentStarted {
        /// Daemon-assigned child identifier.
        subagent_id: String,
        /// The parent's pending `delegate` tool call.
        parent_tool_call_id: String,
        /// Delegated task, truncated for display.
        task: String,
        /// Model selector the child runs on.
        model: String,
        /// Tools the child may call.
        tools: Vec<String>,
        /// A Task started by the front agent's `task_start`: it runs in the background and
        /// reports its result into the conversation instead of blocking the parent call.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        background: bool,
    },
    /// A child agent reached a terminal state; its result went to the parent.
    #[serde(rename = "subagent.ended")]
    SubagentEnded {
        /// Child agent identifier.
        subagent_id: String,
        /// Terminal state.
        status: SubagentStatus,
        /// Sanitized detail for failed or cancelled children.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// Redacted final answer of a completed child, truncated for display.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    /// Subagent lifecycle transition.
    #[serde(rename = "subagent.lifecycle")]
    SubagentLifecycle {
        /// Child agent identifier.
        subagent_id: String,
        /// Non-terminal state: `running` or `waiting_human`.
        status: String,
    },
    /// Subagent progress update.
    #[serde(rename = "subagent.progress")]
    SubagentProgress {
        /// Child agent identifier.
        subagent_id: String,
        /// Redacted progress payload.
        progress: Value,
    },
    /// Agent-owned browser tab created.
    #[serde(rename = "tab.opened")]
    TabOpened {
        /// Browser target identifier.
        tab_id: String,
        /// Initial URL with sensitive components redacted.
        url: String,
    },
    /// Browser tab closed.
    #[serde(rename = "tab.closed")]
    TabClosed {
        /// Browser target identifier.
        tab_id: String,
    },
    /// Browser tab discarded or unloaded.
    #[serde(rename = "tab.asleep")]
    TabAsleep {
        /// Browser target identifier.
        tab_id: String,
    },
    /// An agent-client identity drove the session and cannot approve its later confirmations.
    #[serde(rename = "session.driver_recorded")]
    SessionDriverRecorded {
        /// Paired device id, or `local` for the Unix socket.
        device_id: String,
    },
    /// First event in a session log.
    #[serde(rename = "session.created")]
    SessionCreated {
        /// Initial conversation title.
        title: String,
        /// Provider-qualified model.
        model: String,
        /// Initial permission ceiling; historical events default to Guard.
        #[serde(default)]
        mode: PolicyMode,
        /// Canonical absolute workspace path; historical events have no workspace.
        #[serde(default)]
        workspace: Option<String>,
        /// Private session; historical events are not private.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        incognito: bool,
        /// Browser space; historical events are shared.
        #[serde(default, skip_serializing_if = "BrowserSpace::is_shared")]
        browser_space: BrowserSpace,
    },
    /// Runtime has no active turn.
    #[serde(rename = "session.settled")]
    SessionSettled {},
    /// User explicitly stopped the session; pending messages are retained.
    #[serde(rename = "session.stopped")]
    SessionStopped {},
    /// Session removed by an administrator; the log remains as a tombstone and nothing
    /// can be appended after it.
    #[serde(rename = "session.deleted")]
    SessionDeleted {},
    /// User-visible list metadata changed; omitted fields are unchanged.
    #[serde(rename = "session.updated")]
    SessionUpdated {
        /// Replacement title.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// New pinned flag.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pinned: Option<bool>,
        /// New archived flag.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        archived: Option<bool>,
    },
    /// Sidecar ready handshake completed with an exact tool-gate match.
    #[serde(rename = "runtime.started")]
    RuntimeStarted {
        /// Actually exposed tool names.
        tools: Vec<String>,
        /// Measured startup latency in milliseconds.
        startup_ms: u64,
    },
    /// Sanitized runtime failure; credentials and secrets must not appear here.
    #[serde(rename = "runtime.error")]
    RuntimeError {
        /// Machine-readable error code.
        code: String,
        /// Redacted failure detail.
        message: String,
    },
    /// What an agent is doing in a browser tab, derived by the daemon from the tool call and
    /// the page itself (labels come from the page, never from the model).
    #[serde(rename = "browser.state")]
    BrowserState(Box<BrowserState>),
}

/// Typed browser situation for one tab, rendered as a live card per Task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BrowserState {
    /// Task (background subagent) acting in the tab; absent for the front agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// Browser target identifier.
    pub tab_id: String,
    /// Current URL with sensitive components redacted.
    pub url: String,
    /// Host of `url`, empty for non-network pages.
    pub host: String,
    /// Page title.
    pub title: String,
    /// What the agent is doing in the tab.
    pub phase: BrowserPhase,
    /// The tool call behind this state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<BrowserActionState>,
    /// Classification of the page.
    pub page_kind: PageKind,
    /// Facts read from the page.
    pub facts: PageFacts,
    /// Opaque JPEG thumbnail ID, fetched through the authenticated session route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
}

/// Agent activity in a tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserPhase {
    /// Navigating or waiting for the page to load.
    Loading,
    /// Reading the page (snapshot, screenshot).
    Reading,
    /// Clicking, typing or otherwise interacting.
    Acting,
    /// Waiting for a person (login, code, choice, approval).
    WaitingHuman,
    /// The Task finished with this tab.
    Done,
    /// The last browser action failed.
    Error,
}

/// The tool call that produced a browser state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BrowserActionState {
    /// Tool name.
    pub tool: String,
    /// Runtime call identifier.
    pub tool_call_id: String,
    /// Human phrase such as "Clicking" or "Opening example.com".
    pub phrase: String,
    /// Accessible label of the target element, read from the page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

/// Page classification for card layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PageKind {
    /// Sign-in form (password field).
    Login,
    /// One-time code entry.
    Otp,
    /// A set of options to pick from.
    Choice,
    /// Human verification challenge.
    Captcha,
    /// Other form.
    Form,
    /// Search results or another list of items.
    List,
    /// Reading content.
    Article,
    /// Payment or order confirmation.
    Checkout,
    /// Error page.
    Error,
    /// Nothing more specific.
    Other,
}

/// Facts extracted from the page, bounded for display.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PageFacts {
    /// Main heading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading: Option<String>,
    /// Form fields by accessible label (at most 8).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<PageField>,
    /// Number of items in the main list or results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_count: Option<u32>,
    /// Error text shown by the page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_text: Option<String>,
}

/// One form field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PageField {
    /// Accessible label.
    pub label: String,
    /// Input kind: `text`, `email`, `password`, `code`, `checkbox`, `radio`, `select`, `button`, ….
    pub kind: String,
}
