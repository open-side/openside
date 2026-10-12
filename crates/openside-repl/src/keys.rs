//! Playwright key names (`Enter`, `Shift+Tab`, `ControlOrMeta+a`) to `Input.dispatchKeyEvent`.

use serde_json::{json, Value};

const ALT: i64 = 1;
const CONTROL: i64 = 2;
const META: i64 = 4;
const SHIFT: i64 = 8;

/// keyDown/keyUp parameter pairs for one chord; modifiers are pressed around the main key.
pub(crate) fn chord(spec: &str) -> Result<Vec<Value>, String> {
    let parts: Vec<&str> = if spec == "+" {
        vec!["+"]
    } else {
        spec.split('+').collect()
    };
    let (main, prefix) = parts
        .split_last()
        .ok_or_else(|| format!("invalid key {spec:?}"))?;
    let mut modifiers = 0;
    let mut modifier_keys = Vec::new();
    for modifier in prefix {
        let (bit, key, code, virtual_key) = match *modifier {
            "Alt" => (ALT, "Alt", "AltLeft", 18),
            "Control" | "ControlOrMeta" => (CONTROL, "Control", "ControlLeft", 17),
            "Meta" => (META, "Meta", "MetaLeft", 91),
            "Shift" => (SHIFT, "Shift", "ShiftLeft", 16),
            other => return Err(format!("unknown key modifier {other:?}")),
        };
        modifiers |= bit;
        modifier_keys.push((key, code, virtual_key));
    }
    let mut events = Vec::new();
    let mut held = 0;
    for (key, code, virtual_key) in &modifier_keys {
        held |= match *key {
            "Alt" => ALT,
            "Control" => CONTROL,
            "Meta" => META,
            _ => SHIFT,
        };
        events.push(json!({"type":"rawKeyDown","key":key,"code":code,"windowsVirtualKeyCode":virtual_key,"modifiers":held}));
    }
    let (down, up) = key(main, modifiers)?;
    events.push(down);
    events.push(up);
    for (key, code, virtual_key) in modifier_keys.iter().rev() {
        held &= !match *key {
            "Alt" => ALT,
            "Control" => CONTROL,
            "Meta" => META,
            _ => SHIFT,
        };
        events.push(json!({"type":"keyUp","key":key,"code":code,"windowsVirtualKeyCode":virtual_key,"modifiers":held}));
    }
    Ok(events)
}

fn key(name: &str, modifiers: i64) -> Result<(Value, Value), String> {
    let named = |key: &str, code: &str, virtual_key: i64, text: &str| {
        (
            key.to_owned(),
            code.to_owned(),
            virtual_key,
            text.to_owned(),
        )
    };
    let (key, code, virtual_key, text) = match name {
        "Enter" => named("Enter", "Enter", 13, "\r"),
        "Tab" => named("Tab", "Tab", 9, ""),
        "Escape" => named("Escape", "Escape", 27, ""),
        "Backspace" => named("Backspace", "Backspace", 8, ""),
        "Delete" => named("Delete", "Delete", 46, ""),
        "Insert" => named("Insert", "Insert", 45, ""),
        "Home" => named("Home", "Home", 36, ""),
        "End" => named("End", "End", 35, ""),
        "PageUp" => named("PageUp", "PageUp", 33, ""),
        "PageDown" => named("PageDown", "PageDown", 34, ""),
        "ArrowLeft" => named("ArrowLeft", "ArrowLeft", 37, ""),
        "ArrowUp" => named("ArrowUp", "ArrowUp", 38, ""),
        "ArrowRight" => named("ArrowRight", "ArrowRight", 39, ""),
        "ArrowDown" => named("ArrowDown", "ArrowDown", 40, ""),
        "Space" | " " => named(" ", "Space", 32, " "),
        _ if name.len() > 1
            && name.starts_with('F')
            && name[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)) =>
        {
            let number: i64 = name[1..].parse().map_err(|_| "invalid function key")?;
            named(name, name, 111 + number, "")
        }
        _ if name.chars().count() == 1 => {
            let character = name.chars().next().ok_or("empty key")?;
            let code = if character.is_ascii_alphabetic() {
                format!("Key{}", character.to_ascii_uppercase())
            } else if character.is_ascii_digit() {
                format!("Digit{character}")
            } else {
                String::new()
            };
            let virtual_key = if character.is_ascii_alphanumeric() {
                i64::from(u32::from(character.to_ascii_uppercase()))
            } else {
                0
            };
            let text = if modifiers & SHIFT != 0 && character.is_ascii_lowercase() {
                character.to_ascii_uppercase().to_string()
            } else {
                name.to_owned()
            };
            (text.clone(), code, virtual_key, text)
        }
        _ => return Err(format!("unknown key {name:?}")),
    };
    let mut down = json!({
        "type": if text.is_empty() || modifiers & (ALT | CONTROL | META) != 0 { "rawKeyDown" } else { "keyDown" },
        "key": key, "code": code, "windowsVirtualKeyCode": virtual_key, "modifiers": modifiers,
    });
    if !text.is_empty() && modifiers & (ALT | CONTROL | META) == 0 {
        down["text"] = json!(text);
        down["unmodifiedText"] = json!(text);
    }
    // Headless Chromium has no native key bindings for editing chords on every platform.
    if modifiers & (CONTROL | META) != 0 && key.eq_ignore_ascii_case("a") {
        down["commands"] = json!(["selectAll"]);
    }
    let up = json!({"type":"keyUp","key":key,"code":code,"windowsVirtualKeyCode":virtual_key,"modifiers":modifiers});
    Ok((down, up))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_press_modifiers_around_the_key() {
        let events = chord("Control+a").unwrap();
        assert_eq!(events.len(), 4);
        assert_eq!(events[0]["key"], "Control");
        assert_eq!(events[1]["commands"], json!(["selectAll"]));
        assert_eq!(events[1]["modifiers"], 2);
        assert_eq!(events[3]["modifiers"], 0);
        assert_eq!(chord("Enter").unwrap()[0]["text"], "\r");
        assert_eq!(chord("Shift+a").unwrap()[1]["text"], "A");
        assert_eq!(chord("F5").unwrap()[0]["windowsVirtualKeyCode"], 116);
        assert!(chord("Hyper+a").is_err());
        assert!(chord("NotAKey").is_err());
    }
}
