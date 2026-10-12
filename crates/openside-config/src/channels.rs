use std::{net::SocketAddr, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

/// Chat-platform adapters (`openside-channels`). Adapters are plain clients of the
/// public daemon API; they hold no session state of their own beyond the
/// thread→session map in `state_dir`.
///
/// Secrets are never values here: every secret is an environment variable *name*
/// (`*_env`) or a command argv (`*_cmd`, run without a shell, stdout is the value).
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ChannelsConfig {
    /// Daemon API base URL (`OPENSIDE_API_URL`); defaults to `http://<daemon.listen>`.
    pub api_url: Option<String>,
    /// File holding the daemon bearer token; takes precedence over `token_env`.
    pub token_file: Option<PathBuf>,
    /// Environment variable holding the daemon bearer token.
    pub token_env: String,
    /// Local speech-to-text argv; the audio file path is appended and stdout is the transcript.
    pub stt_cmd: Option<Vec<String>>,
    /// Deadline for `stt_cmd`, in seconds.
    pub stt_timeout_secs: u32,
    /// Audio attachments larger than this many bytes are refused.
    pub max_audio_bytes: u64,
    /// Minimum milliseconds between edits of a streaming reply (platform rate limits).
    pub edit_interval_ms: u64,
    /// Thread→session map directory (default `<data_dir>/channels`, mode 0700).
    pub state_dir: Option<PathBuf>,
    /// Telegram Bot API long polling.
    pub telegram: TelegramConfig,
    /// Slack Events API receiver.
    pub slack: SlackConfig,
    /// Discord Interactions endpoint.
    pub discord: DiscordConfig,
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
            telegram: TelegramConfig::default(),
            slack: SlackConfig::default(),
            discord: DiscordConfig::default(),
        }
    }
}

/// Telegram adapter; the whole chat (or forum topic) is one thread.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct TelegramConfig {
    /// Run this adapter.
    pub enabled: bool,
    /// Bot API base URL.
    pub api_base: String,
    /// Environment variable holding the bot token.
    pub token_env: Option<String>,
    /// Command argv printing the bot token.
    pub token_cmd: Option<Vec<String>>,
    /// Served chat IDs.
    #[serde(deserialize_with = "id_list")]
    #[schemars(with = "Vec<String>")]
    pub allowed_chats: Vec<String>,
    /// Served user IDs.
    #[serde(deserialize_with = "id_list")]
    #[schemars(with = "Vec<String>")]
    pub allowed_users: Vec<String>,
    /// `getUpdates` long-poll timeout in seconds.
    pub poll_timeout_secs: u32,
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_base: "https://api.telegram.org".into(),
            token_env: None,
            token_cmd: None,
            allowed_chats: Vec::new(),
            allowed_users: Vec::new(),
            poll_timeout_secs: 30,
        }
    }
}

/// Slack adapter; a thread is a Slack thread (or the whole direct-message channel).
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct SlackConfig {
    /// Run this adapter.
    pub enabled: bool,
    /// Web API base URL.
    pub api_base: String,
    /// Address of the HTTP receiver (`/slack/events`, `/slack/interactivity`).
    pub listen: SocketAddr,
    /// Environment variable holding the bot token.
    pub bot_token_env: Option<String>,
    /// Command argv printing the bot token.
    pub bot_token_cmd: Option<Vec<String>>,
    /// Environment variable holding the signing secret.
    pub signing_secret_env: Option<String>,
    /// Command argv printing the signing secret.
    pub signing_secret_cmd: Option<Vec<String>>,
    /// Served workspace (team) IDs.
    #[serde(deserialize_with = "id_list")]
    #[schemars(with = "Vec<String>")]
    pub allowed_workspaces: Vec<String>,
    /// Served channel IDs.
    #[serde(deserialize_with = "id_list")]
    #[schemars(with = "Vec<String>")]
    pub allowed_channels: Vec<String>,
    /// Served user IDs.
    #[serde(deserialize_with = "id_list")]
    #[schemars(with = "Vec<String>")]
    pub allowed_users: Vec<String>,
}

impl Default for SlackConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_base: "https://slack.com/api".into(),
            listen: SocketAddr::from(([127, 0, 0, 1], 8787)),
            bot_token_env: None,
            bot_token_cmd: None,
            signing_secret_env: None,
            signing_secret_cmd: None,
            allowed_workspaces: Vec::new(),
            allowed_channels: Vec::new(),
            allowed_users: Vec::new(),
        }
    }
}

/// Discord adapter over the HTTP Interactions endpoint (no Gateway websocket);
/// input arrives as slash commands, so a channel is one thread.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct DiscordConfig {
    /// Run this adapter.
    pub enabled: bool,
    /// REST API base URL.
    pub api_base: String,
    /// Address of the HTTP receiver (`/discord/interactions`).
    pub listen: SocketAddr,
    /// Application public key (hex); public by design, used for Ed25519 verification.
    pub public_key: Option<String>,
    /// Environment variable holding the bot token.
    pub bot_token_env: Option<String>,
    /// Command argv printing the bot token.
    pub bot_token_cmd: Option<Vec<String>>,
    /// Served guild IDs.
    #[serde(deserialize_with = "id_list")]
    #[schemars(with = "Vec<String>")]
    pub allowed_guilds: Vec<String>,
    /// Served channel IDs.
    #[serde(deserialize_with = "id_list")]
    #[schemars(with = "Vec<String>")]
    pub allowed_channels: Vec<String>,
    /// Served user IDs.
    #[serde(deserialize_with = "id_list")]
    #[schemars(with = "Vec<String>")]
    pub allowed_users: Vec<String>,
}

