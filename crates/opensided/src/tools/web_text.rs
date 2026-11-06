//! Readable text for `webfetch` and the page scripts `websearch`/`webfetch` run in the agent
//! browser's isolated world. Page content is untrusted data: the output is plain text, never
//! markup the app renders.

/// Elements whose content is never readable text.
const SKIPPED: &[&str] = &[
    "script", "style", "noscript", "template", "svg", "iframe", "object", "canvas", "nav",
    "select", "button",
];

/// Elements that start a new line.
const BLOCKS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "br",
    "dd",
    "div",
    "dl",
    "dt",
    "figcaption",
    "figure",
    "footer",
    "form",
    "header",
    "hr",
    "li",
    "main",
    "ol",
    "p",
    "pre",
    "section",
    "table",
    "tr",
    "ul",
];

/// Title and readable text of an HTML document. The text of `<main>` or `<article>` wins
/// when it carries enough of the page, so menus and footers do not crowd out the content.
pub(crate) fn readable(html: &str) -> (Option<String>, String) {
    let mut all = Text::default();
    let mut focused = Text::default();
    let mut title = String::new();
    let mut in_title = false;
    let mut focus_depth = 0usize;
    let mut rest = html;
    while let Some(at) = rest.find('<') {
        let text = &rest[..at];
        if in_title {
            title.push_str(text);
        } else {
            all.push(text);
            if focus_depth > 0 {
                focused.push(text);
            }
        }
        rest = &rest[at..];
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.find("-->").map_or("", |end| &after[end + 3..]);
            continue;
        }
        let Some((tag, after)) = tag(rest) else {
            // A lone `<` is text.
            if !in_title {
                all.push("<");
                if focus_depth > 0 {
                    focused.push("<");
                }
            }
            rest = &rest[1..];
            continue;
        };
        rest = after;
        if tag.name == "title" {
            in_title = !tag.closing;
            continue;
        }
        if !tag.closing && SKIPPED.contains(&tag.name.as_str()) && !tag.self_closing {
            // Scan for the closing tag itself: script text is not markup and may hold
            // quotes or `<` that would derail tag parsing.
            rest = find_ci(rest, &format!("</{}", tag.name))
                .and_then(|end| rest[end..].find('>').map(|close| &rest[end + close + 1..]))
                .unwrap_or("");
            continue;
        }
        if matches!(tag.name.as_str(), "main" | "article") && tag.closing {
            focus_depth = focus_depth.saturating_sub(1);
        }
        structure(&mut all, &tag);
        if focus_depth > 0 {
            structure(&mut focused, &tag);
        }
        if matches!(tag.name.as_str(), "main" | "article") && !tag.closing {
            focus_depth += 1;
        }
    }
    if !in_title {
        all.push(rest);
        if focus_depth > 0 {
            focused.push(rest);
        }
    }
    let title = collapse(&decode(&title));
    let focused = focused.finish();
    let text = if focused.chars().count() >= 200 {
        focused
    } else {
        all.finish()
    };
    ((!title.is_empty()).then_some(title), text)
}

fn structure(text: &mut Text, tag: &Tag) {
    if let Some(level) = heading(&tag.name) {
        text.newline(2);
        if !tag.closing {
            text.raw(&format!("{} ", "#".repeat(level)));
        }
    } else if tag.name == "li" && !tag.closing {
        text.newline(1);
        text.raw("- ");
    } else if matches!(tag.name.as_str(), "p" | "blockquote" | "pre") {
        text.newline(2);
    } else if BLOCKS.contains(&tag.name.as_str()) {
        text.newline(1);
    } else if matches!(tag.name.as_str(), "td" | "th") && !tag.closing {
        text.space();
    }
}

/// Byte offset of the first ASCII-case-insensitive occurrence of `needle` (ASCII) in `haystack`.
fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let needle = needle.as_bytes();
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
}

struct Tag {
    name: String,
    closing: bool,
    self_closing: bool,
}

