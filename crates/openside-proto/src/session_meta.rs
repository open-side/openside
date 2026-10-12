use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Longest accepted session title, in characters.
pub const MAX_SESSION_TITLE_CHARS: usize = 200;

/// Title of a session created without one, until its first message names it.
pub const DEFAULT_SESSION_TITLE: &str = "New session";

/// Longest title derived from a first message, in characters (including the ellipsis).
const DERIVED_TITLE_CHARS: usize = 80;

/// Single-line display form of a title: control and bidirectional-override characters act as
/// whitespace, whitespace runs collapse and the ends are trimmed. `None` when nothing printable
/// remains.
///
/// Titles are rendered in list rows and notifications, where a newline would break the layout and
/// an override character could make one conversation impersonate another.
pub fn normalize_title(raw: &str) -> Option<String> {
    let mut title = String::with_capacity(raw.len());
    let mut separate = false;
    for c in raw.chars() {
        if c.is_whitespace()
            || c.is_control()
            || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            separate = !title.is_empty();
        } else {
            if separate {
                title.push(' ');
                separate = false;
            }
            title.push(c);
        }
    }
    (!title.is_empty()).then_some(title)
}

/// Title for a conversation that has none yet: the message's first non-blank line, shortened.
pub fn title_from_message(text: &str) -> Option<String> {
    let title = text.lines().find_map(normalize_title)?;
    if title.chars().count() <= DERIVED_TITLE_CHARS {
        return Some(title);
    }
    let cut: String = title.chars().take(DERIVED_TITLE_CHARS - 1).collect();
    Some(format!("{}…", cut.trim_end()))
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_single_line_without_control_or_override_characters() {
        assert_eq!(
            normalize_title("  a\n\tb \u{202e}c\0d  ").as_deref(),
            Some("a b c d")
        );
        assert_eq!(normalize_title("한글 제목").as_deref(), Some("한글 제목"));
        assert_eq!(
            normalize_title("👨\u{200d}👩\u{200d}👧 family").as_deref(),
            Some("👨\u{200d}👩\u{200d}👧 family")
        );
        assert_eq!(normalize_title(" \n\u{202e}\0 "), None);
    }

    #[test]
    fn derived_title_is_the_first_nonblank_line_shortened() {
        assert_eq!(
            title_from_message("\n\n  Plan the trip  \nmore").as_deref(),
            Some("Plan the trip")
        );
        assert_eq!(title_from_message("  \n "), None);
        let long = "word ".repeat(40);
        let title = title_from_message(&long).unwrap_or_default();
        assert!(title.chars().count() <= DERIVED_TITLE_CHARS && title.ends_with('…'));
    }
}