impl Default for DiscordConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_base: "https://discord.com/api/v10".into(),
            listen: SocketAddr::from(([127, 0, 0, 1], 8788)),
            public_key: None,
            bot_token_env: None,
            bot_token_cmd: None,
            allowed_guilds: Vec::new(),
            allowed_channels: Vec::new(),
            allowed_users: Vec::new(),
        }
    }
}

/// Platform IDs are numbers on Telegram/Discord; accept both so TOML stays natural.
fn id_list<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Id {
        Text(String),
        Number(i64),
    }
    Ok(Vec::<Id>::deserialize(deserializer)?
        .into_iter()
        .map(|id| match id {
            Id::Text(text) => text,
            Id::Number(number) => number.to_string(),
        })
        .collect())
}

fn bad_argv(argv: &Option<Vec<String>>) -> bool {
    argv.as_ref().is_some_and(|argv| {
        argv.is_empty() || argv[0].trim().is_empty() || argv.iter().any(|arg| arg.contains('\0'))
    })
}

fn has_blank(list: &[String]) -> bool {
    list.iter().any(|id| id.trim().is_empty())
}

impl ChannelsConfig {
    /// First invalid `(key, reason)`. Allow-lists of enabled adapters are checked here
    /// so a bad file fails at load, not at the first message.
    pub(crate) fn validate(&self) -> Result<(), (String, &'static str)> {
        const ARGV: &str = "provide a non-empty executable and arguments without NUL bytes";
        const ALLOW: &str =
            "an enabled adapter needs at least one allowed_* entry (it serves nobody otherwise)";
        const NO_BLANK: &str = "IDs must not be empty";
        for (key, argv) in [
            ("channels.stt_cmd", &self.stt_cmd),
            ("channels.telegram.token_cmd", &self.telegram.token_cmd),
            ("channels.slack.bot_token_cmd", &self.slack.bot_token_cmd),
            (
                "channels.slack.signing_secret_cmd",
                &self.slack.signing_secret_cmd,
            ),
            (
                "channels.discord.bot_token_cmd",
                &self.discord.bot_token_cmd,
            ),
        ] {
            if bad_argv(argv) {
                return Err((key.into(), ARGV));
            }
        }
        if self.stt_timeout_secs == 0 {
            return Err((
                "channels.stt_timeout_secs".into(),
                "must be greater than zero",
            ));
        }
        if self.max_audio_bytes == 0 {
            return Err((
                "channels.max_audio_bytes".into(),
                "must be greater than zero",
            ));
        }
        if self.edit_interval_ms < 200 {
            return Err((
                "channels.edit_interval_ms".into(),
                "must be at least 200 to respect platform rate limits",
            ));
        }
        if self.telegram.poll_timeout_secs == 0 {
            return Err((
                "channels.telegram.poll_timeout_secs".into(),
                "must be greater than zero",
            ));
        }
        if self.token_env.trim().is_empty() {
            return Err((
                "channels.token_env".into(),
                "must name an environment variable",
            ));
        }
        for (key, list) in [
            (
                "channels.telegram.allowed_chats",
                &self.telegram.allowed_chats,
            ),
            (
                "channels.telegram.allowed_users",
                &self.telegram.allowed_users,
            ),
            (
                "channels.slack.allowed_workspaces",
                &self.slack.allowed_workspaces,
            ),
            (
                "channels.slack.allowed_channels",
                &self.slack.allowed_channels,
            ),
            ("channels.slack.allowed_users", &self.slack.allowed_users),
            (
                "channels.discord.allowed_guilds",
                &self.discord.allowed_guilds,
            ),
            (
                "channels.discord.allowed_channels",
                &self.discord.allowed_channels,
            ),
            (
                "channels.discord.allowed_users",
                &self.discord.allowed_users,
            ),
        ] {
            if has_blank(list) {
                return Err((key.into(), NO_BLANK));
            }
        }
        if self.telegram.enabled
            && self.telegram.allowed_chats.is_empty()
            && self.telegram.allowed_users.is_empty()
        {
            return Err(("channels.telegram".into(), ALLOW));
        }
        if self.slack.enabled
            && self.slack.allowed_workspaces.is_empty()
            && self.slack.allowed_channels.is_empty()
            && self.slack.allowed_users.is_empty()
        {
            return Err(("channels.slack".into(), ALLOW));
        }
        if self.discord.enabled
            && self.discord.allowed_guilds.is_empty()
            && self.discord.allowed_channels.is_empty()
            && self.discord.allowed_users.is_empty()
        {
            return Err(("channels.discord".into(), ALLOW));
        }
        Ok(())
    }
}
