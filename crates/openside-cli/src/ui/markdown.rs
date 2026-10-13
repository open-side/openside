//! Streaming, line-oriented markdown for a terminal: bold, emphasis, code spans and fences,
//! headings, lists, quotes and links (text followed by the dim URL), wrapped to the terminal.
//!
//! Text arrives in arbitrary fragments, so a line is rendered only once its newline is in; the
//! live view keeps its spinner up while a line is still being written. Without colour the
//! markers stay (`**bold**`, `` `code` ``) because the style that replaced them is invisible.

use anstyle::{AnsiColor, Effects, Style};

use super::{text::wrap_spans, Glyph, Role, Span, Ui};

const INDENT: &str = "  ";
const MAX_MEASURE: usize = 100;

pub(crate) struct Markdown {
    ui: Ui,
    pending: String,
    fence: Option<String>,
    last_blank: bool,
}

impl Markdown {
    pub(crate) fn new(ui: Ui) -> Self {
        Self {
            ui,
            pending: String::new(),
            fence: None,
            last_blank: true,
        }
    }

    /// Whether a partial line is waiting for its newline.
    pub(crate) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Adds a fragment and returns the finished lines, rendered.
    pub(crate) fn push(&mut self, fragment: &str) -> String {
        self.pending.push_str(fragment);
        let mut out = String::new();
        while let Some(end) = self.pending.find('\n') {
            let line: String = self.pending[..end].trim_end_matches('\r').to_owned();
            self.pending.drain(..=end);
            self.line(&line, &mut out);
        }
        out
    }

    /// Renders the unfinished last line.
    pub(crate) fn finish(&mut self) -> String {
        let mut out = String::new();
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.line(&line, &mut out);
        }
        out
    }

    fn measure(&self) -> usize {
        self.ui
            .width
            .min(MAX_MEASURE)
            .saturating_sub(INDENT.len())
            .max(20)
    }

    fn blank(&mut self, out: &mut String) {
        if !self.last_blank {
            out.push('\n');
            self.last_blank = true;
        }
    }

    fn emit(&mut self, out: &mut String, lines: Vec<String>) {
        for line in lines {
            out.push_str(INDENT);
            out.push_str(&line);
            out.push('\n');
        }
        self.last_blank = false;
    }

    fn line(&mut self, line: &str, out: &mut String) {
        let ui = self.ui;
        let trimmed = line.trim_start();
        if let Some(open) = self.fence.clone() {
            if trimmed.starts_with(&open)
                && trimmed
                    .trim_end_matches(open.chars().next().unwrap_or('`'))
                    .is_empty()
            {
                self.fence = None;
                return;
            }
            let bar = ui.paint(Role::Dim, ui.glyph(Glyph::Quote));
            let code = ui.paint_style(code_style(), &super::clean(line));
            self.emit(out, vec![format!("{bar} {code}")]);
            return;
        }
        if let Some(marker) = fence_marker(trimmed) {
            let lang = trimmed
                .trim_start_matches(marker.chars().next().unwrap_or('`'))
                .trim();
            self.fence = Some(marker);
            if !lang.is_empty() {
                let label = ui.paint(Role::Dim, &super::clean(lang));
                self.emit(out, vec![label]);
            }
            return;
        }
        if trimmed.is_empty() {
            self.blank(out);
            return;
        }
        if let Some((level, title)) = heading(trimmed) {
            self.blank(out);
            let style = if level <= 2 {
                ui.style(Role::Brand).effects(Effects::BOLD)
            } else {
                ui.style(Role::Strong)
            };
            let spans = inline(&ui, title, style);
            let lines = wrap_spans(&spans, self.measure(), self.measure());
            let rendered = lines.iter().map(|spans| ui.render(spans)).collect();
            self.emit(out, rendered);
            return;
        }
        if is_rule(trimmed) {
            self.blank(out);
            let rule = ui.rule(self.measure().min(40));
            self.emit(out, vec![rule]);
            return;
        }
        if let Some(rest) = trimmed.strip_prefix('>') {
            let quote = ui.paint(Role::Dim, ui.glyph(Glyph::Quote));
            let style = ui.style(Role::Dim);
            let spans = inline(&ui, rest.trim_start(), style);
            let lines = wrap_spans(&spans, self.measure() - 2, self.measure() - 2);
            let rendered = lines
                .iter()
                .map(|spans| format!("{quote} {}", ui.render(spans)))
                .collect();
            self.emit(out, rendered);
            return;
        }
        if let Some((depth, marker, rest)) = list_item(line) {
            let pad = "  ".repeat(depth);
            let bullet = match marker {
                Marker::Bullet if depth == 0 => ui.paint(Role::Brand, ui.glyph(Glyph::Bullet)),
                Marker::Bullet => ui.paint(Role::Dim, ui.glyph(Glyph::Bullet)),
                Marker::Number(number) => ui.paint(Role::Dim, &format!("{number}.")),
            };
            let hang = pad.len() + super::text::visible_width(&bullet) + 1;
            let spans = inline(&ui, rest, Style::new());
            let room = self.measure().saturating_sub(hang).max(10);
            let lines = wrap_spans(&spans, room, room);
            let rendered = lines
                .iter()
                .enumerate()
                .map(|(index, spans)| {
                    if index == 0 {
                        format!("{pad}{bullet} {}", ui.render(spans))
                    } else {
                        format!("{}{}", " ".repeat(hang), ui.render(spans))
                    }
                })
                .collect();
            self.emit(out, rendered);
            return;
        }
        if trimmed.starts_with('|') {
            // Table rows keep their pipes; the separator row becomes a rule.
            let rendered = if trimmed.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
                ui.rule(super::text::width(trimmed).min(self.measure()))
            } else {
                ui.render(&inline(&ui, trimmed, Style::new()))
            };
            self.emit(out, vec![rendered]);
            return;
        }
        let spans = inline(&ui, trimmed, Style::new());
        let lines = wrap_spans(&spans, self.measure(), self.measure());
        let rendered = lines.iter().map(|spans| ui.render(spans)).collect();
        self.emit(out, rendered);
    }
}

