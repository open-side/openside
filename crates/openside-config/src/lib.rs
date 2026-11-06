//! Portable TOML configuration and diagnostics.
//!
//! Missing default configuration files use defaults. An explicitly selected file
//! must exist. Environment overrides are applied after TOML, except browser
//! discovery, which gives an explicitly configured executable precedence.
//! Credential tables stay opaque and are never included in debug output.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod browser_extensions;
mod channels;
/// Private local crash-report storage, shared by daemon and diagnostics.
pub mod crash_reports;
/// Typed storage registry and generated directory documentation.
pub mod data_layout;
mod discovery;
/// Non-mutating installation and configuration diagnostics.
pub mod doctor;
mod drm;
mod mcp;
mod memory;
mod native_messaging;
mod paths;
mod plugin_sources;
mod preferences;
mod profile;
mod remote;
mod runtime_env;
mod skills;
mod sync;
pub mod tailscale;
mod ui;

use std::ffi::OsString;
use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use channels::{
    valid_channel_id, ChannelAccountConfig, ChannelKind, ChannelOptions, ChannelsConfig,
    LegacyChannelSecret,
};
pub use discovery::{agent_command, discover_browser, find_on_path, AgentCommand, BrowserNotFound};
pub use mcp::{
    valid_mcp_secret_name, valid_mcp_server_id, valid_mcp_url, McpConfig, McpServerConfig,
    McpTransport, MAX_MCP_SERVERS,
};
pub use memory::{EmbeddingConfig, MemoryConfig, MemoryRetentionSetting, EMBEDDING_MODEL_FILES};
pub use paths::Paths;
pub use plugin_sources::{
    load_plugin_sources, normalized_origin, parse_plugin_source, plugin_dir, PluginAuth,
    PluginErrors, PluginLoad, PluginOutcome, PluginProbe, PluginProvider, PluginRule,
    PluginSessionHeader, PluginSource,
};
pub use preferences::{
    ConfirmationSetting, FollowUpSetting, NotificationKindSetting, PreferencesConfig,
    AGENT_TAB_CLEANUP_MINUTES, MAX_PREFERENCE_NAME_CHARS,
};
pub use profile::AgentProfile;
pub use remote::{
    expand_home, read_secret, CloudflareConfig, CloudflareTunnel, OtpConfig, RelayConfig,
    RemoteConfig, RemoteMode, ServiceConfig, TailscaleConfig,
};
pub use runtime_env::runtime_environment;
pub use skills::SkillsConfig;
pub use sync::SyncConfig;
pub use ui::UiConfig;

/// Complete configuration; unspecified sections use their documented defaults.
#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Daemon storage and transport settings.
    pub daemon: DaemonConfig,
    /// Optional encrypted vault and unlock-command settings.
    pub vault: VaultConfig,
    /// Human-step ticket lifetimes; every TTL is at least one second.
    pub tickets: TicketsConfig,
    /// Bun/TypeScript sidecar launch settings.
    pub agent: AgentConfig,
    /// Durable teammate scheduling; separate from ephemeral Task concurrency.
    pub team: TeamConfig,
    /// Model selection, without hardcoded model identifiers.
    pub models: ModelsConfig,
    /// Opaque credential-source settings interpreted only by the daemon's policy owner.
    #[serde(default = "empty_table")]
    #[schemars(with = "std::collections::BTreeMap<String, Value>")]
    pub credentials: Value,
    /// Chromium-compatible executable discovery settings.
    pub browser: BrowserConfig,
    /// Optional command sandbox settings.
    pub sandbox: SandboxConfig,
    /// Default session permission mode.
    pub policy: PolicyConfig,
    /// Outbound allow-list beyond loopback and configured endpoints.
    pub network: NetworkConfig,
    /// `websearch` backend: the agent browser's results page, or a search API with a Vault key.
    pub search: SearchConfig,
    /// Optional static web UI served by the daemon.
    pub ui: UiConfig,
    /// Local memory: retention default, browsing history, embedding model.
    pub memory: MemoryConfig,
    /// Agent Skills: library location, project skills, other agents' skill directories.
    pub skills: SkillsConfig,
    /// External MCP servers whose tools agents can call; values of their secrets live in the
    /// Vault.
    pub mcp: McpConfig,
    /// The person's Settings choices that the daemon applies live.
    pub preferences: PreferencesConfig,
    /// Chat-platform adapters (`openside-channels`): Telegram, Slack, Discord.
    pub channels: ChannelsConfig,
    /// End-to-end encrypted sync of settings, pinned tabs, vault items and tagged memories.
    pub sync: SyncConfig,
    /// Remote devices: public HTTPS URL for pairing links, push contact.
    pub remote: RemoteConfig,
    /// Forwarded SMS one-time codes for pending `secret_input` steps.
    pub otp: OtpConfig,
    /// Login-service behaviour (sleep inhibition while sessions run).
    pub service: ServiceConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            daemon: DaemonConfig::default(),
            vault: VaultConfig::default(),
            tickets: TicketsConfig::default(),
            agent: AgentConfig::default(),
            team: TeamConfig::default(),
            models: ModelsConfig::default(),
            credentials: empty_table(),
            browser: BrowserConfig::default(),
            sandbox: SandboxConfig::default(),
            policy: PolicyConfig::default(),
            network: NetworkConfig::default(),
            search: SearchConfig::default(),
            ui: UiConfig::default(),
            memory: MemoryConfig::default(),
            skills: SkillsConfig::default(),
            mcp: McpConfig::default(),
            preferences: PreferencesConfig::default(),
            channels: ChannelsConfig::default(),
            sync: SyncConfig::default(),
            remote: RemoteConfig::default(),
            otp: OtpConfig::default(),
            service: ServiceConfig::default(),
        }
    }
}

