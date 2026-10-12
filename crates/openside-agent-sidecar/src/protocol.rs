use openside_core::{ApprovalTier, DeferKind, MessageId, SessionId, TicketId};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Serialize, Deserialize)]
pub(crate) struct Envelope<T> {
    pub v: u8,
    #[serde(flatten)]
    pub message: T,
}

impl<T> Envelope<T> {
    pub fn new(message: T) -> Self {
        Self { v: 1, message }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum DaemonMessage {
    Open {
        session: SessionId,
        model: Model,
        system_prompt_parts: Vec<String>,
        tools: Vec<Tool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        resume_state: Option<State>,
        config: Config,
    },
    UserMessage {
        message_id: MessageId,
        text: String,
        mode: MessageMode,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        parts: Vec<Part>,
    },
    Abort,
    ToolResult {
        rpc_id: String,
        outcome: Outcome,
    },
    TicketResolved {
        ticket_id: TicketId,
        tool_call_id: String,
        resolution: Resolution,
    },
    SubagentCancel {
        subagent_id: String,
    },
    SubagentSteer {
        subagent_id: String,
        text: String,
        control_id: String,
    },
    DiscardDeferred,
    Shutdown,
    Error {
        code: ErrorCode,
        message: String,
        fatal: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        message_id: Option<MessageId>,
    },
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Model {
    pub id: String,
    pub fallback: Vec<String>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: Map<String, Value>,
    pub approval: ApprovalTier,
    pub deferrable: bool,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Config {
    pub credentials: Map<String, Value>,
    pub agent: Map<String, Value>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct State {
    pub format: String,
    pub data: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MessageMode {
    Queue,
    Steer,
    Now,
}

/// Generic content part; `data_base64` and `artifact_ref` may both be present.
#[derive(Serialize, Deserialize)]
pub(crate) struct Part {
    #[serde(rename = "type")]
    pub kind: openside_core::ContentKind,
    pub mime: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum Outcome {
    Done {
        for_model: Value,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        parts: Vec<Part>,
    },
    Deferred {
        ticket_id: TicketId,
        kind: DeferKind,
    },
    Denied {
        reason: String,
        hint: Option<String>,
    },
    Error {
        message: String,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum Resolution {
    Approved,
    Denied { reason: String },
    Result { for_model: Value },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum SidecarMessage {
    Hello {
        protocol: u8,
        nonce: String,
        pid: u32,
        versions: Versions,
    },
    Ready {
        tools: Vec<String>,
        pending_deferred: Vec<Deferred>,
        startup_ms: u64,
    },
    Event {
        event: Event,
    },
    MessageAccepted {
        message_id: MessageId,
    },
    MessageResult {
        message_id: MessageId,
        outcome: RunOutcome,
        reason: Option<String>,
    },
    ToolCall {
        rpc_id: String,
        tool_call_id: String,
        name: String,
        args: Map<String, Value>,
        subagent: Option<String>,
        approval_ticket: Option<TicketId>,
    },
    SubagentSteerResult {
        control_id: String,
        delivered: bool,
    },
    Deferred {
        calls: Vec<Deferred>,
    },
    StateSnapshot {
        state: State,
    },
    Settled,
    /// Earlier messages left the agent's model context (compaction); `subagent` absent = parent.
    ContextCompacted {
        subagent: Option<String>,
    },
    Error {
        code: ErrorCode,
        message: String,
        fatal: bool,
        message_id: Option<MessageId>,
    },
    Bye,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Versions {
    pub python: String,
    pub pydantic_ai: String,
    pub pydantic_ai_harness: String,
    pub openside_agent: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Deferred {
    pub tool_call_id: String,
    pub ticket_id: TicketId,
    pub kind: DeferKind,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RunOutcome {
    Completed,
    Aborted,
    Failed,
    Deferred,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ErrorCode {
    Protocol,
    BadRequest,
    StaleTicket,
    ModelError,
    AuthUnavailable,
    ToolGate,
    Internal,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind")]
pub(crate) enum Event {
    #[serde(rename = "turn.started")]
    TurnStarted,
    #[serde(rename = "turn.ended")]
    TurnEnded,
    #[serde(rename = "assistant.delta")]
    AssistantDelta { text: String },
    #[serde(rename = "assistant.final")]
    AssistantFinal { text: String },
    #[serde(rename = "thinking.delta")]
    ThinkingDelta { text: String },
    #[serde(rename = "subagent.lifecycle")]
    SubagentLifecycle {
        subagent_id: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    #[serde(rename = "subagent.progress")]
    SubagentProgress {
        subagent_id: String,
        progress: Value,
    },
    #[serde(rename = "usage")]
    Usage {
        model: String,
        input_tokens: u64,
        output_tokens: u64,
    },
}