/// The tag at the start of `input` (which starts with `<`) and the text after it.
fn tag(input: &str) -> Option<(Tag, &str)> {
    let body = &input[1..];
    let (closing, body) = match body.strip_prefix('/') {
        Some(body) => (true, body),
        None => (false, body),
    };
    let first = body.chars().next()?;
    if !(first.is_ascii_alphabetic() || (!closing && matches!(first, '!' | '?'))) {
        return None;
    }
    // Attribute values may contain `>`; only an unquoted one ends the tag.
    let mut quote = None;
    let mut end = None;
    for (index, char) in body.char_indices() {
        match (quote, char) {
            (Some(open), c) if c == open => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(char),
            (None, '>') => {
                end = Some(index);
                break;
            }
            _ => {}
        }
    }
    let end = end?;
    let inner = &body[..end];
    let name: String = inner
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect::<String>()
        .to_ascii_lowercase();
    Some((
        Tag {
            name,
            closing,
            self_closing: inner.ends_with('/'),
        },
        &body[end + 1..],
    ))
}

fn heading(name: &str) -> Option<usize> {
    match name.as_bytes() {
        [b'h', level @ b'1'..=b'6'] => Some(usize::from(level - b'0')),
        _ => None,
    }
}

/// Plain text built from decoded runs, with whitespace collapsed inside lines and at most
/// one blank line between blocks.
#[derive(Default)]
struct Text {
    out: String,
    pending_space: bool,
    pending_lines: usize,
}

impl Text {
    fn push(&mut self, raw: &str) {
        let decoded = decode(raw);
        if decoded.starts_with(char::is_whitespace) {
            self.pending_space = true;
        }
        let trailing = decoded.ends_with(char::is_whitespace);
        let mut words = decoded.split_whitespace().peekable();
        while let Some(word) = words.next() {
            self.flush();
            self.out.push_str(word);
            self.pending_space = trailing || words.peek().is_some();
        }
    }
    fn raw(&mut self, text: &str) {
        self.flush();
        self.out.push_str(text);
        self.pending_space = false;
    }
    fn flush(&mut self) {
        if self.out.is_empty() {
            self.pending_lines = 0;
            self.pending_space = false;
            return;
        }
        if self.pending_lines > 0 {
            for _ in 0..self.pending_lines {
                self.out.push('\n');
            }
        } else if self.pending_space && !self.out.ends_with([' ', '\n']) {
            self.out.push(' ');
        }
        self.pending_lines = 0;
        self.pending_space = false;
    }
    fn newline(&mut self, lines: usize) {
        self.pending_lines = self.pending_lines.max(lines);
    }
    fn space(&mut self) {
        self.pending_space = true;
    }
    fn finish(self) -> String {
        self.out
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_owned()
    }
}