// Opaque credential settings may contain values supplied by a user.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("credentials", &"[redacted]")
            .finish_non_exhaustive()
    }
}

fn empty_table() -> Value {
    Value::Object(serde_json::Map::new())
}

/// Scheduler admission for independently running teammate home conversations.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct TeamConfig {
    /// Positive concurrency ceiling; additional handoffs remain durably queued.
    #[serde(deserialize_with = "positive_team_slots")]
    #[schemars(range(min = 1))]
    pub max_running: usize,
    /// Limits durable active teammates, excluding the chief.
    pub max_active_teammates: usize,
}

impl Default for TeamConfig {
    fn default() -> Self {
        Self {
            max_running: 3,
            max_active_teammates: 5,
        }
    }
}

fn positive_team_slots<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<usize, D::Error> {
    let value = usize::deserialize(deserializer)?;
    if value == 0 {
        Err(serde::de::Error::custom(
            "team.max_running must be positive",
        ))
    } else {
        Ok(value)
    }
}

/// Human-step expiration settings, in seconds, without environment overrides.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct TicketsConfig {
    /// Confirmation expires as a denied approval.
    #[serde(deserialize_with = "positive_seconds")]
    #[schemars(range(min = 1))]
    pub confirm_ttl_secs: u32,
    /// Secret-input expiration.
    #[serde(deserialize_with = "positive_seconds")]
    #[schemars(range(min = 1))]
    pub secret_input_ttl_secs: u32,
    /// Device-approval expiration.
    #[serde(deserialize_with = "positive_seconds")]
    #[schemars(range(min = 1))]
    pub device_approval_ttl_secs: u32,
    /// Non-secret information request expiration.
    #[serde(deserialize_with = "positive_seconds")]
    #[schemars(range(min = 1))]
    pub info_ttl_secs: u32,
    /// Takeover expiration.
    #[serde(deserialize_with = "positive_seconds")]
    #[schemars(range(min = 1))]
    pub takeover_ttl_secs: u32,
}
impl Default for TicketsConfig {
    fn default() -> Self {
        Self {
            confirm_ttl_secs: 600,
            secret_input_ttl_secs: 300,
            device_approval_ttl_secs: 1800,
            info_ttl_secs: 86400,
            takeover_ttl_secs: 1800,
        }
    }
}
fn positive_seconds<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let value = u32::deserialize(deserializer)?;
    if value == 0 {
        Err(serde::de::Error::custom(
            "ticket TTL must be at least one second",
        ))
    } else {
        Ok(value)
    }
}

/// Storage and daemon endpoints.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct DaemonConfig {
    /// Override the discovered data directory (`OPENSIDE_DATA_DIR`).
    pub data_dir: Option<PathBuf>,
    /// Override the Unix socket path (`OPENSIDE_SOCKET`).
    pub socket: Option<PathBuf>,
    /// Optional TCP listener (`OPENSIDE_LISTEN`); the daemon enforces loopback.
    pub listen: Option<SocketAddr>,
}

/// Vault settings; passphrase command output is never part of configuration.
#[derive(Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct VaultConfig {
    /// Encrypted vault file (`OPENSIDE_VAULT_PATH`); unset means `<data_dir>/vault.db`.
    pub path: Option<PathBuf>,
    /// Unlock command argv for a passphrase vault, executed without a shell at daemon
    /// start. Unset leaves a passphrase vault locked until a person unlocks it.
    ///
    /// Output must only unlock the vault, must be zeroized by the consumer,
    /// and must never be logged. Doctor never executes this command.
    pub passphrase_cmd: Option<Vec<String>>,
    /// How the vault file is unlocked; `auto` picks the operating system keyring.
    pub unlock: VaultUnlockSetting,
    /// Access policy for items without a per-item override.
    pub default_policy: VaultPolicySetting,
    /// Lock the vault after this long without vault use.
    pub auto_lock: VaultAutoLock,
    /// Require a biometric check (fprintd on Linux, Touch ID on macOS) before a keyring vault
    /// unlocks. Passphrase vaults ignore it.
    pub biometric_unlock: bool,
    /// Where saved sign-ins may be filled.
    pub autofill: VaultAutofillConfig,
}

