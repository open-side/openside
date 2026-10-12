//! Locator engines evaluated in Rust over CDP `DOM.getDocument` and `Accessibility.getFullAXTree`
//! results, so no selector logic ever runs inside the page.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

/// One locator step, produced by the JavaScript prelude.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Step {
    Css {
        selector: String,
    },
    Ref {
        reference: String,
    },
    Role {
        role: String,
        name: Option<Matcher>,
        #[serde(default)]
        states: HashMap<String, Value>,
    },
    Text {
        matcher: Matcher,
    },
    Label {
        matcher: Matcher,
    },
    Attr {
        name: String,
        matcher: Matcher,
    },
    Nth {
        index: i64,
    },
    HasText {
        matcher: Matcher,
        negate: bool,
    },
    Has {
        steps: Vec<Step>,
        negate: bool,
    },
    Visible {
        visible: bool,
    },
}

/// Playwright text matching: substring/case-insensitive by default, whole-string when exact.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum Matcher {
    Regex { source: String, flags: String },
    Text { text: String, exact: bool },
}

pub(crate) fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Matcher {
    pub(crate) fn compile(&self) -> Result<Compiled, String> {
        Ok(match self {
            Self::Text { text, exact } => Compiled::Text {
                text: if *exact {
                    normalize(text)
                } else {
                    normalize(text).to_lowercase()
                },
                exact: *exact,
            },
            Self::Regex { source, flags } => {
                let mut prefix = String::new();
                for flag in flags.chars() {
                    match flag {
                        'i' | 's' | 'm' => prefix.push(flag),
                        'g' | 'u' | 'y' | 'd' | 'v' => {}
                        other => return Err(format!("unsupported RegExp flag {other}")),
                    }
                }
                let pattern = if prefix.is_empty() {
                    source.clone()
                } else {
                    format!("(?{prefix}){source}")
                };
                Compiled::Regex(regex::Regex::new(&pattern).map_err(|_| {
                    format!("NotSupported: RegExp /{source}/ uses syntax outside the supported subset (no lookaround or backreferences)")
                })?)
            }
        })
    }
}

pub(crate) enum Compiled {
    Text { text: String, exact: bool },
    Regex(regex::Regex),
}

impl Compiled {
    pub(crate) fn matches(&self, raw: &str) -> bool {
        let value = normalize(raw);
        match self {
            Self::Text { text, exact: true } => &value == text,
            Self::Text { text, exact: false } => value.to_lowercase().contains(text.as_str()),
            Self::Regex(regex) => regex.is_match(&value),
        }
    }
}

pub(crate) struct Node {
    pub(crate) node_id: i64,
    pub(crate) backend: u64,
    pub(crate) parent: Option<usize>,
    pub(crate) node_type: i64,
    pub(crate) name: String,
    pub(crate) value: String,
    pub(crate) attributes: Vec<(String, String)>,
    pub(crate) children: Vec<usize>,
}

impl Node {
    pub(crate) fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
    pub(crate) fn element(&self) -> bool {
        self.node_type == 1
    }
}

/// Top-frame DOM in document (pre-order) order; indexes double as document positions.
pub(crate) struct Dom {
    pub(crate) nodes: Vec<Node>,
    pub(crate) by_backend: HashMap<u64, usize>,
    pub(crate) by_node_id: HashMap<i64, usize>,
    match_text: Vec<String>,
    text_content: Vec<String>,
}

const SKIPPED_FOR_TEXT: &[&str] = &["script", "style", "noscript", "head", "template"];

/// Host-memory bounds for one parsed document, outside the QuickJS heap limit. The byte budget
/// covers copied names, attribute names and values, node values, and element text, which is
/// stored per ancestor (`text`, `text_content`), so that duplication is charged too.
pub(crate) const MAX_NODES: usize = 200_000;
pub(crate) const MAX_HOST_BYTES: usize = 32 * 1024 * 1024;
const CANCEL_CHECK_EVERY: usize = 1024;
pub(crate) const TOO_LARGE: &str = "page is too large for the REPL host budget";

impl Dom {
    /// Parse a `DOM.getDocument` reply; fails once [`MAX_NODES`] or [`MAX_HOST_BYTES`] would be
    /// exceeded, or when `cancelled` reports true.
    pub(crate) fn parse(document: &Value, cancelled: &dyn Fn() -> bool) -> Result<Self, String> {
        Self::parse_bounded(document, cancelled, MAX_NODES, MAX_HOST_BYTES)
    }

