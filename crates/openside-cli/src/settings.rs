//! CLI defaults kept on this computer: the default session host, account and browser
//! profile. They live in `<config dir>/cli.toml` (beside `config.toml`) and never hold
//! credentials; the daemon's `/v1/settings/cli` (Settings › Developers) reads and writes the
//! `profile` key of the same file. Person preferences (whether CLI sessions stay on the chat
//! list, the MCP switch, ...) belong to the daemon: `settings get` shows them and
//! `settings set` changes them through `/v1/settings/preferences`.

use std::{io::Write, path::PathBuf, str::FromStr};

use clap::{Subcommand, ValueEnum};
use openside_proto::CliProfile;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{client::Client, output_error};

/// The local daemon, reached through its socket.
pub(crate) const LOCAL_HOST: &str = "local";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CliSettings {
    /// Session host name: `local` or a saved remote; absent means `local`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) host: Option<String>,
    /// Team agent id CLI sessions run as; absent means an ordinary conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) account: Option<String>,
    /// Browser profile for new CLI sessions; absent means the account's first (`shared`).
    /// The daemon's `PATCH /v1/settings/cli` rewrites this key in place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) profile: Option<CliProfile>,
    /// Written by older versions before this preference moved to the daemon; read so such a
    /// file still loads, never written back.
    #[serde(default, rename = "save_sessions", skip_serializing)]
    legacy_save_sessions: Option<bool>,
}

/// What the CLI adds to a browser profile (the ids live on [`CliProfile`]): its help text and
/// how a new session's browser is set up for it.
pub(crate) trait ProfileExt {
    fn description(self) -> &'static str;
    /// The `browser_space` and `incognito` fields of a new session.
    fn apply(self, request: &mut openside_proto::CreateSessionRequest);
}

impl ProfileExt for CliProfile {
    fn description(self) -> &'static str {
        match self {
            Self::Shared => "Managed browser profile with your logins (default)",
            Self::Isolated => {
                "Own cookies and storage, wiped when the session is archived or deleted"
            }
            Self::Incognito => "Private browser; browsing is never remembered",
        }
    }

    fn apply(self, request: &mut openside_proto::CreateSessionRequest) {
        request.browser_space = match self {
            Self::Isolated => openside_proto::BrowserSpace::Isolated,
            Self::Shared | Self::Incognito => openside_proto::BrowserSpace::Shared,
        };
        request.incognito = self == Self::Incognito;
    }
}

pub(crate) fn parse_profile(id: &str) -> Result<CliProfile, String> {
    CliProfile::from_id(id).ok_or_else(|| {
        format!("unknown browser profile {id:?}; choose shared, isolated or incognito (see `openside host profiles`)")
    })
}

/// `u0`, `u1:isolated`, `<agent-id>` or `<agent-id>:<profile>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccountSelector {
    pub(crate) account: AccountRef,
    pub(crate) profile: Option<CliProfile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AccountRef {
    /// Position in the active roster, the chief first.
    Index(usize),
    /// Team agent id.
    Id(String),
}

impl AccountSelector {
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let (account, profile) = match text.split_once(':') {
            Some((account, profile)) => (account, Some(parse_profile(profile)?)),
            None => (text, None),
        };
        if account.is_empty() {
            return Err(
                "an account selector needs an account: u0, u1, ... or a team agent id".into(),
            );
        }
        let index = account
            .strip_prefix('u')
            .filter(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|digits| digits.parse().ok());
        Ok(Self {
            account: match index {
                Some(index) => AccountRef::Index(index),
                None => AccountRef::Id(account.to_owned()),
            },
            profile,
        })
    }
}

/// `<config dir>/cli.toml`.
pub(crate) fn file_path() -> Result<PathBuf, String> {
    openside_config::Paths::discover()
        .cli_file()
        .ok_or_else(|| "cannot resolve the configuration directory".to_owned())
}

