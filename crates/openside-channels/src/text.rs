/// Byte index at which to cut `text` so the head has at most `max_chars` characters,
/// preferring the last line break in the second half of the window.
pub(crate) fn split_index(text: &str, max_chars: usize) -> usize {
    let Some((cut, _)) = text.char_indices().nth(max_chars) else {
        return text.len();
    };
    match text[..cut].rfind('\n') {
        Some(newline) if text[..newline].chars().count() >= max_chars / 2 => newline + 1,
        _ => cut,
    }
}

/// Shorten to `max_chars` characters with an ellipsis.
pub(crate) fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

pub(crate) fn unhex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_prefers_late_newlines_and_respects_char_boundaries() {
        assert_eq!(split_index("short", 10), 5);
        assert_eq!(split_index("aaaa\nbbbbbb", 8), 5);
        // A newline too early in the window would leave a stub, so cut mid-line.
        assert_eq!(split_index("a\nbbbbbbbbbbbb", 8), 8);
        let korean = "가나다라마바사";
        let cut = split_index(korean, 3);
        assert_eq!(&korean[..cut], "가나다");
    }

    #[test]
    fn truncate_counts_characters() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
    }

    #[test]
    fn hex_round_trips_and_rejects_garbage() {
        assert_eq!(unhex(&hex(&[0, 15, 255])), Some(vec![0, 15, 255]));
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
    }
}