/// Idle time after which the vault locks itself.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VaultAutoLock {
    /// Lock as soon as the vault is idle (within a few seconds of its last use).
    Immediately,
    /// 1 minute.
    OneMinute,
    /// 5 minutes.
    FiveMinutes,
    /// 15 minutes.
    FifteenMinutes,
    /// 1 hour.
    OneHour,
    /// 4 hours.
    FourHours,
    /// 1 day.
    OneDay,
    /// 1 week.
    #[default]
    OneWeek,
    /// Stay unlocked until locked by hand or the daemon stops.
    Never,
}

impl VaultAutoLock {
    /// The idle time, `None` for never.
    pub const fn idle(self) -> Option<std::time::Duration> {
        let minutes = match self {
            Self::Never => return None,
            Self::Immediately => 0,
            Self::OneMinute => 1,
            Self::FiveMinutes => 5,
            Self::FifteenMinutes => 15,
            Self::OneHour => 60,
            Self::FourHours => 4 * 60,
            Self::OneDay => 24 * 60,
            Self::OneWeek => 7 * 24 * 60,
        };
        Some(std::time::Duration::from_secs(minutes * 60))
    }
}

/// Autofill rules for saved sign-ins.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct VaultAutofillConfig {
    /// Fill saved sign-ins at all.
    pub enabled: bool,
    /// Submit the sign-in form after filling, when the caller names a submit button.
    pub auto_submit: bool,
    /// Also fill plain `http:` pages (`localhost` always counts as secure).
    pub allow_insecure: bool,
    /// Domains never filled, including their subdomains (`example.com`).
    pub excluded_domains: Vec<String>,
}

impl Default for VaultAutofillConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_submit: true,
            allow_insecure: false,
            excluded_domains: Vec::new(),
        }
    }
}

/// Upper bound on excluded autofill domains.
pub const MAX_EXCLUDED_DOMAINS: usize = 500;

impl VaultAutofillConfig {
    /// The offending key and a short reason.
    pub fn validate(&self) -> Result<(), (&'static str, &'static str)> {
        if self.excluded_domains.len() > MAX_EXCLUDED_DOMAINS {
            return Err(("vault.autofill.excluded_domains", "at most 500 domains"));
        }
        // Same acceptance as `openside_vault::normalize_domain`, which the daemon applies;
        // anything it would refuse must fail here rather than be dropped at runtime.
        let domain = |value: &str| {
            let value = value.trim();
            let value = value.strip_prefix("*.").unwrap_or(value);
            if value.is_empty() || value.len() > 253 {
                return false;
            }
            let Ok(url) = url::Url::parse(&if value.contains("://") {
                value.to_owned()
            } else {
                format!("https://{value}")
            }) else {
                return false;
            };
            let Some(url::Host::Domain(host)) = url.host() else {
                return false;
            };
            let host = host.trim_end_matches('.');
            url.username().is_empty()
                && url.password().is_none()
                && !host.is_empty()
                && (host.contains('.') || host.eq_ignore_ascii_case("localhost"))
                && host.split('.').all(|label| {
                    !label.is_empty()
                        && label.len() <= 63
                        && !label.starts_with('-')
                        && !label.ends_with('-')
                        && label
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                })
        };
        if !self.excluded_domains.iter().all(|value| domain(value)) {
            return Err((
                "vault.autofill.excluded_domains",
                "each entry must be a domain name such as example.com",
            ));
        }
        Ok(())
    }
}

impl VaultConfig {
    /// The vault file: `path`, or `vault.db` in the data directory.
    pub fn file(&self, data_dir: &Path) -> PathBuf {
        self.path
            .clone()
            .unwrap_or_else(|| data_layout::DataPath::Vault.path(data_dir))
    }
}

/// Vault unlock setting.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum VaultUnlockSetting {
    /// An existing file keeps the method it was created with; a new vault uses the
    /// platform keyring when one is reachable, otherwise a passphrase a person chooses.
    #[default]
    Auto,
    /// Passphrase (Argon2id-derived key, file format 1), from `passphrase_cmd` or a person.
    Passphrase,
    /// Freedesktop Secret Service (libsecret: GNOME Keyring, KWallet) on the session D-Bus.
    SecretService,
    /// macOS Keychain / Windows Credential Manager.
    Native,
}

/// The unlock method in effect for the vault file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VaultBackend {
    /// Passphrase vault (format 1).
    Passphrase,
    /// Secret Service keyring (format 2, Linux).
    SecretService,
    /// Keychain / Credential Manager (format 2, macOS and Windows).
    Native,
}

impl VaultBackend {
    /// The keyring backend of the operating system this binary was built for.
    pub const fn platform_keyring() -> Option<Self> {
        if cfg!(target_os = "linux") {
            Some(Self::SecretService)
        } else if cfg!(any(target_os = "macos", windows)) {
            Some(Self::Native)
        } else {
            None
        }
    }

    /// Whether the wrapping key lives in an operating-system keyring (file format 2).
    pub const fn is_keyring(self) -> bool {
        !matches!(self, Self::Passphrase)
    }