pub(crate) fn load() -> Result<CliSettings, String> {
    let path = file_path()?;
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            toml::from_str(&text).map_err(|_| format!("{}: invalid CLI settings", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(CliSettings::default()),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

pub(crate) fn save(settings: &CliSettings) -> Result<(), String> {
    let path = file_path()?;
    let text = toml::to_string(settings).map_err(|_| "cannot encode cli.toml")?;
    let dir = path.parent().ok_or("cli.toml has no parent directory")?;
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    // Rename over the old file so a crash cannot leave half of it behind.
    let mut temp = tempfile::NamedTempFile::new_in(dir)
        .map_err(|error| format!("cannot write in {}: {error}", dir.display()))?;
    temp.write_all(text.as_bytes())
        .map_err(|error| error.to_string())?;
    temp.persist(&path)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    Ok(())
}

#[derive(Subcommand)]
pub(crate) enum SettingsCommand {
    /// Print the CLI settings and the daemon's preferences as JSON.
    Get,
    /// Change one of the daemon's preferences.
    Set {
        #[arg(value_name = "key")]
        key: PreferenceKey,
        /// `true`/`false` for switches, minutes for `agent_tab_cleanup_minutes`, a comma list
        /// for `notifications` (empty for none).
        #[arg(value_name = "value", allow_hyphen_values = true)]
        value: String,
    },
    /// Show CLI sessions in the chat list (the daemon's `save_sessions` preference).
    SaveSessions {
        #[arg(value_name = "value", action = clap::ArgAction::Set, value_parser = clap::builder::BoolishValueParser::new())]
        value: bool,
    },
    /// Set the default profile for CLI commands.
    SetDefaultProfile {
        /// A profile (`shared`, `isolated`, `incognito`), an account (`u1`), both
        /// (`u1:isolated`), or `default`.
        #[arg(value_name = "id")]
        id: String,
    },
}

/// A field of `PATCH /v1/settings/preferences`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "snake_case")]
pub(crate) enum PreferenceKey {
    Name,
    Notifications,
    CompletionSound,
    FollowUp,
    Confirmation,
    AgentTabCleanupMinutes,
    SaveSessions,
    McpServer,
    ComputerUse,
}

impl PreferenceKey {
    fn field(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Notifications => "notifications",
            Self::CompletionSound => "completion_sound",
            Self::FollowUp => "follow_up",
            Self::Confirmation => "confirmation",
            Self::AgentTabCleanupMinutes => "agent_tab_cleanup_minutes",
            Self::SaveSessions => "save_sessions",
            Self::McpServer => "mcp_server",
            Self::ComputerUse => "computer_use",
        }
    }

    /// The JSON value for `text`; the daemon validates the allowed values.
    pub(crate) fn value(self, text: &str) -> Result<serde_json::Value, String> {
        let field = self.field();
        match self {
            Self::CompletionSound | Self::SaveSessions | Self::McpServer | Self::ComputerUse => {
                match text.trim().to_ascii_lowercase().as_str() {
                    "true" | "yes" | "on" | "1" => Ok(json!(true)),
                    "false" | "no" | "off" | "0" => Ok(json!(false)),
                    _ => Err(format!("{field} takes true or false")),
                }
            }
            Self::AgentTabCleanupMinutes => u32::from_str(text.trim())
                .map(|minutes| json!(minutes))
                .map_err(|_| format!("{field} takes a number of minutes (0 never closes tabs)")),
            Self::Notifications => Ok(json!(text
                .split(',')
                .map(str::trim)
                .filter(|kind| !kind.is_empty())
                .collect::<Vec<_>>())),
            Self::Name | Self::FollowUp | Self::Confirmation => Ok(json!(text)),
        }
    }
}

impl SettingsCommand {
    /// The account `set-default-profile` names, which only the daemon's roster can resolve.
    pub(crate) fn account(&self) -> Result<Option<AccountSelector>, String> {
        match self {
            Self::SetDefaultProfile { id } if id != "default" && parse_profile(id).is_err() => {
                AccountSelector::parse(id).map(Some)
            }
            _ => Ok(None),
        }
    }

    /// Runs without the daemon: only `set-default-profile` with a profile or `default`.
    pub(crate) fn is_local(&self) -> bool {
        matches!(self, Self::SetDefaultProfile { .. }) && matches!(self.account(), Ok(None))
    }
}

pub(crate) const SETTINGS_AFTER_HELP: &str = "\
get prints this computer's CLI settings and the daemon's preferences.
set <key> <value> changes a daemon preference: name, notifications (comma list), completion_sound,
follow_up (queue|steer), confirmation (skip|ask_first), agent_tab_cleanup_minutes (0 = never),
save_sessions, mcp_server (local socket only), computer_use.
save-sessions true keeps CLI and MCP runs on the chat list. false (default) archives them once they finish.
set-default-profile isolated uses that browser profile for new sessions.
set-default-profile u1 runs new sessions as that account with its first browser profile (shared). Use u1:<profile-id> for both.
One-shot override: --account u1:<profile-id> or --profile <profile-id>.
set-default-profile default goes back to no account and the shared profile.

Examples:
  openside settings get
  openside settings set agent_tab_cleanup_minutes 30
  openside settings set mcp_server false
  openside settings set notifications needs_attention,task_completed
  openside settings save-sessions true
  openside settings set-default-profile isolated
  openside settings set-default-profile u1
  openside settings set-default-profile u1:isolated
  openside settings set-default-profile default";

/// The local settings as `settings get` prints them, with the daemon's `preferences`.
pub(crate) fn effective(
    settings: &CliSettings,
    preferences: serde_json::Value,
) -> serde_json::Value {
    json!({
        "host": settings.host.as_deref().unwrap_or(LOCAL_HOST),
        "account": settings.account,
        "profile": settings.profile.unwrap_or(CliProfile::Shared).id(),
        "path": file_path().ok(),
        "preferences": preferences,
    })
}

const PREFERENCES: &str = "/v1/settings/preferences";

pub(crate) async fn preferences(client: &Client) -> Result<serde_json::Value, String> {
    client.get(PREFERENCES).await
}

/// Whether finished CLI and MCP sessions stay on the chat list. An unreadable preference
/// counts as the default (archive), so a hidden run is never left behind by a failed read.
pub(crate) async fn keeps_sessions(client: &Client) -> bool {
    preferences(client)
        .await
        .is_ok_and(|preferences| preferences["save_sessions"] == json!(true))
}

/// `set-default-profile <profile>|default` without a daemon; `account` is the agent id the
/// daemon resolved for `set-default-profile uN[:profile]`.
pub(crate) fn set_default_profile<W: Write>(
    id: &str,
    account: Option<String>,
    stdout: &mut W,
) -> Result<(), String> {
    let mut settings = load()?;
    if id == "default" {
        settings.account = None;
        settings.profile = None;
    } else if let Ok(profile) = parse_profile(id) {
        settings.profile = Some(profile);
    } else {
        let selector = AccountSelector::parse(id)?;
        settings.account = account;
        settings.profile = selector.profile;
    }
    save(&settings)?;
    writeln!(stdout, "ok").map_err(output_error)
}

/// Every settings command except a local `set-default-profile`.
pub(crate) async fn execute<W: Write, E: Write>(
    command: &SettingsCommand,
    client: &Client,
    account: Option<String>,
    stdout: &mut W,
    stderr: &mut E,
) -> Result<(), String> {
    let patch = |key: PreferenceKey, value: serde_json::Value| {
        let mut body = serde_json::Map::new();
        body.insert(key.field().to_owned(), value);
        serde_json::Value::Object(body)
    };
    let body = match command {
        SettingsCommand::Get => {
            let settings = load()?;
            let preferences = match preferences(client).await {
                Ok(preferences) => preferences,
                Err(error) => {
                    let _ = writeln!(stderr, "openside: daemon preferences unavailable: {error}");
                    serde_json::Value::Null
                }
            };
            let text = serde_json::to_string_pretty(&effective(&settings, preferences))
                .map_err(|e| e.to_string())?;
            return writeln!(stdout, "{text}").map_err(output_error);
        }
        SettingsCommand::SetDefaultProfile { id } => {
            return set_default_profile(id, account, stdout);
        }
        SettingsCommand::SaveSessions { value } => patch(PreferenceKey::SaveSessions, json!(value)),
        SettingsCommand::Set { key, value } => patch(*key, key.value(value)?),
    };
    client
        .json::<serde_json::Value, _>("PATCH", PREFERENCES, Some(&body))
        .await?;
    writeln!(stdout, "ok").map_err(output_error)
}
