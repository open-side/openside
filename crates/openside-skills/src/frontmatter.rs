//! `SKILL.md` = `---` frontmatter `---` Markdown body.
//!
//! The frontmatter is a deliberately small YAML subset parsed by hand, so hostile
//! documents cannot reach a general YAML engine (anchors, aliases, tags, merge keys,
//! multi-document streams are all rejected). Supported values: plain, single-quoted
//! and double-quoted single-line scalars; plain scalars continued on indented lines;
//! `|`/`>` block scalars (chomping `-`/`+`, no indentation indicator); single-line
//! flow lists `[a, "b"]`; block lists of scalars; one-level maps of scalars.
//! A plain scalar that contains `: ` or ends with `:` is an error (quote it) instead
//! of being silently reinterpreted.

use std::collections::BTreeMap;

use crate::error::{Error, Result};

/// Largest accepted `SKILL.md`. The model-facing tool output limit is 32 KiB, so a
/// loaded skill always fits whole.
pub const MAX_SKILL_MD_BYTES: usize = 28 * 1024;
/// Largest accepted frontmatter block.
pub const MAX_FRONTMATTER_BYTES: usize = 8 * 1024;
/// Longest accepted skill name.
pub const MAX_NAME_CHARS: usize = 64;
/// Longest accepted description.
pub const MAX_DESCRIPTION_CHARS: usize = 1024;

// Worst case is a 1024-character value of 4-byte characters plus its key.
const MAX_LINE_BYTES: usize = 6 * 1024;
const MAX_COMPATIBILITY_CHARS: usize = 500;
const MAX_LICENSE_CHARS: usize = 256;
const MAX_KEYS: usize = 32;
const MAX_ITEMS: usize = 64;
const MAX_VALUE_CHARS: usize = 1024;
const MAX_KEY_CHARS: usize = 64;
const KNOWN_KEYS: [&str; 6] = [
    "name",
    "description",
    "license",
    "compatibility",
    "allowed-tools",
    "metadata",
];

/// A frontmatter value outside the known fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// Single string.
    Scalar(String),
    /// List of strings.
    List(Vec<String>),
    /// One-level string map.
    Map(BTreeMap<String, String>),
}

/// Validated frontmatter. Unknown keys survive in [`Frontmatter::extra`] so editing a
/// skill never drops another agent's fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frontmatter {
    /// Lowercase letters, digits and single hyphens; also the directory name.
    pub name: String,
    /// What the skill does and when to use it; shown in the prompt index.
    pub description: String,
    /// SPDX id or free text.
    pub license: Option<String>,
    /// Environment requirements, free text.
    pub compatibility: Option<String>,
    /// Tools a skill author lists for other agents; openside never grants tools from it.
    pub allowed_tools: Vec<String>,
    /// Free-form string map.
    pub metadata: BTreeMap<String, String>,
    /// Any other top-level key.
    pub extra: BTreeMap<String, Value>,
}

/// Parsed `SKILL.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    /// Validated frontmatter.
    pub frontmatter: Frontmatter,
    /// Everything after the closing `---` line, verbatim (CRLF normalised to LF).
    pub body: String,
}

/// Check a skill name against the Agent Skills naming rule.
pub fn validate_name(name: &str) -> Result<()> {
    let bad = |message: &str| Err(Error::invalid("name", message));
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return bad("must be 1-64 characters");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return bad("may only contain lowercase letters, digits and hyphens");
    }
    if name.starts_with('-') || name.ends_with('-') || name.contains("--") {
        return bad("must not start or end with a hyphen or contain `--`");
    }
    Ok(())
}

/// Split `SKILL.md` into normalised frontmatter text (without delimiters) and body.
pub fn split(text: &str) -> Result<(String, String)> {
    if text.len() > MAX_SKILL_MD_BYTES {
        return Err(Error::TooLarge {
            what: "SKILL.md",
            max: MAX_SKILL_MD_BYTES,
        });
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.contains('\0') {
        return Err(Error::invalid("SKILL.md", "contains a NUL byte"));
    }
    let text = text.replace("\r\n", "\n");
    let rest = text
        .strip_prefix("---\n")
        .ok_or(Error::MissingFrontmatter)?;
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line == "---\n" || line == "---" {
            let front = &rest[..offset];
            if front.len() > MAX_FRONTMATTER_BYTES {
                return Err(Error::TooLarge {
                    what: "frontmatter",
                    max: MAX_FRONTMATTER_BYTES,
                });
            }
            return Ok((front.to_owned(), rest[offset + line.len()..].to_owned()));
        }
        offset += line.len();
        // Bound the scan: an unterminated block stops at the frontmatter limit.
        if offset > MAX_FRONTMATTER_BYTES {
            return Err(Error::TooLarge {
                what: "frontmatter",
                max: MAX_FRONTMATTER_BYTES,
            });
        }
    }
    Err(Error::UnterminatedFrontmatter)
}