    /// Human-readable store name for diagnostics.
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Passphrase => "a passphrase",
            Self::SecretService => "the Secret Service keyring (GNOME Keyring, KWallet)",
            Self::Native if cfg!(windows) => "Windows Credential Manager",
            Self::Native => "the macOS Keychain",
        }
    }
}

/// Unlock format of an existing vault file, from its header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoredVault {
    /// No file yet.
    Missing,
    /// Format 1.
    Passphrase,
    /// Format 2.
    Keyring,
}

/// Why no unlock method applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VaultResolveError {
    /// The configured keyring backend does not exist on this operating system.
    UnsupportedPlatform,
    /// The file was created with another method than `vault.unlock` names.
    FormatMismatch,
}

impl VaultResolveError {
    /// Stable machine-readable code.
    pub const fn wire(self) -> &'static str {
        match self {
            Self::UnsupportedPlatform => "unsupported_platform",
            Self::FormatMismatch => "format_mismatch",
        }
    }
}

impl VaultUnlockSetting {
    /// The unlock method for a vault whose file is `stored`. `keyring_reachable` only
    /// matters for `auto` without a file: a new vault never falls back silently to an
    /// unencrypted key; without a keyring it needs a passphrase from a person.
    pub fn resolve(
        self,
        stored: StoredVault,
        keyring_reachable: bool,
    ) -> Result<VaultBackend, VaultResolveError> {
        let keyring = VaultBackend::platform_keyring();
        let backend = match self {
            Self::Auto => match stored {
                StoredVault::Passphrase => VaultBackend::Passphrase,
                StoredVault::Keyring => keyring.ok_or(VaultResolveError::UnsupportedPlatform)?,
                StoredVault::Missing => match keyring {
                    Some(keyring) if keyring_reachable => keyring,
                    _ => VaultBackend::Passphrase,
                },
            },
            Self::Passphrase => VaultBackend::Passphrase,
            Self::SecretService if keyring == Some(VaultBackend::SecretService) => {
                VaultBackend::SecretService
            }
            Self::Native if keyring == Some(VaultBackend::Native) => VaultBackend::Native,
            Self::SecretService | Self::Native => {
                return Err(VaultResolveError::UnsupportedPlatform)
            }
        };
        match (stored, backend.is_keyring()) {
            (StoredVault::Passphrase, true) | (StoredVault::Keyring, false) => {
                Err(VaultResolveError::FormatMismatch)
            }
            _ => Ok(backend),
        }
    }
}

/// Vault access policy names, matching the public API.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VaultPolicySetting {
    /// Release whenever the vault file is decrypted.
    Always,
    /// Release only while the platform lock is unlocked.
    #[default]
    WhileUnlocked,
    /// Never release.
    Never,
}

impl fmt::Debug for VaultConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VaultConfig")
            .field("path", &self.path)
            .field(
                "passphrase_cmd",
                &self.passphrase_cmd.as_ref().map(|_| "[redacted]"),
            )
            .field("unlock", &self.unlock)
            .field("default_policy", &self.default_policy)
            .field("auto_lock", &self.auto_lock)
            .field("biometric_unlock", &self.biometric_unlock)
            .field("autofill", &self.autofill)
            .finish()
    }
}

/// Sidecar launch settings; argv is executed directly, never through a shell.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AgentConfig {
    /// Explicit executable and arguments (`OPENSIDE_AGENT_COMMAND`, JSON array).
    pub command: Option<Vec<String>>,
    /// Sidecar project directory (`OPENSIDE_AGENT_DIR`), used as given; only the automatic
    /// search requires a `package.json` named `openside-agent`.
    pub dir: Option<PathBuf>,
    /// Bun executable for the default launch; `bun` on PATH when unset (`OPENSIDE_BUN`).
    pub bun: Option<PathBuf>,
    /// Deadline for receiving the sidecar ready response, in milliseconds.
    pub startup_timeout_ms: u64,
    /// Model retries after a denied or failed tool call before the run fails.
    pub tool_retries: u32,
    /// Optional legacy message-count trigger; size budgeting is the default.
    pub compaction_max_messages: Option<u32>,
    /// History budget estimated by the sidecar as serialized UTF-8 bytes / 3.
    pub compaction_max_tokens: u32,
    /// Recent messages retained verbatim; tool-call boundaries may extend this tail.
    pub compaction_keep_messages: u32,
    /// Tool and system-prompt profile: `full`, `browser-agent` or `ultrabrowse`.
    pub profile: AgentProfile,
    /// `front` (default): a fast front agent answers every message and runs long work as
    /// background Tasks; `single`: one agent does everything in the conversation itself.
    pub orchestration: Orchestration,
}

/// How user messages reach agents (`docs/specs/sidecar-protocol.md` §11).
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Orchestration {
    /// Front agent plus background Tasks.
    #[default]
    Front,
    /// One agent per session, long tools block its conversation.
    Single,
}

