use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Vault access policy: when an item's secrets may be released to a fill/OTP/passkey tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VaultPolicy {
    /// Whenever the vault file is decrypted.
    Always,
    /// Only while the platform lock (keyring/passphrase session) is unlocked.
    WhileUnlocked,
    /// Never.
    Never,
}

/// Per-item policy change; `default` removes the override so the vault default applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VaultPolicyOverride {
    /// Override to `always`.
    Always,
    /// Override to `while_unlocked`.
    WhileUnlocked,
    /// Override to `never`.
    Never,
    /// Inherit the vault default.
    Default,
}

/// Value-free metadata of one vault item. Usernames, passwords, TOTP seeds and passkey
/// private keys are never part of the API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VaultItem {
    /// Item id used by `vault_*` tools (`site`).
    pub id: String,
    /// Allowed origins, normalized `scheme://[*.]host:port`.
    pub origins: Vec<String>,
    /// Per-item override; null inherits `default_policy`.
    pub policy: Option<VaultPolicy>,
    /// Policy actually enforced.
    pub effective_policy: VaultPolicy,
    /// Whether a TOTP seed is stored.
    pub has_totp: bool,
    /// RP IDs of stored passkeys.
    pub passkey_rp_ids: Vec<String>,
}

/// `GET /v1/vault/items`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VaultItemList {
    /// Whether the daemon has an unlocked vault; false lists no items.
    pub available: bool,
    /// Policy for items without an override (`vault.default_policy`).
    pub default_policy: VaultPolicy,
    /// Items, in vault order.
    pub items: Vec<VaultItem>,
}

/// `PATCH /v1/vault/items/{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateVaultItemRequest {
    /// New override.
    pub policy: VaultPolicyOverride,
}

/// One vault audit record. Never contains a secret value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VaultAuditRecord {
    /// When the decision was recorded (second precision).
    pub at: DateTime<Utc>,
    /// Caller label: `opensided` for tools and the daemon's own startup unlock, `device:<id>`
    /// for a paired device's API call, `local` for the Unix socket.
    pub who: String,
    /// `fill`, `otp`, `otp_register`, `passkey_register`, `passkey_login`, `login_save`, `get`,
    /// `policy_update`, `item_add`, `item_delete`, or the lifecycle steps `setup`, `unlock`
    /// and `lock`.
    pub what: String,
    /// Item id, `unknown-site`, or `*` for listings and lifecycle steps.
    pub item: String,
    /// Normalized page origin, `invalid-origin`, or `-` when not page-bound.
    pub origin: String,
    /// `allowed`, `denied`, `filled`, `fill-failed`, `completed`, `failed`, `updated`,
    /// `registered`, `added`, `deleted`, `unlocked` or `locked`.
    pub result: String,
    /// Agent session (task) id for tool calls.
    pub session_id: Option<String>,
    /// Model tool-call id for tool calls.
    pub tool_call_id: Option<String>,
    /// Denial reason (e.g. `origin_mismatch`, `policy_never`) or new policy.
    pub detail: Option<String>,
}

/// `GET /v1/vault/audit`, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VaultAuditList {
    /// Matching records.
    pub records: Vec<VaultAuditRecord>,
}

/// How the vault file is unlocked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VaultBackend {
    /// A passphrase a person chooses (or `vault.passphrase_cmd`); never stored.
    Passphrase,
    /// A random key in the Linux Secret Service keyring (GNOME Keyring, KWallet).
    SecretService,
    /// A random key in the macOS Keychain or Windows Credential Manager.
    Native,
}

/// Vault lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VaultState {
    /// No vault file yet; `POST /v1/vault/setup` (or adding the first item with a keyring
    /// backend) creates it.
    Uninitialized,
    /// The file exists but is not decrypted; `POST /v1/vault/unlock`.
    Locked,
    /// Decrypted; vault tools and item routes work.
    Unlocked,
    /// The configured method cannot open this vault here; see `reason` and `openside doctor`.
    Unavailable,
}

/// `GET /v1/vault`: lifecycle state without any value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VaultStatus {
    /// Current state.
    pub state: VaultState,
    /// Unlock method in effect; null when `unavailable` has no usable method.
    pub backend: Option<VaultBackend>,
    /// Why the vault is locked or unavailable: `keyring_unreachable`, `keyring_locked`,
    /// `keyring_key_mismatch`, `passphrase_cmd_failed` (from the daemon-start attempt),
    /// `unsupported_platform`, `format_mismatch`, `unreadable` or `probe_timeout`.
    pub reason: Option<String>,
    /// Policy for items without an override (`vault.default_policy`).
    pub default_policy: VaultPolicy,
}

/// `POST /v1/vault/setup` and `POST /v1/vault/unlock`. The passphrase is required for a
/// passphrase vault and must be absent for a keyring vault. It only travels to the daemon,
/// which never returns, logs or stores it.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VaultUnlockRequest {
    /// Vault passphrase (setup: at least 8 characters).
    #[serde(default)]
    pub passphrase: Option<String>,
}

impl std::fmt::Debug for VaultUnlockRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultUnlockRequest")
            .field(
                "passphrase",
                &self.passphrase.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

/// `POST /v1/vault/items`: a login (password, optional username and authenticator) or an
/// authenticator alone (`totp` without `password`). Values only travel to the daemon; the
/// response is value-free [`VaultItem`] metadata.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateVaultItemRequest {
    /// New item id, `[a-z0-9:_-]{1,64}`.
    pub id: String,
    /// Allowed origins (`https://host`, `https://*.host`, `http://localhost:port`), at least one.
    pub origins: Vec<String>,
    /// Username for a login.
    #[serde(default)]
    pub username: Option<String>,
    /// Password for a login.
    #[serde(default)]
    pub password: Option<String>,
    /// Authenticator: an `otpauth://totp/...` URI or a bare base32 setup key.
    #[serde(default)]
    pub totp: Option<String>,
    /// Per-item policy; null inherits the vault default.
    #[serde(default)]
    pub policy: Option<VaultPolicy>,
}

impl std::fmt::Debug for CreateVaultItemRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted = |value: &Option<String>| value.as_ref().map(|_| "[REDACTED]");
        f.debug_struct("CreateVaultItemRequest")
            .field("id", &self.id)
            .field("origins", &self.origins)
            .field("username", &redacted(&self.username))
            .field("password", &redacted(&self.password))
            .field("totp", &redacted(&self.totp))
            .field("policy", &self.policy)
            .finish()
    }
}