/// Parse and validate a whole `SKILL.md`.
pub fn parse(text: &str) -> Result<Document> {
    let (front, body) = split(text)?;
    let frontmatter = Frontmatter::from_entries(parse_entries(&front)?)?;
    Ok(Document { frontmatter, body })
}

/// Replace only the body, keeping the original frontmatter bytes (comments, block
/// scalars) untouched. The existing document must itself be valid.
pub fn replace_body(text: &str, body: &str) -> Result<String> {
    parse(text)?;
    let (front, _) = split(text)?;
    let out = format!("---\n{front}---\n{body}");
    parse(&out)?;
    Ok(out)
}

/// Serialise a document. The result always parses back to an equal [`Document`].
pub fn render(document: &Document) -> Result<String> {
    document.frontmatter.validate()?;
    let f = &document.frontmatter;
    let mut out = String::from("---\n");
    out.push_str(&format!("name: {}\n", scalar(&f.name)));
    out.push_str(&format!("description: {}\n", scalar(&f.description)));
    if let Some(license) = &f.license {
        out.push_str(&format!("license: {}\n", scalar(license)));
    }
    if let Some(compatibility) = &f.compatibility {
        out.push_str(&format!("compatibility: {}\n", scalar(compatibility)));
    }
    if !f.allowed_tools.is_empty() {
        out.push_str("allowed-tools:\n");
        for tool in &f.allowed_tools {
            out.push_str(&format!("  - {}\n", scalar(tool)));
        }
    }
    if !f.metadata.is_empty() {
        out.push_str("metadata:\n");
        for (key, value) in &f.metadata {
            out.push_str(&format!("  {key}: {}\n", scalar(value)));
        }
    }
    for (key, value) in &f.extra {
        match value {
            Value::Scalar(text) => out.push_str(&format!("{key}: {}\n", scalar(text))),
            Value::List(items) if items.is_empty() => out.push_str(&format!("{key}: []\n")),
            Value::List(items) => {
                out.push_str(&format!("{key}:\n"));
                for item in items {
                    out.push_str(&format!("  - {}\n", scalar(item)));
                }
            }
            Value::Map(map) if map.is_empty() => out.push_str(&format!("{key}: {{}}\n")),
            Value::Map(map) => {
                out.push_str(&format!("{key}:\n"));
                for (k, v) in map {
                    out.push_str(&format!("  {k}: {}\n", scalar(v)));
                }
            }
        }
    }
    out.push_str("---\n");
    out.push_str(&document.body);
    // Guarantees every rendered document passes the same size limits as `parse`.
    split(&out)?;
    Ok(out)
}

impl Frontmatter {
    fn from_entries(entries: Vec<(String, Value)>) -> Result<Self> {
        let mut map: BTreeMap<String, Value> = entries.into_iter().collect();
        let mut scalar_field = |key: &str, required: bool| -> Result<Option<String>> {
            match map.remove(key) {
                None if required => Err(Error::invalid(key, "is required")),
                None => Ok(None),
                Some(Value::Scalar(text)) => Ok(Some(text)),
                Some(_) => Err(Error::invalid(key, "expected a single text value")),
            }
        };
        let name = scalar_field("name", true)?.unwrap_or_default();
        let description = scalar_field("description", true)?.unwrap_or_default();
        let license = scalar_field("license", false)?.filter(|text| !text.is_empty());
        let compatibility = scalar_field("compatibility", false)?.filter(|text| !text.is_empty());
        let allowed_tools = match map.remove("allowed-tools") {
            None => Vec::new(),
            Some(Value::Scalar(text)) => text
                .split(|c: char| c.is_whitespace() || c == ',')
                .filter(|part| !part.is_empty())
                .map(str::to_owned)
                .collect(),
            Some(Value::List(items)) => items,
            Some(Value::Map(_)) => {
                return Err(Error::invalid("allowed-tools", "expected a list or text"))
            }
        };
        let metadata = match map.remove("metadata") {
            None => BTreeMap::new(),
            Some(Value::Scalar(text)) if text.is_empty() => BTreeMap::new(),
            Some(Value::Map(entries)) => entries,
            Some(_) => return Err(Error::invalid("metadata", "expected key: value pairs")),
        };
        let frontmatter = Self {
            name,
            description,
            license,
            compatibility,
            allowed_tools,
            metadata,
            extra: map,
        };
        frontmatter.validate()?;
        Ok(frontmatter)
    }

