//! Remote devices: QR pairing, device listing, Web Push subscriptions, OTP ingest and
//! the schema-driven settings document (`docs/REMOTE.md`, `docs/specs/remote-ui.md`).

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::CapabilityScope;

/// Routes callable without a bearer token: `(method, path)`. The one-time pairing code in
/// the body is the credential; an artifact preview is authorized by its signed query.
/// Platform hooks carry their per-account cryptographic signature in headers.
pub const PUBLIC_ROUTES: &[(&str, &str)] = &[
    ("POST", "/v1/pairing/exchange"),
    ("GET", "/v1/sessions/{id}/artifacts/{path}/preview"),
    ("POST", "/v1/hooks/{kind}/{account_id}"),
];

/// Seconds a pairing offer stays valid.
pub const PAIRING_TTL_SECS: i64 = 180;

/// Create a one-time pairing offer from the local socket or a verified local person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreatePairingRequest {
    /// Label the paired device will carry.
    pub name: String,
    /// Scopes the device token will hold; at least one.
    pub scopes: Vec<CapabilityScope>,
    /// Operator-selected identity role, fixed in the offer and not chosen by its redeemer.
    #[serde(default)]
    pub kind: DeviceKind,
    /// Transport the link should use; default: the first entry of `remote.mode`, or the
    /// plain `remote.public_url` when no transport is configured.
    #[serde(default)]
    pub via: Option<PairingTransport>,
    /// Establish an import authenticator for the extension through this local-person offer.
    #[serde(default)]
    pub import_delivery: bool,
}

/// How a paired device reaches the daemon from outside its network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PairingTransport {
    /// Tailscale Serve at `remote.public_url`.
    Tailscale,
    /// Cloudflare Tunnel run by the daemon.
    Cloudflare,
    /// CLI/MCP use Noise with the pairing-pinned daemon key; relay-served PWA code
    /// still requires trust in the relay operator.
    Relay,
}

/// A one-time pairing offer, valid for [`PAIRING_TTL_SECS`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PairingOffer {
    /// One-time code; exchanged at most once. Show it only to the person pairing.
    pub code: String,
    /// Pairing path on the daemon's origin, or the relay origin for relay links.
    /// The code is a fragment, absent from ordinary HTTP requests and Referer
    /// headers but readable by the page's scripts, including relay-served code.
    pub path: String,
    /// Link for the QR code: `<base>/pair#<code>` for direct transports, or
    /// `<relay>/pair#c=<code>&t=relay&d=<daemon id>&k=<daemon key>` for the relay. `null`
    /// when no transport or public URL is configured.
    pub url: Option<String>,
    /// Scopes the resulting token will hold.
    pub scopes: Vec<CapabilityScope>,
    /// Instant after which the code is refused.
    pub expires_at: DateTime<Utc>,
    /// Transport `url` uses; `null` for a plain `remote.public_url` or no URL.
    pub transport: Option<PairingTransport>,
    /// Operator-selected role the resulting token will hold.
    #[serde(default)]
    pub kind: DeviceKind,
}

/// Remote transports the daemon runs (`GET /v1/remote/status`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteStatus {
    /// Enabled transports in pairing preference order.
    pub modes: Vec<PairingTransport>,
    /// `remote.public_url`.
    pub public_url: Option<String>,
    /// The supervised `cloudflared`, when enabled.
    pub cloudflare: Option<CloudflareStatus>,
    /// The relay registration, when enabled.
    pub relay: Option<RelayStatus>,
    /// Tailscale installation and the openside Serve handler.
    pub tailscale: TailscaleStatus,
    /// Persisted transport settings, without secret values or executable overrides.
    pub configuration: RemoteConfiguration,
    /// This request proved local-person authority to issue an offer.
    pub pairing_allowed: bool,
    /// This request proved local-person authority to change transport settings.
    pub configure_allowed: bool,
    /// A missing listener prevents a persisted transport from applying live.
    pub restart_required: bool,
}

/// Supervised `cloudflared` state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CloudflareStatus {
    /// `starting`, `running`, `restarting` or `failed` (cannot start at all).
    pub state: String,
    /// Public HTTPS URL once known (quick tunnels change it on every restart).
    pub url: Option<String>,
    /// Times the process exited and was started again.
    pub restarts: u32,
    /// Latest failure, without secrets.
    pub last_error: Option<String>,
}

/// Relay registration state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RelayStatus {
    /// `connecting`, `connected`, `backoff` or `failed` (local registration setup failed).
    pub state: String,
    /// Relay origin.
    pub url: String,
    /// Routing id the relay knows this daemon by.
    pub daemon_id: String,
    /// Daemon X25519 public key (base64url) carried in relay pairing links.
    pub daemon_key: String,
    /// Devices connected through the relay now.
    pub clients: u32,
    /// Latest failure, without secrets.
    pub last_error: Option<String>,
    /// Noise encrypts every daemon/device stream end-to-end.
    pub e2ee: bool,
}

