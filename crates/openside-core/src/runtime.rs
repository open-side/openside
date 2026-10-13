//! Runtime-independent boundary for the Python sidecar and daemon-owned tools.
//!
//! The daemon owns message queues and executes tools. A session opens only after the
//! runtime reports ready; its exposed tools must match the supplied specifications.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::ids::{MessageId, SessionId, TicketId};
use crate::queue::QueueMode;

/// Daemon-authenticated source of a conversational message.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum MessageAuthorKind {
    /// Authenticated owner ingress.
    Person,
    /// A paired automation client.
    AgentClient,
    /// A relayed channel thread, not owner steering.
    Channel,
    /// Schedules, team reports and historical unknown sources.
    #[default]
    Internal,
}

/// Opaque conversation snapshot, interpreted only by the sidecar.
#[derive(Clone, PartialEq, Eq)]
pub struct AgentState {
    /// Snapshot format identifier.
    pub format: String,
    /// Serialized snapshot; never include its content in diagnostics.
    pub data: String,
}

impl std::fmt::Debug for AgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentState")
            .field("format", &self.format)
            .field("data_len", &self.data.len())
            .finish()
    }
}

/// Policy tier assigned by the daemon to an exposed tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalTier {
    /// Read-only operation.
    Read,
    /// File or browser mutation.
    Write,
    /// Command execution.
    Exec,
    /// Sensitive operation requiring explicit authorization.
    Sensitive,
}

/// Daemon-owned tool specification; the parameter schema is the only schema source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSpec {
    /// Unique registered tool name.
    pub name: String,
    /// Model-facing description.
    pub description: String,
    /// JSON Schema encoded as text, keeping core independent of a JSON library.
    pub parameters_json: String,
    /// Policy tier used by the daemon.
    pub approval: ApprovalTier,
    /// Whether the tool may suspend a run on a ticket.
    pub deferrable: bool,
}

/// Daemon-assigned authority for selecting sidecar instructions and team tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamRole {
    /// Coordinates delegation and routes decisions to the person.
    Chief,
    /// Owns an area but must route delegation and decisions through the chief.
    Teammate,
}

/// Trusted session identity, kept separate from user-supplied conversation text.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamContext {
    /// Determines the permitted team tool surface, not a user-editable persona.
    pub role: TeamRole,
    /// Stable registry identity; display-name edits do not change it.
    pub agent_id: String,
    /// Rendered name and persona, limited to 1,500 Unicode scalar characters.
    pub persona: String,
    /// Name/role-only roster, limited to 2,000 Unicode scalar characters.
    pub roster: String,
}

/// Parameters for opening or restoring a sidecar session.
#[derive(Clone, PartialEq, Eq)]
pub struct SessionSpec {
    /// Durable session identifier.
    pub session: SessionId,
    /// Explicit provider-qualified model from configuration.
    pub model: String,
    /// Configured fallback model selectors, in priority order.
    pub fallback_models: Vec<String>,
    /// Ordered system instructions.
    pub system_prompt_parts: Vec<String>,
    /// Exact tool allowlist and parameter schemas.
    pub tools: Vec<ToolSpec>,
    /// Last consistent run-end snapshot, when restoring a session.
    pub resume_state: Option<AgentState>,
    /// Validated non-secret provider request-shaping metadata; credentials use private leases.
    pub credentials_json: String,
    /// Sidecar runtime options as a JSON object (`open.config.agent`).
    pub agent_json: String,
    /// Absent for legacy sessions, which retain their existing orchestration.
    pub team: Option<TeamContext>,
}

impl std::fmt::Debug for SessionSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionSpec")
            .field("session", &self.session)
            .field("model", &self.model)
            .field("fallback_models", &self.fallback_models)
            .field("credentials_json", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

/// Reverse-RPC invocation executed by the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    /// Stable model tool-call identifier, preserved across ticket resumption.
    pub tool_call_id: String,
    /// Registered tool name.
    pub name: String,
    /// Arguments encoded as JSON.
    pub arguments_json: String,
    /// Originating subagent, or absent for the parent.
    pub subagent: Option<String>,
    /// Previously resolved approval permitting this invocation to be retried.
    pub approval_ticket: Option<TicketId>,
}

/// How a suspended tool call is resumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeferKind {
    /// Resume by approving or denying execution of the original tool.
    Approval,
    /// Resume by providing a result produced outside the sidecar.
    External,
}

/// Daemon tool outcome containing only model-safe data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolOutcome {
    /// Completed execution.
    Done {
        /// Redacted model-facing result encoded as JSON.
        for_model_json: String,
    },
    /// Completed execution whose result also carries binary content for the model,
    /// e.g. a screenshot for vision models.
    DoneWithParts {
        /// Redacted model-facing result encoded as JSON.
        for_model_json: String,
        /// Content appended to the tool result in the model input.
        parts: Vec<crate::ContentPart>,
    },
    /// Suspended execution until a durable ticket is resolved.
    Deferred {
        /// Durable ticket identifier.
        ticket: TicketId,
        /// Required resumption semantics.
        kind: DeferKind,
    },
    /// Policy or authorization rejected execution.
    Denied {
        /// Sanitized reason.
        reason: String,
        /// Optional model-facing alternative.
        hint: Option<String>,
    },
    /// Execution failed.
    Failed {
        /// Sanitized failure detail.
        message: String,
    },
}

