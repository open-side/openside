//! Keyboard model: key names (`Enter`, `Shift+Tab`, `ControlOrMeta+a`, `KeyA`, `é`) on a US
//! layout, the set of keys currently held down, and the `Input.dispatchKeyEvent` parameters each
//! transition produces.

pub(crate) mod hangul;

use std::sync::LazyLock;

use serde_json::{json, Value};

pub(crate) const ALT: i64 = 1;
pub(crate) const CONTROL: i64 = 2;
pub(crate) const META: i64 = 4;
pub(crate) const SHIFT: i64 = 8;

/// `(code, key, shifted key, windowsVirtualKeyCode, location)`. Location 1/2 are the left/right
/// duplicates and 3 the numeric keypad (read as with NumLock on).
type Row = (&'static str, &'static str, &'static str, i64, i64);

const LETTERS: &str = "abcdefghijklmnopqrstuvwxyz";

#[rustfmt::skip]
const FIXED: &[Row] = &[
    ("Backquote", "`", "~", 192, 0), ("Minus", "-", "_", 189, 0), ("Equal", "=", "+", 187, 0),
    ("Backslash", "\\", "|", 220, 0), ("BracketLeft", "[", "{", 219, 0), ("BracketRight", "]", "}", 221, 0),
    ("Semicolon", ";", ":", 186, 0), ("Quote", "'", "\"", 222, 0), ("Comma", ",", "<", 188, 0),
    ("Period", ".", ">", 190, 0), ("Slash", "/", "?", 191, 0), ("Space", " ", " ", 32, 0),
    ("Backspace", "Backspace", "", 8, 0), ("Tab", "Tab", "", 9, 0), ("Enter", "Enter", "", 13, 0),
    ("Escape", "Escape", "", 27, 0), ("Delete", "Delete", "", 46, 0), ("Insert", "Insert", "", 45, 0),
    ("Home", "Home", "", 36, 0), ("End", "End", "", 35, 0), ("PageUp", "PageUp", "", 33, 0),
    ("PageDown", "PageDown", "", 34, 0), ("ArrowLeft", "ArrowLeft", "", 37, 0),
    ("ArrowUp", "ArrowUp", "", 38, 0), ("ArrowRight", "ArrowRight", "", 39, 0),
    ("ArrowDown", "ArrowDown", "", 40, 0), ("CapsLock", "CapsLock", "", 20, 0),
    ("NumLock", "NumLock", "", 144, 0), ("ScrollLock", "ScrollLock", "", 145, 0),
    ("Pause", "Pause", "", 19, 0), ("PrintScreen", "PrintScreen", "", 44, 0),
    ("ContextMenu", "ContextMenu", "", 93, 0),
    ("ShiftLeft", "Shift", "", 16, 1), ("ShiftRight", "Shift", "", 16, 2),
    ("ControlLeft", "Control", "", 17, 1), ("ControlRight", "Control", "", 17, 2),
    ("AltLeft", "Alt", "", 18, 1), ("AltRight", "Alt", "", 18, 2),
    ("MetaLeft", "Meta", "", 91, 1), ("MetaRight", "Meta", "", 92, 2),
    ("NumpadEnter", "Enter", "", 13, 3), ("NumpadMultiply", "*", "", 106, 3),
    ("NumpadAdd", "+", "", 107, 3), ("NumpadSubtract", "-", "", 109, 3),
    ("NumpadDecimal", ".", "", 110, 3), ("NumpadDivide", "/", "", 111, 3),
];

const DIGIT_SHIFTED: &str = ")!@#$%^&*(";

/// A key on the layout and whether the shifted layer was asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Resolved {
    code: String,
    key: String,
    key_code: i64,
    location: i64,
    /// What the key types, empty for keys that type nothing.
    text: String,
}

impl Resolved {
    fn bit(&self) -> i64 {
        match self.key.as_str() {
            "Alt" => ALT,
            "Control" => CONTROL,
            "Meta" => META,
            "Shift" => SHIFT,
            _ => 0,
        }
    }
}