impl Orchestration {
    /// Configuration and wire name (`open.config.agent.orchestration`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Front => "front",
            Self::Single => "single",
        }
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            command: None,
            dir: None,
            bun: None,
            startup_timeout_ms: 30_000,
            tool_retries: 3,
            compaction_max_messages: None,
            compaction_max_tokens: 64_000,
            compaction_keep_messages: 20,
            profile: AgentProfile::Full,
            orchestration: Orchestration::Front,
        }
    }
}

impl AgentConfig {
    /// Sidecar runtime options sent as `open.config.agent`; launch settings stay in Rust.
    pub fn runtime_options(&self) -> serde_json::Value {
        serde_json::json!({
            "tool_retries": self.tool_retries,
            "compaction_max_messages": self.compaction_max_messages,
            "compaction_max_tokens": self.compaction_max_tokens,
            "compaction_keep_messages": self.compaction_keep_messages,
            "profile": self.profile.as_str(),
            "orchestration": self.orchestration.as_str(),
        })
    }
}

/// Models are configured by the user and resolved by the sidecar.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ModelsConfig {
    /// Default model; an unset value requires explicit session model selection.
    pub default: Option<String>,
    /// Ordered fallback chain.
    pub fallback: Vec<String>,
    /// Fast role: front agent, titles, summaries and memory extraction; unset uses the session model.
    pub front: Option<String>,
    /// Standard role: subagents and Tasks started without an explicit model; unset uses the session model.
    pub standard: Option<String>,
    /// Deep role: Tasks the agent marks as hard reasoning, planning, judging or synthesis; unset uses the session model.
    pub deep: Option<String>,
    /// Visual role: requests whose newest input carries an image (attachments, screenshots); unset uses the requesting agent's model.
    pub visual: Option<String>,
    /// Image generation model for the `generate_image` tool; unset picks the best available
    /// image model automatically.
    pub image: Option<String>,
    /// Image generation turned off: `generate_image` stays hidden even when an image model is
    /// available. Kept apart from `image` so "off" survives restarts without a sentinel model id.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub image_off: bool,
}

impl ModelsConfig {
    /// Every configured model id: the role models, then the fallback chain.
    pub fn selected(&self) -> impl Iterator<Item = &String> {
        self.default
            .iter()
            .chain(&self.front)
            .chain(&self.standard)
            .chain(&self.deep)
            .chain(&self.visual)
            .chain(&self.image)
            .chain(&self.fallback)
    }
}

/// Discovery prefers an explicit executable over `OPENSIDE_BROWSER`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct BrowserConfig {
    /// Configured Chromium-compatible executable.
    pub executable: Option<PathBuf>,
    /// Bundle directory containing `chrome` or `chromium`.
    pub bundle_dir: Option<PathBuf>,
    /// Opt-in unpacked extensions; relative paths resolve beside the configuration file.
    pub extensions: Vec<PathBuf>,
    /// Patched-browser gateway use: `auto` or `off`.
    pub gateway: BrowserGatewaySetting,
    /// UI language for the `openside-browser` launcher, e.g. `ko`; unset follows the OS locale.
    pub language: Option<String>,
}

/// Whether the daemon uses the patched browser's gateway.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BrowserGatewaySetting {
    /// Use the gateway when the launched browser publishes one; otherwise CDP only.
    #[default]
    Auto,
    /// CDP only, even with the patched browser.
    Off,
}

/// Bubblewrap is optional unless explicitly required.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct SandboxConfig {
    /// Missing bubblewrap disables shell tools when true.
    pub required: bool,
    /// Explicit bubblewrap executable; otherwise discover it on PATH.
    pub bwrap: Option<PathBuf>,
}

/// Default permission mode, matching the public API's serialized names.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PolicyModeSetting {
    /// Inspect only, without mutations.
    ReadOnly,
    /// Ask before working outside approved roots.
    #[default]
    Guard,
    /// Full access; sensitive operations still require approval.
    Full,
}

/// Session policy defaults.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PolicyConfig {
    /// Default mode: `guard`.
    pub mode: PolicyModeSetting,
    /// When non-empty, browser/vault/sensitive actions may only target these origin globs.
    pub allow_origins: Vec<String>,
    /// Origin globs always denied for browser/vault/sensitive actions.
    pub deny_origins: Vec<String>,
}

/// Entries use `openside_policy::Egress` syntax (`host`, `*.domain`, `host:port`,
/// `scheme://host[:port]`). Never widens the bash sandbox, which has no network.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkConfig {
    /// Extra outbound destinations; loopback is always allowed.
    pub allow: Vec<String>,
}

/// Where `websearch` gets results. API providers take their key from the Vault item whose
/// origins name the API's origin; without one (or in incognito) the browser is used.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct SearchConfig {
    /// `browser` (default), `brave` or `tavily`.
    pub provider: SearchProvider,
    /// Results-page URL template containing `{query}` for the browser backend; unset uses
    /// the browser's default engine when it is a known one, else DuckDuckGo.
    pub engine: Option<String>,
    /// Endpoint override for the API provider (defaults to the provider's public API).
    pub api_url: Option<String>,
}