/// Asynchronous daemon tool execution; cancellation drops the returned future.
pub trait HostToolHandler: Send + Sync {
    /// Execute one invocation without blocking the transport reader.
    fn call(&self, call: ToolCall) -> Pin<Box<dyn Future<Output = ToolOutcome> + Send + '_>>;

    /// Resolve private per-attempt authentication through the daemon's policy owner, not a tool.
    fn credentials(
        &self,
        _request: crate::CredentialRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<crate::CredentialSnapshot, crate::CredentialError>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async { Err(crate::CredentialError::unavailable()) })
    }

    /// Commit a lease-bound outcome before another resolve; no provider error text is accepted.
    fn credential_report(
        &self,
        _report: crate::CredentialReport,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<crate::CredentialReportAck, crate::CredentialError>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async { Err(crate::CredentialError::unavailable()) })
    }

    /// The runtime dropped earlier messages from one agent's model context (`None` = the
    /// parent, otherwise a subagent id), so tool results delivered before now may no longer be
    /// visible to it. The transport calls this inline, in wire order, before it dispatches any
    /// later tool call; implementations must not block.
    fn context_compacted(&self, subagent: Option<String>) {
        let _ = subagent;
    }
}

/// Model-safe resolution of a deferred tool call; secret input never crosses this boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TicketResolution {
    /// Authorize execution of the original tool call.
    Approved,
    /// Deny execution of the original tool call.
    Denied {
        /// Sanitized rejection or expiration reason.
        reason: String,
    },
    /// Supply the result of an externally completed operation.
    Result {
        /// Redacted result encoded as JSON, never a secret value.
        for_model_json: String,
    },
}

/// Correlation between a deferred model tool call and its durable ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredCall {
    /// Stable model tool-call identifier.
    pub tool_call_id: String,
    /// Durable ticket identifier.
    pub ticket: TicketId,
    /// Required resumption semantics.
    pub kind: DeferKind,
}

/// Daemon command for one running or waiting subagent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubagentControl {
    /// Stop the child; its parent `delegate` call receives a cancelled result.
    Cancel,
    /// Deliver text at the child's next model-request boundary.
    Steer {
        /// Steering text.
        text: String,
    },
}

/// Terminal disposition of one run, including suspension on human intervention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// The run completed normally.
    Completed,
    /// The run was cancelled.
    Aborted,
    /// The run failed.
    Failed,
    /// The run paused on tickets; resolution resumes without a new message.
    Deferred,
}

/// Errors surfaced by a runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeError {
    /// The runtime process could not be started or died.
    Unavailable(String),
    /// The runtime rejected a command or its exposed tool gate did not match.
    Rejected(String),
    /// No configured credential source was available; callers must fail closed.
    AuthUnavailable(String),
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(m) => write!(f, "runtime unavailable: {m}"),
            Self::Rejected(m) => write!(f, "runtime rejected command: {m}"),
            Self::AuthUnavailable(m) => write!(f, "authentication unavailable: {m}"),
        }
    }
}

impl std::error::Error for RuntimeError {}

/// Normalized sidecar observations; tool lifecycle events belong to the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeEvent {
    /// Streaming assistant text.
    AssistantDelta {
        /// Text fragment.
        text: String,
    },
    /// Complete visible assistant text.
    AssistantFinal {
        /// Complete visible text.
        text: String,
    },
    /// A short model-written conversation title, separate from assistant output.
    SessionTitleProposed {
        /// Proposed title; the daemon validates it before publication.
        title: String,
    },
    /// Streaming model thinking text.
    ThinkingDelta {
        /// Text fragment.
        text: String,
    },
    /// A model request started.
    TurnStarted,
    /// A model request ended; this is not run completion.
    TurnEnded,
    /// A message entered conversation history.
    MessageAccepted {
        /// Durable message identifier.
        id: MessageId,
    },
    /// A run containing this message ended.
    MessageResult {
        /// Durable message identifier.
        id: MessageId,
        /// Run disposition.
        outcome: RunOutcome,
        /// Sanitized failure or cancellation detail.
        reason: Option<String>,
    },
    /// The run suspended on unresolved tool calls.
    Deferred {
        /// Tool calls whose tickets must all be resolved before resumption.
        calls: Vec<DeferredCall>,
    },
    /// Consistent run-end conversation snapshot.
    StateSnapshot {
        /// Opaque sidecar snapshot.
        state: AgentState,
    },
    /// A subagent changed lifecycle state.
    SubagentLifecycle {
        /// Child agent identifier.
        subagent_id: String,
        /// Runtime-defined lifecycle state.
        status: String,
        /// Final answer of a completed child.
        output: Option<String>,
        /// Explicit live inventory; absence must not imply an empty inventory.
        known_subagents: Option<Vec<String>>,
        /// Stable primary failure classification, separate from recovery failures.
        error_code: Option<String>,
        /// Untrusted exception detail; the host must redact before recording.
        diagnostic: Option<String>,
    },
    /// A subagent reported progress.
    SubagentProgress {
        /// Child agent identifier.
        subagent_id: String,
        /// Redacted progress encoded as JSON.
        progress_json: String,
    },
    /// Token usage for a model request.
    Usage {
        /// Provider-qualified model selector.
        model: String,
        /// Input token count.
        input_tokens: u64,
        /// Output token count.
        output_tokens: u64,
    },
    /// No run or sidecar message remains active.
    Settled,
    /// A command, provider, or transport failed.
    Error {
        /// Sanitized failure with authentication failures explicitly classified.
        error: RuntimeError,
    },
}

