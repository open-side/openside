use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde_json::Value;

use crate::BrowserError;

/// A retained accessibility node in document order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotNode {
    /// Stable across this controller's snapshots, never reused while it tracks the tab.
    pub reference: Option<String>,
    /// Accessibility role.
    pub role: String,
    /// Accessible name, with embedded whitespace compacted.
    pub name: String,
    /// Current value; protected fields are redacted.
    pub value: String,
    /// Depth after removal of redundant wrappers.
    pub depth: usize,
}

/// A complete, untruncated accessibility observation. `text` holds exactly one line per
/// entry of `nodes`, in the same order, so line numbers and node indexes are interchangeable.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Retained nodes in document order.
    pub nodes: Vec<SnapshotNode>,
    /// Compact line-oriented representation.
    pub text: String,
    /// UTF-8 byte count of the raw `getFullAXTree` result JSON.
    pub raw_bytes: usize,
    /// Backend node of the observed document's root; a different value means the tab
    /// navigated to a new document since an older snapshot.
    pub document_root: Option<u64>,
}

/// Reference-level changes between two observations of the same document.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SnapshotDiff {
    /// References present only in the newer snapshot.
    pub added: Vec<String>,
    /// References present only in the older snapshot.
    pub removed: Vec<String>,
    /// Retained references whose role, name, value, or depth changed.
    pub changed: Vec<String>,
}

impl Snapshot {
    /// Compare stable references in document order, without truncating either snapshot.
    pub fn diff(&self, newer: &Self) -> SnapshotDiff {
        fn indexed(snapshot: &Snapshot) -> HashMap<&str, &SnapshotNode> {
            snapshot
                .nodes
                .iter()
                .filter_map(|node| node.reference.as_deref().map(|reference| (reference, node)))
                .collect()
        }
        let old = indexed(self);
        let new = indexed(newer);
        SnapshotDiff {
            added: newer
                .nodes
                .iter()
                .filter_map(|node| node.reference.as_ref())
                .filter(|reference| !old.contains_key(reference.as_str()))
                .cloned()
                .collect(),
            removed: self
                .nodes
                .iter()
                .filter_map(|node| node.reference.as_ref())
                .filter(|reference| !new.contains_key(reference.as_str()))
                .cloned()
                .collect(),
            changed: newer
                .nodes
                .iter()
                .filter_map(|node| {
                    let reference = node.reference.as_ref()?;
                    old.get(reference.as_str())
                        .filter(|previous| **previous != node)
                        .map(|_| reference.clone())
                })
                .collect(),
        }
    }

    /// Show every non-empty text-entry value as `<redacted>`, like a protected field. Hosts
    /// call this for a document that received a secret fill: a code split over one-character
    /// boxes is too short for output redaction, yet the boxes together spell it out.
    pub fn mask_field_values(&mut self) {
        const MASK: &str = "<redacted>";
        let editable = |node: &SnapshotNode| {
            matches!(
                node.role.as_str(),
                "textbox" | "searchbox" | "spinbutton" | "combobox"
            ) && !node.value.is_empty()
                && node.value != MASK
        };
        if !self.nodes.iter().any(editable) {
            return;
        }
        // `render` writes exactly one line per node and ends a valued node's line with
        // ` = {value:?}`; when that shape does not hold, render again without hidden names.
        let lines: Vec<&str> = self.text.lines().collect();
        let mut text =
            (lines.len() == self.nodes.len()).then(|| String::with_capacity(self.text.len()));
        for (index, node) in self.nodes.iter_mut().enumerate() {
            let masked = editable(node);
            if let Some(out) = &mut text {
                let line = lines[index];
                if masked {
                    match line.strip_suffix(format!(" = {:?}", node.value).as_str()) {
                        Some(head) => {
                            out.push_str(head);
                            out.push_str(&format!(" = {MASK:?}"));
                        }
                        None => text = None,
                    }
                } else {
                    out.push_str(line);
                }
                if let Some(out) = &mut text {
                    out.push('\n');
                }
            }
            if masked {
                node.value = MASK.to_owned();
            }
        }
        self.text = text.unwrap_or_else(|| render(&self.nodes, &vec![false; self.nodes.len()]));
    }
}

