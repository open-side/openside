use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Longest accepted session title, in characters.
pub const MAX_SESSION_TITLE_CHARS: usize = 200;

/// Partial session metadata update; omitted fields keep their current value.
///
/// Applied as one durable `session.updated` event, so list state replays from the log.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateSessionRequest {
    /// New non-blank title, at most [`MAX_SESSION_TITLE_CHARS`] characters after trimming.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Pinned sessions are listed before unpinned ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    /// Archived sessions are hidden from the default session list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
}

impl UpdateSessionRequest {
    /// True when no field would change anything.
    pub fn is_empty(&self) -> bool {
        self.title.is_none() && self.pinned.is_none() && self.archived.is_none()
    }
}
