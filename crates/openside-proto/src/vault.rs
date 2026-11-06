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
    /// `policy_update`, `item_add`, `item_delete`, `export`, `clear`, `recovery_key`,
    /// `settings_update`, or the lifecycle steps `setup`, `unlock`, `lock` and `recover`.
    pub what: String,
    /// Item id, `unknown-site`, or `*` for listings and lifecycle steps.
    pub item: String,
    /// Normalized page origin, `invalid-origin`, or `-` when not page-bound.
    pub origin: String,
    /// `allowed`, `denied`, `filled`, `fill-failed`, `completed`, `failed`, `updated`,
    /// `registered`, `added`, `deleted`, `unlocked`, `locked`, `exported`, `cleared`,
    /// `generated` or `recovered`.
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
    /// `biometric_required` (a keyring vault with biometric unlock on waits for a person),
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
    /// Verify with the OS biometric prompt first (fingerprint, Touch ID). Required to unlock
    /// a keyring vault while `biometric_unlock` is on; local callers only.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub biometric: bool,
}

impl std::fmt::Debug for VaultUnlockRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultUnlockRequest")
            .field(
                "passphrase",
                &self.passphrase.as_ref().map(|_| "[REDACTED]"),
            )
            .field("biometric", &self.biometric)
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

/// Idle time after which the vault locks itself (`vault.auto_lock`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VaultAutoLock {
    /// As soon as the vault is idle (within seconds of its last use).
    Immediately,
    /// 1 minute without vault use.
    OneMinute,
    /// 5 minutes without vault use.
    FiveMinutes,
    /// 15 minutes without vault use.
    FifteenMinutes,
    /// 1 hour without vault use.
    OneHour,
    /// 4 hours without vault use.
    FourHours,
    /// 1 day without vault use.
    OneDay,
    /// 1 week without vault use (default).
    OneWeek,
    /// Only by hand or when the daemon stops.
    Never,
}

/// Where saved sign-ins may be filled (`vault.autofill`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VaultAutofillSettings {
    /// Fill saved sign-ins at all; off refuses every fill with `autofill_off`.
    pub enabled: bool,
    /// Click the submit button after filling when the caller names one.
    pub auto_submit: bool,
    /// Also fill plain `http:` pages; `localhost` always counts as secure. Off refuses with
    /// `insecure_origin`.
    pub allow_insecure: bool,
    /// Normalized domains never filled, subdomains included (`excluded_domain`).
    pub excluded_domains: Vec<String>,
}

/// Partial autofill update; absent fields keep their value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateVaultAutofillSettings {
    /// New `enabled`.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// New `auto_submit`.
    #[serde(default)]
    pub auto_submit: Option<bool>,
    /// New `allow_insecure`.
    #[serde(default)]
    pub allow_insecure: Option<bool>,
    /// Replacement list (domains or URLs; normalized to lowercase host names).
    #[serde(default)]
    pub excluded_domains: Option<Vec<String>>,
}

/// `GET /v1/vault/biometrics`: whether this computer can verify a person biometrically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VaultBiometrics {
    /// A sensor with an enrolled finger (or Touch ID) is usable now.
    pub available: bool,
    /// `fprintd` (Linux) or `local_authentication` (macOS) when available.
    pub mechanism: Option<String>,
    /// Why not: `unsupported_platform`, `no_sensor`, `not_enrolled`, `service_unavailable`,
    /// or `passphrase_vault` (biometric unlock applies to keyring vaults only).
    pub reason: Option<String>,
}

/// Recovery key state; never the key itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VaultRecoveryStatus {
    /// A recovery file exists next to the vault.
    pub configured: bool,
    /// It still opens the vault (false after the vault was re-keyed elsewhere; generate a new
    /// key).
    pub current: bool,
    /// When the key was generated.
    pub created_at: Option<DateTime<Utc>>,
}

/// `GET /v1/vault/settings`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VaultSettings {
    /// Idle time before the vault locks itself.
    pub auto_lock: VaultAutoLock,
    /// Unlocking a keyring vault requires a biometric check.
    pub biometric_unlock: bool,
    /// Whether biometric unlock can be turned on here.
    pub biometrics: VaultBiometrics,
    /// Autofill rules.
    pub autofill: VaultAutofillSettings,
    /// Policy for items without an override.
    pub default_policy: VaultPolicy,
    /// Incognito sessions never reach the vault. Always true; it cannot be turned off.
    pub disable_in_incognito: bool,
    /// Recovery key state.
    pub recovery: VaultRecoveryStatus,
}

