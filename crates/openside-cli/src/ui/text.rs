//! Display width, truncation and word wrapping that understand wide (CJK) characters and
//! styled spans.

use anstyle::Style;

use super::Span;

/// Terminal columns one character occupies.
pub(crate) fn char_width(character: char) -> usize {
    match character as u32 {
        0..=0x1f
        | 0x7f..=0x9f
        | 0x200b..=0x200f
        | 0x2028..=0x202e
        | 0x2060..=0x2064
        | 0xfe00..=0xfe0f
        | 0xfeff => 0,
        0x0300..=0x036f | 0x20d0..=0x20ff => 0,
        0x1100..=0x115f
        | 0x2e80..=0x303e
        | 0x3041..=0x33ff
        | 0x3400..=0x4dbf
        | 0x4e00..=0x9fff
        | 0xa000..=0xa4cf
        | 0xac00..=0xd7a3
        | 0xf900..=0xfaff
        | 0xfe30..=0xfe4f
        | 0xff00..=0xff60
        | 0xffe0..=0xffe6
        | 0x1f300..=0x1f64f
        | 0x1f900..=0x1f9ff
        | 0x20000..=0x3fffd => 2,
        _ => 1,
    }
}

pub(crate) fn width(text: &str) -> usize {
    text.chars().map(char_width).sum()
}

/// Width of text that may contain SGR escape sequences.
pub(crate) fn visible_width(text: &str) -> usize {
    let mut total = 0;
    let mut chars = text.chars();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' {
            // CSI: ESC [ params final-byte.
            if chars.next() == Some('[') {
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            continue;
        }
        total += char_width(character);
    }
    total
}

/// Shortens to at most `max` columns, ending in `ellipsis` when something was cut.
pub(crate) fn truncate(text: &str, max: usize, ellipsis: &str) -> String {
    if width(text) <= max {
        return text.to_owned();
    }
    let keep = max.saturating_sub(width(ellipsis));
    let mut out = String::new();
    let mut used = 0;
    for character in text.chars() {
        let next = char_width(character);
        if used + next > keep {
            break;
        }
        out.push(character);
        used += next;
    }
    if max >= width(ellipsis) {
        out.push_str(ellipsis);
    }
    out
}

/// Spans cut to `max` columns, the last kept span ending in `ellipsis`.
pub(crate) fn truncate_spans(spans: &[Span], max: usize, ellipsis: &str) -> Vec<Span> {
    let total: usize = spans.iter().map(|span| width(&span.text)).sum();
    if total <= max {
        return spans.to_vec();
    }
    let keep = max.saturating_sub(width(ellipsis));
    let mut out: Vec<Span> = Vec::new();
    let mut used = 0;
    for span in spans {
        let mut text = String::new();
        for character in span.text.chars() {
            let next = char_width(character);
            if used + next > keep {
                break;
            }
            text.push(character);
            used += next;
        }
        let full = text.chars().count() == span.text.chars().count();
        if !text.is_empty() {
            out.push(Span::new(text, span.style));
        }
        if !full {
            break;
        }
    }
    let style = out.last().map_or_else(Style::new, |span| span.style);
    if max >= width(ellipsis) {
        out.push(Span::new(ellipsis, style));
    }
    out
}

struct Word {
    pieces: Vec<Span>,
    width: usize,
}

fn words(spans: &[Span]) -> Vec<Word> {
    let mut out: Vec<Word> = Vec::new();
    let mut open = false;
    for span in spans {
        let mut run = String::new();
        for character in span.text.chars() {
            if character.is_whitespace() {
                if !run.is_empty() {
                    push_piece(&mut out, &mut open, std::mem::take(&mut run), span.style);
                }
                open = false;
            } else {
                run.push(character);
            }
        }
        if !run.is_empty() {
            push_piece(&mut out, &mut open, run, span.style);
        }
    }
    out
}

fn push_piece(words: &mut Vec<Word>, open: &mut bool, text: String, style: Style) {
    let piece_width = width(&text);
    if *open {
        if let Some(word) = words.last_mut() {
            word.pieces.push(Span::new(text, style));
            word.width += piece_width;
            return;
        }
    }
    words.push(Word {
        pieces: vec![Span::new(text, style)],
        width: piece_width,
    });
    *open = true;
}

/// Greedy word wrap of styled text. `first` and `rest` are the columns available on the first
/// and later lines; a word wider than a line is broken by character.
pub(crate) fn wrap_spans(spans: &[Span], first: usize, rest: usize) -> Vec<Vec<Span>> {
    let rest = rest.max(1);
    let mut lines: Vec<Vec<Span>> = Vec::new();
    let mut line: Vec<Span> = Vec::new();
    let mut used = 0;
    let mut limit = first.max(1);
    for word in words(spans) {
        let gap = usize::from(used > 0);
        if used + gap + word.width <= limit {
            if gap == 1 {
                line.push(Span::plain(" "));
            }
            used += gap + word.width;
            line.extend(word.pieces);
            continue;
        }
        if used > 0 {
            lines.push(std::mem::take(&mut line));
            used = 0;
            limit = rest;
        }
        if word.width <= limit {
            used = word.width;
            line.extend(word.pieces);
            continue;
        }
        // Wider than a whole line: break it at the edge, character by character.
        for piece in word.pieces {
            for character in piece.text.chars() {
                let next = char_width(character);
                if used + next > limit && used > 0 {
                    lines.push(std::mem::take(&mut line));
                    used = 0;
                    limit = rest;
                }
                match line.last_mut() {
                    Some(last) if last.style == piece.style => last.text.push(character),
                    _ => line.push(Span::new(character.to_string(), piece.style)),
                }
                used += next;
            }
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// Plain-text wrap, one string per line.
#[cfg(test)]
pub(crate) fn wrap_plain(text: &str, first: usize, rest: usize) -> Vec<String> {
    wrap_spans(&[Span::plain(text)], first, rest)
        .into_iter()
        .map(|line| line.into_iter().map(|span| span.text).collect())
        .collect()
}
