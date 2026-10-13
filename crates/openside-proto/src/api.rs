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
    /// Resolve or reject confirm, info and choice tickets; positive confirm/choice decisions
    /// additionally require a person device.
    Approve,
    /// Resolve, complete, or reject secret-input, login, device-approval, and takeover tickets.
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

/// Where a session's agent tabs browse, fixed at creation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserSpace {
    /// The managed browser profile, with the logins and cookies it already holds.
    #[default]
    Shared,
    /// An isolated browser context in the same managed browser: own cookies and storage,
    /// kept in memory and wiped when the session is archived or deleted.
    Isolated,
}

impl BrowserSpace {
    /// Whether this is the default shared profile (serde skip helper).
    pub fn is_shared(&self) -> bool {
        *self == Self::Shared
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
    /// Preserves the durable caller authority rather than inferring it from message text.
    #[serde(default)]
    pub author_kind: crate::MessageAuthorKind,
    /// Present only for daemon-authored team deliveries.
    #[serde(default)]
    pub from_agent: Option<String>,
}

/// Session metadata and pending queue, in delivery order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Session {
    /// Immutable owner tag; null keeps earlier conversations separate from agent homes.
    #[serde(default)]
    pub agent_id: Option<String>,
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
    /// Short, redacted description of the current work, rebuilt from session events.
    #[serde(default)]
    pub activity: String,
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
    /// Browser space fixed at creation; historical sessions are shared.
    #[serde(default)]
    pub browser_space: BrowserSpace,
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
    /// Existing active agent for a new non-home thread; cannot rebind an existing session.
    #[serde(default)]
    pub agent_id: Option<String>,
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
    /// Browse in the shared managed profile (default) or an isolated space; immutable
    /// afterwards. Incognito sessions already have their own browser and refuse `isolated`.
    #[serde(default, skip_serializing_if = "BrowserSpace::is_shared")]
    pub browser_space: BrowserSpace,
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
    /// Sign in to a site through a form the daemon built from the page's own login form
    /// (`request_login`); the values go straight into the page, never to the model.
    Login,
    /// Pick one of the choices a page offers (`request_choice`); the daemon clicks the page's
    /// own control.
    Choice,
    /// Approve a proposed teammate using the attached draft.
    Hire,
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
    /// Device behind a direct API tool call; `local` identifies the Unix socket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    /// Agent-client identities that drove this session before the ticket opened.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub driver_ids: Vec<String>,
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
    /// `request_otp` calls for that origin. Registration requires `approve` and a person device.
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

/// Kind of a field on a `login` ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoginFieldKind {
    /// The account name: user id, email address or phone number.
    Identifier,
    /// The password.
    Password,
    /// Any other text the form asks for.
    Text,
    /// A tick box such as "keep me signed in"; answered with a boolean.
    Checkbox,
}

/// One field of the page's login form, in page order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TicketLoginField {
    /// Daemon-assigned id the answer's `values` are keyed by.
    pub id: String,
    /// What the field holds.
    pub kind: LoginFieldKind,
    /// The page's own label for the field (untrusted page text, trimmed and capped).
    pub label: String,
    /// The page's placeholder, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    /// Autocomplete token for the card's input (`username`, `email`, `tel`,
    /// `current-password`, `one-time-code`), so the person's own password manager can fill it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autocomplete: Option<String>,
    /// Imported non-secret address values offered only when the person chooses one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(extend("x-person-only" = []))]
    pub suggestions: Vec<String>,
    /// Whether the input hides what is typed (passwords).
    pub masked: bool,
    /// Whether the page requires a value.
    pub required: bool,
}

/// Kind of an alternative sign-in control on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoginAlternativeKind {
    /// Sign in with another provider's account.
    Provider,
    /// Sign in with a passkey or security key.
    Passkey,
    /// Sign in by scanning a QR code.
    Qr,
    /// Any other sign-in mode the page offers (one-time number, a login-mode tab).
    Other,
}

/// Identity provider behind an alternative sign-in control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoginProvider {
    /// Google.
    Google,
    /// Apple.
    Apple,
    /// Kakao.
    Kakao,
    /// Naver.
    Naver,
    /// Facebook.
    Facebook,
    /// GitHub.
    Github,
    /// Microsoft.
    Microsoft,
}

/// An alternative sign-in control on the page; answering with its id clicks it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TicketLoginAlternative {
    /// Daemon-assigned id.
    pub id: String,
    /// The control's accessible name on the page (untrusted page text).
    pub label: String,
    /// What kind of sign-in it starts.
    pub kind: LoginAlternativeKind,
    /// The provider, for `provider` controls the daemon recognized.
    #[serde(default)]
    pub provider: Option<LoginProvider>,
}