/// The node a snapshot reference names: a backend DOM node, qualified by the frame whose
/// document holds it and the out-of-process iframe target whose session hosts it. Backend ids
/// are per renderer process, so `backend` alone is ambiguous once frames are included.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RefTarget {
    /// Frame id of the node's document; None for the top frame.
    pub frame: Option<String>,
    /// OOPIF target whose session addresses the node; None for the tab's page session.
    pub host: Option<String>,
    /// Backend DOM node id within `host`.
    pub backend: u64,
}

#[derive(Default)]
pub(crate) struct References {
    ids: HashMap<String, String>,
    next: usize,
    document_root: Option<u64>,
    current: HashMap<String, RefTarget>,
}

impl References {
    pub(crate) fn invalidate(&mut self) {
        self.current.clear();
    }

    /// Retire every reference handed out so far: later captures number nodes afresh, so an
    /// old `eN` never resolves again, even to a node that survived.
    pub(crate) fn void(&mut self) {
        self.ids.clear();
        self.current.clear();
    }

    pub(crate) fn resolve(&self, reference: &str, document_root: u64) -> Option<RefTarget> {
        (self.document_root == Some(document_root))
            .then(|| self.current.get(reference).cloned())
            .flatten()
    }
}

/// The accessibility tree of one child frame, to be spliced under its owner element's node.
pub(crate) struct FrameTree {
    pub(crate) frame: String,
    pub(crate) host: Option<String>,
    /// Parent frame id; None when the parent is the top frame.
    pub(crate) parent: Option<String>,
    /// Backend node of the owner `<iframe>` in the parent frame's document.
    pub(crate) owner: u64,
    pub(crate) ax: Value,
}