    /// Enforce every documented rule; [`render`] and [`parse`] share it.
    pub fn validate(&self) -> Result<()> {
        validate_name(&self.name)?;
        text_field("description", &self.description, MAX_DESCRIPTION_CHARS)?;
        if self.description.trim().is_empty() {
            return Err(Error::invalid("description", "must not be empty"));
        }
        for (field, value, max) in [
            ("license", &self.license, MAX_LICENSE_CHARS),
            (
                "compatibility",
                &self.compatibility,
                MAX_COMPATIBILITY_CHARS,
            ),
        ] {
            if let Some(value) = value {
                if value.is_empty() {
                    return Err(Error::invalid(field, "must not be empty"));
                }
                text_field(field, value, max)?;
            }
        }
        if self.allowed_tools.len() > MAX_ITEMS {
            return Err(Error::invalid("allowed-tools", "too many entries"));
        }
        for tool in &self.allowed_tools {
            if tool.trim().is_empty() {
                return Err(Error::invalid("allowed-tools", "entries must not be empty"));
            }
            text_field("allowed-tools", tool, MAX_VALUE_CHARS)?;
        }
        validate_map("metadata", &self.metadata)?;
        if self.extra.len() + KNOWN_KEYS.len() > MAX_KEYS {
            return Err(Error::invalid("frontmatter", "too many keys"));
        }
        for (key, value) in &self.extra {
            validate_key(key)?;
            if KNOWN_KEYS.contains(&key.as_str()) {
                return Err(Error::invalid(key.as_str(), "is a reserved field"));
            }
            match value {
                Value::Scalar(text) => text_field(key, text, MAX_VALUE_CHARS)?,
                Value::List(items) => {
                    if items.len() > MAX_ITEMS {
                        return Err(Error::invalid(key.as_str(), "too many entries"));
                    }
                    for item in items {
                        text_field(key, item, MAX_VALUE_CHARS)?;
                    }
                }
                Value::Map(map) => validate_map(key, map)?,
            }
        }
        Ok(())
    }
}

fn validate_map(field: &str, map: &BTreeMap<String, String>) -> Result<()> {
    if map.len() > MAX_ITEMS {
        return Err(Error::invalid(field, "too many entries"));
    }
    for (key, value) in map {
        validate_key(key)?;
        text_field(field, value, MAX_VALUE_CHARS)?;
    }
    Ok(())
}

fn validate_key(key: &str) -> Result<()> {
    let mut chars = key.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
    if !first_ok
        || key.chars().count() > MAX_KEY_CHARS
        || !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return Err(Error::invalid(
            "frontmatter",
            format!("key `{key}` must be 1-64 letters, digits, `_`, `-` or `.`"),
        ));
    }
    Ok(())
}

fn text_field(field: &str, value: &str, max_chars: usize) -> Result<()> {
    if value.chars().count() > max_chars {
        return Err(Error::invalid(
            field,
            format!("must be at most {max_chars} characters"),
        ));
    }
    if value
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(Error::invalid(field, "contains a control character"));
    }
    Ok(())
}