    fn parse_bounded(
        document: &Value,
        cancelled: &dyn Fn() -> bool,
        max_nodes: usize,
        max_bytes: usize,
    ) -> Result<Self, String> {
        let too_large = || TOO_LARGE.to_owned();
        let root = &document["root"];
        let mut nodes: Vec<Node> = Vec::new();
        let mut stack: Vec<(&Value, Option<usize>)> = vec![(root, None)];
        let mut bytes = 0usize;
        while let Some((value, parent)) = stack.pop() {
            let index = nodes.len();
            if index >= max_nodes {
                return Err(too_large());
            }
            if index.is_multiple_of(CANCEL_CHECK_EVERY) && cancelled() {
                return Err("execution stopped".into());
            }
            let flat = value["attributes"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default();
            let node_value = value["nodeValue"].as_str().unwrap_or_default();
            // Charged before copying, so an oversized node is refused rather than cloned.
            bytes += node_value.len()
                + flat
                    .iter()
                    .map(|item| item.as_str().map_or(0, str::len))
                    .sum::<usize>();
            if bytes > max_bytes {
                return Err(too_large());
            }
            let attributes = flat
                .chunks(2)
                .filter_map(|pair| {
                    Some((
                        pair[0].as_str()?.to_owned(),
                        pair.get(1)?.as_str()?.to_owned(),
                    ))
                })
                .collect();
            nodes.push(Node {
                node_id: value["nodeId"].as_i64().ok_or("DOM node without nodeId")?,
                backend: value["backendNodeId"]
                    .as_u64()
                    .ok_or("DOM node without backendNodeId")?,
                parent,
                node_type: value["nodeType"].as_i64().unwrap_or_default(),
                name: value["localName"]
                    .as_str()
                    .filter(|name| !name.is_empty())
                    .or_else(|| value["nodeName"].as_str())
                    .unwrap_or_default()
                    .to_ascii_lowercase(),
                value: node_value.to_owned(),
                attributes,
                children: Vec::new(),
            });
            if let Some(parent) = parent {
                nodes[parent].children.push(index);
            }
            // Iframe documents, shadow roots, and template contents are deliberately not entered:
            // locators are scoped to the top-frame light DOM, matching CSS querySelectorAll.
            if let Some(children) = value["children"].as_array() {
                for child in children.iter().rev() {
                    stack.push((child, Some(index)));
                }
            }
        }
        let by_backend = nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.backend, index))
            .collect();
        let by_node_id = nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.node_id, index))
            .collect();
        // Pre-order means every descendant has a larger index, so a reverse pass builds text bottom-up.
        let mut match_text = vec![String::new(); nodes.len()];
        let mut text_content = vec![String::new(); nodes.len()];
        let mut text_bytes = bytes;
        for index in (0..nodes.len()).rev() {
            if index.is_multiple_of(CANCEL_CHECK_EVERY) && cancelled() {
                return Err("execution stopped".into());
            }
            let node = &nodes[index];
            if node.node_type == 3 {
                text_bytes += 2 * node.value.len();
                if text_bytes > max_bytes {
                    return Err(too_large());
                }
                match_text[index] = node.value.clone();
                text_content[index] = node.value.clone();
                continue;
            }
            let total: usize = node
                .children
                .iter()
                .map(|&child| match_text[child].len() + text_content[child].len() + 1)
                .sum();
            text_bytes += total;
            if text_bytes > max_bytes {
                return Err(too_large());
            }
            let mut matched = String::new();
            let mut content = String::new();
            for &child in &node.children {
                content.push_str(&text_content[child]);
                if !SKIPPED_FOR_TEXT.contains(&nodes[child].name.as_str()) {
                    // CDP drops whitespace-only text nodes, so element boundaries approximate the
                    // whitespace that rendered text (innerText, text matching) would contain.
                    let piece = &match_text[child];
                    if nodes[child].element()
                        && !piece.is_empty()
                        && !matched.is_empty()
                        && !matched.ends_with(char::is_whitespace)
                    {
                        matched.push(' ');
                    }
                    matched.push_str(piece);
                }
            }
            if node.name == "input"
                && matches!(
                    node.attribute("type")
                        .map(str::to_ascii_lowercase)
                        .as_deref(),
                    Some("button" | "submit" | "reset")
                )
            {
                matched = node.attribute("value").unwrap_or_default().to_owned();
            }
            match_text[index] = matched;
            text_content[index] = content;
        }
        Ok(Self {
            nodes,
            by_backend,
            by_node_id,
            match_text,
            text_content,
        })
    }

    pub(crate) fn text(&self, index: usize) -> &str {
        &self.match_text[index]
    }

    pub(crate) fn text_content(&self, index: usize) -> &str {
        &self.text_content[index]
    }

    /// Strict descendant test (an element does not contain itself).
    pub(crate) fn contains(&self, ancestor: usize, mut node: usize) -> bool {
        while let Some(parent) = self.nodes[node].parent {
            if parent == ancestor {
                return true;
            }
            node = parent;
        }
        false
    }

    /// The form a control belongs to: its `form` attribute, else the nearest ancestor form.
    pub(crate) fn form_of(&self, control: usize) -> Option<usize> {
        if let Some(id) = self.nodes[control].attribute("form") {
            return self
                .by_id(id)
                .filter(|&form| self.nodes[form].name == "form");
        }
        let mut node = control;
        while let Some(parent) = self.nodes[node].parent {
            if self.nodes[parent].name == "form" {
                return Some(parent);
            }
            node = parent;
        }
        None
    }

    /// Submit buttons owned by `form`, including ones placed elsewhere with `form=<id>`.
    pub(crate) fn submitters(&self, form: usize) -> Vec<usize> {
        (0..self.nodes.len())
            .filter(|&index| {
                let node = &self.nodes[index];
                let submit = match node.name.as_str() {
                    "button" => node
                        .attribute("type")
                        .is_none_or(|kind| kind.eq_ignore_ascii_case("submit")),
                    "input" => node.attribute("type").is_some_and(|kind| {
                        kind.eq_ignore_ascii_case("submit") || kind.eq_ignore_ascii_case("image")
                    }),
                    _ => false,
                };
                submit && self.form_of(index) == Some(form)
            })
            .collect()
    }

    fn descendants(&self, scope: usize) -> impl Iterator<Item = usize> + '_ {
        let end = self.subtree_end(scope);
        (scope + 1..end).filter(|&index| self.nodes[index].element())
    }

    fn subtree_end(&self, scope: usize) -> usize {
        let mut end = scope + 1;
        while end < self.nodes.len() && self.contains(scope, end) {
            end += 1;
        }
        end
    }

    fn by_id(&self, id: &str) -> Option<usize> {
        self.nodes
            .iter()
            .position(|node| node.element() && node.attribute("id") == Some(id))
    }

    /// Playwright `internal:text`: the innermost elements whose text matches.
    pub(crate) fn text_matches(&self, scope: usize, matcher: &Compiled) -> Vec<usize> {
        self.descendants(scope)
            .filter(|&index| {
                !SKIPPED_FOR_TEXT.contains(&self.nodes[index].name.as_str())
                    && !self.in_head(index)
                    && matcher.matches(self.text(index))
                    && !self.nodes[index].children.iter().any(|&child| {
                        self.nodes[child].element() && matcher.matches(self.text(child))
                    })
            })
            .collect()
    }

    fn in_head(&self, mut index: usize) -> bool {
        while let Some(parent) = self.nodes[index].parent {
            if self.nodes[parent].name == "head" {
                return true;
            }
            index = parent;
        }
        false
    }

    pub(crate) fn attribute_matches(
        &self,
        scope: usize,
        name: &str,
        matcher: &Compiled,
    ) -> Vec<usize> {
        self.descendants(scope)
            .filter(|&index| {
                self.nodes[index]
                    .attribute(name)
                    .is_some_and(|value| matcher.matches(value))
            })
            .collect()
    }

    /// Playwright `getByLabel`: `<label>` association, `aria-labelledby`, then `aria-label`.
    pub(crate) fn label_matches(&self, scope: usize, matcher: &Compiled) -> Vec<usize> {
        const LABELABLE: &[&str] = &[
            "input", "select", "textarea", "button", "meter", "output", "progress",
        ];
        let mut found = Vec::new();
        for index in 0..self.nodes.len() {
            let node = &self.nodes[index];
            if !node.element() {
                continue;
            }
            if node.name == "label" && matcher.matches(self.text(index)) {
                let target = match node.attribute("for") {
                    Some(id) => self.by_id(id),
                    None => self
                        .descendants(index)
                        .find(|&child| LABELABLE.contains(&self.nodes[child].name.as_str())),
                };
                found.extend(target);
            }
            if let Some(ids) = node.attribute("aria-labelledby") {
                let text = ids
                    .split_whitespace()
                    .filter_map(|id| self.by_id(id))
                    .map(|label| self.text(label).to_owned())
                    .collect::<Vec<_>>()
                    .join(" ");
                if !text.is_empty() && matcher.matches(&text) {
                    found.push(index);
                }
            }
            if node
                .attribute("aria-label")
                .is_some_and(|label| matcher.matches(label))
            {
                found.push(index);
            }
        }
        found.retain(|&index| self.contains(scope, index));
        found
    }
}

