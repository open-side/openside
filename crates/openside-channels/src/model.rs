use serde::{Deserialize, Serialize};

/// One conversation on one platform. `thread` is empty when the whole chat is the thread.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ThreadKey {
    /// `telegram`, `slack` or `discord`.
    pub platform: String,
    /// Chat / channel identifier.
    pub chat: String,
    /// Platform thread identifier, empty for the chat's main conversation.
    pub thread: String,
}

impl ThreadKey {
    /// Build a key.
    pub fn new(platform: &str, chat: impl Into<String>, thread: impl Into<String>) -> Self {
        Self {
            platform: platform.into(),
            chat: chat.into(),
            thread: thread.into(),
        }
    }

    /// Stable map key; platform IDs never contain `|`.
    pub fn id(&self) -> String {
        format!("{}|{}|{}", self.platform, self.chat, self.thread)
    }
}

/// Platform-opaque handle of a message the bot posted (Telegram message id,
/// Slack `ts`, Discord message id).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MsgRef(pub String);

/// One inline button; `data` round-trips through the platform as callback data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Button {
    /// Visible label.
    pub label: String,
    /// Callback payload, at most 64 bytes (Telegram limit).
    pub data: String,
    /// Rendered as a destructive/secondary style where the platform has one.
    pub danger: bool,
}

/// Reference to an audio attachment that [`crate::Messenger::download_audio`] can fetch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioRef {
    /// Platform-specific handle (Telegram `file_id`, Slack/Discord download URL).
    pub reference: String,
    /// Declared size in bytes, when the platform reports it.
    pub size: Option<u64>,
}

/// A user message already past the platform allow-list.
#[derive(Clone, Debug)]
pub struct Inbound {
    /// Conversation the message belongs to.
    pub thread: ThreadKey,
    /// Platform user identifier.
    pub user: String,
    /// Platform message identifier, used to make daemon delivery idempotent.
    pub message_id: String,
    /// Message text, empty for audio-only messages.
    pub text: String,
    /// Audio attachment, if any.
    pub audio: Option<AudioRef>,
}

/// A button press already past the platform allow-list.
#[derive(Clone, Debug)]
pub struct Press {
    /// Conversation the card lives in.
    pub thread: ThreadKey,
    /// Platform user identifier of the presser.
    pub user: String,
    /// Callback payload of the pressed button.
    pub data: String,
    /// The card message.
    pub message: MsgRef,
    /// Platform acknowledgement handle (Telegram callback query id), else empty.
    pub token: String,
}