fn make(row: &Row, shifted: bool) -> Resolved {
    let (code, key, shift_key, key_code, location) = *row;
    let key = if shifted && !shift_key.is_empty() {
        shift_key
    } else {
        key
    };
    let text = match key {
        "Enter" => "\r".to_owned(),
        _ if key.chars().count() == 1 => key.to_owned(),
        _ => String::new(),
    };
    Resolved {
        code: code.to_owned(),
        key: key.to_owned(),
        key_code,
        location,
        text,
    }
}

static ROWS: LazyLock<Vec<Row>> = LazyLock::new(build_rows);

/// Every row of the layout, generated rows included.
fn build_rows() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for (index, digit) in "1234567890".char_indices() {
        let code: &'static str = [
            "Digit1", "Digit2", "Digit3", "Digit4", "Digit5", "Digit6", "Digit7", "Digit8",
            "Digit9", "Digit0",
        ][index];
        let key = &"1234567890"[index..=index];
        let shifted = &DIGIT_SHIFTED[(index + 1) % 10..=(index + 1) % 10];
        rows.push((
            code,
            key,
            shifted,
            48 + i64::from(digit.to_digit(10).unwrap_or(0)),
            0,
        ));
    }
    for (index, _) in LETTERS.char_indices() {
        const CODES: [&str; 26] = [
            "KeyA", "KeyB", "KeyC", "KeyD", "KeyE", "KeyF", "KeyG", "KeyH", "KeyI", "KeyJ", "KeyK",
            "KeyL", "KeyM", "KeyN", "KeyO", "KeyP", "KeyQ", "KeyR", "KeyS", "KeyT", "KeyU", "KeyV",
            "KeyW", "KeyX", "KeyY", "KeyZ",
        ];
        const UPPER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
        rows.push((
            CODES[index],
            &LETTERS[index..=index],
            &UPPER[index..=index],
            65 + index as i64,
            0,
        ));
    }
    rows.extend_from_slice(FIXED);
    const FUNCTION: [&str; 24] = [
        "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13", "F14",
        "F15", "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24",
    ];
    for (index, name) in FUNCTION.iter().enumerate() {
        rows.push((name, name, "", 112 + index as i64, 0));
    }
    const PAD: [&str; 10] = [
        "Numpad0", "Numpad1", "Numpad2", "Numpad3", "Numpad4", "Numpad5", "Numpad6", "Numpad7",
        "Numpad8", "Numpad9",
    ];
    for (index, code) in PAD.iter().enumerate() {
        rows.push((code, &"0123456789"[index..=index], "", 96 + index as i64, 3));
    }
    rows
}

/// Find a key by `code` (`KeyA`), key name (`ArrowLeft`), or the character it types (`a`, `A`,
/// `!`). A character from the shifted layer resolves with its shifted key and text, as when
/// Shift is held; `shifted` asks for that layer for a key given by name.
pub(crate) fn resolve(name: &str, shifted: bool) -> Result<Resolved, String> {
    let rows = &*ROWS;
    let name = match name {
        "Control" | "ControlOrMeta" => "ControlLeft",
        "Shift" => "ShiftLeft",
        "Alt" => "AltLeft",
        "Meta" => "MetaLeft",
        other => other,
    };
    if let Some(row) = rows.iter().find(|row| row.0 == name) {
        return Ok(make(row, shifted));
    }
    if name.chars().count() == 1 {
        if let Some(row) = rows.iter().find(|row| row.1 == name && row.4 != 3) {
            return Ok(make(row, shifted));
        }
        if let Some(row) = rows
            .iter()
            .find(|row| row.2 == name && !row.2.is_empty() && row.4 == 0)
        {
            return Ok(make(row, true));
        }
    } else if let Some(row) = rows.iter().find(|row| row.1 == name && row.4 != 3) {
        return Ok(make(row, shifted));
    }
    Err(format!("unknown key {name:?}"))
}

/// Whether `name` is a key the layout can type as a key press (so `type` presses it rather
/// than inserting it as text).
pub(crate) fn on_layout(name: &str) -> bool {
    resolve(name, false).is_ok()
}

/// Modifier bit of a modifier key name; `None` for any other key.
pub(crate) fn modifier_bit(name: &str) -> Option<i64> {
    resolve(name, false)
        .ok()
        .map(|key| key.bit())
        .filter(|bit| *bit != 0)
}