/// Search backend.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SearchProvider {
    /// The search engine's results page, read in a background agent tab.
    #[default]
    Browser,
    /// Brave Search API (`X-Subscription-Token`).
    Brave,
    /// Tavily Search API (bearer token).
    Tavily,
}

impl SearchConfig {
    fn validate(&self) -> Result<(), (&'static str, &'static str)> {
        let web = |value: &str| {
            url::Url::parse(&value.replace("{query}", "q")).is_ok_and(|url| {
                matches!(url.scheme(), "http" | "https")
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
            })
        };
        if self
            .engine
            .as_deref()
            .is_some_and(|engine| !engine.contains("{query}") || !web(engine))
        {
            return Err((
                "search.engine",
                "use an http(s) results-page URL containing {query}",
            ));
        }
        if self.api_url.as_deref().is_some_and(|api| !web(api)) {
            return Err(("search.api_url", "use an http(s) URL without credentials"));
        }
        Ok(())
    }
}

/// Loaded configuration together with resolved application locations.
#[derive(Clone, Debug)]
pub struct Loaded {
    /// Parsed configuration after environment overrides.
    pub config: Config,
    /// Resolved default paths, with daemon data-dir override applied.
    pub paths: Paths,
    /// Existing configuration file used, or none when defaults were loaded.
    pub source: Option<PathBuf>,
    /// Non-fatal configuration notices.
    pub warnings: Vec<String>,
}

impl Loaded {
    /// Daemon Unix socket: `daemon.socket`/`OPENSIDE_SOCKET`, else `<runtime_dir>/opensided.sock`.
    /// The daemon and the CLI both use this so their defaults cannot drift.
    pub fn socket_path(&self) -> PathBuf {
        self.config
            .daemon
            .socket
            .clone()
            .unwrap_or_else(|| self.paths.runtime_dir.join("opensided.sock"))
    }
}

/// A configuration error exposes the file and key, never raw input values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError {
    /// File selected for loading, including for environment override errors.
    pub path: PathBuf,
    /// Invalid key, or `$` for a file-level error.
    pub key: String,
    /// Sanitized reason and remediation.
    pub message: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}: {}", self.path.display(), self.key, self.message)
    }
}

impl std::error::Error for ConfigError {}

fn error(path: &Path, key: impl Into<String>, message: impl Into<String>) -> ConfigError {
    ConfigError {
        path: path.to_owned(),
        key: key.into(),
        message: message.into(),
    }
}

fn env_value(env: &impl Fn(&str) -> Option<OsString>, key: &str) -> Option<OsString> {
    env(key).filter(|value| !value.is_empty())
}

/// Load explicit or discovered TOML and then apply the documented env overrides.
pub fn load(explicit: Option<&Path>) -> Result<Loaded, ConfigError> {
    load_with(explicit, Paths::discover(), &|key| std::env::var_os(key))
}

fn parse_config(path: &Path, text: &str) -> Result<Config, ConfigError> {
    let value: toml::Value = toml::from_str(text).map_err(|_| {
        error(
            path,
            "$",
            "invalid TOML syntax; check the configuration file",
        )
    })?;
    let sections: &[(&str, &[&str])] = &[
        ("daemon", &["data_dir", "socket", "listen"]),
        (
            "vault",
            &[
                "path",
                "passphrase_cmd",
                "unlock",
                "default_policy",
                "auto_lock",
                "biometric_unlock",
                "autofill",
            ],
        ),
        (
            "tickets",
            &[
                "confirm_ttl_secs",
                "secret_input_ttl_secs",
                "device_approval_ttl_secs",
                "info_ttl_secs",
                "takeover_ttl_secs",
            ],
        ),
        (
            "agent",
            &[
                "command",
                "dir",
                "bun",
                "startup_timeout_ms",
                "tool_retries",
                "compaction_max_messages",
                "compaction_max_tokens",
                "compaction_keep_messages",
                "profile",
                "orchestration",
            ],
        ),
        ("team", &["max_running", "max_active_teammates"]),
        (
            "models",
            &[
                "default",
                "fallback",
                "front",
                "standard",
                "deep",
                "visual",
                "image",
                "image_off",
            ],
        ),
        ("credentials", &[]),
        (
            "browser",
            &[
                "executable",
                "bundle_dir",
                "gateway",
                "extensions",
                "language",
            ],
        ),
        ("sandbox", &["required", "bwrap"]),
        ("policy", &["mode", "allow_origins", "deny_origins"]),
        ("network", &["allow"]),
        ("search", &["provider", "engine", "api_url"]),
        ("ui", &["dir"]),
        ("memory", &["default_retention", "history", "embedding"]),
        ("skills", &["dir", "project", "agents"]),
        ("mcp", &["servers"]),
        (
            "preferences",
            &[
                "name",
                "notifications",
                "completion_sound",
                "follow_up",
                "confirmation",
                "agent_tab_cleanup_minutes",
                "save_sessions",
                "mcp_server",
                "computer_use",
            ],
        ),
        (
            "sync",
            &[
                "webdav_url",
                "webdav_username",
                "webdav_password_cmd",
                "dir",
                "passphrase_cmd",
                "settings",
                "memory_tags",
            ],
        ),
        (
            "channels",
            &[
                "api_url",
                "token_file",
                "token_env",
                "stt_cmd",
                "stt_timeout_secs",
                "max_audio_bytes",
                "edit_interval_ms",
                "state_dir",
                "accounts",
                "telegram",
                "slack",
                "discord",
            ],
        ),
        (
            "remote",
            &[
                "enabled",
                "mode",
                "public_url",
                "push_contact",
                "tailscale",
                "cloudflare",
                "relay",
            ],
        ),
        ("otp", &["senders", "sites", "ttl_secs"]),
        ("service", &["inhibit_sleep", "inhibit_cmd"]),
    ];
    if let Some(table) = value.as_table() {
        for (section, value) in table {
            let Some((_, keys)) = sections.iter().find(|(known, _)| known == section) else {
                return Err(error(path, section, "unknown configuration section"));
            };
            if let Some(table) = value.as_table() {
                if section != "credentials" {
                    for key in table.keys() {
                        if !keys.contains(&key.as_str()) {
                            return Err(error(
                                path,
                                format!("{section}.{key}"),
                                "unknown configuration key",
                            ));
                        }
                    }
                }
            } else {
                return Err(error(path, section, "expected a TOML table"));
            }
        }
    }
    let mut config: Config = serde_path_to_error::deserialize(value).map_err(|err| {
        error(
            path,
            err.path().to_string(),
            "invalid value type or unsupported option",
        )
    })?;
    browser_extensions::resolve(&mut config.browser.extensions, path)?;
    Ok(config)
}