fn code_style() -> Style {
    Style::new().fg_color(Some(AnsiColor::Cyan.into()))
}

fn fence_marker(line: &str) -> Option<String> {
    for marker in ['`', '~'] {
        let count = line.chars().take_while(|c| *c == marker).count();
        if count >= 3 {
            return Some(marker.to_string().repeat(count));
        }
    }
    None
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line.chars().take_while(|c| *c == '#').count();
    let rest = line.get(level..)?;
    ((1..=6).contains(&level) && rest.starts_with(' '))
        .then(|| (level, rest.trim().trim_end_matches('#').trim_end()))
}

fn is_rule(line: &str) -> bool {
    let mut chars = line.chars().filter(|c| !c.is_whitespace());
    let Some(first) = chars.next() else {
        return false;
    };
    matches!(first, '-' | '*' | '_')
        && line.chars().filter(|c| !c.is_whitespace()).count() >= 3
        && line.chars().all(|c| c == first || c.is_whitespace())
}

enum Marker {
    Bullet,
    Number(u32),
}

fn list_item(line: &str) -> Option<(usize, Marker, &str)> {
    let indent = line.chars().take_while(|c| *c == ' ').count();
    let rest = &line[indent..];
    let depth = (indent / 2).min(4);
    for bullet in ["- ", "* ", "+ "] {
        if let Some(text) = rest.strip_prefix(bullet) {
            return Some((depth, Marker::Bullet, text.trim_start()));
        }
    }
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if (1..=9).contains(&digits) {
        let after = &rest[digits..];
        if let Some(text) = after
            .strip_prefix(". ")
            .or_else(|| after.strip_prefix(") "))
        {
            return Some((
                depth,
                Marker::Number(rest[..digits].parse().ok()?),
                text.trim_start(),
            ));
        }
    }
    None
}

fn with_effect(style: Style, effect: Effects) -> Style {
    style.effects(style.get_effects() | effect)
}