pub(crate) fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// HTML character references: numeric ones and the named ones prose commonly uses.
pub(crate) fn decode(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let end = rest[1..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '#'))
            .map(|end| end + 1)
            .unwrap_or(rest.len());
        let name = &rest[1..end];
        let value = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            "ndash" => Some('–'),
            "mdash" => Some('—'),
            "hellip" => Some('…'),
            "lsquo" => Some('‘'),
            "rsquo" => Some('’'),
            "ldquo" => Some('“'),
            "rdquo" => Some('”'),
            "copy" => Some('©'),
            "reg" => Some('®'),
            "middot" => Some('·'),
            _ => name.strip_prefix('#').and_then(|number| {
                match number.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => number.parse().ok(),
                }
                .and_then(char::from_u32)
            }),
        };
        match value {
            Some(value) => {
                out.push(value);
                rest = rest[end..].strip_prefix(';').unwrap_or(&rest[end..]);
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Reads up to `limit` organic results from a search engine's results page. Engine-neutral:
/// a result is a heading inside (or around) a link to another site; the engine's own redirect
/// links (`uddg`, `q`, `url`, Bing's base64 `u`) are decoded to their target.
pub(crate) fn serp_script(limit: usize) -> String {
    format!(
        r#"(() => {{
  const site = (h) => h.split('.').slice(-2).join('.');
  const own = site(location.hostname);
  const web = (u) => u.protocol === 'http:' || u.protocol === 'https:';
  const target = (href) => {{
    let u;
    try {{ u = new URL(href, location.href); }} catch (e) {{ return null; }}
    if (!web(u)) return null;
    if (site(u.hostname) !== own) return u;
    for (const key of ['uddg', 'url', 'q', 'u']) {{
      let value = u.searchParams.get(key);
      if (!value) continue;
      if (key === 'u' && value.startsWith('a1')) {{
        try {{ value = atob(value.slice(2).replace(/-/g, '+').replace(/_/g, '/')); }} catch (e) {{ continue; }}
      }}
      try {{ const d = new URL(value); if (web(d) && site(d.hostname) !== own) return d; }} catch (e) {{}}
    }}
    return null;
  }};
  const clean = (s) => (s || '').replace(/\s+/g, ' ').trim();
  const results = [];
  const seen = new Set();
  for (const heading of document.querySelectorAll('h2, h3')) {{
    const link = heading.closest('a[href]') || heading.querySelector('a[href]');
    if (!link) continue;
    const url = target(link.getAttribute('href'));
    if (!url) continue;
    url.hash = '';
    const title = clean(heading.innerText || heading.textContent);
    if (!title || seen.has(url.href)) continue;
    let box = link.contains(heading) ? link : heading;
    for (let el = box.parentElement; el && el !== document.body; el = el.parentElement) {{
      if (el.querySelectorAll('h2, h3').length > 1) break;
      box = el;
    }}
    const snippet = clean((box.innerText || '').split(title).join(' '));
    seen.add(url.href);
    results.push({{ title: title.slice(0, 300), url: url.href, snippet: snippet.slice(0, 500) }});
    if (results.length >= {limit}) break;
  }}
  return {{ url: location.href, results }};
}})()"#
    )
}

/// Most text nodes the page script visits; with the character cap it bounds the work a huge or
/// hostile page can make the reader do.
pub(crate) const PAGE_NODE_LIMIT: usize = 50_000;

/// Title and rendered text (main or article when present) of the page. The script walks
/// visible text nodes and stops at `limit` characters or [`PAGE_NODE_LIMIT`] nodes instead of
/// materializing `innerText` of the whole document.
pub(crate) fn page_script(limit: usize) -> String {
    format!(
        r#"(() => {{
  const skip = new Set(['SCRIPT','STYLE','NOSCRIPT','TEMPLATE','SVG','IFRAME','OBJECT','CANVAS','NAV','SELECT','BUTTON']);
  const blocks = 'p,div,li,h1,h2,h3,h4,h5,h6,tr,td,th,section,article,main,header,footer,blockquote,pre,dd,dt,figcaption,table,ul,ol';
  const collect = (root, cap) => {{
    const parts = [];
    let size = 0, nodes = 0, block = null;
    if (!root) return '';
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {{
      acceptNode(node) {{
        for (let el = node.parentElement; el && el !== root.parentElement; el = el.parentElement) {{
          if (skip.has(el.tagName.toUpperCase())) return NodeFilter.FILTER_REJECT;
        }}
        const parent = node.parentElement;
        if (parent && parent.checkVisibility && !parent.checkVisibility()) return NodeFilter.FILTER_REJECT;
        return NodeFilter.FILTER_ACCEPT;
      }}
    }});
    while (size < cap && nodes < {nodes}) {{
      const node = walker.nextNode();
      if (!node) break;
      nodes += 1;
      const text = node.data.replace(/\s+/g, ' ');
      if (!text.trim()) continue;
      const owner = node.parentElement ? node.parentElement.closest(blocks) : null;
      if (owner !== block && parts.length) parts.push('\n');
      block = owner;
      parts.push(text);
      size += text.length;
    }}
    return parts.join('').replace(/ *\n */g, '\n').replace(/\n{{3,}}/g, '\n\n').trim().slice(0, cap);
  }};
  const main = document.querySelector('main, article, [role=main]');
  const pick = main && collect(main, 200).length >= 200 ? main : document.body;
  return {{ url: location.href, title: (document.title || '').slice(0, 1000), text: collect(pick, {limit}) }};
}})()"#,
        nodes = PAGE_NODE_LIMIT
    )
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Html,
    Text,
}

