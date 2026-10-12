//! Element state used to decide whether an action may proceed: visibility, enabled and checked
//! state, read from the accessibility tree and box model.

use serde_json::{json, Value};

use super::locate::NodeRef;
use super::{
    error::{OpError, OpResult},
    Driver,
};
use crate::dom::Dom;

impl Driver {
    pub(super) async fn visible(&self, node: &NodeRef) -> OpResult<bool> {
        let Ok(model) = self.node_cmd(node, "DOM.getBoxModel", json!({})).await else {
            return Ok(false);
        };
        if model["model"]["width"].as_f64().unwrap_or_default() <= 0.0
            || model["model"]["height"].as_f64().unwrap_or_default() <= 0.0
        {
            return Ok(false);
        }
        // `visibility: hidden` keeps a layout box; Chromium marks such nodes notVisible in AX.
        let node = self.ax_node(node).await?;
        let hidden = node["ignored"] == true
            && node["ignoredReasons"].as_array().is_some_and(|reasons| {
                reasons.iter().any(|reason| {
                    matches!(reason["name"].as_str(), Some("notVisible" | "notRendered"))
                })
            });
        Ok(!hidden)
    }

    pub(super) async fn property(&self, node: &NodeRef, name: &str) -> OpResult<Option<Value>> {
        let node = self.ax_node(node).await?;
        Ok(node["properties"].as_array().and_then(|properties| {
            properties
                .iter()
                .find(|property| property["name"] == name)
                .map(|property| property["value"]["value"].clone())
        }))
    }

    /// Whether the element can take input: not disabled itself, not under a disabled
    /// `<fieldset>` (its first `<legend>` is exempt), not an option in a disabled `<optgroup>`,
    /// and not under `aria-disabled="true"`.
    pub(super) async fn enabled(&self, dom: &Dom, index: usize) -> OpResult<bool> {
        if disabled_by_markup(dom, index) {
            return Ok(false);
        }
        let disabled = self.property(&dom.node_ref(index), "disabled").await?;
        Ok(!matches!(disabled, Some(Value::Bool(true))))
    }

    pub(super) async fn checked(&self, node: &NodeRef) -> OpResult<bool> {
        match self.property(node, "checked").await? {
            Some(Value::String(state)) => Ok(state == "true"),
            Some(Value::Bool(state)) => Ok(state),
            _ => Err(OpError::new("Not a checkbox or radio button")),
        }
    }
}

const FORM_CONTROLS: &[&str] = &["button", "input", "select", "textarea"];

fn disabled_by_markup(dom: &Dom, index: usize) -> bool {
    let node = &dom.nodes[index];
    let control = FORM_CONTROLS.contains(&node.name.as_str());
    if (control || matches!(node.name.as_str(), "option" | "optgroup"))
        && node.attribute("disabled").is_some()
    {
        return true;
    }
    let aria = |node: &crate::dom::Node| {
        node.attribute("aria-disabled")
            .is_some_and(|value| value.eq_ignore_ascii_case("true"))
    };
    if aria(node) {
        return true;
    }
    let (mut child, mut next) = (index, node.parent);
    while let Some(parent) = next {
        let ancestor = &dom.nodes[parent];
        if ancestor.element() {
            if aria(ancestor) {
                return true;
            }
            let disabled = ancestor.attribute("disabled").is_some();
            if disabled && ancestor.name == "optgroup" && node.name == "option" {
                return true;
            }
            if disabled
                && ancestor.name == "fieldset"
                && control
                && !in_first_legend(dom, parent, child)
            {
                return true;
            }
        }
        child = parent;
        next = ancestor.parent;
    }
    false
}

/// Whether `child` (a direct child of `fieldset`, or the ancestor chain's step below it) is the
/// fieldset's first `<legend>`, whose descendants stay enabled under a disabled fieldset.
fn in_first_legend(dom: &Dom, fieldset: usize, child: usize) -> bool {
    dom.nodes[fieldset]
        .children
        .iter()
        .copied()
        .find(|&candidate| dom.nodes[candidate].name == "legend")
        == Some(child)
}