/// Session metadata returned by the runtime's ready handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    /// Actually exposed tool names, checked against the supplied allowlist.
    pub tools: Vec<String>,
    /// Deferred calls restored from the snapshot.
    pub pending_deferred: Vec<DeferredCall>,
    /// Measured startup latency in milliseconds.
    pub startup_ms: u64,
}

/// A ready agent session, owning one sidecar connection.
pub trait AgentSession: Send {
    /// Metadata from the ready handshake.
    fn info(&self) -> &SessionInfo;

    /// Send a message; while running, mode chooses steering versus queued delivery.
    fn send(
        &mut self,
        id: MessageId,
        text: String,
        mode: QueueMode,
    ) -> impl Future<Output = Result<(), RuntimeError>> + Send;

    /// Send a message with attached content parts. Runtimes without content support
    /// reject non-empty parts instead of silently dropping them.
    fn send_with_parts(
        &mut self,
        id: MessageId,
        text: String,
        parts: Vec<crate::ContentPart>,
        mode: QueueMode,
        author_kind: MessageAuthorKind,
    ) -> impl Future<Output = Result<(), RuntimeError>> + Send {
        async move {
            if parts.is_empty() && author_kind == MessageAuthorKind::Person {
                self.send(id, text, mode).await
            } else {
                Err(RuntimeError::Rejected(
                    "attachments or non-person provenance are not supported".into(),
                ))
            }
        }
    }

    /// Abort the running turn; aborting an idle session is harmless.
    fn abort(&mut self) -> impl Future<Output = Result<(), RuntimeError>> + Send;

    /// Resolve the same deferred tool call without injecting a user message.
    fn resolve_ticket(
        &mut self,
        ticket: TicketId,
        tool_call_id: String,
        resolution: TicketResolution,
    ) -> impl Future<Output = Result<(), RuntimeError>> + Send;

    /// Control a subagent this session started; runtimes without subagents reject it.
    fn control_subagent(
        &mut self,
        subagent_id: String,
        control: SubagentControl,
    ) -> impl Future<Output = Result<(), RuntimeError>> + Send {
        let _ = (subagent_id, control);
        async { Err(RuntimeError::Rejected("subagents are not supported".into())) }
    }

    /// Drop a restored continuation whose terminal output is already durable, so a
    /// restore cannot run it a second time. Runtimes without snapshots reject it.
    fn discard_deferred(&mut self) -> impl Future<Output = Result<(), RuntimeError>> + Send {
        async { Err(RuntimeError::Rejected("discard is not supported".into())) }
    }

    /// Next observation, or `None` when the process or connection is gone.
    fn next_event(&mut self) -> impl Future<Output = Option<RuntimeEvent>> + Send;

    /// Request orderly shutdown and reap the owned process.
    fn shutdown(&mut self) -> impl Future<Output = Result<(), RuntimeError>> + Send;
}

/// Factory for ready sessions with daemon-owned tool execution.
pub trait AgentRuntime: Send + Sync {
    /// Concrete session type.
    type Session: AgentSession;

    /// Open or restore a session, returning only after ready and tool-gate validation.
    ///
    /// A gate mismatch is `Rejected("tool_gate: ...")`; authentication failure during
    /// startup is `AuthUnavailable`, not a silently substituted credential source.
    fn open_session(
        &self,
        spec: SessionSpec,
        tools: Arc<dyn HostToolHandler>,
    ) -> impl Future<Output = Result<Self::Session, RuntimeError>> + Send;
}

#[cfg(test)]
mod tests {
    use super::{AgentState, SessionSpec};

    #[test]
    fn snapshot_debug_discloses_length_not_content() {
        let state = AgentState {
            format: "test-format".into(),
            data: "opaque-canary".into(),
        };
        let debug = format!("{state:?}");
        assert!(debug.contains("test-format"));
        assert!(debug.contains("data_len: 13"));
        assert!(!debug.contains(&state.data));
    }

    #[test]
    fn session_spec_debug_never_discloses_private_credentials() {
        let spec = SessionSpec {
            session: crate::SessionId::new("privacy"),
            model: "anthropic:test".into(),
            fallback_models: Vec::new(),
            system_prompt_parts: Vec::new(),
            tools: Vec::new(),
            resume_state: None,
            credentials_json: "{\"_vault_keys\":{\"anthropic\":\"debug-canary-secret\"}}".into(),
            agent_json: "{}".into(),
            team: None,
        };
        let debug = format!("{spec:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("debug-canary-secret"));
    }
}