fn syntax(line: usize, message: impl Into<String>) -> Error {
    Error::Syntax {
        line,
        message: message.into(),
    }
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

fn is_skippable(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.is_empty() || trimmed.starts_with('#')
}

/// Split `key: rest` (or `key:`); the key must be a bare identifier.
fn split_key(line: &str) -> Option<(&str, &str)> {
    let colon = line.find(':')?;
    let (key, rest) = (&line[..colon], &line[colon + 1..]);
    let key_trimmed = key.trim();
    if key_trimmed.is_empty()
        || key_trimmed != key.trim_start()
        || !rest.is_empty() && !rest.starts_with(' ')
    {
        return None;
    }
    validate_key(key_trimmed).ok()?;
    Some((key_trimmed, rest))
}

fn parse_entries(front: &str) -> Result<Vec<(String, Value)>> {
    let lines: Vec<&str> = front
        .strip_suffix('\n')
        .unwrap_or(front)
        .split('\n')
        .collect();
    for (index, line) in lines.iter().enumerate() {
        if line.len() > MAX_LINE_BYTES {
            return Err(syntax(index + 2, "line is too long"));
        }
        if line.contains('\r') {
            return Err(syntax(index + 2, "carriage returns are not allowed"));
        }
        if !is_skippable(line) && line.trim_start_matches(' ').starts_with('\t') {
            return Err(syntax(index + 2, "tabs cannot indent lines"));
        }
    }
    let mut out: Vec<(String, Value)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let number = i + 2;
        if is_skippable(line) {
            i += 1;
            continue;
        }
        if indent_of(line) != 0 {
            return Err(syntax(number, "unexpected indentation"));
        }
        let (key, rest) = split_key(line).ok_or_else(|| syntax(number, "expected `key: value`"))?;
        if out.iter().any(|(existing, _)| existing == key) {
            return Err(syntax(number, format!("duplicate key `{key}`")));
        }
        if out.len() >= MAX_KEYS {
            return Err(syntax(number, "too many keys"));
        }
        let (value, next) = parse_value(&lines, i + 1, rest, number)?;
        out.push((key.to_owned(), value));
        i = next;
    }
    Ok(out)
}

/// End (exclusive) of the lines that belong to the entry whose header is just before `from`.
fn nested_end(lines: &[&str], from: usize) -> usize {
    let mut end = from;
    while end < lines.len() && (is_blank(lines[end]) || indent_of(lines[end]) > 0) {
        end += 1;
    }
    end
}

fn parse_value(lines: &[&str], from: usize, rest: &str, number: usize) -> Result<(Value, usize)> {
    let head = rest.trim();
    if head.is_empty() || head.starts_with('#') {
        return parse_nested(lines, from, number);
    }
    if head.starts_with('|') || head.starts_with('>') {
        return parse_block_scalar(lines, from, head, number);
    }
    if head.starts_with('[') {
        return Ok((Value::List(parse_flow_list(head, number)?), from));
    }
    if let Some(rest) = head.strip_prefix('{') {
        let tail = rest.trim_start();
        return match tail.strip_prefix('}') {
            Some(after) if is_skippable(after) => Ok((Value::Map(BTreeMap::new()), from)),
            _ => Err(syntax(number, "flow mappings are not supported")),
        };
    }
    let (text, plain) = parse_inline(head, number)?;
    if !plain {
        return Ok((Value::Scalar(text), from));
    }
    let mut text = text;
    let mut next = from;
    while next < lines.len() && indent_of(lines[next]) > 0 && !is_skippable(lines[next]) {
        let piece = cut_comment(lines[next].trim());
        check_plain(piece, next + 2)?;
        if !piece.is_empty() {
            text.push(' ');
            text.push_str(piece);
        }
        next += 1;
    }
    Ok((Value::Scalar(text), next))
}

