use chrono::{DateTime, Utc};
use openside_core::{MessageId, MessageStatus, QueueMode, SessionId, TicketId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Capability required to access an HTTP operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityScope {
    /// Inspect sessions, events, and tickets.
    Read,
    /// Create sessions and modify or deliver messages.
    Steer,
    /// Resolve or reject confirmation and information tickets.
    Approve,
    /// Resolve, complete, or reject secret-input, device-approval, and takeover tickets.
    HumanStep,
    /// Pair and revoke devices.
    Admin,
    /// Drive session browser routes (`browser_repl`) without steering the agent; for
    /// sandboxed tools, MCP servers and extensions.
    Browse,
    /// Use saved vault credentials through vault routes; never reveals values.
    VaultUse,
    /// Deliver one-time codes to the OTP ingest route.
    OtpIngest,
}

impl CapabilityScope {
    /// Whether a token holding `self` may call a route requiring `required`: `admin`
    /// grants every scope and `steer` grants `browse` (a steering client can already make
    /// the agent browse). No other scope implies another.
    pub fn grants(self, required: CapabilityScope) -> bool {
        self == required || self == Self::Admin || (self == Self::Steer && required == Self::Browse)
    }
}

/// Public permission modes, mirroring `openside_policy::Mode`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PolicyMode {
    /// Read only; deny mutations.
    ReadOnly,
    /// Guard; work inside approved roots, ask elsewhere.
    #[default]
    Guard,
    /// Full access; sensitive operations still require approval.
    Full,
}

impl From<PolicyMode> for openside_policy::Mode {
    fn from(mode: PolicyMode) -> Self {
        match mode {
            PolicyMode::ReadOnly => Self::ReadOnly,
            PolicyMode::Guard => Self::Guard,
            PolicyMode::Full => Self::FullAccess,
        }
    }
}

impl From<openside_policy::Mode> for PolicyMode {
    fn from(mode: openside_policy::Mode) -> Self {
        match mode {
            openside_policy::Mode::ReadOnly => Self::ReadOnly,
            openside_policy::Mode::Guard => Self::Guard,
            openside_policy::Mode::FullAccess => Self::Full,
        }
    }
}

/// Minimal daemon settings, with no credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Settings {
    /// Provider-qualified default model.
    pub default_model: Option<String>,
    /// Configured fallback model selectors, in priority order.
    pub fallback_models: Vec<String>,
    /// Default permission ceiling for new sessions.
    pub policy_mode: PolicyMode,
}

/// Durable session lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// No running turn.
    Idle,
    /// A turn has started but not ended.
    Running,
    /// Idle with at least one pending human-intervention ticket.
    WaitingHuman,
    /// Explicitly stopped.
    Stopped,
}

/// A message with durable lifecycle status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Message {
    /// Idempotency key, assigned by the client or daemon.
    pub id: MessageId,
    /// Latest text, including edits made before delivery.
    pub text: String,
    /// Pending or terminal disposition.
    pub status: MessageStatus,
}

/// Session metadata and pending queue, in delivery order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Session {
    /// Stable session identifier.
    pub id: SessionId,
    /// User-visible conversation title.
    pub title: String,
    /// Provider-qualified model.
    pub model: String,
    /// Permission ceiling selected when the session was created.
    pub mode: PolicyMode,
    /// Canonical absolute workspace path used as the Guard root and file-tool cwd.
    pub workspace: Option<String>,
    /// Rebuilt from turn and session events.
    pub status: SessionStatus,
    /// Latest durable sequence, or zero before creation.
    pub last_seq: u64,
    /// Steering first, then follow-up messages.
    pub queue: Vec<Message>,
    /// Listed before unpinned sessions.
    #[serde(default)]
    pub pinned: bool,
    /// Hidden from the default session list.
    #[serde(default)]
    pub archived: bool,
    /// Private session fixed at creation: its browsing is never written to memory.
    #[serde(default)]
    pub incognito: bool,
}

/// Health response without environment or credential details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Health {
    /// Whether the daemon is ready to serve requests.
    pub ok: bool,
    /// Public API version, currently `v1`.
    pub api_version: String,
}

