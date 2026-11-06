//! Key combinations such as `ctrl+shift+t` or `Return`, parsed once so every backend maps the
//! same closed set of keys.

use serde::Serialize;

use crate::Error;

/// A modifier held while the key is pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Modifier {
    /// Control.
    Ctrl,
    /// Shift.
    Shift,
    /// Alt (Option on macOS).
    Alt,
    /// Super/Windows key on Linux, Command on macOS.
    Super,
}

/// A named non-character key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum NamedKey {
    Return,
    Tab,
    Escape,
    Backspace,
    Delete,
    Space,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    F(u8),
}

/// The pressed key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Key {
    /// A printable character, lowercase for letters.
    Char(char),
    /// A named key.
    Named(NamedKey),
}

/// Modifiers plus one key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct KeyCombo {
    /// Held modifiers, each at most once.
    pub modifiers: Vec<Modifier>,
    /// The key.
    pub key: Key,
}

impl KeyCombo {
    /// Parse `mod+mod+key`; names are case-insensitive.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let invalid = || Error::Invalid(format!("Unknown key combination {text:?}"));
        let parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let (key, modifiers) = match parts.split_last() {
            // A lone "+" or a trailing "+" means the plus key itself.
            Some((last, rest)) if last.is_empty() && text.ends_with('+') => {
                ("+", &rest[..rest.len().saturating_sub(1)])
            }
            Some((last, rest)) => (*last, rest),
            None => return Err(invalid()),
        };
        let mut held = Vec::new();
        for name in modifiers {
            let modifier = match name.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => Modifier::Ctrl,
                "shift" => Modifier::Shift,
                "alt" | "option" | "opt" => Modifier::Alt,
                "super" | "cmd" | "command" | "meta" | "win" | "logo" => Modifier::Super,
                _ => return Err(invalid()),
            };
            if held.contains(&modifier) {
                return Err(invalid());
            }
            held.push(modifier);
        }
        let mut chars = key.chars();
        let key = match (chars.next(), chars.next()) {
            (Some(c), None) if !c.is_control() => Key::Char(c.to_ascii_lowercase()),
            _ => Key::Named(match key.to_ascii_lowercase().as_str() {
                "return" | "enter" => NamedKey::Return,
                "tab" => NamedKey::Tab,
                "escape" | "esc" => NamedKey::Escape,
                "backspace" => NamedKey::Backspace,
                "delete" | "del" => NamedKey::Delete,
                "space" => NamedKey::Space,
                "up" => NamedKey::Up,
                "down" => NamedKey::Down,
                "left" => NamedKey::Left,
                "right" => NamedKey::Right,
                "home" => NamedKey::Home,
                "end" => NamedKey::End,
                "pageup" | "page_up" => NamedKey::PageUp,
                "pagedown" | "page_down" => NamedKey::PageDown,
                "insert" => NamedKey::Insert,
                name => match name.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
                    Some(n @ 1..=12) => NamedKey::F(n),
                    _ => return Err(invalid()),
                },
            }),
        };
        Ok(Self {
            modifiers: held,
            key,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_combinations_and_rejects_unknown_names() {
        assert_eq!(
            KeyCombo::parse("Ctrl+Shift+T").ok(),
            Some(KeyCombo {
                modifiers: vec![Modifier::Ctrl, Modifier::Shift],
                key: Key::Char('t')
            })
        );
        assert_eq!(
            KeyCombo::parse("Enter").ok().map(|combo| combo.key),
            Some(Key::Named(NamedKey::Return))
        );
        assert_eq!(
            KeyCombo::parse("cmd+F5").ok().map(|combo| combo.key),
            Some(Key::Named(NamedKey::F(5)))
        );
        assert_eq!(
            KeyCombo::parse("ctrl++").ok().map(|combo| combo.key),
            Some(Key::Char('+'))
        );
        for bad in ["", "hyper+a", "ctrl+ctrl+a", "F13", "rm -rf"] {
            assert!(KeyCombo::parse(bad).is_err(), "{bad}");
        }
    }
}
