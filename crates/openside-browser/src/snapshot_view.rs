//! Bounded, lossless views of a [`Snapshot`]: line diffs between consecutive observations of
//! one document, and pages of a snapshot (or of the subtree under one reference) that fit an
//! output budget. A view that leaves lines out always says which lines and how to get them.

use std::fmt::Write;

use serde::Serialize;

use crate::{BrowserError, Snapshot};

/// Above this many line pairs the middle of a diff is emitted as one replacement hunk: still
/// exact, just less minimal, without quadratic time or memory on huge rewrites.
const LCS_CELLS: usize = 4_000_000;

/// One replaced run of lines. `old_start` is the 1-based first line in the older text; for a
/// pure insertion it is the line after which `added` goes (0 = before the first line).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// 1-based position in the older text.
    pub old_start: usize,
    /// Lines of the older text replaced by this hunk, verbatim.
    pub removed: Vec<String>,
    /// Lines of the newer text inserted in their place.
    pub added: Vec<String>,
}

/// Line-level changes that turn one snapshot text into another.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SnapshotPatch {
    /// Hunks in ascending, non-overlapping order.
    pub hunks: Vec<Hunk>,
}

impl SnapshotPatch {
    /// Exact line diff from `old` to `new`.
    pub fn between(old: &str, new: &str) -> Self {
        let old: Vec<&str> = old.lines().collect();
        let new: Vec<&str> = new.lines().collect();
        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let old_mid = &old[prefix..old.len() - suffix];
        let new_mid = &new[prefix..new.len() - suffix];
        let mut hunks = Vec::new();
        if old_mid.is_empty() && new_mid.is_empty() {
            return Self { hunks };
        }
        if old_mid.len().saturating_mul(new_mid.len()) > LCS_CELLS
            || old_mid.is_empty()
            || new_mid.is_empty()
        {
            hunks.push(hunk(prefix, old_mid, new_mid));
            return Self { hunks };
        }
        // lcs[i][j]: common subsequence length of old_mid[i..] and new_mid[j..].
        let width = new_mid.len() + 1;
        let mut lcs = vec![0_u32; (old_mid.len() + 1) * width];
        for i in (0..old_mid.len()).rev() {
            for j in (0..new_mid.len()).rev() {
                lcs[i * width + j] = if old_mid[i] == new_mid[j] {
                    lcs[(i + 1) * width + j + 1] + 1
                } else {
                    lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        let (mut run_old, mut run_new) = (0, 0);
        let flush = |hunks: &mut Vec<Hunk>, i: usize, j: usize, run_old: usize, run_new: usize| {
            if i > run_old || j > run_new {
                hunks.push(hunk(
                    prefix + run_old,
                    &old_mid[run_old..i],
                    &new_mid[run_new..j],
                ));
            }
        };
        while i < old_mid.len() && j < new_mid.len() {
            if old_mid[i] == new_mid[j] {
                flush(&mut hunks, i, j, run_old, run_new);
                i += 1;
                j += 1;
                run_old = i;
                run_new = j;
            } else if lcs[(i + 1) * width + j] >= lcs[i * width + j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
        flush(&mut hunks, old_mid.len(), new_mid.len(), run_old, run_new);
        Self { hunks }
    }

    /// Whether both texts were identical.
    pub fn is_empty(&self) -> bool {
        self.hunks.is_empty()
    }

    /// The text a model receives for a consecutive snapshot of the same document.
    pub fn for_agent(&self) -> String {
        if self.is_empty() {
            return "No changes since this tab's previous snapshot.\n".to_owned();
        }
        format!(
            "Diff since this tab's previous snapshot (line numbers refer to it; refs are unchanged; pass full: true for the whole tree):\n{}",
            self.render()
        )
    }

    /// `@@ -start,count +start,count @@` headers followed by `-`/`+` lines, without context.
    pub fn render(&self) -> String {
        let mut text = String::new();
        let mut shift: isize = 0;
        for hunk in &self.hunks {
            let new_start = hunk.old_start as isize + shift;
            let _ = writeln!(
                text,
                "@@ -{},{} +{},{} @@",
                hunk.old_start,
                hunk.removed.len(),
                new_start,
                hunk.added.len()
            );
            for line in &hunk.removed {
                let _ = writeln!(text, "-{line}");
            }
            for line in &hunk.added {
                let _ = writeln!(text, "+{line}");
            }
            shift += hunk.added.len() as isize - hunk.removed.len() as isize;
        }
        text
    }

    /// Inverse of [`render`](Self::render); `None` for malformed input.
    pub fn parse(text: &str) -> Option<Self> {
        let mut hunks: Vec<Hunk> = Vec::new();
        for line in text.lines() {
            if let Some(header) = line.strip_prefix("@@ -") {
                let (old, _) = header.split_once(' ')?;
                let (start, _) = old.split_once(',')?;
                hunks.push(Hunk {
                    old_start: start.parse().ok()?,
                    removed: Vec::new(),
                    added: Vec::new(),
                });
            } else if let Some(removed) = line.strip_prefix('-') {
                hunks.last_mut()?.removed.push(removed.to_owned());
            } else {
                let added = line.strip_prefix('+')?;
                hunks.last_mut()?.added.push(added.to_owned());
            }
        }
        Some(Self { hunks })
    }

    /// Apply to the older text; `None` when the removed lines do not match it.
    pub fn apply(&self, old: &str) -> Option<String> {
        let old: Vec<&str> = old.lines().collect();
        let mut out: Vec<&str> = Vec::with_capacity(old.len());
        let mut next = 0;
        for hunk in &self.hunks {
            // Replacements start at old_start; pure insertions go after line old_start.
            let start = if hunk.removed.is_empty() {
                hunk.old_start
            } else {
                hunk.old_start.checked_sub(1)?
            };
            if start < next || start + hunk.removed.len() > old.len() {
                return None;
            }
            out.extend_from_slice(&old[next..start]);
            if old[start..start + hunk.removed.len()]
                .iter()
                .zip(&hunk.removed)
                .any(|(a, b)| a != b)
            {
                return None;
            }
            out.extend(hunk.added.iter().map(String::as_str));
            next = start + hunk.removed.len();
        }
        out.extend_from_slice(&old[next..]);
        let mut text = out.join("\n");
        if !out.is_empty() {
            text.push('\n');
        }
        Some(text)
    }
}

fn hunk(prefix: usize, removed: &[&str], added: &[&str]) -> Hunk {
    Hunk {
        old_start: if removed.is_empty() {
            prefix
        } else {
            prefix + 1
        },
        removed: removed.iter().map(|line| (*line).to_owned()).collect(),
        added: added.iter().map(|line| (*line).to_owned()).collect(),
    }
}

/// A subtree the agent can request on its own with `ref`.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ScopeHint {
    /// Reference to pass back as the snapshot scope.
    #[serde(rename = "ref")]
    pub reference: String,
    /// Accessibility role of the subtree root.
    pub role: String,
    /// Accessible name of the subtree root.
    pub name: String,
    /// Lines in the subtree, root included.
    pub lines: usize,
}

/// Why a view is not the whole requested text, and how to read the rest.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Omission {
    /// Always `snapshot_over_budget`.
    pub code: &'static str,
    /// Instructions for the agent.
    pub message: String,
    /// First and last shown line (1-based, within the requested scope).
    pub shown_lines: [usize; 2],
    /// Lines in the requested scope.
    pub total_lines: usize,
    /// Lines of the scope that are not in this view.
    pub omitted_lines: usize,
    /// Element references on the omitted lines.
    pub omitted_refs: usize,
    /// `from_line` that continues after this view; absent on the last page. When the view ends
    /// inside a line this is that line.
    pub next_from_line: Option<usize>,
    /// `from_char` (Unicode scalar values already shown of `next_from_line`) to pass together
    /// with it; present only when the view ends inside a line.
    pub next_from_char: Option<usize>,
    /// Largest referenced subtrees outside this view, to request individually.
    pub scopes: Vec<ScopeHint>,
}

/// One bounded page of snapshot text.
#[derive(Clone, Debug)]
pub struct SnapshotView {
    /// The shown lines, verbatim.
    pub text: String,
    /// Present whenever any line of the requested scope is not shown.
    pub omission: Option<Omission>,
}

/// JSON-escaped length of a line plus its escaped newline: what the line costs in tool output.
fn cost(line: &str) -> usize {
    serde_json::to_string(line).map_or(line.len(), |quoted| quoted.len() - 2) + 2
}

/// JSON-escaped length of one character, as `serde_json` writes it inside a string.
fn char_cost(c: char) -> usize {
    match c {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
        c if c < '\u{20}' => 6,
        c => c.len_utf8(),
    }
}

/// Scope hints listed per omission; enough to cover a page's main regions.
const SCOPE_HINTS: usize = 12;

impl Snapshot {
    /// A page of at most about `budget` escaped bytes starting at the 1-based `from_line` of
    /// the whole snapshot or of the subtree under `scope`, and at character `from_char` of that
    /// line (0 = its start). Lines are never dropped silently: when anything is left out the
    /// view carries an [`Omission`]. A single line larger than the budget is split across
    /// pages at character boundaries; the pages reassemble to exactly that line.
    pub fn view(
        &self,
        scope: Option<&str>,
        from_line: usize,
        from_char: usize,
        budget: usize,
    ) -> Result<SnapshotView, BrowserError> {
        let lines: Vec<&str> = self.text.lines().collect();
        let (first, end) = match scope {
            None => (0, self.nodes.len()),
            Some(reference) => {
                let root = self
                    .nodes
                    .iter()
                    .position(|node| node.reference.as_deref() == Some(reference))
                    .ok_or(BrowserError::StaleReference)?;
                (root, subtree_end(self, root))
            }
        };
        let total = end - first;
        let start = first + from_line.saturating_sub(1).min(total);
        let resume = if from_char == 0 {
            0
        } else {
            let line =
                lines
                    .get(start)
                    .filter(|_| start < end)
                    .ok_or(BrowserError::InvalidAction(
                        "from_char needs a from_line inside the snapshot",
                    ))?;
            line.char_indices()
                .map(|(index, _)| index)
                .chain(std::iter::once(line.len()))
                .nth(from_char)
                .ok_or(BrowserError::InvalidAction(
                    "from_char is past the end of that line",
                ))?
        };
        let mut used = 0;
        let mut stop = start;
        let mut text = String::new();
        // Characters of line `stop` shown so far, when the page ends inside it.
        let mut partial = None;
        while stop < end {
            let line = lines[stop];
            let rest = if stop == start { &line[resume..] } else { line };
            let size = cost(rest);
            if used + size > budget {
                if stop == start {
                    // One line alone is over budget: show as much of it as fits (at least
                    // one character) and continue inside it on the next page.
                    let mut bytes = 0;
                    let mut chars = 0;
                    let mut spent = 0;
                    for (index, c) in rest.char_indices() {
                        let width = char_cost(c);
                        if chars > 0 && spent + width > budget {
                            break;
                        }
                        spent += width;
                        bytes = index + c.len_utf8();
                        chars += 1;
                    }
                    text.push_str(&rest[..bytes]);
                    if bytes == rest.len() {
                        // Only the newline was left over; it costs two bytes.
                        text.push('\n');
                        stop += 1;
                    } else {
                        partial = Some(from_char + chars);
                    }
                }
                break;
            }
            used += size;
            text.push_str(rest);
            text.push('\n');
            stop += 1;
        }
        if start == first && from_char == 0 && stop == end && partial.is_none() {
            return Ok(SnapshotView {
                text,
                omission: None,
            });
        }
        // A line shown from its middle (or cut) is not a whole shown line.
        let first_whole = start + usize::from(from_char > 0 && stop > start);
        let shown_whole = stop - first_whole.min(stop);
        let cut_line_start_shown = partial.is_some() && (stop > start || from_char == 0);
        let outside = (first..first_whole).chain(stop..end);
        let omitted_refs = outside
            .clone()
            .filter(|&index| {
                !(cut_line_start_shown && index == stop)
                    && self.nodes[index].reference.is_some()
                    && self.nodes[index].role != "StaticText"
            })
            .count();
        let mut scopes = Vec::new();
        let mut index = first;
        while index < end && scopes.len() < SCOPE_HINTS {
            let node = &self.nodes[index];
            let subtree = subtree_end(self, index);
            let shown = index >= first_whole && subtree <= stop;
            let large = subtree - index > 1;
            match (&node.reference, shown, large) {
                (Some(reference), false, true) if node.role != "StaticText" && index != first => {
                    scopes.push(ScopeHint {
                        reference: reference.clone(),
                        role: node.role.clone(),
                        name: node.name.clone(),
                        lines: subtree - index,
                    });
                    index = subtree;
                }
                _ => index += 1,
            }
        }
        let touched_end = stop + usize::from(partial.is_some());
        let shown_lines = [start - first + 1, touched_end - first];
        let next_from_line = (stop < end).then_some(stop - first + 1);
        let omitted_lines = total - shown_whole;
        let mut message = format!(
            "Snapshot is larger than one tool result: showing lines {}-{} of {}; {} lines with {} element refs are not (completely) in this result. Nothing was dropped:",
            shown_lines[0], shown_lines[1], total, omitted_lines, omitted_refs
        );
        if let Some(next) = next_from_line {
            let _ = write!(
                message,
                " call browser_snapshot again with this snapshot_id and from_line: {next}"
            );
            if let Some(chars) = partial {
                let _ = write!(message, ", from_char: {chars}");
            }
            message.push_str(" (same ref) for the next part of this same capture;");
        }
        if let Some(chars) = partial {
            let _ = write!(message, " line {} is longer than one result, so this part ends inside it after {chars} characters and the next part continues exactly there;", stop - first + 1);
        }
        message.push_str(
            " pass this snapshot_id with ref set to one of `scopes` to read just that region.",
        );
        Ok(SnapshotView {
            text,
            omission: Some(Omission {
                code: "snapshot_over_budget",
                message,
                shown_lines,
                total_lines: total,
                omitted_lines,
                omitted_refs,
                next_from_line,
                next_from_char: partial,
                scopes,
            }),
        })
    }
}

/// Index one past the last descendant of `root`.
fn subtree_end(snapshot: &Snapshot, root: usize) -> usize {
    let depth = snapshot.nodes[root].depth;
    snapshot.nodes[root + 1..]
        .iter()
        .position(|node| node.depth <= depth)
        .map_or(snapshot.nodes.len(), |offset| root + 1 + offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SnapshotNode;

    fn texts(lines: &[&str]) -> String {
        lines.iter().map(|line| format!("{line}\n")).collect()
    }

    #[test]
    fn patches_round_trip_through_render_parse_and_apply() {
        let cases = [
            (vec!["a", "b", "c"], vec!["a", "b", "c"]),
            (vec!["a", "b", "c"], vec!["a", "x", "c"]),
            (vec!["a", "b"], vec!["a", "b", "c", "d"]),
            (vec!["a", "b", "c"], vec!["c"]),
            (vec![], vec!["a"]),
            (vec!["a"], vec![]),
            (vec!["x", "a", "b", "y", "c"], vec!["a", "q", "b", "c", "z"]),
            (vec!["p", "q"], vec!["n", "p", "q"]),
        ];
        for (old, new) in cases {
            let (old, new) = (texts(&old), texts(&new));
            let patch = SnapshotPatch::between(&old, &new);
            assert_eq!(patch.is_empty(), old == new);
            let parsed = SnapshotPatch::parse(&patch.render()).unwrap();
            assert_eq!(parsed, patch);
            assert_eq!(
                parsed.apply(&old).as_deref(),
                Some(new.as_str()),
                "{old:?} -> {new:?}"
            );
        }
        let patch = SnapshotPatch::between("a\nb\n", "a\nc\n");
        assert_eq!(patch.render(), "@@ -2,1 +2,1 @@\n-b\n+c\n");
        assert_eq!(patch.apply("a\nz\n"), None, "removed lines must match");
    }

    fn snapshot(lines: usize) -> Snapshot {
        let mut nodes = vec![SnapshotNode {
            reference: Some("e1".into()),
            role: "RootWebArea".into(),
            name: "Page".into(),
            value: String::new(),
            depth: 0,
        }];
        for section in 0..lines / 10 {
            nodes.push(SnapshotNode {
                reference: Some(format!("s{section}")),
                role: "list".into(),
                name: format!("Section {section}"),
                value: String::new(),
                depth: 1,
            });
            for item in 0..9 {
                nodes.push(SnapshotNode {
                    reference: Some(format!("b{section}-{item}")),
                    role: "button".into(),
                    name: format!("Button {section}-{item}"),
                    value: String::new(),
                    depth: 2,
                });
            }
        }
        let text = nodes
            .iter()
            .map(|node| {
                format!(
                    "{}[{}] {} {:?}\n",
                    " ".repeat(node.depth),
                    node.reference.as_deref().unwrap_or_default(),
                    node.role,
                    node.name
                )
            })
            .collect();
        Snapshot {
            nodes,
            text,
            raw_bytes: 0,
            document_root: None,
        }
    }

    #[test]
    fn pages_cover_every_line_and_say_what_is_left_out() {
        let snapshot = snapshot(300);
        let whole = snapshot.view(None, 1, 0, usize::MAX).unwrap();
        assert!(whole.omission.is_none());
        assert_eq!(whole.text, snapshot.text);
        let mut joined = String::new();
        let mut from = 1;
        let mut pages = 0;
        loop {
            let page = snapshot.view(None, from, 0, 2000).unwrap();
            let omission = page.omission.expect("over budget pages carry an omission");
            assert_eq!(omission.code, "snapshot_over_budget");
            assert_eq!(omission.total_lines, snapshot.nodes.len());
            assert!(omission.omitted_refs > 0);
            assert!(!omission.scopes.is_empty());
            assert!(omission.message.contains("Nothing was dropped"));
            joined.push_str(&page.text);
            pages += 1;
            match omission.next_from_line {
                Some(next) => from = next,
                None => break,
            }
        }
        assert!(pages > 2);
        assert_eq!(
            joined, snapshot.text,
            "pages concatenate to the full snapshot"
        );
        let scoped = snapshot.view(Some("s3"), 1, 0, usize::MAX).unwrap();
        assert!(scoped.omission.is_none());
        assert_eq!(scoped.text.lines().count(), 10);
        assert!(scoped.text.starts_with(" [s3] list"));
        assert!(matches!(
            snapshot.view(Some("missing"), 1, 0, 100),
            Err(BrowserError::StaleReference)
        ));
    }

    #[test]
    fn one_line_larger_than_the_budget_continues_inside_the_line_without_loss() {
        let mut snapshot = snapshot(20);
        // Quotes, backslashes, control characters and multi-byte text all change the escaped
        // size per character.
        let huge = "q\"\\é\n\u{1}日".repeat(3000);
        let index = snapshot.nodes.len() - 1;
        snapshot.nodes[index].name = huge;
        snapshot.text = snapshot
            .nodes
            .iter()
            .map(|node| {
                format!(
                    "{}[{}] {} {:?}\n",
                    " ".repeat(node.depth),
                    node.reference.as_deref().unwrap_or_default(),
                    node.role,
                    node.name
                )
            })
            .collect();
        let budget = 3000;
        let (mut line, mut chars) = (1, 0);
        let (mut joined, mut pages, mut split_pages) = (String::new(), 0, 0);
        loop {
            let page = snapshot.view(None, line, chars, budget).unwrap();
            let escaped = serde_json::to_string(&page.text).unwrap().len() - 2;
            assert!(escaped <= budget + 2, "page of {escaped} escaped bytes");
            joined.push_str(&page.text);
            pages += 1;
            let omission = page
                .omission
                .expect("every page of a large snapshot says so");
            match (omission.next_from_line, omission.next_from_char) {
                (Some(next), Some(inside)) => {
                    split_pages += 1;
                    assert!(omission.message.contains(&format!("from_char: {inside}")));
                    (line, chars) = (next, inside);
                }
                (Some(next), None) => (line, chars) = (next, 0),
                (None, cut) => {
                    assert!(cut.is_none());
                    break;
                }
            }
        }
        assert!(split_pages > 5, "{split_pages} pages inside the huge line");
        assert!(pages > split_pages);
        assert_eq!(
            joined, snapshot.text,
            "the pages reassemble to exactly the capture"
        );

        let last = snapshot.text.lines().count();
        let line_chars = snapshot.text.lines().last().unwrap().chars().count();
        assert!(matches!(
            snapshot.view(None, last, line_chars + 1, budget),
            Err(BrowserError::InvalidAction(_))
        ));
        assert!(matches!(
            snapshot.view(None, last + 5, 3, budget),
            Err(BrowserError::InvalidAction(_))
        ));
    }

    #[test]
    fn character_cost_matches_the_json_encoder() {
        let sample: String = (0..0x250u32)
            .filter_map(char::from_u32)
            .chain("日🙂\u{7f}".chars())
            .collect();
        let expected = serde_json::to_string(&sample).unwrap().len() - 2;
        assert_eq!(sample.chars().map(char_cost).sum::<usize>(), expected);
    }
}