fn parse_nested(lines: &[&str], from: usize, number: usize) -> Result<(Value, usize)> {
    let end = nested_end(lines, from);
    let nested: Vec<(usize, &str)> = (from..end)
        .filter(|&i| !is_skippable(lines[i]))
        .map(|i| (i, lines[i]))
        .collect();
    let Some(&(_, first)) = nested.first() else {
        return Ok((Value::Scalar(String::new()), end));
    };
    let indent = indent_of(first);
    let is_list = first.trim() == "-" || first.trim_start().starts_with("- ");
    if nested.len() > MAX_ITEMS {
        return Err(syntax(number, "too many entries"));
    }
    if is_list {
        let mut items = Vec::new();
        for (i, line) in nested {
            let item_number = i + 2;
            let trimmed = line.trim_start();
            if indent_of(line) != indent || !(trimmed == "-" || trimmed.starts_with("- ")) {
                return Err(syntax(
                    item_number,
                    "expected a `- ` list item at the same indentation",
                ));
            }
            let item = trimmed[1..].trim();
            items.push(if item.is_empty() {
                String::new()
            } else {
                parse_inline(item, item_number)?.0
            });
        }
        return Ok((Value::List(items), end));
    }
    let mut map = BTreeMap::new();
    for (i, line) in nested {
        let item_number = i + 2;
        if indent_of(line) != indent {
            return Err(syntax(item_number, "inconsistent indentation"));
        }
        let (key, value) = split_key(line.trim_start())
            .ok_or_else(|| syntax(item_number, "expected `key: value`"))?;
        let value = value.trim();
        let text = if value.is_empty() || value.starts_with('#') {
            String::new()
        } else {
            parse_inline(value, item_number)?.0
        };
        if map.insert(key.to_owned(), text).is_some() {
            return Err(syntax(item_number, format!("duplicate key `{key}`")));
        }
    }
    Ok((Value::Map(map), end))
}

fn parse_block_scalar(
    lines: &[&str],
    from: usize,
    head: &str,
    number: usize,
) -> Result<(Value, usize)> {
    let folded = head.starts_with('>');
    let mut chomp = '\0';
    let mut tail = &head[1..];
    if let Some(c) = tail.chars().next().filter(|c| matches!(c, '-' | '+')) {
        chomp = c;
        tail = &tail[1..];
    }
    if !is_skippable(tail) || tail.trim_start().starts_with(|c: char| c.is_ascii_digit()) {
        return Err(syntax(number, "unsupported block scalar header"));
    }
    let end = nested_end(lines, from);
    let region = &lines[from..end];
    let content_indent = region
        .iter()
        .find(|line| !is_blank(line))
        .map(|line| indent_of(line));
    let Some(content_indent) = content_indent else {
        let keep = if chomp == '+' {
            "\n".repeat(region.len())
        } else {
            String::new()
        };
        return Ok((Value::Scalar(keep), end));
    };
    let mut content: Vec<String> = Vec::new();
    for (offset, line) in region.iter().enumerate() {
        if is_blank(line) {
            content.push(String::new());
        } else if indent_of(line) < content_indent {
            return Err(syntax(
                from + offset + 2,
                "block scalar line is indented less than its first line",
            ));
        } else {
            content.push(line[content_indent..].to_owned());
        }
    }
    let mut trailing_blank = 0;
    while content.last().is_some_and(String::is_empty) {
        content.pop();
        trailing_blank += 1;
    }
    let mut text = if folded {
        fold(&content)
    } else {
        content.join("\n")
    };
    match chomp {
        '-' => {}
        '+' => text.push_str(&"\n".repeat(1 + trailing_blank)),
        _ => text.push('\n'),
    }
    Ok((Value::Scalar(text), end))
}

fn fold(lines: &[String]) -> String {
    let mut out = String::new();
    let mut previous: Option<&str> = None;
    let mut blanks = 0;
    for line in lines {
        if line.is_empty() {
            blanks += 1;
            continue;
        }
        match previous {
            Some(prev) => {
                let more_indented = prev.starts_with(' ') || line.starts_with(' ');
                if blanks > 0 {
                    out.push_str(&"\n".repeat(blanks + usize::from(more_indented)));
                } else if more_indented {
                    out.push('\n');
                } else {
                    out.push(' ');
                }
            }
            None => out.push_str(&"\n".repeat(blanks)),
        }
        out.push_str(line);
        previous = Some(line);
        blanks = 0;
    }
    out
}

fn cut_comment(text: &str) -> &str {
    let bytes = text.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'#' && (i == 0 || bytes[i - 1] == b' ') {
            return text[..i].trim_end();
        }
    }
    text
}

fn check_plain(text: &str, line: usize) -> Result<()> {
    if text.contains(": ") || text.ends_with(':') {
        return Err(syntax(line, "plain text containing `: ` must be quoted"));
    }
    Ok(())
}