/// `PATCH /v1/vault/settings`; absent fields keep their value. Loosening a setting (longer
/// auto-lock, biometric unlock off, autofill or auto-submit on, insecure pages allowed, fewer
/// excluded domains, a less strict default policy) needs the local socket or a person's
/// device; tightening is open to every caller with vault access. Turning biometric unlock on
/// needs the local socket and a passing check.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateVaultSettingsRequest {
    /// New idle time.
    #[serde(default)]
    pub auto_lock: Option<VaultAutoLock>,
    /// Require biometrics to unlock.
    #[serde(default)]
    pub biometric_unlock: Option<bool>,
    /// Autofill changes.
    #[serde(default)]
    pub autofill: Option<UpdateVaultAutofillSettings>,
    /// New default policy.
    #[serde(default)]
    pub default_policy: Option<VaultPolicy>,
    /// Only `true` is accepted.
    #[serde(default)]
    pub disable_in_incognito: Option<bool>,
}

/// A confirmed, freshly authenticated step: `POST /v1/vault/export` and
/// `POST /v1/vault/recovery-key`. `confirm` must be true. Fresh authentication is the
/// passphrase (passphrase vaults), a biometric check (`biometric`), or a person's unlock in
/// the last 5 minutes.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VaultConfirmRequest {
    /// The person confirmed this step.
    #[serde(default)]
    pub confirm: bool,
    /// Vault passphrase, re-entered.
    #[serde(default)]
    pub passphrase: Option<String>,
    /// Verify with the OS biometric prompt.
    #[serde(default)]
    pub biometric: bool,
}

impl std::fmt::Debug for VaultConfirmRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultConfirmRequest")
            .field("confirm", &self.confirm)
            .field(
                "passphrase",
                &self.passphrase.as_ref().map(|_| "[REDACTED]"),
            )
            .field("biometric", &self.biometric)
            .finish()
    }
}

/// `POST /v1/vault/export`: every saved sign-in and authenticator as CSV. Served with
/// `Cache-Control: no-store`; store it somewhere private and delete it after importing.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VaultExport {
    /// `csv`.
    pub format: String,
    /// Suggested file name.
    pub filename: String,
    /// `name,url,username,password,otpauth` rows with a header line.
    pub content: String,
    /// Items written.
    pub exported: u32,
    /// Items left out because CSV cannot carry them (passkey-only).
    pub skipped: u32,
}

impl std::fmt::Debug for VaultExport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultExport")
            .field("format", &self.format)
            .field("filename", &self.filename)
            .field("content", &"[REDACTED]")
            .field("exported", &self.exported)
            .field("skipped", &self.skipped)
            .finish()
    }
}

/// `POST /v1/vault/recovery-key`: the new key, shown once. Any earlier key stops working.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VaultRecoveryKey {
    /// 52 base32 characters in dash-separated groups of four.
    pub recovery_key: String,
    /// When it was generated.
    pub created_at: DateTime<Utc>,
}

impl std::fmt::Debug for VaultRecoveryKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultRecoveryKey")
            .field("recovery_key", &"[REDACTED]")
            .field("created_at", &self.created_at)
            .finish()
    }
}

/// `POST /v1/vault/recover`: open the vault with its recovery key and set a new unlock key.
/// A passphrase vault needs `passphrase` (the new one, at least 8 characters); a keyring
/// vault takes none and is re-wrapped with the keyring's key.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VaultRecoverRequest {
    /// The saved recovery key; case, spaces and dashes are ignored.
    pub recovery_key: String,
    /// New passphrase (passphrase vaults).
    #[serde(default)]
    pub passphrase: Option<String>,
}

impl std::fmt::Debug for VaultRecoverRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultRecoverRequest")
            .field("recovery_key", &"[REDACTED]")
            .field(
                "passphrase",
                &self.passphrase.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

/// `POST /v1/vault/clear` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VaultClearResult {
    /// Sign-ins, authenticators and passkeys deleted. Model keys, channel and MCP secrets
    /// stay.
    pub removed: u32,
}
