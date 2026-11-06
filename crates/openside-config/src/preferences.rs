//! `[preferences]`: the person's own choices from Settings (name, task notifications,
//! follow-up and confirmation defaults, agent tab clean-up, CLI session listing, the MCP
//! server switch). The daemon applies a change made through `PATCH
//! /v1/settings/preferences` immediately; nothing here names a path, executable or listener.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Longest accepted person name, in characters.
pub const MAX_PREFERENCE_NAME_CHARS: usize = 80;

/// Idle delays, in minutes, after which agent-opened tabs close; `0` never closes them.
pub const AGENT_TAB_CLEANUP_MINUTES: [u32; 7] = [0, 5, 10, 15, 30, 60, 120];

/// What a task notification is about.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKindSetting {
    /// An approval, question, sign-in or other human step is waiting.
    NeedsAttention,
    /// A chat finished its work.
    TaskCompleted,
    /// A routine run finished.
    RoutineCompleted,
}

/// How a message sent while a chat is working is delivered by default.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FollowUpSetting {
    /// After the current turn.
    #[default]
    Queue,
    /// Into the running turn at its next step.
    Steer,
}

/// Whether a chat asks before it changes anything.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationSetting {
    /// Writing files and running commands always ask, even where the permission mode allows
    /// them.
    AskFirst,
    /// The permission mode alone decides; sensitive actions still ask.
    #[default]
    Skip,
}

/// Person preferences; every key is optional in the file.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct PreferencesConfig {
    /// How the app addresses the person; unset shows no name.
    pub name: Option<String>,
    /// Kinds of task notification sent to this computer's and paired devices' browsers.
    pub notifications: Vec<NotificationKindSetting>,
    /// Play a sound in the app when a chat finishes.
    pub completion_sound: bool,
    /// Default delivery of a message sent while a chat is working: `queue` or `steer`.
    pub follow_up: FollowUpSetting,
    /// Default for new chats: `ask_first` or `skip`.
    pub confirmation: ConfirmationSetting,
    /// Close tabs an agent opened after this many idle minutes: 0 (never), 5, 10, 15, 30,
    /// 60 or 120.
    pub agent_tab_cleanup_minutes: u32,
    /// Keep sessions started by the CLI or an MCP client in the chat list once they finish.
    pub save_sessions: bool,
    /// Allow `openside mcp` to serve MCP clients.
    pub mcp_server: bool,
    /// Let agents ask to connect a desktop app and act in its window. Off by default.
    pub computer_use: bool,
}

impl Default for PreferencesConfig {
    fn default() -> Self {
        Self {
            name: None,
            notifications: vec![
                NotificationKindSetting::NeedsAttention,
                NotificationKindSetting::TaskCompleted,
                NotificationKindSetting::RoutineCompleted,
            ],
            completion_sound: true,
            follow_up: FollowUpSetting::default(),
            confirmation: ConfirmationSetting::default(),
            agent_tab_cleanup_minutes: 15,
            save_sessions: false,
            mcp_server: true,
            computer_use: false,
        }
    }
}

impl PreferencesConfig {
    /// The offending key and a short reason, so the API can refuse a write before it is saved.
    pub fn validate(&self) -> Result<(), (&'static str, &'static str)> {
        if self.name.as_deref().is_some_and(|name| {
            name.trim().is_empty()
                || name.trim() != name
                || name.chars().count() > MAX_PREFERENCE_NAME_CHARS
                || name.chars().any(char::is_control)
        }) {
            return Err((
                "preferences.name",
                "use 1-80 characters on one line, without surrounding spaces",
            ));
        }
        if self
            .notifications
            .iter()
            .enumerate()
            .any(|(index, kind)| self.notifications[..index].contains(kind))
        {
            return Err(("preferences.notifications", "list each kind once"));
        }
        if !AGENT_TAB_CLEANUP_MINUTES.contains(&self.agent_tab_cleanup_minutes) {
            return Err((
                "preferences.agent_tab_cleanup_minutes",
                "use 0, 5, 10, 15, 30, 60 or 120",
            ));
        }
        Ok(())
    }
}
