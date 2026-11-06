use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What a task notification is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    /// An approval, question, sign-in or other human step is waiting.
    NeedsAttention,
    /// A chat finished its work.
    TaskCompleted,
    /// A routine run finished.
    RoutineCompleted,
}

/// How a message sent while a chat is working is delivered when the client does not choose.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FollowUpMode {
    /// After the current turn.
    #[default]
    Queue,
    /// Into the running turn at its next step.
    Steer,
}

/// Whether a chat asks before it changes anything.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatConfirmation {
    /// Writing files and running commands always ask, even where the permission mode
    /// allows them; an explicit allow rule still applies.
    AskFirst,
    /// The permission mode alone decides; sensitive actions still ask.
    #[default]
    Skip,
}

/// The person's Settings choices, applied by the daemon as soon as they change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Preferences {
    /// How the app addresses the person; null shows no name.
    pub name: Option<String>,
    /// Kinds of task notification pushed to subscribed browsers; empty sends none.
    pub notifications: Vec<NotificationKind>,
    /// Play a sound in the app when a chat finishes.
    pub completion_sound: bool,
    /// Delivery the app and channels use for a message sent while a chat is working.
    pub follow_up: FollowUpMode,
    /// Confirmation for new chats that do not choose one.
    pub confirmation: ChatConfirmation,
    /// Tabs an agent opened close after this many idle minutes; 0 never closes them.
    pub agent_tab_cleanup_minutes: u32,
    /// Keep sessions started by the CLI or an MCP client in the chat list once they finish;
    /// otherwise they are archived when their run ends.
    pub save_sessions: bool,
    /// Whether `openside mcp` serves MCP clients; while off the daemon refuses its requests
    /// (403 `mcp_server_disabled`) except reading these preferences. On the local socket the
    /// MCP client identifies itself, so this holds against agents only while their shell is
    /// sandboxed; an unsandboxed shell runs as the person and can reach the socket directly.
    pub mcp_server: bool,
    /// Whether agents may ask to connect a desktop app (computer use).
    pub computer_use: bool,
}

/// Partial preference update; omitted fields keep their value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdatePreferencesRequest {
    /// 1-80 characters on one line; an empty string clears the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Each kind at most once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications: Option<Vec<NotificationKind>>,
    /// Completion sound on or off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_sound: Option<bool>,
    /// Default follow-up delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_up: Option<FollowUpMode>,
    /// Default confirmation for new chats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmation: Option<ChatConfirmation>,
    /// One of 0, 5, 10, 15, 30, 60, 120.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_tab_cleanup_minutes: Option<u32>,
    /// Keep CLI and MCP sessions in the chat list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save_sessions: Option<bool>,
    /// Allow `openside mcp`; changing it needs local access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_server: Option<bool>,
    /// Allow computer use; changing it needs local access. Turning it off stops every
    /// connected app at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub computer_use: Option<bool>,
}

impl UpdatePreferencesRequest {
    /// True when no field is present.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// How full a chat's context window was at its latest model request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextUsage {
    /// Input tokens of the latest request by the chat's own agent (cache reads included).
    pub used_tokens: u64,
    /// The model's context window; null when the runtime did not report one.
    pub window_tokens: Option<u64>,
}

/// Versions of what runs on this computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct About {
    /// The background service (`opensided`) version.
    pub version: String,
    /// Agent runtime package versions this service accepts, in Cargo/npm comparator syntax.
    pub runtime_versions: String,
    /// The agent runtime version reported by the latest started chat since this service
    /// started; null before any chat ran.
    pub runtime_version: Option<String>,
    /// Where releases are published.
    pub releases_url: String,
}

/// Result of asking the release feed for the newest version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateCheck {
    /// The installed version.
    pub current: String,
    /// Newest published version; null when nothing is published yet.
    pub latest: Option<String>,
    /// `latest` is newer than `current`.
    pub update_available: bool,
    /// Release page to open; null when nothing is published.
    pub url: Option<String>,
}
