use std::{collections::HashSet, net::SocketAddr, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

/// Credential-free adapter settings; account secrets belong to the local Vault.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ChannelsConfig {
    /// Public daemon origin for the standalone legacy runner.
    pub api_url: Option<String>,
    /// A private file overrides the environment-based daemon token.
    pub token_file: Option<PathBuf>,
    /// Environment variable name, never its bearer-token value.
    pub token_env: String,
    /// Local-only speech-to-text executable; GUI writes cannot change it.
    pub stt_cmd: Option<Vec<String>>,
    /// Bound execution time for untrusted audio transcription.
    pub stt_timeout_secs: u32,
    /// Bound attachment storage before transcription begins.
    pub max_audio_bytes: u64,
    /// Throttle edits to respect platform rate limits.
    pub edit_interval_ms: u64,
    /// Account state root; defaults to the daemon's private data directory.
    pub state_dir: Option<PathBuf>,
    /// Independent identities, including normalized legacy tables.
    pub accounts: Vec<ChannelAccountConfig>,
}

impl Default for ChannelsConfig {
    fn default() -> Self {
        Self {
            api_url: None,
            token_file: None,
            token_env: "OPENSIDE_TOKEN".into(),
            stt_cmd: None,
            stt_timeout_secs: 120,
            max_audio_bytes: 25 * 1024 * 1024,
            edit_interval_ms: 1000,
            state_dir: None,
            accounts: Vec::new(),
        }
    }
}

/// Supported inbound messaging services; accounts never fall back across identities.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    /// Long-polling bot identity.
    Telegram,
    /// Signed workspace events and interactive replies.
    Slack,
    /// Signed application interactions.
    Discord,
}

impl ChannelKind {
    /// Stable discriminator used by config, API paths and Vault metadata.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Telegram => "telegram",
            Self::Slack => "slack",
            Self::Discord => "discord",
        }
    }
    /// Trusted default credential destination for remotely managed accounts.
    pub fn api_base(self) -> &'static str {
        match self {
            Self::Telegram => "https://api.telegram.org",
            Self::Slack => "https://slack.com/api",
            Self::Discord => "https://discord.com/api/v10",
        }
    }
}

/// One platform identity, with no credential values.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChannelAccountConfig {
    /// Stable per-kind identifier, also used to isolate the thread store.
    pub id: String,
    /// Determines protocol and credential destination.
    pub kind: ChannelKind,
    /// User-provided account label; not an upstream identity.
    pub label: String,
    /// Disabled accounts retain settings but own no adapter tasks.
    #[serde(default)]
    pub enabled: bool,
    /// Agent for newly created channel sessions; existing sessions keep their owner.
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Typed Vault entry identifier, never a secret value.
    #[serde(default)]
    pub secret_id: Option<String>,
    /// Non-secret routing and platform verification settings.
    #[serde(default)]
    pub options: ChannelOptions,
    /// Compatibility input only; GUI-created accounts always use secret_id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_secret: Option<LegacyChannelSecret>,
}

/// Public account settings; nonempty allow-lists are intersected, never unioned.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ChannelOptions {
    /// Local-socket-only override for test servers or explicitly trusted deployments.
    pub api_base: Option<String>,
    /// Webhook listener; managed defaults bind an ephemeral loopback port.
    pub listen: Option<SocketAddr>,
    /// Public Ed25519 key authenticating Discord interactions.
    pub public_key: Option<String>,
    /// Telegram chat restriction; empty means no additional chat constraint.
    #[serde(deserialize_with = "id_list")]
    pub allowed_chats: Vec<String>,
    /// User restriction intersected with all other configured allow-lists.
    #[serde(deserialize_with = "id_list")]
    pub allowed_users: Vec<String>,
    /// Slack team restriction; required only if no other allow-list is set.
    #[serde(deserialize_with = "id_list")]
    pub allowed_workspaces: Vec<String>,
    /// Slack or Discord channel restriction.
    #[serde(deserialize_with = "id_list")]
    pub allowed_channels: Vec<String>,
    /// Discord guild restriction.
    #[serde(deserialize_with = "id_list")]
    pub allowed_guilds: Vec<String>,
    /// Positive Telegram long-poll deadline, in seconds.
    pub poll_timeout_secs: u32,
}