fn load_with(
    explicit: Option<&Path>,
    mut paths: Paths,
    env: &impl Fn(&str) -> Option<OsString>,
) -> Result<Loaded, ConfigError> {
    let selected = explicit
        .map(Path::to_owned)
        .unwrap_or_else(|| paths.config_file.clone());
    let required = explicit.is_some() || env_value(env, "OPENSIDE_CONFIG").is_some();
    paths.config_file = selected.clone();
    let (mut config, source) = match std::fs::read_to_string(&selected) {
        Ok(text) => (parse_config(&selected, &text)?, Some(selected.clone())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound && !required => {
            (Config::default(), None)
        }
        Err(err) => {
            let reason = if err.kind() == std::io::ErrorKind::NotFound {
                "explicit configuration file does not exist"
            } else {
                "cannot read configuration file; check permissions and UTF-8 encoding"
            };
            return Err(error(&selected, "$", reason));
        }
    };
    for (env_key, target) in [
        ("OPENSIDE_DATA_DIR", &mut config.daemon.data_dir),
        ("OPENSIDE_SOCKET", &mut config.daemon.socket),
        ("OPENSIDE_AGENT_DIR", &mut config.agent.dir),
        ("OPENSIDE_BUN", &mut config.agent.bun),
        ("OPENSIDE_VAULT_PATH", &mut config.vault.path),
        ("OPENSIDE_UI_DIR", &mut config.ui.dir),
    ] {
        if let Some(value) = env_value(env, env_key) {
            *target = Some(PathBuf::from(value));
        }
    }
    if let Some(value) = env_value(env, "OPENSIDE_LISTEN") {
        config.daemon.listen = Some(
            value
                .to_str()
                .and_then(|value| value.parse().ok())
                .ok_or_else(|| {
                    error(
                        &selected,
                        "daemon.listen",
                        "OPENSIDE_LISTEN must be an IP:port address",
                    )
                })?,
        );
    }
    if let Some(value) = env_value(env, "OPENSIDE_AGENT_COMMAND") {
        config.agent.command = Some(
            value
                .to_str()
                .and_then(|value| serde_json::from_str(value).ok())
                .ok_or_else(|| {
                    error(
                        &selected,
                        "agent.command",
                        "OPENSIDE_AGENT_COMMAND must be a JSON array of strings",
                    )
                })?,
        );
    }
    if let Some(value) = env_value(env, "OPENSIDE_MODEL") {
        config.models.default = Some(
            value
                .into_string()
                .map_err(|_| error(&selected, "models.default", "OPENSIDE_MODEL must be UTF-8"))?,
        );
    }
    validate(&config, &selected)?;
    if let Some(dir) = &config.daemon.data_dir {
        paths.data_dir = dir.clone();
    }
    Ok(Loaded {
        config,
        paths,
        source,
        warnings: Vec::new(),
    })
}

fn validate(config: &Config, path: &Path) -> Result<(), ConfigError> {
    for (key, argv) in [
        ("agent.command", &config.agent.command),
        ("vault.passphrase_cmd", &config.vault.passphrase_cmd),
    ] {
        if let Some(argv) = argv {
            if argv.is_empty()
                || argv[0].trim().is_empty()
                || argv.iter().any(|arg| arg.contains('\0'))
            {
                return Err(error(
                    path,
                    key,
                    "provide a non-empty executable and arguments without NUL bytes",
                ));
            }
        }
    }
    if let Err((key, reason)) = config.vault.autofill.validate() {
        return Err(error(path, key, reason));
    }
    if config.agent.startup_timeout_ms == 0 {
        return Err(error(
            path,
            "agent.startup_timeout_ms",
            "must be greater than zero",
        ));
    }
    if config.agent.compaction_keep_messages < 2
        || config
            .agent
            .compaction_max_messages
            .is_some_and(|maximum| config.agent.compaction_keep_messages >= maximum)
    {
        return Err(error(
            path,
            "agent.compaction_keep_messages",
            "must be at least two and less than compaction_max_messages when configured",
        ));
    }
    if config.agent.compaction_max_tokens == 0 {
        return Err(error(
            path,
            "agent.compaction_max_tokens",
            "must be greater than zero",
        ));
    }
    if config
        .models
        .default
        .as_ref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(error(
            path,
            "models.default",
            "must not be empty when configured",
        ));
    }
    if config
        .models
        .fallback
        .iter()
        .any(|value| value.trim().is_empty())
    {
        return Err(error(
            path,
            "models.fallback",
            "model identifiers must not be empty",
        ));
    }
    for (key, value) in [
        ("models.standard", &config.models.standard),
        ("models.deep", &config.models.deep),
        ("models.visual", &config.models.visual),
        ("models.image", &config.models.image),
    ] {
        if value
            .as_ref()
            .is_some_and(|value| !value.contains(':') || value.trim().is_empty())
        {
            return Err(error(
                path,
                key,
                "must be an explicit provider:model identifier when configured",
            ));
        }
    }
    for (key, value) in [
        ("daemon.data_dir", &config.daemon.data_dir),
        ("daemon.socket", &config.daemon.socket),
        ("vault.path", &config.vault.path),
        ("agent.dir", &config.agent.dir),
        ("agent.bun", &config.agent.bun),
        ("browser.executable", &config.browser.executable),
        ("browser.bundle_dir", &config.browser.bundle_dir),
        ("sandbox.bwrap", &config.sandbox.bwrap),
        (
            "memory.embedding.model_dir",
            &config.memory.embedding.model_dir,
        ),
    ] {
        if value
            .as_ref()
            .is_some_and(|value| value.as_os_str().is_empty())
        {
            return Err(error(path, key, "path must not be empty"));
        }
    }
    if let Err(reason) = browser_extensions::validate(&config.browser.extensions) {
        return Err(error(path, "browser.extensions", reason));
    }
    if config
        .browser
        .language
        .as_deref()
        .is_some_and(|tag| !is_language_tag(tag))
    {
        return Err(error(
            path,
            "browser.language",
            "use a language code such as ko, en or pt-BR",
        ));
    }
    if let Err((key, reason)) = config.skills.validate() {
        return Err(error(path, key, reason));
    }
    if let Err((key, reason)) = config.mcp.validate() {
        return Err(error(path, key, reason));
    }
    if let Err((key, reason)) = config.preferences.validate() {
        return Err(error(path, key, reason));
    }
    if let Err((key, reason)) = config.channels.validate() {
        return Err(error(path, key, reason));
    }
    if let Err((key, reason)) = config.sync.validate() {
        return Err(error(path, key, reason));
    }
    if let Err((key, reason)) = config.search.validate() {
        return Err(error(path, key, reason));
    }
    if let Err((key, reason)) = remote::validate(config) {
        return Err(error(path, key, reason));
    }
    if !config.credentials.is_object() {
        return Err(error(
            path,
            "credentials",
            "expected an opaque credential-source table",
        ));
    }
    Ok(())
}

