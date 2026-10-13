use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Per-identity state; no status authorizes fallback to another account.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChannelAccountStatus {
    /// Adapter is running, or an explicit read-only connectivity test passed.
    Connected,
    /// Owner disabled this identity; configuration and encrypted credentials remain.
    Disabled,
    /// Credentials were refused; reconnect by replacing this account's credentials.
    Expired,
    /// Network, listener or credential-source failure; no other account is selected.
    Unavailable,
    /// Vault cannot release this account's credentials.
    VaultLocked,
    /// The legacy runner or absent daemon TCP listener prevents live application.
    RestartRequired,
}

/// Credential-free platform settings; allow-lists are intersected.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ChannelAccountOptions {
    /// Custom destination is writable only through the local socket.
    pub api_base: Option<String>,
    /// Remote writes permit only unprivileged loopback webhook listeners.
    pub listen: Option<String>,
    /// Public Ed25519 verification key for Discord inbound interactions.
    pub public_key: Option<String>,
    /// Telegram chat allow-list; at least one relevant allow-list is required.
    pub allowed_chats: Vec<String>,
    /// User allow-list intersected with channel and workspace restrictions.
    pub allowed_users: Vec<String>,
    /// Slack team allow-list.
    pub allowed_workspaces: Vec<String>,
    /// Slack or Discord channel allow-list.
    pub allowed_channels: Vec<String>,
    /// Discord guild allow-list.
    pub allowed_guilds: Vec<String>,
    /// Positive Telegram long-poll timeout; absent uses the configured default.
    pub poll_timeout_secs: Option<u32>,
}

/// Metadata-only account response; credential fields are intentionally absent.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub struct ChannelAccount {
    /// Stable per-kind id, independent of the account label.
    pub id: String,
    /// Supported service discriminator.
    pub kind: String,
    /// User-chosen label, never a credential suffix.
    pub label: String,
    /// Desired enablement; status separately reports effective availability.
    pub enabled: bool,
    /// Owner of newly created sessions; changing this does not reroute existing ones.
    pub agent_id: Option<String>,
    /// Resolved supervised owner, including inherited profile bindings; null when inactive.
    #[serde(default)]
    pub effective_agent_id: Option<String>,
    /// Non-secret platform options.
    pub options: ChannelAccountOptions,
    /// Whether the unlocked Vault contains the typed entry, or legacy references exist.
    pub secret_present: bool,
    /// `vault` for managed accounts, `legacy` for file-only environment/command sources.
    pub credential_source: String,
    /// Effective state of this account only.
    pub status: ChannelAccountStatus,
    /// Live application unavailable; never implies another identity is in use.
    pub restart_required: bool,
    /// Actual loopback webhook listener, null for polling or inactive accounts.
    pub receiver_address: Option<String>,
    /// Exact signed callback URL to paste into the platform; null for Telegram or legacy.
    #[serde(default)]
    pub webhook_url: Option<String>,
}
/// Read-scoped settings inventory, with no upstream credentials.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub struct ChannelAccountList {
    /// All configured identities, including disabled and normalized legacy accounts.
    pub accounts: Vec<ChannelAccount>,
}

/// Write-only input; Debug is masked and allocations are zeroized on drop.
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelCredentials {
    /// Bot credential supplied through a secure form or hidden stdin.
    pub token: String,
    /// Required for Slack inbound verification, forbidden for other services.
    #[serde(default)]
    pub signing_secret: Option<String>,
}
impl std::fmt::Debug for ChannelCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl Drop for ChannelCredentials {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.token.zeroize();
        if let Some(s) = self.signing_secret.as_mut() {
            s.zeroize();
        }
    }
}

/// Create one labelled identity with encrypted write-only credentials.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddChannelAccountRequest {
    /// Stable identifier chosen independently from the service's own account id.
    pub id: String,
    /// Human-readable account label.
    pub label: String,
    /// Defaults off until the owner explicitly enables the configured allow-lists.
    #[serde(default)]
    pub enabled: bool,
    /// Optional teammate assignment for new sessions.
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Non-secret settings editable entirely through the GUI.
    #[serde(default)]
    pub options: ChannelAccountOptions,
    /// Encrypted immediately, never echoed in the resulting account response.
    pub credentials: ChannelCredentials,
}
/// Partial account change; credential replacement rotates only this identity.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct UpdateChannelAccountRequest {
    /// Omitted keeps the existing label.
    pub label: Option<String>,
    /// Omitted keeps existing desired enablement.
    pub enabled: Option<bool>,
    /// Empty string removes the assignment; omitted leaves it unchanged.
    pub agent_id: Option<String>,
    /// Replace the complete non-secret options object when present.
    pub options: Option<ChannelAccountOptions>,
    /// Replace this identity's credentials, stopping old tasks before persistence.
    pub credentials: Option<ChannelCredentials>,
}
/// Result of a read-only platform identity check, with sanitized failure codes.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub struct ChannelAccountTest {
    /// Account tested; other accounts are unaffected.
    pub account_id: String,
    /// Service discriminator.
    pub kind: String,
    /// Whether the upstream identity check succeeded.
    pub ok: bool,
    /// Tested credential state, independent of desired enablement.
    pub status: ChannelAccountStatus,
    /// Allow-listed machine-readable code, never upstream error text.
    pub code: String,
    /// Whether enabled operation still needs a daemon listener or legacy runner restart.
    pub restart_required: bool,
}