/// Splice child-frame trees (parents before children) into the top frame's
/// `getFullAXTree` reply. Child node ids are prefixed with their frame id, since AX ids are
/// only unique per document, and every child node is tagged with its frame and host so refs
/// address it on the right session. A frame whose owner node is not in the tree is dropped.
pub(crate) fn merge_frames(main: &mut Value, frames: Vec<FrameTree>) {
    let Some(nodes) = main["nodes"].as_array_mut() else {
        return;
    };
    for mut tree in frames {
        let Some(Value::Array(children)) = tree.ax.get_mut("nodes").map(Value::take) else {
            continue;
        };
        let prefix = |id: &str| format!("{}:{id}", tree.frame);
        let owner = nodes.iter_mut().find(|node| {
            node["backendDOMNodeId"].as_u64() == Some(tree.owner)
                && node["opensideFrame"].as_str() == tree.parent.as_deref()
        });
        let root = children
            .iter()
            .find(|node| node["parentId"].is_null())
            .and_then(|node| node["nodeId"].as_str())
            .map(prefix);
        let (Some(owner), Some(root)) = (owner, root) else {
            continue;
        };
        match owner["childIds"].as_array_mut() {
            Some(ids) => ids.push(Value::String(root)),
            None => owner["childIds"] = Value::Array(vec![Value::String(root)]),
        }
        for mut node in children {
            if let Some(id) = node["nodeId"].as_str() {
                node["nodeId"] = Value::String(prefix(id));
            }
            // A leaf's missing array must stay absent; inserting null defeats serde's default.
            if let Some(ids) = node.get_mut("childIds").and_then(Value::as_array_mut) {
                for id in ids {
                    if let Some(child) = id.as_str() {
                        *id = Value::String(prefix(child));
                    }
                }
            }
            node["opensideFrame"] = Value::String(tree.frame.clone());
            if let Some(host) = &tree.host {
                node["opensideHost"] = Value::String(host.clone());
            }
            nodes.push(node);
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AxNode {
    node_id: String,
    #[serde(default)]
    ignored: bool,
    #[serde(default)]
    role: Value,
    #[serde(default)]
    name: Value,
    #[serde(default)]
    value: Value,
    #[serde(default)]
    child_ids: Vec<String>,
    #[serde(rename = "backendDOMNodeId")]
    backend_dom_node_id: Option<u64>,
    #[serde(default)]
    properties: Vec<Value>,
    #[serde(default, rename = "opensideFrame")]
    frame: Option<String>,
    #[serde(default, rename = "opensideHost")]
    host: Option<String>,
}

impl AxNode {
    /// Reference key: frame-qualified, because backend ids repeat across renderer processes.
    fn key(&self) -> Option<String> {
        let id = self.backend_dom_node_id?;
        Some(match &self.frame {
            None => format!("dom:{id}"),
            Some(frame) => format!("dom:{frame}:{id}"),
        })
    }

    fn target(&self) -> Option<RefTarget> {
        Some(RefTarget {
            frame: self.frame.clone(),
            host: self.host.clone(),
            backend: self.backend_dom_node_id?,
        })
    }
}

fn ax_text(value: &Value) -> String {
    match &value["value"] {
        Value::String(text) => text.split_whitespace().collect::<Vec<_>>().join(" "),
        Value::Number(number) => number.to_string(),
        Value::Bool(value) => value.to_string(),
        _ => String::new(),
    }
}

/// Roles whose accessible name already says everything; their text children repeat it.
fn text_leaf(role: &str) -> bool {
    matches!(role, "StaticText" | "image")
}

fn interactive(role: &str) -> bool {
    matches!(
        role,
        "button"
            | "link"
            | "textbox"
            | "searchbox"
            | "checkbox"
            | "radio"
            | "combobox"
            | "listbox"
            | "option"
            | "slider"
            | "spinbutton"
            | "switch"
            | "tab"
            | "menuitem"
            | "menuitemcheckbox"
            | "menuitemradio"
            | "treeitem"
    )
}

/// Backend DOM nodes whose layout box (document coordinates, from `DOMSnapshot.captureSnapshot`
/// of the main document) overlaps the rectangle `[left, left + width) x [top, top + height)`.
/// The box is the node's own, not clipped by scrolling ancestors; a node without a layout box
/// is never reported, and `process` keeps it only as an ancestor of one that is.
pub(crate) fn nodes_in_view(
    dom: &Value,
    left: f64,
    top: f64,
    width: f64,
    height: f64,
) -> Result<HashSet<u64>, BrowserError> {
    let invalid = || BrowserError::InvalidResponse("DOM snapshot");
    let document = dom["documents"].get(0).ok_or_else(invalid)?;
    let backend = document["nodes"]["backendNodeId"]
        .as_array()
        .ok_or_else(invalid)?;
    let layout_nodes = document["layout"]["nodeIndex"]
        .as_array()
        .ok_or_else(invalid)?;
    let bounds = document["layout"]["bounds"]
        .as_array()
        .ok_or_else(invalid)?;
    let (right, bottom) = (left + width, top + height);
    let mut visible = HashSet::new();
    for (index, rect) in layout_nodes.iter().zip(bounds) {
        let (Some(index), Some(rect)) = (index.as_u64(), rect.as_array()) else {
            continue;
        };
        let [x, y, w, h] = [0, 1, 2, 3].map(|axis| rect.get(axis).and_then(Value::as_f64));
        let (Some(x), Some(y), Some(w), Some(h)) = (x, y, w, h) else {
            continue;
        };
        if x < right && x + w >= left && y < bottom && y + h >= top {
            if let Some(id) = backend
                .get(usize::try_from(index).map_err(|_| invalid())?)
                .and_then(Value::as_u64)
            {
                visible.insert(id);
            }
        }
    }
    Ok(visible)
}

/// Per AX node: kept when it is a root, is in `view`, or has a kept descendant.
fn kept_in_view(
    nodes: &[AxNode],
    indexes: &HashMap<&str, usize>,
    view: &HashSet<u64>,
) -> Vec<bool> {
    let mut parent: Vec<Option<usize>> = vec![None; nodes.len()];
    for (index, node) in nodes.iter().enumerate() {
        for child in &node.child_ids {
            if let Some(&child) = indexes.get(child.as_str()) {
                parent[child].get_or_insert(index);
            }
        }
    }
    let mut kept: Vec<bool> = parent.iter().map(Option::is_none).collect();
    for (index, node) in nodes.iter().enumerate() {
        // Child-frame trees are merged only when their owner frame is in view, and their
        // backend ids are not comparable with the top document's layout snapshot.
        let shown = node.frame.is_some()
            || node
                .backend_dom_node_id
                .is_some_and(|id| view.contains(&id));
        if !shown {
            continue;
        }
        let mut at = Some(index);
        while let Some(current) = at {
            // A kept node's ancestors were kept when it was marked.
            if std::mem::replace(&mut kept[current], true) {
                break;
            }
            at = parent[current];
        }
    }
    kept
}

/// Compact tree of a full AX tree. With `view`, only nodes overlapping the viewport and their
/// ancestors are kept; refs of the omitted nodes that were handed out before still resolve.
pub(crate) fn process(
    mut raw: Value,
    refs: &mut References,
    view: Option<&HashSet<u64>>,
) -> Result<Snapshot, BrowserError> {
    let raw_bytes = serde_json::to_vec(&raw)
        .map_err(|_| BrowserError::InvalidResponse("AX JSON"))?
        .len();
    let raw_nodes = raw
        .as_object_mut()
        .and_then(|object| object.remove("nodes"))
        .ok_or(BrowserError::InvalidResponse("AX nodes"))?;
    let nodes: Vec<AxNode> =
        serde_json::from_value(raw_nodes).map_err(|_| BrowserError::InvalidResponse("AX nodes"))?;
    // Document identity also guards against navigation events arriving after an observation.
    let document_root = nodes
        .iter()
        .find(|node| node.frame.is_none() && node.role["value"] == "RootWebArea")
        .and_then(|node| node.backend_dom_node_id);
    if document_root.is_some() && document_root != refs.document_root {
        refs.ids.clear();
        refs.document_root = document_root;
    }
    refs.current.clear();
    let indexes: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.node_id.as_str(), index))
        .collect();
    let children: HashSet<&str> = nodes
        .iter()
        .flat_map(|node| node.child_ids.iter().map(String::as_str))
        .collect();
    let kept = view.map(|view| kept_in_view(&nodes, &indexes, view));
    // (AX index, depth, index in `retained` of the nearest retained ancestor)
    let mut stack: Vec<(usize, usize, Option<usize>)> = nodes
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, node)| !children.contains(node.node_id.as_str()))
        .map(|(index, _)| (index, 0, None))
        .collect();
    let mut retained: Vec<SnapshotNode> = Vec::new();
    let mut seen = HashSet::new();
    while let Some((index, depth, parent)) = stack.pop() {
        let node = &nodes[index];
        if !seen.insert(index) {
            continue;
        }
        if kept.as_ref().is_some_and(|kept| !kept[index]) {
            continue;
        }
        let role = ax_text(&node.role);
        let name = ax_text(&node.name);
        // List bullets and text that only repeats its parent's accessible name or value (link,
        // button, heading and cell labels, a field's typed text) carry no information; skip
        // them with their subtrees.
        if role == "ListMarker"
            || (text_leaf(&role)
                && parent.is_some_and(|parent: usize| {
                    retained[parent].name == name
                        || (!name.is_empty() && retained[parent].value == name)
                }))
        {
            continue;
        }
        let drop_node = node.ignored
            || matches!(role.as_str(), "none" | "InlineTextBox")
            || (role == "generic" && name.is_empty())
            // Landmarks stay even with one child: dropping them would re-indent their whole
            // subtree in the next diff once they gain a second child.
            || (node.child_ids.len() == 1
                && name.is_empty()
                && !interactive(&role)
                && !matches!(
                    role.as_str(),
                    "main" | "navigation" | "banner" | "contentinfo" | "complementary" | "form"
                        | "search" | "region" | "article" | "dialog" | "list" | "table" | "grid"
                ));
        let (next_depth, next_parent) = if drop_node {
            (depth, parent)
        } else {
            (depth + 1, Some(retained.len()))
        };
        for child in node.child_ids.iter().rev() {
            if let Some(index) = indexes.get(child.as_str()) {
                stack.push((*index, next_depth, next_parent));
            }
        }
        if drop_node {
            continue;
        }
        let reference = if interactive(&role) || !name.is_empty() {
            let key = node.key().unwrap_or_else(|| format!("ax:{}", node.node_id));
            let reference = refs.ids.entry(key).or_insert_with(|| {
                refs.next += 1;
                format!("e{}", refs.next)
            });
            if let Some(target) = node.target() {
                refs.current.insert(reference.clone(), target);
            }
            Some(reference.clone())
        } else {
            None
        };
        let protected = node
            .properties
            .iter()
            .any(|property| property["name"] == "protected" && property["value"]["value"] == true);
        retained.push(SnapshotNode {
            reference,
            role,
            name,
            value: if protected {
                "<redacted>".to_owned()
            } else {
                ax_text(&node.value)
            },
            depth,
        });
    }
    if let Some(kept) = &kept {
        // An omitted node keeps the ref it already had, so a ref from an earlier capture
        // still acts on it; it only gets no new number, which would be unseen text.
        for (node, _) in nodes.iter().zip(kept).filter(|(_, kept)| !**kept) {
            if let (Some(key), Some(target)) = (node.key(), node.target()) {
                if let Some(reference) = refs.ids.get(&key) {
                    refs.current.insert(reference.clone(), target);
                }
            }
        }
    }
    let (nodes, hidden_names) = compact(retained);
    Ok(Snapshot {
        text: render(&nodes, &hidden_names),
        nodes,
        raw_bytes,
        document_root,
    })
}