/// `ll`, `lll`, optionally followed by `-` or `_` and a 2–4 letter region or script (`pt-BR`,
/// `zh_Hant`). The launcher exports it as `LANGUAGE`, so nothing else may get through.
fn is_language_tag(tag: &str) -> bool {
    let (language, rest) = match tag.find(['-', '_']) {
        Some(at) => (&tag[..at], Some(&tag[at + 1..])),
        None => (tag, None),
    };
    (2..=3).contains(&language.len())
        && language.bytes().all(|byte| byte.is_ascii_lowercase())
        && rest.map_or(true, |region| {
            (2..=4).contains(&region.len()) && region.bytes().all(|byte| byte.is_ascii_alphabetic())
        })
}

/// Parse and validate a configuration document exactly as [`load`] does for the file,
/// without environment overrides (used by the settings API before writing the file).
pub fn parse_document(path: &Path, text: &str) -> Result<Config, ConfigError> {
    let config = parse_config(path, text)?;
    validate(&config, path)?;
    Ok(config)
}

/// Generate the JSON Schema used to document every configuration key.
pub fn config_schema() -> Value {
    schemars::schema_for!(Config).to_value()
}

/// A portable, commented example covering every known configuration key.
///
/// Optional model identifiers and credential values are deliberately omitted.
pub fn example_toml() -> String {
    include_str!("example.toml").to_owned()
}

#[cfg(test)]
mod tests;