/// The modifiers and the main key of a chord such as `Control+Shift+T`; a trailing `+` is the
/// plus key itself (`Shift++`).
pub(crate) fn split_chord(spec: &str) -> Result<(Vec<&str>, &str), String> {
    let (modifiers, main) = if spec == "+" {
        (Vec::new(), "+")
    } else if let Some(prefix) = spec.strip_suffix("++") {
        (prefix.split('+').collect(), "+")
    } else {
        let mut parts: Vec<&str> = spec.split('+').collect();
        let main = parts.pop().unwrap_or_default();
        (parts, main)
    };
    if main.is_empty() || modifiers.iter().any(|modifier| modifier.is_empty()) {
        return Err(format!("invalid key {spec:?}"));
    }
    for modifier in &modifiers {
        if modifier_bit(modifier).is_none() {
            return Err(format!("unknown key modifier {modifier:?}"));
        }
    }
    resolve(main, false)?;
    Ok((modifiers, main))
}

/// Blink editing commands for chords that Chromium does not bind natively under headless input.
fn commands(modifiers: i64, key: &str) -> Vec<&'static str> {
    if modifiers & (CONTROL | META) == 0 || modifiers & ALT != 0 {
        return Vec::new();
    }
    let shift = modifiers & SHIFT != 0;
    let command = match (key.to_ascii_lowercase().as_str(), shift) {
        ("a", _) => "selectAll",
        ("c", _) => "copy",
        ("x", _) => "cut",
        ("v", _) => "paste",
        ("z", false) => "undo",
        ("z", true) | ("y", _) => "redo",
        ("arrowleft", false) => "moveWordLeft",
        ("arrowleft", true) => "moveWordLeftAndModifySelection",
        ("arrowright", false) => "moveWordRight",
        ("arrowright", true) => "moveWordRightAndModifySelection",
        ("backspace", _) => "deleteWordBackward",
        ("delete", _) => "deleteWordForward",
        ("home", false) => "moveToBeginningOfDocument",
        ("home", true) => "moveToBeginningOfDocumentAndModifySelection",
        ("end", false) => "moveToEndOfDocument",
        ("end", true) => "moveToEndOfDocumentAndModifySelection",
        _ => return Vec::new(),
    };
    vec![command]
}

/// Keys held down, by code, in press order.
#[derive(Default)]
pub(crate) struct Held {
    down: Vec<String>,
}

impl Held {
    /// Modifier bits of the keys held now.
    pub(crate) fn modifiers(&self) -> i64 {
        self.down
            .iter()
            .filter_map(|code| resolve(code, false).ok())
            .fold(0, |bits, key| bits | key.bit())
    }

    /// The `Input.dispatchKeyEvent` parameters for pressing or releasing `name` now, without
    /// recording the transition (see [`Held::apply`]).
    pub(crate) fn event(&self, name: &str, down: bool) -> Result<Value, String> {
        let held = self.modifiers();
        let key = resolve(name, held & SHIFT != 0)?;
        let already = self.down.contains(&key.code);
        let modifiers = if down {
            held | key.bit()
        } else {
            self.without(&key.code)
        };
        if !down {
            return Ok(json!({
                "type": "keyUp", "modifiers": modifiers, "key": key.key, "code": key.code,
                "windowsVirtualKeyCode": key.key_code, "location": key.location,
            }));
        }
        let text = if modifiers & (ALT | CONTROL | META) != 0 {
            ""
        } else {
            key.text.as_str()
        };
        let mut event = json!({
            "type": if text.is_empty() { "rawKeyDown" } else { "keyDown" },
            "modifiers": modifiers, "key": key.key, "code": key.code,
            "windowsVirtualKeyCode": key.key_code, "location": key.location,
            "autoRepeat": already, "isKeypad": key.location == 3,
        });
        if !text.is_empty() {
            event["text"] = json!(text);
            event["unmodifiedText"] = json!(text);
        }
        let commands = commands(modifiers, &key.key);
        if !commands.is_empty() {
            event["commands"] = json!(commands);
        }
        Ok(event)
    }