fn squeezed(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Lossless reductions over the retained tree, keeping one node per output line:
/// label text next to the control it names, text pieces that only spell out their parent's
/// content-derived name, and nameless single-child wrappers left behind by earlier drops.
/// Returns the nodes plus, per node, whether its name is printed elsewhere (a row whose name
/// is the concatenation of its cells) and is left out of the rendering.
fn compact(nodes: Vec<SnapshotNode>) -> (Vec<SnapshotNode>, Vec<bool>) {
    // Content concatenations longer than this are not compared, bounding memory on deep pages.
    const CONTENT_CAP: usize = 4096;
    let count = nodes.len();
    let mut removed = vec![false; count];
    for (index, node) in nodes.iter().enumerate() {
        let names_control = |other: Option<&SnapshotNode>| {
            other.is_some_and(|other| {
                other.depth == node.depth && interactive(&other.role) && other.name == node.name
            })
        };
        removed[index] = node.role == "StaticText"
            && (names_control(nodes.get(index + 1))
                || index
                    .checked_sub(1)
                    .is_some_and(|previous| names_control(nodes.get(previous))));
    }
    let children = |removed: &[bool]| {
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); count];
        let mut ancestors: Vec<usize> = Vec::new();
        for index in (0..count).filter(|&index| !removed[index]) {
            while ancestors
                .last()
                .is_some_and(|&top| nodes[top].depth >= nodes[index].depth)
            {
                ancestors.pop();
            }
            if let Some(&parent) = ancestors.last() {
                children[parent].push(index);
            }
            ancestors.push(index);
        }
        children
    };
    let tree = children(&removed);
    let mut content: Vec<Option<String>> = vec![None; count];
    let mut hidden = vec![false; count];
    let order: Vec<usize> = (0..count).rev().filter(|&index| !removed[index]).collect();
    for index in order {
        let node = &nodes[index];
        if tree[index].is_empty() {
            content[index] = Some(squeezed(&node.name));
            continue;
        }
        let mut joined = Some(String::new());
        for &child in &tree[index] {
            let piece = if nodes[child].name.is_empty() {
                content[child].clone()
            } else {
                Some(squeezed(&nodes[child].name))
            };
            joined = joined
                .zip(piece)
                .map(|(joined, piece)| joined + &piece)
                .filter(|joined| joined.len() <= CONTENT_CAP);
        }
        if !node.name.is_empty() && joined.as_deref() == Some(squeezed(&node.name).as_str()) {
            if tree[index]
                .iter()
                .all(|&child| nodes[child].role == "StaticText")
            {
                for &child in &tree[index] {
                    removed[child] = true;
                }
            } else if !interactive(&node.role) {
                hidden[index] = true;
            }
        }
        content[index] = if node.name.is_empty() {
            joined
        } else {
            Some(squeezed(&node.name))
        };
    }
    let tree = children(&removed);
    let collapsed: Vec<bool> = (0..count)
        .map(|index| {
            let node = &nodes[index];
            !removed[index]
                && node.name.is_empty()
                && node.value.is_empty()
                // Only presentational wrappers: collapsing a landmark would re-indent the whole
                // page in the next diff as soon as it gains a second child.
                && matches!(
                    node.role.as_str(),
                    "listitem" | "paragraph" | "LabelText" | "group" | "Section" | "time"
                        | "strong" | "emphasis" | "Abbr" | "superscript" | "subscript"
                )
                && tree[index].len() == 1
        })
        .collect();
    let mut kept = Vec::with_capacity(count);
    let mut kept_hidden = Vec::with_capacity(count);
    // (original depth, collapsed) of the current ancestors; collapsed ones lift their subtree.
    let mut ancestors: Vec<(usize, bool)> = Vec::new();
    let mut lifted = 0;
    for (index, mut node) in nodes.into_iter().enumerate() {
        if removed[index] {
            continue;
        }
        while ancestors
            .last()
            .is_some_and(|&(depth, _)| depth >= node.depth)
        {
            if let Some((_, true)) = ancestors.pop() {
                lifted -= 1;
            }
        }
        ancestors.push((node.depth, collapsed[index]));
        if collapsed[index] {
            lifted += 1;
            continue;
        }
        node.depth -= lifted;
        kept.push(node);
        kept_hidden.push(hidden[index]);
    }
    (kept, kept_hidden)
}

