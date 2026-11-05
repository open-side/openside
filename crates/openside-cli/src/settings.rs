//! CLI defaults kept on this computer: the default session host, account and browser
//! profile, and whether CLI sessions stay on the chat list. They live in
//! `<config dir>/cli.toml` (beside `config.toml`) and never hold credentials.

use std::{io::Write, path::PathBuf};

use clap::{Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::output_error;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) profile: Option<BrowserProfile>,
    /// Keep CLI sessions on the chat list; false archives them once they finish.
    #[serde(default)]
    pub(crate) save_sessions: bool,
}

/// A browser profile on a host: how a new session's browser is set up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub(crate) enum BrowserProfile {
    /// The managed profile with the person's logins.
    Shared,
    /// Own cookies and storage, wiped when the session is archived or deleted.
    Isolated,
    /// A private browser whose browsing is never remembered.
    Incognito,
}

pub(crate) const PROFILES: [BrowserProfile; 3] = [
    BrowserProfile::Shared,
    BrowserProfile::Isolated,
    BrowserProfile::Incognito,
];

impl BrowserProfile {
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Isolated => "isolated",
            Self::Incognito => "incognito",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Shared => "Managed browser profile with your logins (default)",
            Self::Isolated => {
                "Own cookies and storage, wiped when the session is archived or deleted"
            }
            Self::Incognito => "Private browser; browsing is never remembered",
        }
    }

    pub(crate) fn parse(id: &str) -> Result<Self, String> {
        PROFILES
            .into_iter()
            .find(|profile| profile.id() == id)
            .ok_or_else(|| {
                format!("unknown browser profile {id:?}; choose shared, isolated or incognito (see `openside host profiles`)")
            })
    }

    /// The `browser_space` and `incognito` fields of a new session.
    pub(crate) fn apply(self, request: &mut openside_proto::CreateSessionRequest) {
        request.browser_space = match self {
            Self::Isolated => openside_proto::BrowserSpace::Isolated,
            Self::Shared | Self::Incognito => openside_proto::BrowserSpace::Shared,
        };
        request.incognito = self == Self::Incognito;
    }
}

/// `u0`, `u1:isolated`, `<agent-id>` or `<agent-id>:<profile>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccountSelector {
    pub(crate) account: AccountRef,
    pub(crate) profile: Option<BrowserProfile>,
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
            Some((account, profile)) => (account, Some(BrowserProfile::parse(profile)?)),
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
        .config_file
        .parent()
        .map(|dir| dir.join("cli.toml"))
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
    /// Print the effective CLI settings as JSON.
    Get,
    /// Show CLI sessions in the chat list.
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

impl SettingsCommand {
    /// The account `set-default-profile` names, which only the daemon's roster can resolve.
    pub(crate) fn account(&self) -> Result<Option<AccountSelector>, String> {
        match self {
            Self::SetDefaultProfile { id }
                if id != "default" && BrowserProfile::parse(id).is_err() =>
            {
                AccountSelector::parse(id).map(Some)
            }
            _ => Ok(None),
        }
    }
}

pub(crate) const SETTINGS_AFTER_HELP: &str = "\
save-sessions true keeps CLI runs on the chat list. false (default) archives them once they finish.
set-default-profile isolated uses that browser profile for new sessions.
set-default-profile u1 runs new sessions as that account with its first browser profile (shared). Use u1:<profile-id> for both.
One-shot override: --account u1:<profile-id> or --profile <profile-id>.
set-default-profile default goes back to no account and the shared profile.

Examples:
  openside settings save-sessions true
  openside settings save-sessions false
  openside settings set-default-profile isolated
  openside settings set-default-profile u1
  openside settings set-default-profile u1:isolated
  openside settings set-default-profile default
  openside settings get";

/// The settings as `settings get` prints them.
pub(crate) fn effective(settings: &CliSettings) -> serde_json::Value {
    json!({
        "host": settings.host.as_deref().unwrap_or(LOCAL_HOST),
        "account": settings.account,
        "profile": settings.profile.unwrap_or(BrowserProfile::Shared).id(),
        "save_sessions": settings.save_sessions,
        "path": file_path().ok(),
    })
}

/// `settings get`, `save-sessions` and `set-default-profile <profile>|default` run locally;
/// `set-default-profile uN[:profile]` arrives with the account resolved to its agent id.
pub(crate) fn execute<W: Write>(
    command: &SettingsCommand,
    account: Option<String>,
    stdout: &mut W,
) -> Result<(), String> {
    let mut settings = load()?;
    match command {
        SettingsCommand::Get => {}
        SettingsCommand::SaveSessions { value } => {
            settings.save_sessions = *value;
            save(&settings)?;
        }
        SettingsCommand::SetDefaultProfile { id } => {
            if id == "default" {
                settings.account = None;
                settings.profile = None;
            } else if let Ok(profile) = BrowserProfile::parse(id) {
                settings.profile = Some(profile);
            } else {
                let selector = AccountSelector::parse(id)?;
                settings.account = account;
                settings.profile = selector.profile;
            }
            save(&settings)?;
        }
    }
    let text = serde_json::to_string_pretty(&effective(&settings)).map_err(|e| e.to_string())?;
    if matches!(command, SettingsCommand::Get) {
        writeln!(stdout, "{text}").map_err(output_error)
    } else {
        writeln!(stdout, "ok").map_err(output_error)
    }
}