pub(crate) struct AxNode {
    pub(crate) backend: u64,
    pub(crate) role: String,
    pub(crate) name: String,
    pub(crate) properties: HashMap<String, Value>,
}

pub(crate) fn ax_text(value: &Value) -> String {
    match &value["value"] {
        Value::String(text) => normalize(text),
        Value::Number(number) => number.to_string(),
        Value::Bool(value) => value.to_string(),
        _ => String::new(),
    }
}

/// The node for `backend` in an `Accessibility.getPartialAXTree` reply.
pub(crate) fn ax_node_of(tree: &Value, backend: u64) -> Option<Value> {
    tree["nodes"]
        .as_array()?
        .iter()
        .find(|node| node["backendDOMNodeId"].as_u64() == Some(backend))
        .cloned()
}

/// A policy decision needs the target's role and name. A node the browser does not report
/// cannot be named, so the decision cannot be made and the operation fails closed; a reported
/// node with an empty name is decided on as unnamed.
pub(crate) fn decidable(node: Option<Value>) -> Result<Value, String> {
    node.ok_or_else(|| {
        "the target has no accessibility node to authorize; stopped before input".into()
    })
}

/// Convert an `Accessibility.getFullAXTree` reply under the same node and byte budgets as
/// [`Dom::parse`]; names are charged before they are copied.
pub(crate) fn ax_nodes(tree: &Value) -> Result<Vec<AxNode>, String> {
    let nodes = tree["nodes"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    if nodes.len() > MAX_NODES {
        return Err(TOO_LARGE.into());
    }
    let mut bytes = 0usize;
    let mut converted = Vec::new();
    for node in nodes.iter().filter(|node| node["ignored"] != true) {
        let Some(backend) = node["backendDOMNodeId"].as_u64() else {
            continue;
        };
        let properties = node["properties"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        bytes += [&node["role"], &node["name"]]
            .into_iter()
            .chain(properties.iter().map(|property| &property["value"]))
            .map(|value| value["value"].as_str().map_or(0, str::len))
            .sum::<usize>();
        if bytes > MAX_HOST_BYTES {
            return Err(TOO_LARGE.into());
        }
        converted.push(AxNode {
            backend,
            role: ax_text(&node["role"]),
            name: ax_text(&node["name"]),
            properties: properties
                .iter()
                .filter_map(|property| {
                    Some((
                        property["name"].as_str()?.to_owned(),
                        property["value"]["value"].clone(),
                    ))
                })
                .collect(),
        });
    }
    Ok(converted)
}

/// Chromium reports a few internal role names where ARIA (and Playwright) use another.
pub(crate) fn role_matches(query: &str, actual: &str) -> bool {
    query == actual
        || matches!(
            (query, actual),
            ("img", "image")
                | ("option", "MenuListOption")
                | ("option", "ListBoxOption")
                | ("none", "presentation")
                | ("presentation", "none")
        )
}

/// Compare a getByRole state option (`checked`, `level`, ...) with an AX property value.
pub(crate) fn state_matches(
    name: &str,
    expected: &Value,
    properties: &HashMap<String, Value>,
) -> bool {
    let actual = properties.get(name);
    match (name, expected) {
        ("level", expected) => actual.and_then(Value::as_i64) == expected.as_i64(),
        (_, Value::Bool(expected)) => {
            let on = match actual {
                Some(Value::Bool(value)) => *value,
                Some(Value::String(value)) => value == "true" || value == "mixed",
                _ => false,
            };
            on == *expected
        }
        (_, Value::String(expected)) => actual.and_then(Value::as_str) == Some(expected.as_str()),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    fn document() -> Dom {
        let document: Value = serde_json::from_str(
            r##"{"root":{"nodeId":1,"backendNodeId":1,"nodeType":9,"nodeName":"#document","children":[
            {"nodeId":2,"backendNodeId":2,"nodeType":1,"localName":"html","children":[
                {"nodeId":3,"backendNodeId":3,"nodeType":1,"localName":"head","children":[
                    {"nodeId":4,"backendNodeId":4,"nodeType":1,"localName":"title","children":[{"nodeId":5,"backendNodeId":5,"nodeType":3,"nodeValue":"Hello"}]}]},
                {"nodeId":6,"backendNodeId":6,"nodeType":1,"localName":"body","children":[
                    {"nodeId":7,"backendNodeId":7,"nodeType":1,"localName":"p","attributes":["id","p"],"children":[
                        {"nodeId":8,"backendNodeId":8,"nodeType":3,"nodeValue":"Hello "},
                        {"nodeId":9,"backendNodeId":9,"nodeType":1,"localName":"b","children":[{"nodeId":10,"backendNodeId":10,"nodeType":3,"nodeValue":"world"}]}]},
                    {"nodeId":11,"backendNodeId":11,"nodeType":1,"localName":"label","attributes":["for","name"],"children":[{"nodeId":12,"backendNodeId":12,"nodeType":3,"nodeValue":"Your name"}]},
                    {"nodeId":13,"backendNodeId":13,"nodeType":1,"localName":"input","attributes":["id","name","placeholder","Type here"]},
                    {"nodeId":14,"backendNodeId":14,"nodeType":1,"localName":"input","attributes":["type","submit","value","Send it"]}]}]}]}}"##,
        )
        .unwrap();
        Dom::parse(&document, &|| false).unwrap()
    }

    #[test]
    fn text_engine_returns_innermost_match_and_skips_head() {
        let dom = document();
        let matcher = |text: &str, exact| {
            Matcher::Text {
                text: text.into(),
                exact,
            }
            .compile()
            .unwrap()
        };
        let backend = |found: Vec<usize>| {
            found
                .iter()
                .map(|&index| dom.nodes[index].backend)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            backend(dom.text_matches(0, &matcher("hello world", false))),
            vec![7]
        );
        assert_eq!(
            backend(dom.text_matches(0, &matcher("world", true))),
            vec![9]
        );
        assert_eq!(
            backend(dom.text_matches(0, &matcher("hello", true))),
            Vec::<u64>::new()
        );
        assert_eq!(
            backend(dom.text_matches(0, &matcher("send", false))),
            vec![14]
        );
        assert_eq!(
            backend(dom.label_matches(0, &matcher("your name", false))),
            vec![13]
        );
        assert_eq!(
            backend(dom.attribute_matches(0, "placeholder", &matcher("type", false))),
            vec![13]
        );
        let regex = Matcher::Regex {
            source: "^hello\\s+WORLD$".into(),
            flags: "i".into(),
        }
        .compile()
        .unwrap();
        assert_eq!(backend(dom.text_matches(0, &regex)), vec![7]);
        assert!(Matcher::Regex {
            source: "(?=x)".into(),
            flags: String::new()
        }
        .compile()
        .is_err());
        assert_eq!(dom.text_content(dom.by_backend[&7]), "Hello world");
    }

    #[test]
    fn parse_enforces_node_text_and_cancellation_budgets() {
        let document: Value = serde_json::from_str(
            r#"{"root":{"nodeId":1,"backendNodeId":1,"nodeType":9,"nodeName":"x","children":[
            {"nodeId":2,"backendNodeId":2,"nodeType":1,"localName":"p","children":[
                {"nodeId":3,"backendNodeId":3,"nodeType":3,"nodeValue":"0123456789"}]}]}}"#,
        )
        .unwrap();
        assert!(Dom::parse_bounded(&document, &|| false, 3, 1024).is_ok());
        assert!(Dom::parse_bounded(&document, &|| false, 2, 1024)
            .is_err_and(|error| error.contains("too large")));
        // Text is counted once per ancestor that stores it.
        assert!(Dom::parse_bounded(&document, &|| false, 3, 30)
            .is_err_and(|error| error.contains("too large")));
        assert!(Dom::parse_bounded(&document, &|| true, 3, 1024)
            .is_err_and(|error| error.contains("stopped")));
        // Attribute bytes are charged before they are copied.
        let attributes: Value = serde_json::from_str(&format!(
            r#"{{"root":{{"nodeId":1,"backendNodeId":1,"nodeType":1,"localName":"p","attributes":["title","{}"]}}}}"#,
            "a".repeat(2000)
        ))
        .unwrap();
        assert!(Dom::parse_bounded(&attributes, &|| false, 3, 4096).is_ok());
        assert!(Dom::parse_bounded(&attributes, &|| false, 3, 1024)
            .is_err_and(|error| error.contains("too large")));
    }

    #[test]
    fn policy_needs_the_ax_node() {
        let tree = serde_json::json!({"nodes": [
            {"backendDOMNodeId": 5, "role": {"value": "button"}, "name": {"value": ""}}
        ]});
        // Present but unnamed: decided on, with an empty name.
        let node = decidable(ax_node_of(&tree, 5)).unwrap();
        assert_eq!(ax_text(&node["name"]), "");
        // Absent (another node's reply, or none at all): refused before any input.
        for missing in [ax_node_of(&tree, 6), ax_node_of(&serde_json::json!({}), 5)] {
            assert!(decidable(missing).is_err_and(|error| error.contains("no accessibility node")));
        }
    }
}