/// Session creation request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateSessionRequest {
    /// Conversation title, or a daemon-generated title when omitted.
    pub title: Option<String>,
    /// Model override, or settings default when omitted.
    pub model: Option<String>,
    /// Permission override, or settings default when omitted.
    pub mode: Option<PolicyMode>,
    /// Absolute workspace path, canonicalized by the daemon before session creation.
    pub workspace: Option<String>,
    /// Create a private session; immutable afterwards. Browsing history is not indexed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub incognito: bool,
}

/// Session listing response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionList {
    /// Sessions sorted by identifier for stable results.
    pub sessions: Vec<Session>,
}

/// Public delivery mode; `now` is queued durably before immediate steering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageMode {
    /// Follow-up after the current turn.
    Queue,
    /// Inject at the running turn's next boundary.
    Steer,
    /// Start or steer immediately.
    Now,
}

impl From<MessageMode> for QueueMode {
    fn from(mode: MessageMode) -> Self {
        match mode {
            MessageMode::Queue => Self::Queue,
            MessageMode::Steer | MessageMode::Now => Self::Steer,
        }
    }
}

/// Message submission with an optional client-generated idempotency key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmitMessageRequest {
    /// Reusing this id never changes or redelivers the existing message.
    pub message_id: Option<MessageId>,
    /// User-authored text.
    pub text: String,
    /// Requested delivery timing.
    pub mode: MessageMode,
    /// Files for the model and the transcript, at most [`crate::MAX_ATTACHMENTS`] with
    /// [`crate::MAX_ATTACHMENT_BYTES`] decoded bytes in total (413 `attachments_too_large`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<crate::AttachmentUpload>,
}

/// Pending message text replacement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditMessageRequest {
    /// Replacement text; only pending messages may be edited.
    pub text: String,
}

/// Pending message reorder within its delivery lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveMessageRequest {
    /// Zero-based index inside the message's own lane (steer or queue); values past the
    /// end move it last. A follow-up becomes a steer only through promote.
    pub position: u32,
}

/// Human action required to resolve a suspended tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TicketKind {
    /// Authorize or reject a sensitive operation.
    Confirm,
    /// Provide a secret directly to the daemon, never to the model or event log.
    SecretInput,
    /// Complete approval on another device.
    DeviceApproval,
    /// Answer a non-secret question.
    Info,
    /// Perform an action directly in the browser.
    Takeover,
}

/// Resumption semantics for the suspended tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TicketDeferral {
    /// Approval permits the original tool call to execute again.
    Approval,
    /// An external action supplies the original tool call's result.
    External,
}

/// Durable ticket lifecycle; only pending tickets accept a terminal transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TicketStatus {
    /// Awaiting human intervention.
    Pending,
    /// The requested intervention completed.
    Resolved,
    /// The user or daemon rejected the intervention.
    Rejected,
    /// The deadline passed.
    Expired,
    /// The intervention requires local desktop access.
    NeedsDesktop,
}

/// Identity of the agent that requested human intervention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TicketOrigin {
    /// Absent for the parent agent, otherwise the originating subagent.
    pub subagent_id: Option<String>,
}

/// One selectable non-secret response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TicketOption {
    /// Stable machine-facing answer value.
    pub value: String,
    /// Human-facing label.
    pub label: String,
}

/// Redacted invocation displayed for an approval deferral.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TicketTool {
    /// Registered tool name.
    pub name: String,
    /// Invocation arguments with secrets already removed.
    pub args: Value,
}

/// Browser destination for human intervention and origin validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TicketTarget {
    /// Browser target identifier.
    pub tab_id: Option<String>,
    /// Expected page origin, verified before secret input.
    pub origin: Option<String>,
    /// Snapshot field reference receiving daemon-only input.
    pub field_ref: Option<String>,
}

/// Where the site sends the one-time code a `request_otp` ticket asks for, as the agent read
/// it on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OtpChannel {
    /// An authenticator app (TOTP).
    Authenticator,
    /// A text message.
    Sms,
    /// An email.
    Email,
    /// A printed backup or recovery code.
    BackupCode,
    /// The page does not say.
    Unknown,
}

