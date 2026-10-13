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
        /// Caller-derived authority; historical logs fail closed for immediate hiring.
        #[serde(default)]
        author_kind: crate::MessageAuthorKind,
        /// Only daemon team deliveries may identify a team sender.
        #[serde(default)]
        from_agent: Option<String>,
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
        /// Atomic presentation outcome for a resumed approval; ordinary calls omit it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        card: Option<crate::Card>,
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
        /// Same-row presentation committed with the authoritative transition.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        card: Option<crate::Card>,
    },
    /// Pending ticket rejected.
    #[serde(rename = "ticket.rejected")]
    TicketRejected {
        /// Ticket identifier.
        ticket_id: TicketId,
        /// Sanitized rejection detail.
        reason: String,
        /// Value-free cancellation receipt committed with rejection.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        card: Option<crate::Card>,
    },
    /// Pending ticket expired.
    #[serde(rename = "ticket.expired")]
    TicketExpired {
        /// Ticket identifier.
        ticket_id: TicketId,
        /// Inactive receipt committed with expiry.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        card: Option<crate::Card>,
    },
    /// Validated read-only presentation, idempotently keyed by its original tool call.
    #[serde(rename = "card.presented")]
    CardPresented {
        /// Daemon-normalized data with local raster references and no input values.
        card: crate::Card,
        /// Original tool invocation; its result contains only the card identity.
        tool_call_id: String,
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
        /// Immutable owner tag; historical creation events remain unassigned.
        #[serde(default)]
        agent_id: Option<String>,
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
        /// Daemon-approved replacement ceiling for an agent's home conversation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<PolicyMode>,
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
    /// A model-written conversation title; explicit titles and later user renames win.
    #[serde(rename = "session.title_proposed")]
    SessionTitleProposed {
        /// Sanitized conversation title of at most six words, never a URL.
        title: String,
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
        /// Calm user-facing copy; `message` remains technical detail.
        #[serde(default)]
        user_message: String,
        /// Whether resubmitting the last user message is a useful next step.
        #[serde(default)]
        retryable: bool,
    },
    /// What an agent is doing in a browser tab, derived by the daemon from the tool call and
    /// the page itself (labels come from the page, never from the model).
    #[serde(rename = "browser.state")]
    BrowserState(Box<BrowserState>),
    /// A completed sign-in saved value-free connection metadata.
    #[serde(rename = "connection.saved")]
    ConnectionSaved {
        /// Opaque registry ID.
        connection_id: String,
        /// Registrable site, never a callback URL.
        site: String,
        /// Write-time-masked account label.
        identity: Option<String>,
    },
    /// A granted agent use; no request values enter the event.
    #[serde(rename = "connection.used")]
    ConnectionUsed {
        /// Opaque registry ID.
        connection_id: String,
        /// Registrable site.
        site: String,
        /// Page opening or in-page fetch.
        mode: crate::ConnectionUseMode,
        /// HTTP method for fetch, absent for page opening.
        method: Option<String>,
    },
    /// A completed hire, correlated to its human authorization.
    #[serde(rename = "team.hired")]
    TeamHired {
        /// Stable registry identity, independent of later renames.
        agent_id: String,
        /// Display name at the time of hiring.
        name: String,
        /// Approving ticket, absent for an immediate person-requested hire.
        ticket_id: Option<String>,
        /// Authoritative person message permitting immediate hiring.
        request_message_id: Option<MessageId>,
        /// Common card projection, absent in historical records.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        card: Option<crate::Card>,
    },
    /// A durable work packet shared between the sender and recipient homes.
    #[serde(rename = "team.handoff")]
    TeamHandoff {
        /// Stable identity used for report and event deduplication.
        handoff_id: String,
        /// Registry identity of the delegating chief.
        from_agent: String,
        /// Registry identity accountable for the result.
        to_agent: String,
        /// Business purpose; `kind` remains the event discriminator.
        handoff_kind: crate::HandoffKind,
        /// Human-readable goal from the accepted packet.
        goal: String,
        /// Caller-provided identity unique among open handoffs.
        dedupe_key: String,
        /// Common card projection, absent in historical records.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        card: Option<crate::Card>,
    },
    /// A settled handoff whose report is delivered through the durable outbox.
    #[serde(rename = "team.handoff_ended")]
    TeamHandoffEnded {
        /// Correlates the receipt with the original handoff card.
        handoff_id: String,
        /// Terminal outcome; never `open`.
        status: crate::HandoffStatus,
        /// Redacted outcome text, when available.
        summary: Option<String>,
        /// Common receipt replacing the active handoff card.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        card: Option<crate::Card>,
    },
    /// A teammate requests a decision from the chief, not another teammate.
    #[serde(rename = "team.question")]
    TeamQuestion {
        /// Stable identity retained through answer delivery.
        question_id: String,
        /// Registry identity of the blocked teammate.
        from_agent: String,
        /// Decision question shown in the chief's home.
        text: String,
        /// Candidate answers supplied as context, not authority.
        options: Vec<String>,
        /// Common card projection, absent in historical records.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        card: Option<crate::Card>,
    },
    /// The chief's answer was durably queued for the asking teammate.
    #[serde(rename = "team.answered")]
    TeamAnswered {
        /// Correlates with the original decision question.
        question_id: String,
        /// Inactive receipt for the answered question.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        card: Option<crate::Card>,
    },
    /// Cross-store ownership updates have completed.
    #[serde(rename = "team.transferred")]
    TeamTransferred {
        /// Previous accountable registry identity.
        from_agent: String,
        /// New accountable registry identity.
        to_agent: String,
        /// Routine identities retained across ownership changes.
        routine_ids: Vec<String>,
        /// Responsibility lines moved to the new owner.
        responsibilities: Vec<String>,
        /// Common card projection, absent in historical records.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        card: Option<crate::Card>,
    },
    /// A teammate is retired after its ownership saga.
    #[serde(rename = "team.retired")]
    TeamRetired {
        /// Retained registry identity whose home is now archived.
        agent_id: String,
        /// Explicit successor; no implicit channel fallback is authorized.
        successor: Option<String>,
        /// Common card projection, absent in historical records.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        card: Option<crate::Card>,
    },
    /// A retired profile is active again without reclaiming transferred work.
    #[serde(rename = "team.restored")]
    TeamRestored {
        /// Existing registry identity, not a newly hired agent.
        agent_id: String,
        /// Common card projection, absent in historical records.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        card: Option<crate::Card>,
    },
}

impl EventKind {
    /// Keep operational diagnostics out of the conversation's default reading path.
    pub fn runtime_error(code: impl Into<String>, message: impl Into<String>) -> Self {
        let code = code.into();
        let (user_message, retryable) = match code.as_str() {
            "interrupted" => ("The agent stopped. Try again.", true),
            "browser_tab_limit" => ("Too many tabs are open. Close a tab and try again.", true),
            "auth_unavailable" => (
                "Your model connection needs attention. Check Settings.",
                false,
            ),
            "sandbox_unavailable" => (
                "Extra protection for commands isn't available on this computer.",
                false,
            ),
            "tool_gate" => (
                "The agent couldn't start safely. Check for an update.",
                false,
            ),
            "model_error" | "runtime_error" | "runtime_unavailable" => {
                ("Something went wrong on my side. Try again.", true)
            }
            _ => ("Something went wrong on my side.", false),
        };
        Self::RuntimeError {
            code,
            message: message.into(),
            user_message: user_message.into(),
            retryable,
        }
    }
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
    /// PNG data URL, at most 32×32 pixels and 8 KiB decoded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub favicon: Option<String>,
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