/// Tailscale Serve state for the openside handler, not unrelated services.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TailscaleStatus {
    /// Whether tailscale is available on PATH.
    pub installed: bool,
    /// `unavailable`, `stopped`, `running` or `failed`.
    pub state: String,
    /// HTTPS origin of the active openside handler.
    pub url: Option<String>,
    /// Latest failure, without command output or secrets.
    pub last_error: Option<String>,
}

/// Enable or disable openside's Tailscale Serve handler.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigureTailscaleRequest {
    /// Whether the transport should run.
    pub enabled: bool,
}

/// Configure the managed Cloudflare Tunnel; token values remain file/env-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetCloudflareRequest {
    /// Whether the transport should run.
    pub enabled: bool,
    /// `quick` or `named`.
    pub tunnel: String,
    /// Named tunnel hostname.
    pub hostname: Option<String>,
    /// Local file containing the named tunnel's token, or null to use the environment.
    pub token_file: Option<String>,
}

/// Configure a self-hosted relay; its registration token remains file/env-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetRelayRequest {
    /// Whether the daemon should register with the relay.
    pub enabled: bool,
    /// Relay origin.
    pub url: Option<String>,
    /// Local registration-token file, or null to use the environment.
    pub token_file: Option<String>,
}

/// Tailscale configuration presented to the web setup screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TailscaleConfiguration {
    /// Whether Tailscale is configured.
    pub enabled: bool,
    /// Persisted direct pairing origin.
    pub public_url: Option<String>,
}

/// Transport settings safe to inspect; no registration or tunnel token values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteConfiguration {
    /// Tailscale settings.
    pub tailscale: TailscaleConfiguration,
    /// Cloudflare settings.
    pub cloudflare: SetCloudflareRequest,
    /// Relay settings.
    pub relay: SetRelayRequest,
}

/// Socket-only bootstrap for a desktop browser; never returned to TCP callers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LocalAccess {
    /// Per-boot locality proof; keep only in browser sessionStorage.
    pub secret: String,
    /// Loopback launch URL with credentials in its fragment, or null without a listener.
    pub url: Option<String>,
    /// Local person-device identifier.
    pub device_id: String,
    /// Bearer token for the local person, distinct from the per-boot secret.
    pub token: String,
    /// Token scopes.
    pub scopes: Vec<CapabilityScope>,
    /// Fixed person-device identity.
    pub kind: DeviceKind,
}

/// A bounded durable event page for transports that buffer server-sent events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EventPoll {
    /// Events in ascending per-session sequence order.
    pub events: Vec<crate::Event>,
    /// Exclusive replay cursor, including filtered subagent events.
    pub last_seq: u64,
}

/// Exchange a pairing code for a device token (no bearer token needed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExchangePairingRequest {
    /// Code from the pairing offer.
    pub code: String,
}

/// Trust role fixed at pairing, not selected anew by each authenticated request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    /// A web, phone, extension or channel client that conveys a person's response.
    #[default]
    PersonDevice,
    /// A CLI, MCP or API agent that must wait for a person device's approval.
    AgentClient,
}

/// A paired device (never its token).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Device {
    /// Revocation identifier (`DELETE /v1/devices/{id}`).
    pub device_id: String,
    /// Label given at pairing; empty for devices paired before labels were stored.
    pub name: String,
    /// Scopes the token holds.
    pub scopes: Vec<CapabilityScope>,
    /// Pairing-time trust role; legacy devices without recorded source default to person_device.
    #[serde(default)]
    pub kind: DeviceKind,
    /// Pairing time, when known.
    pub created_at: Option<DateTime<Utc>>,
}

/// Paired devices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceList {
    /// Devices in pairing order.
    pub devices: Vec<Device>,
}

/// Seconds a pending access request stays open.
pub const ACCESS_REQUEST_TTL_SECS: i64 = 600;

/// Ask for scopes the calling device does not hold yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequestAccessRequest {
    /// Scopes wanted in addition to the ones the device holds; at least one.
    pub scopes: Vec<CapabilityScope>,
}

/// A device waiting for someone on the daemon's computer to allow more scopes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccessRequest {
    /// Decision identifier (`POST /v1/access-requests/{id}/approve`).
    pub id: String,
    /// The requesting device.
    pub device_id: String,
    /// Label given at pairing.
    pub name: String,
    /// Scopes asked for.
    pub scopes: Vec<CapabilityScope>,
    /// When the request was made.
    pub created_at: DateTime<Utc>,
    /// Display time after which the request is dropped; the deadline itself is monotonic.
    pub expires_at: DateTime<Utc>,
}

