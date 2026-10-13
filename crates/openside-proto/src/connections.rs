use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Browser profile whose sign-in metadata this row describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionProfile {
    /// Managed shared agent profile.
    Agent,
    /// Person's browser; sharing is a separate local-only operation.
    User,
}

/// Confidence and liveness; only an observed sign-in or probe establishes active status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionStatus {
    /// Discovered sign-in suggested by cookies/history; authentication is not verified.
    Candidate,
    /// Imported cookies that have not been checked.
    Unverified,
    /// Observed authenticated state.
    Active,
    /// Every fingerprint key is absent or expired.
    Expired,
    /// The page or document response requests authentication.
    NeedsLogin,
}

/// Evidence that originally established or refreshed the sign-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionSource {
    /// Completed login ticket or request.
    Login,
    /// Saved password or passkey sign-in.
    Vault,
    /// Human takeover crossed the login boundary.
    Takeover,
    /// OTP submission followed by an authenticated page.
    Otp,
    /// Explicit browser import.
    Import,
    /// Weak cookie/history sweep.
    Sweep,
    /// Explicit local sharing from the person's browser.
    Share,
}

/// Identity labels are descriptive and never grant authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionIdentitySource {
    /// Generic account controls or landmarks.
    Probe,
    /// Agent-confirmed page text.
    Agent,
    /// Login ticket identity.
    LoginTicket,
    /// Linked Vault identity.
    Vault,
    /// No account label is known.
    None,
}

/// Per-agent access; absent teammates default to ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionAccess {
    /// May use the sign-in subject to ordinary action policy.
    Allowed,
    /// Must obtain a Confirm ticket before use.
    Ask,
    /// Cannot use or list the sign-in through agent tools.
    Denied,
}

/// A grant is bound to a stable agent ID, not an identity label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectionGrant {
    /// Stable team agent ID, or chief before team integration.
    pub agent_id: String,
    /// Additional gate; never bypasses Browse or write policy.
    pub access: ConnectionAccess,
}

/// Authentication material and the identity HMAC are deliberately absent from this view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Connection {
    /// Opaque c_-prefixed identifier.
    pub id: String,
    /// Registrable domain under the public and private suffix lists.
    pub site: String,
    /// Cookie jar ownership, independent of the calling device.
    pub profile: ConnectionProfile,
    /// Write-time-masked label; never a raw email or account key.
    pub identity: Option<String>,
    /// Evidence behind the label, not a trust level.
    pub identity_source: ConnectionIdentitySource,
    /// Authentication evidence: candidate means a discovered sign-in, not verified authentication; active means checked sign-in. Agent access is separate.
    pub status: ConnectionStatus,
    /// Strongest recorded sign-in source.
    pub source: ConnectionSource,
    /// Bounded PNG data URL; no external URL is fetched by the client.
    pub favicon: Option<String>,
    /// Same-site HTTP(S) URL without query, fragment or credentials.
    pub home_url: Option<String>,
    /// Optional saved login link; not the login's contents.
    pub vault_item_id: Option<String>,
    /// Shared cookie-set identifier for site account switchers.
    pub jar_group: Option<String>,
    /// Null selects the shared agent profile.
    #[serde(default)]
    pub account_space: Option<String>,
    /// Number of recorded accounts in the same site/profile.
    pub accounts_on_site: u32,
    /// UTC time the row was first created.
    pub created_at: DateTime<Utc>,
    /// Most recent authoritative sign-in or explicit probe.
    pub verified_at: Option<DateTime<Utc>>,
    /// Most recent successful agent use, if any.
    pub used_at: Option<DateTime<Utc>>,
    /// Stable agent ID associated with used_at.
    pub used_by: Option<String>,
    /// Latest live fingerprint expiry, or null for session cookies.
    pub expires_at: Option<DateTime<Utc>>,
    /// Per-agent authorization, independent of authentication status; unspecified teammates ask.
    pub grants: Vec<ConnectionGrant>,
}

/// Admin listing separates weak candidates from ordinary cards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConnectionList {
    /// Whether a managed browser is configured and available.
    pub browser_available: bool,
    /// Non-candidate rows in stable site/ID order.
    pub connections: Vec<Connection>,
    /// Discovered sign-ins suggested by cookies/history, not verified connections or agent authorization.
    pub candidates: Vec<Connection>,
}

/// Admin metadata patch; omitted fields remain unchanged.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateConnectionRequest {
    /// Upsert these grants without replacing unrelated agents.
    pub grants: Option<Vec<ConnectionGrant>>,
    /// New HTTP(S) home URL, restricted to the connection's site.
    pub home_url: Option<String>,
}

/// Forget is the default; stronger actions must be explicitly requested.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RevokeConnectionRequest {
    /// Also clear every shared-profile account's site data.
    #[serde(default)]
    pub sign_out: bool,
    /// Also delete the linked saved login, if present.
    #[serde(default)]
    pub delete_vault_item: bool,
}

/// Select an existing shared session, or the default sign-in session.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReloginConnectionRequest {
    /// Omission selects the chief agent's durable home session.
    pub session_id: Option<crate::SessionId>,
}

/// Admin list filters; candidate separation is retained after filtering.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectionListQuery {
    /// Exclude connections explicitly denied to this agent.
    pub agent: Option<String>,
    /// Restrict to one liveness/confidence state.
    pub status: Option<ConnectionStatus>,
}

/// Closed audit vocabulary; arbitrary page text is never an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionAuditAction {
    /// A sign-in source produced metadata.
    Detected,
    /// A probe verified authenticated state.
    Verified,
    /// All fingerprint keys disappeared or expired.
    Expired,
    /// An authentication challenge was observed.
    NeedsLogin,
    /// An administrator requested authentication again.
    Relogin,
    /// An agent opened a signed-in page.
    Open,
    /// An agent issued an in-page request.
    Fetch,
    /// An administrator changed grants.
    Grant,
    /// Metadata and grants were removed without deleting cookies.
    Forget,
    /// Site authentication was cleared.
    SignOut,
}