/// Parses the inline markers of one line into styled spans.
pub(crate) fn inline(ui: &Ui, text: &str, base: Style) -> Vec<Span> {
    let chars: Vec<char> = super::clean(text).chars().collect();
    let mut spans = Vec::new();
    parse(ui, &chars, base, &mut spans);
    // Merge neighbours with the same style so wrapping sees whole words.
    let mut merged: Vec<Span> = Vec::new();
    for span in spans {
        match merged.last_mut() {
            Some(last) if last.style == span.style => last.text.push_str(&span.text),
            _ => merged.push(span),
        }
    }
    merged
}

fn find(chars: &[char], from: usize, needle: &[char]) -> Option<usize> {
    if needle.is_empty() || chars.len() < needle.len() {
        return None;
    }
    (from..=chars.len() - needle.len()).find(|&at| chars[at..at + needle.len()] == *needle)
}

fn parse(ui: &Ui, chars: &[char], style: Style, out: &mut Vec<Span>) {
    let markers = !ui.color;
    let mut buffer = String::new();
    let flush = |buffer: &mut String, out: &mut Vec<Span>| {
        if !buffer.is_empty() {
            out.push(Span::new(std::mem::take(buffer), style));
        }
    };
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        let next = chars.get(at + 1).copied();
        let prev_word = at > 0 && chars[at - 1].is_alphanumeric();
        if c == '\\' && next.is_some_and(|n| n.is_ascii_punctuation()) {
            buffer.push(chars[at + 1]);
            at += 2;
            continue;
        }
        if c == '`' {
            let ticks = chars[at..].iter().take_while(|x| **x == '`').count();
            let fence = vec!['`'; ticks];
            if let Some(end) = find(chars, at + ticks, &fence) {
                flush(&mut buffer, out);
                let code: String = chars[at + ticks..end].iter().collect();
                let code = code.trim().to_owned();
                let text = if markers { format!("`{code}`") } else { code };
                out.push(Span::new(text, code_style()));
                at = end + ticks;
                continue;
            }
        }
        if (c == '*' || c == '_') && next == Some(c) {
            let delimiter = [c, c];
            if let Some(end) = find(chars, at + 2, &delimiter).filter(|end| *end > at + 2) {
                if !chars[at + 2].is_whitespace() && !(c == '_' && prev_word) {
                    flush(&mut buffer, out);
                    if markers {
                        out.push(Span::new(format!("{c}{c}"), style));
                    }
                    parse(
                        ui,
                        &chars[at + 2..end],
                        with_effect(style, Effects::BOLD),
                        out,
                    );
                    if markers {
                        out.push(Span::new(format!("{c}{c}"), style));
                    }
                    at = end + 2;
                    continue;
                }
            }
        }
        if (c == '*' || c == '_')
            && next.is_some_and(|n| n != c && !n.is_whitespace())
            && !(c == '_' && prev_word)
        {
            let closing = (at + 2..chars.len()).find(|&end| {
                chars[end] == c
                    && !chars[end - 1].is_whitespace()
                    && chars.get(end + 1) != Some(&c)
                    && !(c == '_' && chars.get(end + 1).is_some_and(|n| n.is_alphanumeric()))
            });
            if let Some(end) = closing {
                flush(&mut buffer, out);
                if markers {
                    out.push(Span::new(c.to_string(), style));
                }
                parse(
                    ui,
                    &chars[at + 1..end],
                    with_effect(style, Effects::ITALIC),
                    out,
                );
                if markers {
                    out.push(Span::new(c.to_string(), style));
                }
                at = end + 1;
                continue;
            }
        }
        if c == '[' {
            if let Some(close) = find(chars, at + 1, &[']', '(']) {
                if let Some(end) = find(chars, close + 2, &[')']) {
                    let label: Vec<char> = chars[at + 1..close].to_vec();
                    let url: String = chars[close + 2..end].iter().collect();
                    if !label.is_empty() && !url.contains(char::is_whitespace) {
                        flush(&mut buffer, out);
                        let label_text: String = label.iter().collect();
                        parse(ui, &label, with_effect(style, Effects::UNDERLINE), out);
                        if label_text != url {
                            out.push(Span::new(format!(" ({url})"), ui.style(Role::Dim)));
                        }
                        at = end + 1;
                        continue;
                    }
                }
            }
        }
        buffer.push(c);
        at += 1;
    }
    flush(&mut buffer, out);
}