    fn without(&self, code: &str) -> i64 {
        self.down
            .iter()
            .filter(|held| held.as_str() != code)
            .filter_map(|held| resolve(held, false).ok())
            .fold(0, |bits, key| bits | key.bit())
    }

    /// Record that `name` went down or up, after its event was delivered.
    pub(crate) fn apply(&mut self, name: &str, down: bool) {
        let Ok(key) = resolve(name, false) else {
            return;
        };
        if down {
            if !self.down.contains(&key.code) {
                self.down.push(key.code);
            }
        } else {
            self.down.retain(|code| *code != key.code);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_resolves_names_codes_and_characters() {
        let a = resolve("a", false).unwrap();
        assert_eq!(
            (a.code.as_str(), a.key_code, a.text.as_str()),
            ("KeyA", 65, "a")
        );
        let capital = resolve("A", false).unwrap();
        assert_eq!((capital.key.as_str(), capital.text.as_str()), ("A", "A"));
        let bang = resolve("!", false).unwrap();
        assert_eq!((bang.code.as_str(), bang.key_code), ("Digit1", 49));
        assert_eq!(resolve("KeyQ", false).unwrap().key, "q");
        assert_eq!(resolve("KeyQ", true).unwrap().key, "Q");
        assert_eq!(resolve("Slash", true).unwrap().key, "?");
        assert_eq!(resolve("Numpad7", false).unwrap().location, 3);
        assert_eq!(resolve("ShiftRight", false).unwrap().location, 2);
        assert_eq!(resolve("Enter", false).unwrap().text, "\r");
        assert_eq!(resolve("F12", false).unwrap().key_code, 123);
        assert_eq!(resolve("Escape", false).unwrap().text, "");
        assert!(resolve("é", false).is_err());
        assert!(resolve("NotAKey", false).is_err());
    }

    #[test]
    fn chords_split_on_plus_and_keep_the_plus_key() {
        assert_eq!(
            split_chord("Control+Shift+T").unwrap(),
            (vec!["Control", "Shift"], "T")
        );
        assert_eq!(split_chord("+").unwrap(), (vec![], "+"));
        assert_eq!(split_chord("Shift++").unwrap(), (vec!["Shift"], "+"));
        assert!(split_chord("Hyper+a").is_err());
        assert!(split_chord("Control+").is_err());
        assert!(split_chord("").is_err());
    }

    #[test]
    fn held_modifiers_shift_the_layer_and_suppress_text() {
        let mut held = Held::default();
        held.apply("Shift", true);
        let shifted = held.event("a", true).unwrap();
        assert_eq!(shifted["key"], "A");
        assert_eq!(shifted["text"], "A");
        assert_eq!(shifted["modifiers"], SHIFT);
        held.apply("Control", true);
        let chord = held.event("a", true).unwrap();
        assert_eq!(chord["type"], "rawKeyDown");
        assert!(chord.get("text").is_none());
        assert_eq!(chord["commands"], json!(["selectAll"]));
        assert_eq!(chord["modifiers"], SHIFT | CONTROL);
        assert_eq!(held.event("Control", false).unwrap()["modifiers"], SHIFT);
    }

    #[test]
    fn a_key_pressed_twice_without_release_is_an_auto_repeat() {
        let mut held = Held::default();
        assert_eq!(held.event("b", true).unwrap()["autoRepeat"], false);
        held.apply("b", true);
        assert_eq!(held.event("b", true).unwrap()["autoRepeat"], true);
        held.apply("b", false);
        assert_eq!(held.event("b", true).unwrap()["autoRepeat"], false);
    }

    #[test]
    fn typed_characters_carry_text_and_named_keys_do_not() {
        let held = Held::default();
        let enter = held.event("Enter", true).unwrap();
        assert_eq!(
            (enter["type"].as_str(), enter["text"].as_str()),
            (Some("keyDown"), Some("\r"))
        );
        let tab = held.event("Tab", true).unwrap();
        assert_eq!(tab["type"], "rawKeyDown");
        let plus = held.event("+", true).unwrap();
        assert_eq!(plus["code"], "Equal");
        assert_eq!(plus["text"], "+");
        assert_eq!(held.event("5", true).unwrap()["windowsVirtualKeyCode"], 53);
    }
}