/// One line per node. Text runs are bare quoted strings: their references are not actionable
/// targets, so printing them would only cost tokens. A container whose only child is a
/// control with the same accessible name (a table cell around a checkbox) omits the name.
fn render(nodes: &[SnapshotNode], hidden_names: &[bool]) -> String {
    use std::fmt::Write;
    let mut text = String::new();
    for (index, node) in nodes.iter().enumerate() {
        for _ in 0..node.depth {
            text.push(' ');
        }
        if node.role == "StaticText" {
            let _ = writeln!(text, "{:?}", node.name);
            continue;
        }
        if let Some(reference) = &node.reference {
            let _ = write!(text, "[{reference}] ");
        }
        text.push_str(&node.role);
        let only_child = nodes.get(index + 1).filter(|child| {
            child.depth == node.depth + 1
                && nodes
                    .get(index + 2)
                    .map_or(true, |next| next.depth <= node.depth)
        });
        let repeated = hidden_names[index]
            || (!interactive(&node.role)
                && only_child
                    .is_some_and(|child| interactive(&child.role) && child.name == node.name));
        if !node.name.is_empty() && !repeated {
            let _ = write!(text, " {:?}", node.name);
        }
        if !node.value.is_empty() {
            let _ = write!(text, " = {:?}", node.value);
        }
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wrappers_refs_diff_and_protected_values() {
        let raw = json!({"nodes": [
            {"nodeId":"0","ignored":true,"childIds":["1"]},
            {"nodeId":"1","role":{"value":"generic"},"childIds":["2","3"]},
            {"nodeId":"2","backendDOMNodeId":42,"role":{"value":"button"},"name":{"value":"Save"}},
            {"nodeId":"3","backendDOMNodeId":43,"role":{"value":"textbox"},"name":{"value":"Password"},"value":{"value":"CANARY-secret"},"properties":[{"name":"protected","value":{"value":true}}]}
        ]});
        let mut refs = References::default();
        let old = process(raw.clone(), &mut refs, None).unwrap();
        assert_eq!(old.nodes.len(), 2);
        assert_eq!(old.nodes[0].depth, 0);
        assert_eq!(old.nodes[0].reference.as_deref(), Some("e1"));
        assert!(!old.text.contains("CANARY"));
        let mut changed = raw;
        changed["nodes"][2]["name"]["value"] = json!("Saved");
        let new = process(changed, &mut refs, None).unwrap();
        assert_eq!(old.diff(&new).changed, vec!["e1"]);
        assert!(old.diff(&new).added.is_empty());
        let absent = process(json!({"nodes":[]}), &mut refs, None).unwrap();
        assert_eq!(new.diff(&absent).removed, vec!["e1", "e2"]);
        let reappeared = process(json!({"nodes":[{"nodeId":"99","backendDOMNodeId":42,"role":{"value":"button"},"name":{"value":"Saved"}}]}), &mut refs, None).unwrap();
        assert_eq!(reappeared.nodes[0].reference.as_deref(), Some("e1"));
        let new_document = process(json!({"nodes":[{"nodeId":"root","backendDOMNodeId":100,"role":{"value":"RootWebArea"},"name":{"value":"New page"},"childIds":["button"]},{"nodeId":"button","backendDOMNodeId":101,"role":{"value":"button"},"name":{"value":"New action"}}]}), &mut refs, None).unwrap();
        assert_eq!(new_document.nodes[0].reference.as_deref(), Some("e3"));
        assert_eq!(new_document.nodes[1].reference.as_deref(), Some("e4"));
        let backend = |refs: &References, reference: &str, root: u64| {
            refs.resolve(reference, root).map(|target| target.backend)
        };
        assert_eq!(backend(&refs, "e1", 100), None);
        assert_eq!(backend(&refs, "e4", 100), Some(101));
        assert_eq!(backend(&refs, "e4", 99), None);
        refs.invalidate();
        assert_eq!(backend(&refs, "e4", 100), None);
    }

    #[test]
    fn child_frames_are_spliced_under_their_owner_with_qualified_refs() {
        let mut main = json!({"nodes":[
            {"nodeId":"1","backendDOMNodeId":1,"role":{"value":"RootWebArea"},"name":{"value":"Top"},"childIds":["2","3"]},
            {"nodeId":"2","backendDOMNodeId":5,"role":{"value":"button"},"name":{"value":"Go"}},
            {"nodeId":"3","backendDOMNodeId":6,"role":{"value":"Iframe"},"name":{"value":"pay"}}
        ]});
        // The child document reuses AX id "2" and backend id 5: another renderer process.
        let child = json!({"nodes":[
            {"nodeId":"1","backendDOMNodeId":1,"role":{"value":"RootWebArea"},"name":{"value":"Pay"},"childIds":["2"]},
            {"nodeId":"2","parentId":"1","backendDOMNodeId":5,"role":{"value":"button"},"name":{"value":"Go"}}
        ]});
        merge_frames(
            &mut main,
            vec![FrameTree {
                frame: "F".into(),
                host: Some("F".into()),
                parent: None,
                owner: 6,
                ax: child,
            }],
        );
        let mut refs = References::default();
        let snapshot = process(main, &mut refs, None).unwrap();
        let lines: Vec<&str> = snapshot.text.lines().collect();
        assert_eq!(
            lines,
            [
                "[e1] RootWebArea \"Top\"",
                " [e2] button \"Go\"",
                " [e3] Iframe \"pay\"",
                "  [e4] RootWebArea \"Pay\"",
                "   [e5] button \"Go\"",
            ]
        );
        assert_eq!(
            snapshot
                .nodes
                .iter()
                .map(|node| node.depth)
                .collect::<Vec<_>>(),
            [0, 1, 1, 2, 3]
        );
        assert_eq!(snapshot.document_root, Some(1));
        let top = refs.resolve("e2", 1).unwrap();
        assert_eq!((top.frame, top.host, top.backend), (None, None, 5));
        let inner = refs.resolve("e5", 1).unwrap();
        assert_eq!(
            (inner.frame.as_deref(), inner.host.as_deref(), inner.backend),
            (Some("F"), Some("F"), 5)
        );
    }

    #[test]
    fn masking_hides_every_text_entry_value_and_keeps_the_rest() {
        let boxes: Vec<Value> = (1..=3)
            .map(|n| json!({"nodeId":format!("b{n}"),"backendDOMNodeId":n,"role":{"value":"textbox"},"name":{"value":format!("Digit {n}")},"value":{"value":n.to_string()}}))
            .chain([
                json!({"nodeId":"b4","backendDOMNodeId":4,"role":{"value":"textbox"},"name":{"value":"Empty"}}),
                json!({"nodeId":"b5","backendDOMNodeId":5,"role":{"value":"button"},"name":{"value":"Verify 2"}}),
            ])
            .collect();
        let mut snapshot =
            process(json!({"nodes":boxes}), &mut References::default(), None).unwrap();
        assert!(snapshot.text.contains("= \"2\""));
        snapshot.mask_field_values();
        let values: Vec<&str> = snapshot
            .nodes
            .iter()
            .map(|node| node.value.as_str())
            .collect();
        assert_eq!(values, ["<redacted>", "<redacted>", "<redacted>", "", ""]);
        assert_eq!(
            snapshot.text,
            "[e1] textbox \"Digit 1\" = \"<redacted>\"\n[e2] textbox \"Digit 2\" = \"<redacted>\"\n[e3] textbox \"Digit 3\" = \"<redacted>\"\n[e4] textbox \"Empty\"\n[e5] button \"Verify 2\"\n"
        );
    }

    fn page() -> Value {
        json!({"nodes":[
            {"nodeId":"root","backendDOMNodeId":1,"role":{"value":"RootWebArea"},"name":{"value":"Page"},"childIds":["main","footer"]},
            {"nodeId":"main","backendDOMNodeId":2,"role":{"value":"main"},"childIds":["list","note"]},
            {"nodeId":"list","backendDOMNodeId":3,"role":{"value":"list"},"childIds":["a","b"]},
            {"nodeId":"a","backendDOMNodeId":4,"role":{"value":"link"},"name":{"value":"Visible"}},
            {"nodeId":"b","backendDOMNodeId":5,"role":{"value":"link"},"name":{"value":"Below"}},
            {"nodeId":"note","backendDOMNodeId":6,"role":{"value":"paragraph"},"name":{"value":"Far below"}},
            {"nodeId":"footer","backendDOMNodeId":7,"role":{"value":"contentinfo"},"childIds":["c","d"]},
            {"nodeId":"c","backendDOMNodeId":8,"role":{"value":"button"},"name":{"value":"Cookies"}},
            {"nodeId":"d","backendDOMNodeId":9,"role":{"value":"button"},"name":{"value":"Top"}}
        ]})
    }

    #[test]
    fn viewport_keeps_visible_nodes_with_ancestors_and_old_refs_resolve() {
        let view: HashSet<u64> = [4, 8].into();
        let mut refs = References::default();
        let full = process(page(), &mut refs, None).unwrap();
        let reference = |name: &str| {
            full.nodes
                .iter()
                .find(|node| node.name == name)
                .and_then(|node| node.reference.clone())
                .unwrap()
        };
        let (below, visible) = (reference("Below"), reference("Visible"));
        let part = process(page(), &mut refs, Some(&view)).unwrap();
        let names: Vec<&str> = part.nodes.iter().map(|node| node.name.as_str()).collect();
        assert_eq!(names, ["Page", "", "", "Visible", "", "Cookies"]);
        assert_eq!(part.nodes.len(), part.text.lines().count());
        assert_eq!(
            part.nodes
                .iter()
                .find(|node| node.name == "Visible")
                .unwrap()
                .reference,
            Some(visible)
        );
        // Not shown, but still the node it was in the earlier capture.
        assert_eq!(
            refs.resolve(&below, 1).map(|target| target.backend),
            Some(5)
        );

        let mut fresh = References::default();
        let first = process(page(), &mut fresh, Some(&view)).unwrap();
        assert!(!first.text.contains("Below") && !first.text.contains("Far below"));
        assert_eq!(fresh.resolve("e3", 1).map(|target| target.backend), Some(8));
        assert_eq!(fresh.resolve("e9", 1), None);
    }

    #[test]
    fn nodes_in_view_uses_document_coordinates_and_main_document_only() {
        let dom = json!({"documents":[{
            "nodes":{"backendNodeId":[10,11,12,13]},
            "layout":{
                "nodeIndex":[0,1,2,3],
                "bounds":[[0.0,0.0,800.0,3000.0],[10.0,590.0,100.0,40.0],[10.0,700.0,100.0,40.0],[10.0,2000.0,100.0,40.0]]
            }
        }]});
        // Scrolled by 600 with a 400-high viewport: document y in [600, 1000).
        let view = nodes_in_view(&dom, 0.0, 600.0, 800.0, 400.0).unwrap();
        let mut ids: Vec<u64> = view.into_iter().collect();
        ids.sort_unstable();
        assert_eq!(ids, [10, 11, 12]);
        assert!(nodes_in_view(&json!({}), 0.0, 0.0, 1.0, 1.0).is_err());
    }
}