impl Default for ChannelOptions {
    fn default() -> Self {
        Self {
            api_base: None,
            listen: None,
            public_key: None,
            allowed_chats: Vec::new(),
            allowed_users: Vec::new(),
            allowed_workspaces: Vec::new(),
            allowed_channels: Vec::new(),
            allowed_guilds: Vec::new(),
            poll_timeout_secs: 30,
        }
    }
}

/// Compatibility-only references for existing files; never offered by the GUI.
#[derive(Clone, Deserialize, Serialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct LegacyChannelSecret {
    /// Environment variable name containing the platform token.
    pub token_env: Option<String>,
    /// Existing local argv whose stdout supplies the token.
    pub token_cmd: Option<Vec<String>>,
    /// Environment variable name containing the Slack signing secret.
    pub signing_secret_env: Option<String>,
    /// Existing local argv whose stdout supplies the Slack signing secret.
    pub signing_secret_cmd: Option<Vec<String>>,
}
impl std::fmt::Debug for LegacyChannelSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

// Legacy tables are parsed only at the file boundary; all consumers use accounts.
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FileChannels {
    api_url: Option<String>,
    token_file: Option<PathBuf>,
    token_env: String,
    stt_cmd: Option<Vec<String>>,
    stt_timeout_secs: u32,
    max_audio_bytes: u64,
    edit_interval_ms: u64,
    state_dir: Option<PathBuf>,
    accounts: Vec<ChannelAccountConfig>,
    telegram: Option<FileTelegram>,
    slack: Option<FileSlack>,
    discord: Option<FileDiscord>,
}
impl Default for FileChannels {
    fn default() -> Self {
        let c = ChannelsConfig::default();
        Self {
            api_url: c.api_url,
            token_file: c.token_file,
            token_env: c.token_env,
            stt_cmd: c.stt_cmd,
            stt_timeout_secs: c.stt_timeout_secs,
            max_audio_bytes: c.max_audio_bytes,
            edit_interval_ms: c.edit_interval_ms,
            state_dir: c.state_dir,
            accounts: c.accounts,
            telegram: None,
            slack: None,
            discord: None,
        }
    }
}
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct FileTelegram {
    enabled: bool,
    api_base: Option<String>,
    token_env: Option<String>,
    token_cmd: Option<Vec<String>>,
    #[serde(deserialize_with = "id_list")]
    allowed_chats: Vec<String>,
    #[serde(deserialize_with = "id_list")]
    allowed_users: Vec<String>,
    poll_timeout_secs: Option<u32>,
}
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct FileSlack {
    enabled: bool,
    api_base: Option<String>,
    listen: Option<SocketAddr>,
    bot_token_env: Option<String>,
    bot_token_cmd: Option<Vec<String>>,
    signing_secret_env: Option<String>,
    signing_secret_cmd: Option<Vec<String>>,
    #[serde(deserialize_with = "id_list")]
    allowed_workspaces: Vec<String>,
    #[serde(deserialize_with = "id_list")]
    allowed_channels: Vec<String>,
    #[serde(deserialize_with = "id_list")]
    allowed_users: Vec<String>,
}
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct FileDiscord {
    enabled: bool,
    api_base: Option<String>,
    listen: Option<SocketAddr>,
    public_key: Option<String>,
    bot_token_env: Option<String>,
    bot_token_cmd: Option<Vec<String>>,
    #[serde(deserialize_with = "id_list")]
    allowed_guilds: Vec<String>,
    #[serde(deserialize_with = "id_list")]
    allowed_channels: Vec<String>,
    #[serde(deserialize_with = "id_list")]
    allowed_users: Vec<String>,
}
fn legacy(
    kind: ChannelKind,
    enabled: bool,
    options: ChannelOptions,
    secret: LegacyChannelSecret,
) -> ChannelAccountConfig {
    ChannelAccountConfig {
        id: format!("legacy-{}", kind.as_str()),
        kind,
        label: kind.as_str().into(),
        enabled,
        agent_id: None,
        secret_id: None,
        options,
        legacy_secret: Some(secret),
    }
}
impl<'de> Deserialize<'de> for ChannelsConfig {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut c = FileChannels::deserialize(d)?;
        if let Some(t) = c.telegram {
            c.accounts.push(legacy(
                ChannelKind::Telegram,
                t.enabled,
                ChannelOptions {
                    api_base: t.api_base,
                    allowed_chats: t.allowed_chats,
                    allowed_users: t.allowed_users,
                    poll_timeout_secs: t.poll_timeout_secs.unwrap_or(30),
                    ..Default::default()
                },
                LegacyChannelSecret {
                    token_env: t.token_env,
                    token_cmd: t.token_cmd,
                    ..Default::default()
                },
            ));
        }
        if let Some(t) = c.slack {
            c.accounts.push(legacy(
                ChannelKind::Slack,
                t.enabled,
                ChannelOptions {
                    api_base: t.api_base,
                    listen: t.listen.or(Some(SocketAddr::from(([127, 0, 0, 1], 8787)))),
                    allowed_workspaces: t.allowed_workspaces,
                    allowed_channels: t.allowed_channels,
                    allowed_users: t.allowed_users,
                    ..Default::default()
                },
                LegacyChannelSecret {
                    token_env: t.bot_token_env,
                    token_cmd: t.bot_token_cmd,
                    signing_secret_env: t.signing_secret_env,
                    signing_secret_cmd: t.signing_secret_cmd,
                },
            ));
        }
        if let Some(t) = c.discord {
            c.accounts.push(legacy(
                ChannelKind::Discord,
                t.enabled,
                ChannelOptions {
                    api_base: t.api_base,
                    listen: t.listen.or(Some(SocketAddr::from(([127, 0, 0, 1], 8788)))),
                    public_key: t.public_key,
                    allowed_guilds: t.allowed_guilds,
                    allowed_channels: t.allowed_channels,
                    allowed_users: t.allowed_users,
                    ..Default::default()
                },
                LegacyChannelSecret {
                    token_env: t.bot_token_env,
                    token_cmd: t.bot_token_cmd,
                    ..Default::default()
                },
            ));
        }
        Ok(Self {
            api_url: c.api_url,
            token_file: c.token_file,
            token_env: c.token_env,
            stt_cmd: c.stt_cmd,
            stt_timeout_secs: c.stt_timeout_secs,
            max_audio_bytes: c.max_audio_bytes,
            edit_interval_ms: c.edit_interval_ms,
            state_dir: c.state_dir,
            accounts: c.accounts,
        })
    }
}
fn id_list<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Id {
        Text(String),
        Number(i64),
    }
    Ok(Vec::<Id>::deserialize(d)?
        .into_iter()
        .map(|id| match id {
            Id::Text(t) => t,
            Id::Number(n) => n.to_string(),
        })
        .collect())
}
/// Safe for account state paths and typed Vault ids; separators are forbidden.
pub fn valid_channel_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}
impl ChannelAccountConfig {
    /// Resolve a local override without copying the credential destination.
    pub fn api_base(&self) -> &str {
        self.options
            .api_base
            .as_deref()
            .unwrap_or(self.kind.api_base())
    }
    /// Fail closed on unsafe identity, conflicting credentials or empty allow-lists.
    pub fn validate(&self) -> Result<(), &'static str> {
        if !valid_channel_id(&self.id)
            || self.label.trim().is_empty()
            || self.label.len() > 128
            || self.label.chars().any(char::is_control)
        {
            return Err("invalid account identity");
        }
        if self.secret_id.is_some() && self.legacy_secret.is_some() {
            return Err("use either Vault or legacy credentials, not both");
        }
        if self
            .secret_id
            .as_ref()
            .is_some_and(|id| id != &format!("channel:{}:{}", self.kind.as_str(), self.id))
        {
            return Err("secret_id must match the account");
        }
        let o = &self.options;
        if o.poll_timeout_secs == 0 {
            return Err("poll_timeout_secs must be greater than zero");
        }
        if [
            &o.allowed_chats,
            &o.allowed_users,
            &o.allowed_workspaces,
            &o.allowed_channels,
            &o.allowed_guilds,
        ]
        .iter()
        .any(|ids| {
            ids.iter().any(|id| {
                id.trim().is_empty() || id.len() > 128 || id.chars().any(char::is_control)
            })
        }) {
            return Err("IDs must not be empty or contain controls");
        }
        let empty = match self.kind {
            ChannelKind::Telegram => o.allowed_chats.is_empty() && o.allowed_users.is_empty(),
            ChannelKind::Slack => {
                o.allowed_workspaces.is_empty()
                    && o.allowed_channels.is_empty()
                    && o.allowed_users.is_empty()
            }
            ChannelKind::Discord => {
                o.allowed_guilds.is_empty()
                    && o.allowed_channels.is_empty()
                    && o.allowed_users.is_empty()
            }
        };
        if self.enabled && empty {
            return Err("an enabled adapter needs at least one allowed_* entry (it serves nobody otherwise)");
        }
        if let Some(s) = &self.legacy_secret {
            for cmd in [&s.token_cmd, &s.signing_secret_cmd] {
                if cmd.as_ref().is_some_and(|argv| {
                    argv.is_empty()
                        || argv[0].trim().is_empty()
                        || argv.iter().any(|a| a.contains('\0'))
                }) {
                    return Err("provide a non-empty executable and arguments without NUL bytes");
                }
            }
        }
        Ok(())
    }
}
impl ChannelsConfig {
    pub(crate) fn validate(&self) -> Result<(), (String, &'static str)> {
        for (key, bad, why) in [
            (
                "stt_timeout_secs",
                self.stt_timeout_secs == 0,
                "must be greater than zero",
            ),
            (
                "max_audio_bytes",
                self.max_audio_bytes == 0,
                "must be greater than zero",
            ),
            (
                "edit_interval_ms",
                self.edit_interval_ms < 200,
                "must be at least 200 to respect platform rate limits",
            ),
            (
                "token_env",
                self.token_env.trim().is_empty(),
                "must name an environment variable",
            ),
            (
                "stt_cmd",
                self.stt_cmd.as_ref().is_some_and(|a| {
                    a.is_empty() || a[0].trim().is_empty() || a.iter().any(|v| v.contains('\0'))
                }),
                "provide a non-empty executable and arguments without NUL bytes",
            ),
        ] {
            if bad {
                return Err((format!("channels.{key}"), why));
            }
        }
        let mut seen = HashSet::new();
        for a in &self.accounts {
            let key = if a.legacy_secret.is_some() {
                format!("channels.{}", a.kind.as_str())
            } else {
                format!("channels.accounts.{}", a.id)
            };
            if !seen.insert((a.kind, a.id.as_str())) {
                return Err((key, "duplicate channel account id"));
            }
            a.validate().map_err(|why| (key, why))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_and_multiple_accounts_load_without_secret_values() {
        let old: ChannelsConfig =
            toml::from_str("[telegram]\nenabled=true\ntoken_env='BOT_TOKEN'\nallowed_chats=[123]")
                .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(old.accounts[0].id, "legacy-telegram");
        assert_eq!(old.accounts[0].options.allowed_chats, ["123"]);
        let multi: ChannelsConfig = toml::from_str("[[accounts]]\nid='work'\nkind='telegram'\nlabel='Work'\nsecret_id='channel:telegram:work'\n[[accounts]]\nid='home'\nkind='telegram'\nlabel='Home'\nsecret_id='channel:telegram:home'").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(multi.accounts.len(), 2);
        multi.validate().unwrap_or_else(|error| panic!("{error:?}"));
        assert!(!serde_json::to_string(&multi)
            .unwrap_or_else(|error| panic!("{error}"))
            .contains("token_cmd"));
    }
    #[test]
    fn channel_default_and_loaded_configs_roundtrip_through_atomic_writer() {
        fn strip(v: &mut serde_json::Value) {
            match v {
                serde_json::Value::Object(o) => {
                    o.retain(|_, v| !v.is_null());
                    for v in o.values_mut() {
                        strip(v);
                    }
                }
                serde_json::Value::Array(a) => {
                    for v in a {
                        strip(v);
                    }
                }
                _ => {}
            }
        }
        for text in ["", "[channels.telegram]\nenabled=true\ntoken_env='BOT_TOKEN'\nallowed_users=[7]",
            "[[channels.accounts]]\nid='work'\nkind='telegram'\nlabel='Work'\nsecret_id='channel:telegram:work'"] {
            let path = std::path::Path::new("config.toml");
            let config = crate::parse_document(path, text).unwrap_or_else(|error| panic!("{error}"));
            let mut json = serde_json::to_value(&config).unwrap_or_else(|error| panic!("{error}"));
            strip(&mut json);
            let output = toml::to_string(&toml::Value::try_from(&json).unwrap_or_else(|error| panic!("{error}"))).unwrap_or_else(|error| panic!("{error}"));
            let restored = crate::parse_document(path, &output).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(serde_json::to_value(&config).unwrap_or_else(|error| panic!("{error}")), serde_json::to_value(&restored).unwrap_or_else(|error| panic!("{error}")));
        }
    }
}