/// One-time-code details of a `secret_input` ticket opened by `request_otp`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TicketOtp {
    /// Where the code comes from.
    pub channel: OtpChannel,
    /// Besides a code (a string `value`), the person may resolve with
    /// `{"totp": "<otpauth:// URI or base32 setup key>"}`: the daemon stores the authenticator
    /// in the vault for the ticket's origin and fills the code it generates, now and on later
    /// `request_otp` calls for that origin.
    pub can_register: bool,
}

/// What a `confirm` ticket opened by `register_authenticator` asks the person to approve:
/// saving the authenticator a site's two-factor setup page shows into the vault, bound to
/// exactly `origin`. The setup key itself is held by the daemon only and is never part of the
/// ticket. Approving (`true`) saves it and fills the page's confirmation code; declining
/// (`false` or rejecting the ticket) discards it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TicketOtpRegister {
    /// Exact page origin the authenticator will be bound to.
    pub origin: String,
    /// Issuer named by the setup page's `otpauth://` URI, when it has one.
    pub issuer: Option<String>,
    /// Account named by the setup page's `otpauth://` URI, when it has one.
    pub account: Option<String>,
}

/// Durable human-intervention ticket correlated with the same deferred tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Ticket {
    /// Stable ticket identifier.
    pub id: TicketId,
    /// Parent session, including for subagent requests.
    pub session_id: SessionId,
    /// Stable model tool-call identifier preserved during resumption.
    pub tool_call_id: String,
    /// Approval or external-result resumption semantics.
    pub deferral: TicketDeferral,
    /// Requested human action.
    pub kind: TicketKind,
    /// Redacted human-facing reason for asking.
    pub summary: String,
    /// Optional redacted instructions for completing the action.
    pub instructions: Option<String>,
    /// Allowed non-secret responses; empty allows free-form information input.
    pub options: Vec<TicketOption>,
    /// Originating agent.
    pub origin: TicketOrigin,
    /// Redacted invocation, required for approval deferrals.
    pub tool: Option<TicketTool>,
    /// Browser destination, when applicable.
    pub target: Option<TicketTarget>,
    /// Optional condition for detecting completion.
    pub resume_when: Option<Value>,
    /// Creation time in RFC 3339 UTC.
    pub created_at: DateTime<Utc>,
    /// Deadline in RFC 3339 UTC.
    pub expires_at: DateTime<Utc>,
    /// Pending or terminal state.
    pub status: TicketStatus,
    /// Redacted resolution; never a secret-input value.
    pub resolution: Option<Value>,
    /// Set on `secret_input` tickets that ask for a one-time code (`request_otp`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub otp: Option<TicketOtp>,
    /// Set on `confirm` tickets that ask to save a site's authenticator
    /// (`register_authenticator`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub otp_register: Option<TicketOtpRegister>,
}

/// Ticket list, optionally filtered by status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TicketList {
    /// Matching durable tickets.
    pub tickets: Vec<Ticket>,
}

/// Human response interpreted according to the ticket kind.
///
/// Secret-input values are daemon-only and must never be persisted or sent to the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolveTicketRequest {
    /// Confirmation, non-secret information, or transient daemon-only secret input: a string,
    /// or `{"totp": "<otpauth:// URI or base32 key>"}` for a `secret_input` ticket whose
    /// `otp.can_register` is true. A `confirm` ticket carrying `otp_register` takes `true` or
    /// `false` like any confirmation.
    pub value: Value,
}

/// Explicit ticket rejection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RejectTicketRequest {
    /// Optional sanitized rejection detail.
    pub reason: Option<String>,
}

/// Administrative device pairing request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PairDeviceRequest {
    /// User-visible device label.
    pub name: String,
    /// Explicit capabilities to grant.
    pub scopes: Vec<CapabilityScope>,
}

/// Administrative pairing result; the secret token is returned once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PairDeviceResponse {
    /// Stable revocation identifier.
    pub device_id: String,
    /// Bearer capability token; never persist in event or application logs.
    pub token: String,
    /// Granted capabilities.
    pub scopes: Vec<CapabilityScope>,
}

/// JSON error response for non-successful HTTP requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApiError {
    /// Machine-readable failure code.
    pub code: String,
    /// Sanitized human-facing detail.
    pub message: String,
}
