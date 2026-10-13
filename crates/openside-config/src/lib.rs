//! Portable TOML configuration and diagnostics.
//!
//! Missing default configuration files use defaults. An explicitly selected file
//! must exist. Environment overrides are applied after TOML, except browser
//! discovery, which gives an explicitly configured executable precedence.
//! Credential tables stay opaque and are never included in debug output.

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
mod memory;
mod native_messaging;
mod paths;
mod profile;
mod remote;
mod runtime_env;
mod skills;
mod sync;
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
pub use memory::{EmbeddingConfig, MemoryConfig, MemoryRetentionSetting, EMBEDDING_MODEL_FILES};
pub use paths::Paths;
pub use profile::AgentProfile;
pub use remote::{
    expand_home, read_secret, CloudflareConfig, CloudflareTunnel, OtpConfig, RelayConfig,
    RemoteConfig, RemoteMode, ServiceConfig,
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
    /// Python sidecar launch settings.
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
    /// Optional static web UI served by the daemon.
    pub ui: UiConfig,
    /// Local memory: retention default, browsing history, embedding model.
    pub memory: MemoryConfig,
    /// Agent Skills: library location, project skills, other agents' skill directories.
    pub skills: SkillsConfig,
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
            ui: UiConfig::default(),
            memory: MemoryConfig::default(),
            skills: SkillsConfig::default(),
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
}

impl Default for TeamConfig {
    fn default() -> Self {
        Self { max_running: 3 }
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
            .finish()
    }
}

/// Sidecar launch settings; argv is executed directly, never through a shell.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AgentConfig {
    /// Explicit executable and arguments (`OPENSIDE_AGENT_COMMAND`, JSON array).
    pub command: Option<Vec<String>>,
    /// Python project directory (`OPENSIDE_AGENT_DIR`).
    pub dir: Option<PathBuf>,
    /// Deadline for receiving the sidecar ready response, in milliseconds.
    pub startup_timeout_ms: u64,
    /// Model retries after a denied or failed tool call before the run fails.
    pub tool_retries: u32,
    /// Conversation length that triggers summarizing compaction.
    pub compaction_max_messages: u32,
    /// Most recent messages kept verbatim by compaction; must be below the maximum.
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
            startup_timeout_ms: 30_000,
            tool_retries: 3,
            compaction_max_messages: 100,
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
    /// Fast model for the front agent; unset uses the session model.
    pub front: Option<String>,
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
            &["path", "passphrase_cmd", "unlock", "default_policy"],
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
                "startup_timeout_ms",
                "tool_retries",
                "compaction_max_messages",
                "compaction_keep_messages",
                "profile",
                "orchestration",
            ],
        ),
        ("team", &["max_running"]),
        ("models", &["default", "fallback", "front"]),
        ("credentials", &[]),
        (
            "browser",
            &["executable", "bundle_dir", "gateway", "extensions"],
        ),
        ("sandbox", &["required", "bwrap"]),
        ("policy", &["mode", "allow_origins", "deny_origins"]),
        ("network", &["allow"]),
        ("ui", &["dir"]),
        ("memory", &["default_retention", "history", "embedding"]),
        ("skills", &["dir", "project", "agents"]),
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
            &["mode", "public_url", "push_contact", "cloudflare", "relay"],
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
    if config.agent.startup_timeout_ms == 0 {
        return Err(error(
            path,
            "agent.startup_timeout_ms",
            "must be greater than zero",
        ));
    }
    if config.agent.compaction_keep_messages >= config.agent.compaction_max_messages {
        return Err(error(
            path,
            "agent.compaction_keep_messages",
            "must be less than agent.compaction_max_messages",
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
        ("daemon.data_dir", &config.daemon.data_dir),
        ("daemon.socket", &config.daemon.socket),
        ("vault.path", &config.vault.path),
        ("agent.dir", &config.agent.dir),
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
    if let Err((key, reason)) = config.skills.validate() {
        return Err(error(path, key, reason));
    }
    if let Err((key, reason)) = config.channels.validate() {
        return Err(error(path, key, reason));
    }
    if let Err((key, reason)) = config.sync.validate() {
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