/// Append-only metadata audit; no credentials, identities or request bodies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConnectionAuditRecord {
    /// UTC event time.
    pub at: DateTime<Utc>,
    /// Connection identifier, retained after removal.
    pub connection_id: String,
    /// Registrable site, never a request URL.
    pub site: String,
    /// Stable acting agent ID.
    pub agent_id: String,
    /// Originating session when the operation has one.
    pub session_id: Option<crate::SessionId>,
    /// Closed operation category.
    pub action: ConnectionAuditAction,
    /// Only method and path for fetch, never queries, headers or bodies.
    pub detail: Option<String>,
}

/// Bounded, newest-first audit response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConnectionAuditList {
    /// Filtered metadata records.
    pub records: Vec<ConnectionAuditRecord>,
}

/// Admin audit filters; all supplied predicates are combined.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectionAuditQuery {
    /// Exact connection ID.
    pub connection: Option<String>,
    /// Exact agent ID.
    pub agent: Option<String>,
    /// One audit action.
    pub action: Option<ConnectionAuditAction>,
    /// Defaults to 100 and is clamped to 1–1000.
    pub limit: Option<usize>,
}

/// Agent use category carried by connection.used events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionUseMode {
    /// Open or reuse a page.
    Open,
    /// Same-origin in-page HTTP request.
    Fetch,
}

/// Stable connection error codes; diagnostic data is not included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionError {
    /// No live registry row exists.
    ConnectionNotFound,
    /// Grant or policy refuses the operation.
    ConnectionDenied,
    /// Authentication must be completed before use.
    ConnectionNeedsLogin,
    /// URL or request origin is outside the connection site.
    ConnectionCrossSite,
    /// The managed browser cannot service the operation.
    ConnectionBrowserUnavailable,
    /// A mutating in-page request needs a bound approval.
    ConnectionWriteNeedsApproval,
}

/// Metadata-only listing; denied rows are excluded for the calling agent.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectionsListArgs {
    /// Optional registrable-domain filter.
    pub site: Option<String>,
}

/// Page-opening arguments, subject to independent grant and Browse gates.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectionOpenArgs {
    /// Opaque registry ID.
    pub connection_id: String,
    /// Optional same-site destination instead of home_url.
    pub url: Option<String>,
}

/// Explicit probe arguments; the probe uses the calling session's Browse policy.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectionCheckArgs {
    /// Opaque registry ID.
    pub connection_id: String,
}

/// Page-derived identity input; excluded from Debug because it is not yet masked.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectionConfirmArgs {
    /// Opaque registry ID; labels never select the authorization target.
    pub connection_id: String,
    /// Transient label to mask and HMAC at the write boundary.
    pub identity: String,
}

/// Request-metadata observation never includes header values or bodies.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectionObserveArgs {
    /// Tab owned by the calling session.
    pub tab_id: String,
}

/// Same-origin request arguments; bodies and queries must never enter audit detail.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectionFetchArgs {
    /// Opaque registry ID used for grant checks.
    pub connection_id: String,
    /// Session-owned tab already at the connection origin.
    pub tab_id: String,
    /// GET/HEAD browse; other methods require bound write approval.
    pub method: String,
    /// Same-origin path bound into the approval descriptor.
    pub path: String,
    /// Request-only query data, excluded from audit.
    pub query: Option<std::collections::BTreeMap<String, String>>,
    /// Request-only JSON data, excluded from audit.
    pub json_body: Option<serde_json::Value>,
    /// Daemon-observed request ID; header values never appear in arguments.
    pub reuse_headers_from: Option<String>,
}

/// Link into the existing human-step ticket UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TicketRef {
    /// Session containing the ticket.
    pub session_id: crate::SessionId,
    /// Stable ticket ID.
    pub ticket_id: crate::TicketId,
}

/// Relogin can finish silently using the Vault or require the person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ReloginResult {
    /// The existing sign-in flow completed without a human ticket.
    SignedIn,
    /// Continue through an existing human-step or approval ticket.
    NeedsYou {
        /// Ticket location, not its secret-input contents.
        ticket: TicketRef,
    },
}

/// A person opts in to a separate persistent sign-in profile for this site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateAccountSpaceRequest {
    /// Registrable site domain, without a URL path.
    pub site: String,
}

/// The new profile and its existing login continuation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CreateAccountSpaceResult {
    /// Persistent profile identifier.
    pub space_id: String,
    /// Connection metadata created for this profile.
    pub connection_id: Option<String>,
    /// Silent completion or the existing human-step ticket.
    pub result: ReloginResult,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connections_events_round_trip_without_changing_old_logs() {
        for value in [
            serde_json::json!({"kind":"connection.saved","connection_id":"c_0000000000000001","site":"example.com","identity":"j•••@example.net"}),
            serde_json::json!({"kind":"connection.used","connection_id":"c_0000000000000001","site":"example.com","mode":"open","method":null}),
            serde_json::json!({"kind":"session.settled"}),
        ] {
            let event: crate::EventKind = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(event).unwrap(), value);
        }
        assert_eq!(
            serde_json::to_value(ConnectionError::ConnectionNotFound).unwrap(),
            "connection_not_found"
        );
        assert_eq!(
            serde_json::to_value(ReloginResult::SignedIn).unwrap(),
            serde_json::json!({"status":"signed_in"})
        );
        assert!(crate::ROUTES
            .iter()
            .filter(|route| route.path.starts_with("/v1/connections"))
            .all(|route| route.scope == crate::CapabilityScope::Admin));
    }
}