/// One single-line scalar; returns `(text, was_plain)`.
fn parse_inline(text: &str, line: usize) -> Result<(String, bool)> {
    match text.chars().next() {
        Some('"') => {
            let (value, rest) = parse_double(text, line)?;
            end_of_value(rest, line)?;
            Ok((value, false))
        }
        Some('\'') => {
            let (value, rest) = parse_single(text, line)?;
            end_of_value(rest, line)?;
            Ok((value, false))
        }
        Some('[' | '{' | '&' | '*' | '!' | '|' | '>' | '%' | '@' | '`') => Err(syntax(
            line,
            "unsupported YAML syntax here; put the text in quotes",
        )),
        _ => {
            let plain = cut_comment(text);
            check_plain(plain, line)?;
            Ok((plain.to_owned(), true))
        }
    }
}

fn end_of_value(rest: &str, line: usize) -> Result<()> {
    if is_skippable(rest) {
        Ok(())
    } else {
        Err(syntax(line, "unexpected text after a quoted value"))
    }
}

fn parse_double(text: &str, line: usize) -> Result<(String, &str)> {
    let mut out = String::new();
    let mut chars = text[1..].char_indices();
    while let Some((index, c)) = chars.next() {
        match c {
            '"' => return Ok((out, &text[index + 2..])),
            '\\' => {
                let (_, escape) = chars
                    .next()
                    .ok_or_else(|| syntax(line, "unterminated quoted value"))?;
                match escape {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    '"' | '\\' | '/' | ' ' => out.push(escape),
                    'x' | 'u' => {
                        let width = if escape == 'x' { 2 } else { 4 };
                        let digits: String = chars.by_ref().take(width).map(|(_, c)| c).collect();
                        let decoded = (digits.len() == width)
                            .then(|| u32::from_str_radix(&digits, 16).ok())
                            .flatten()
                            .and_then(char::from_u32)
                            .ok_or_else(|| syntax(line, "invalid escape sequence"))?;
                        out.push(decoded);
                    }
                    _ => return Err(syntax(line, "unsupported escape sequence")),
                }
            }
            c => out.push(c),
        }
    }
    Err(syntax(line, "unterminated quoted value"))
}

fn parse_single(text: &str, line: usize) -> Result<(String, &str)> {
    let mut out = String::new();
    let mut chars = text[1..].char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        if c == '\'' {
            if chars.next_if(|&(_, next)| next == '\'').is_some() {
                out.push('\'');
            } else {
                return Ok((out, &text[index + 2..]));
            }
        } else {
            out.push(c);
        }
    }
    Err(syntax(line, "unterminated quoted value"))
}

fn parse_flow_list(text: &str, line: usize) -> Result<Vec<String>> {
    let mut rest = text[1..].trim_start();
    let mut items = Vec::new();
    loop {
        if let Some(after) = rest.strip_prefix(']') {
            end_of_value(after, line)?;
            return Ok(items);
        }
        if items.len() >= MAX_ITEMS {
            return Err(syntax(line, "too many entries"));
        }
        let (item, after) = match rest.chars().next() {
            Some('"') => parse_double(rest, line)?,
            Some('\'') => parse_single(rest, line)?,
            Some(_) => {
                let end = rest
                    .find([',', ']'])
                    .ok_or_else(|| syntax(line, "unterminated list"))?;
                let piece = rest[..end].trim();
                if piece.is_empty() || piece.contains(['[', '{']) {
                    return Err(syntax(line, "invalid list entry"));
                }
                check_plain(piece, line)?;
                (piece.to_owned(), &rest[end..])
            }
            None => return Err(syntax(line, "unterminated list")),
        };
        items.push(item);
        rest = after.trim_start();
        if let Some(after) = rest.strip_prefix(',') {
            rest = after.trim_start();
        } else if !rest.starts_with(']') {
            return Err(syntax(line, "expected `,` or `]`"));
        }
    }
}

fn scalar(text: &str) -> String {
    if is_plain_safe(text) {
        text.to_owned()
    } else {
        quote(text)
    }
}

fn is_plain_safe(text: &str) -> bool {
    const RESERVED: [&str; 9] = ["true", "false", "yes", "no", "on", "off", "null", "y", "n"];
    text.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && !text.ends_with(' ')
        && text.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, ' ' | '.' | '_' | '/' | '-' | '+' | '(' | ')' | ',')
        })
        && !RESERVED.contains(&text.to_ascii_lowercase().as_str())
}

fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
