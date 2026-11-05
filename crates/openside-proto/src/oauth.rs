use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Sign-in method a provider supports.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum OAuthMethod {
    /// Authorization page with a loopback callback on the daemon host.
    Browser,
    /// Device-code flow: the person enters a short code on the provider's page.
    Device,
}

/// One provider that openside can sign in to on the daemon host.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OAuthProviderInfo {
    /// Storage provider namespace, also the provider pool and catalog name.
    pub provider: String,
    /// Display name.
    pub name: String,
    /// Supported methods; the first is the default.
    pub methods: Vec<OAuthMethod>,
    /// Methods that accept a pasted code or redirect URL when the callback cannot reach the daemon host.
    pub manual_input_methods: Vec<OAuthMethod>,
}

/// Providers openside can sign in to, in a fixed order.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OAuthProviderList {
    /// Fixed first-release provider order.
    pub providers: Vec<OAuthProviderInfo>,
}

/// An openside-held OAuth account; never tokens, identity, plan or project details.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OAuthAccount {
    /// Credential source, always `pi-oauth`.
    pub source: String,
    /// Storage provider namespace.
    pub provider: String,
    /// Opaque openside-assigned account id.
    pub account_id: String,
    /// Person-assigned display label.
    pub label: String,
    /// Operator-controlled enablement.
    pub enabled: bool,
    /// Remaining measured usage protected from automatic selection, 0–99.
    pub reserve_percent: u8,
    /// Enabled, signed in and carrying every detail the provider requires.
    pub available: bool,
    /// Sanitized readiness explanation.
    pub reason: String,
    /// The provider definitively rejected the stored sign-in; sign in again.
    pub auth_invalid: bool,
    /// Creation time, milliseconds since the Unix epoch.
    pub created_at_ms: u64,
}

/// OAuth accounts sorted by provider, then creation time.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OAuthAccountList {
    /// Accounts sorted by provider, then creation time.
    pub accounts: Vec<OAuthAccount>,
}

/// Start signing in a new account; nothing is stored until the sign-in completes.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreateOAuthAccountRequest {
    /// Non-control person-assigned label, 1–80 characters.
    pub label: String,
    /// Append the new account to the end of its provider pool once signed in.
    #[serde(default)]
    pub join_pool: bool,
    /// Sign-in method; missing uses the provider's default.
    pub method: Option<OAuthMethod>,
}

/// Sign an existing account in again, keeping its label, settings and pool membership.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReloginOAuthAccountRequest {
    /// Sign-in method; missing uses the provider's default.
    pub method: Option<OAuthMethod>,
}

/// Change account metadata; missing fields keep their current values.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UpdateOAuthAccountRequest {
    /// New label, 1–80 characters.
    pub label: Option<String>,
    /// New enablement.
    pub enabled: Option<bool>,
    /// New protected remaining percentage, 0–99.
    pub reserve_percent: Option<u8>,
}

/// Pasted authorization code or redirect URL; write-only, never echoed.
#[derive(Clone, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OAuthInputRequest {
    /// Code or full redirect URL, 1–4096 characters.
    pub code: String,
}

impl std::fmt::Debug for OAuthInputRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthInputRequest").finish_non_exhaustive()
    }
}

/// Whether a transaction adds a new account or signs an existing one in again.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OAuthTransactionKind {
    /// New account.
    Create,
    /// Existing account.
    Relogin,
}

/// Sign-in transaction progress.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OAuthTransactionStatus {
    /// The sign-in helper is starting.
    Starting,
    /// Open `authorization.url` in a browser.
    AwaitingBrowser,
    /// Enter `authorization.user_code` at `authorization.url`.
    AwaitingCode,
    /// The provider is exchanging the authorization for credentials.
    Exchanging,
    /// Signed in; `account` is set.
    Ready,
    /// Sign-in failed; see `reason`.
    Failed,
    /// Cancelled by the person.
    Cancelled,
    /// Not completed in time.
    Expired,
}

/// What the person needs to complete the sign-in.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OAuthAuthorization {
    /// Provider authorization or verification page.
    pub url: String,
    /// Daemon-host loopback page that starts the browser flow, when the provider uses one.
    pub launch_url: Option<String>,
    /// Provider instructions for the person.
    pub instructions: Option<String>,
    /// Device-flow code to enter at `url`.
    pub user_code: Option<String>,
}

/// In-memory sign-in transaction, visible to the device that started it.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OAuthTransaction {
    /// Opaque transaction id.
    pub transaction_id: String,
    /// New account or sign-in again.
    pub kind: OAuthTransactionKind,
    /// Storage provider namespace.
    pub provider: String,
    /// Account the transaction stores into; for `create` it exists only once `ready`.
    pub account_id: String,
    /// Sign-in method in use.
    pub method: OAuthMethod,
    /// Current progress.
    pub status: OAuthTransactionStatus,
    /// Stable reason code for `failed`, `cancelled` and `expired`.
    pub reason: Option<String>,
    /// Authorization details; cleared once the transaction finishes.
    pub authorization: Option<OAuthAuthorization>,
    /// True while the provider is waiting for a pasted code or redirect URL.
    pub manual_input: bool,
    /// Expiry time, milliseconds since the Unix epoch.
    pub expires_at_ms: u64,
    /// The signed-in account once `ready`.
    pub account: Option<OAuthAccount>,
}