/// Requests waiting for a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccessRequestList {
    /// Oldest first.
    pub requests: Vec<AccessRequest>,
}

/// The calling device's own record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MyDevice {
    /// Revocation identifier.
    pub device_id: String,
    /// Label given at pairing.
    pub name: String,
    /// Pairing-time trust role.
    pub kind: DeviceKind,
    /// Scopes the token holds now.
    pub scopes: Vec<CapabilityScope>,
    /// This device's pending request for more access, if one is open.
    pub request: Option<AccessRequest>,
}

/// The daemon's VAPID public key, the PWA's `applicationServerKey`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VapidPublicKey {
    /// Uncompressed P-256 public key, base64url without padding.
    pub public_key: String,
}

/// Subscription keys as `PushSubscription.toJSON().keys` reports them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushKeys {
    /// Subscription P-256 public key, base64url.
    pub p256dh: String,
    /// 16-octet authentication secret, base64url.
    pub auth: String,
}

/// Register a browser push subscription for the calling device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CreatePushSubscriptionRequest {
    /// Push service endpoint; HTTPS, or HTTP on a loopback address.
    pub endpoint: String,
    /// Encryption keys.
    pub keys: PushKeys,
}

/// A stored push subscription (keys omitted).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushSubscription {
    /// Subscription identifier.
    pub id: String,
    /// Device whose token registered it; `null` for the local socket.
    pub device_id: Option<String>,
    /// Push service endpoint.
    pub endpoint: String,
    /// Registration time.
    pub created_at: DateTime<Utc>,
}

/// Push subscriptions visible to the caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushSubscriptionList {
    /// Subscriptions in registration order.
    pub subscriptions: Vec<PushSubscription>,
}

/// Report whether the client owning a subscription is in the foreground. A focused
/// client gets no push (it already shows the event); focus lapses after
/// [`PUSH_FOCUS_TTL_SECS`] without a renewed report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PushPresenceRequest {
    /// The caller's subscription.
    pub subscription_id: String,
    /// `document.visibilityState === "visible" && document.hasFocus()`.
    pub focused: bool,
}

/// Seconds a `focused: true` report suppresses pushes unless renewed.
pub const PUSH_FOCUS_TTL_SECS: i64 = 60;

/// Decrypted push payload the service worker receives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PushNotification {
    /// `ticket` (a human step or approval is waiting) or `settled` (a run finished).
    pub kind: String,
    /// Session concerned.
    pub session_id: String,
    /// Ticket concerned, for `ticket`.
    pub ticket_id: Option<String>,
    /// Notification title.
    pub title: String,
    /// Notification body (redacted, at most 200 characters).
    pub body: String,
    /// Deep link opened on click: `/sessions/{id}` or `/sessions/{id}?ticket={tid}`.
    pub url: String,
}

/// A forwarded SMS message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OtpIngestRequest {
    /// Sender as the phone reports it; must be in `otp.senders`.
    pub sender: String,
    /// Message text; only the code extracted by a configured site pattern is kept.
    pub text: String,
}

/// Outcome of an OTP ingest; never echoes the code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OtpIngestResponse {
    /// Site origins whose pattern matched the text.
    pub matched_sites: Vec<String>,
    /// Pending `secret_input` tickets filled with the code now.
    pub resolved_tickets: Vec<String>,
    /// Codes kept in memory for a step that has not opened yet.
    pub stored: u32,
}

/// The persisted configuration document (`config.toml`) as JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SettingsDocument {
    /// Every configuration key with its stored or default value, except `file_only`
    /// sections. Shape: the openside configuration JSON Schema.
    pub config: Value,
    /// Sections only editable in the file (`credentials`: opaque, may hold values).
    pub file_only: Vec<String>,
    /// Whether the stored document differs from what the running daemon loaded;
    /// changes apply on the next daemon start, except the live policy (`/v1/policy`).
    pub restart_required: bool,
    /// JSON Schema of `config`, so a client can render and validate every key.
    pub schema: Value,
    /// Dotted keys a paired person device may change without local authority; an entry
    /// ending in `.` covers every key under that prefix. Every other key (executables, paths,
    /// listeners, trust lists, sandbox, network, credentials, remote access) needs local
    /// authority: the Unix socket, or a browser opened from the launcher on this computer.
    /// Empty for an agent client that has no local authority.
    pub remote_editable: Vec<String>,
    /// Whether this caller lacks local authority, i.e. is limited to `remote_editable`.
    pub remote: bool,
}

/// Replace the configuration document; validated like `config.toml` before writing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateSettingsRequest {
    /// Complete document; `file_only` sections are kept from the file.
    pub config: Value,
}