/// What a body without a `Content-Type` is, from its first bytes: HTML, other readable text,
/// or `None` (binary or control characters, which are never handed to the model as text).
pub(crate) fn sniff(bytes: &[u8]) -> Option<Kind> {
    let head = &bytes[..bytes.len().min(1024)];
    let head = head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(head);
    let text = match std::str::from_utf8(head) {
        Ok(text) => text,
        // A multi-byte character cut by the 1024-byte window is still text; a body that ends
        // inside one is not.
        Err(error) if error.error_len().is_none() && bytes.len() > 1024 => {
            std::str::from_utf8(&head[..error.valid_up_to()]).unwrap_or_default()
        }
        Err(_) => return None,
    };
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r' | '\x0c'))
    {
        return None;
    }
    let start = text.trim_start().get(..15).unwrap_or(text.trim_start());
    let lower = start.to_ascii_lowercase();
    if lower.starts_with("<!doctype html") || lower.starts_with("<html") || {
        let all = text.to_ascii_lowercase();
        all.contains("<head") || all.contains("<body")
    } {
        Some(Kind::Html)
    } else {
        Some(Kind::Text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_text_keeps_structure_and_drops_scripts_menus_and_markup() {
        let html = r#"<!doctype html><html><head><title>Rivers &amp; Lakes</title>
            <style>p { color: red }</style><script>var x = "<p>not text</p>";</script></head>
            <body><nav><a href="/">Home</a> <a href="/about">About</a></nav>
            <h1>Rivers</h1><p>Rivers carry <b>fresh</b>&nbsp;water&#8212;mostly.</p>
            <!-- hidden <p>comment</p> -->
            <ul><li>Nile</li><li data-x='a>b'>Amazon</li></ul>
            <p>5 &lt; 6 &copy; &unknown; R&D</p></body></html>"#;
        let (title, text) = readable(html);
        assert_eq!(title.as_deref(), Some("Rivers & Lakes"));
        assert_eq!(
            text,
            "# Rivers\n\nRivers carry fresh water—mostly.\n\n- Nile\n- Amazon\n\n5 < 6 © &unknown; R&D"
        );
    }

    #[test]
    fn main_content_wins_over_page_chrome_when_it_carries_the_page() {
        let body = "Long article sentence. ".repeat(12);
        let html = format!(
            "<header>Site banner</header><main><p>{body}</p></main><footer>Contact us</footer>"
        );
        let (_, text) = readable(&html);
        assert_eq!(text, body.trim());
        // A tiny main element is not trusted to be the content.
        let (_, text) = readable("<p>Intro</p><main>Hi</main><p>Outro</p>");
        assert_eq!(text, "Intro\n\nHi\n\nOutro");
    }

    #[test]
    fn untyped_bodies_are_read_only_when_their_first_bytes_are_text() {
        assert_eq!(
            sniff(b"  <!DOCTYPE html><title>x</title>"),
            Some(Kind::Html)
        );
        assert_eq!(
            sniff(b"\xEF\xBB\xBF<html><body>hi</body></html>"),
            Some(Kind::Html)
        );
        assert_eq!(
            sniff("조수 간만의 차이\nplain words".as_bytes()),
            Some(Kind::Text)
        );
        assert_eq!(sniff(b"{\"a\": 1}"), Some(Kind::Text));
        // A multi-byte character cut at the sniff window is still text.
        let mut long = "가".repeat(400).into_bytes();
        long.truncate(1024 + 1);
        assert_eq!(sniff(&long), Some(Kind::Text));
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"), None);
        assert_eq!(sniff(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3"), None);
        assert_eq!(sniff(b"text\0with nul"), None);
        assert_eq!(sniff(&[0xC3]), None);
        assert_eq!(sniff(b"caf\xC3"), None);
    }
}