/// A login saved in the vault for the ticket's origin, offered instead of typing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TicketLoginAccount {
    /// Vault item id; answer with `{"account": item_id}`.
    pub item_id: String,
    /// Masked account name, e.g. `j***@naver.com`.
    pub username_hint: String,
}

/// What a `login` ticket (`request_login`) shows: the page's login form, rebuilt by the
/// daemon. Answer with `{"values": {<field id>: string | bool}, "save": bool}`,
/// `{"account": <item_id>}` or `{"alternative": <id>}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TicketLogin {
    /// Exact page origin the values go to.
    pub origin: String,
    /// The site's name as the page gives it (untrusted page text), or its host.
    pub site_name: String,
    /// The site's icon as a `data:image/...;base64,` URL (at most 8 KiB, fetched by the
    /// daemon from the page's own origin), when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Fields in page order.
    pub fields: Vec<TicketLoginField>,
    /// Other ways to sign in the page offers.
    pub alternatives: Vec<TicketLoginAlternative>,
    /// Logins saved for this origin, when there are several to pick from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(extend("x-person-only" = null))]
    pub accounts: Option<Vec<TicketLoginAccount>>,
    /// The page's own text for its sign-in button.
    pub submit_label: String,
    /// Whether `save: true` may keep the identifier and password in the vault for this
    /// origin (it also needs the `approve` scope and a person device).
    pub can_save: bool,
    /// The page's own error message next to the form (e.g. a wrong password on the previous
    /// attempt; untrusted page text, redacted and capped), when it shows one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_text: Option<String>,
}

/// What a `device_approval` ticket opened by `wait_for_approval` shows: the site asks the
/// person to approve the sign-in on their phone (a push notification in the site's own app).
/// Nobody answers it in openside: the daemon watches the page and resolves the ticket with
/// `{"status":"done","next":…}` once the page moves on; rejecting it cancels the wait.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TicketApproval {
    /// Exact page origin.
    pub origin: String,
    /// The site's name as the page gives it (untrusted page text), or its host.
    pub site_name: String,
    /// The site's icon, as on [`TicketLogin::icon`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// The page's heading and instruction for the approval (untrusted page text, capped).
    pub message: String,
    /// The number the page asks the person to tap or type on the phone, when it shows one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<String>,
}

/// One option of a `choice` ticket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TicketChoiceOption {
    /// Daemon-assigned id; answer with `{"option": id}`.
    pub id: String,
    /// The control's accessible name on the page (untrusted page text), never the model's.
    pub label: String,
    /// Secondary page text of the option, when it has some.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// What a `choice` ticket (`request_choice`) shows: a decision the page asks for, such as
/// which account, which verification method, or whether to trust this device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TicketChoice {
    /// Exact page origin.
    pub origin: String,
    /// The site's name as the page gives it (untrusted page text), or its host.
    pub site_name: String,
    /// The site's icon, as on [`TicketLogin::icon`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// The page's heading for the choice.
    pub question: String,
    /// Options in page order.
    pub options: Vec<TicketChoiceOption>,
    /// Always false: one option is picked.
    pub multiple: bool,
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
    /// Set on `login` tickets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<TicketLogin>,
    /// Set on `choice` tickets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<TicketChoice>,
    /// Set on `device_approval` tickets that wait for a phone sign-in approval
    /// (`wait_for_approval`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<TicketApproval>,
    /// Daemon projection; absence in historical logs is hydrated without rewriting them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<crate::Card>,
    /// Bound page-field metadata for fill-only requests; never contains entered values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form: Option<crate::TicketForm>,
    /// Authoritative teammate proposal; card text alone cannot authorize a hire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hire: Option<crate::HireDraft>,
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
    /// `false` like any confirmation. A `login` ticket takes
    /// `{"values": {<field id>: string | bool}, "save": bool}`, `{"account": <item_id>}` or
    /// `{"alternative": <id>}`; a `choice` ticket takes `{"option": <id>}`.
    pub value: Value,
    /// Optional complete edited proposal, accepted only for a hire decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hire: Option<crate::HireDraft>,
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
    /// Fixed at token creation; agent clients cannot approve their own work.
    #[serde(default)]
    pub kind: crate::DeviceKind,
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
    /// Operator-selected trust role fixed when this token was paired.
    #[serde(default)]
    pub kind: crate::DeviceKind,
    /// One-time extension import authenticator; never returned to a web page or logged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import_secret: Option<String>,
}

/// JSON error response for non-successful HTTP requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApiError {
    /// Machine-readable failure code.
    pub code: String,
    /// Sanitized human-facing detail.
    pub message: String,
}
