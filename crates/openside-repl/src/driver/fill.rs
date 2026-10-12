//! `fill`: which elements accept it, and whether the page kept what was entered.

use std::time::Duration;

use serde_json::{json, Value};

use super::{
    error::{OpError, OpResult},
    keyboard::Keys,
    locate::NodeRef,
    Driver,
};
use crate::dom::Dom;

/// Time a controlled field gets to render the new value before it is read back; frameworks
/// commit state on the next task or animation frame.
const SETTLE: Duration = Duration::from_millis(60);

/// Current text of an editable element as the user would see it.
pub(super) const READ_BACK: &str = "function () {
  if (this instanceof HTMLInputElement || this instanceof HTMLTextAreaElement) return this.value;
  return this.isContentEditable ? this.innerText : this.textContent;
}";

fn same_text(left: &str, right: &str) -> bool {
    let canonical = |text: &str| text.replace("\r\n", "\n").trim_end_matches('\n').to_owned();
    canonical(left) == canonical(right)
}

impl Driver {
    pub(super) async fn editable(&self, dom: &Dom, index: usize) -> OpResult<()> {
        let node = &dom.nodes[index];
        let text_input = node.name == "textarea"
            || (node.name == "input"
                && !matches!(
                    node.attribute("type")
                        .map(str::to_ascii_lowercase)
                        .as_deref(),
                    Some(
                        "checkbox"
                            | "radio"
                            | "file"
                            | "button"
                            | "submit"
                            | "reset"
                            | "image"
                            | "hidden"
                            | "range"
                            | "color"
                    )
                ))
            || node
                .attribute("contenteditable")
                .is_some_and(|value| value != "false");
        if !text_input {
            return Err(OpError::new(
                "Element is not an <input>, <textarea> or [contenteditable] element",
            ));
        }
        if node.attribute("readonly").is_some() {
            return Err(OpError::new("Element is not editable"));
        }
        Ok(())
    }

    /// Replace the content of an editable element with `text` through trusted input (select
    /// all, then insert), then read the value back. `Null` means the page holds exactly `text`;
    /// `{normalized: true, ...}` means it kept something else, as input masks and `maxlength`
    /// do; a field that went empty is an error, as that is a controlled input refusing the value.
    pub(super) async fn fill_value(
        &self,
        dom: &Dom,
        index: usize,
        description: &str,
        text: &str,
        keys: &mut Keys,
    ) -> OpResult<Value> {
        self.editable(dom, index).await?;
        let node = dom.node_ref(index);
        self.focus_on(&node, description).await?;
        self.node_gate(&node, "fill", &mut keys.granted).await?;
        keys.focused = Some(node.clone());
        self.press("ControlOrMeta+a", 0, keys).await?;
        match text {
            "" => self.press("Delete", 0, keys).await?,
            text => self.insert(text, keys).await?,
        }
        tokio::time::sleep(SETTLE).await;
        // A page that navigated or removed the field leaves nothing to compare with.
        let Ok(actual) = self.read_back(&node).await else {
            return Ok(Value::Null);
        };
        if same_text(&actual, text) {
            return Ok(Value::Null);
        }
        if actual.is_empty() {
            return Err(OpError::new(format!(
                "fill did not take: the page reset {description} to empty after the text was entered"
            )));
        }
        let password = dom.nodes[index]
            .attribute("type")
            .is_some_and(|kind| kind.eq_ignore_ascii_case("password"));
        Ok(if password {
            json!({"normalized": true, "length": actual.chars().count()})
        } else {
            json!({"normalized": true, "value": actual})
        })
    }

    async fn read_back(&self, node: &NodeRef) -> OpResult<String> {
        let frame = match &node.frame {
            Some(frame) => frame.clone(),
            None => self.frame().await?["id"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        };
        let world = self
            .host_cmd(
                node.host.as_deref(),
                "Page.createIsolatedWorld",
                json!({"frameId": frame, "worldName": "openside-fill"}),
            )
            .await?;
        let resolved = self
            .node_cmd(
                node,
                "DOM.resolveNode",
                json!({"executionContextId": world["executionContextId"]}),
            )
            .await?;
        let object = resolved["object"]["objectId"]
            .as_str()
            .ok_or_else(|| OpError::new("element is gone"))?
            .to_owned();
        let read = self
            .host_cmd(
                node.host.as_deref(),
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object,
                    "functionDeclaration": READ_BACK,
                    "returnByValue": true,
                    "silent": true,
                }),
            )
            .await;
        let _ = self
            .host_cmd(
                node.host.as_deref(),
                "Runtime.releaseObject",
                json!({"objectId": object}),
            )
            .await;
        Ok(read?["result"]["value"]
            .as_str()
            .unwrap_or_default()
            .to_owned())
    }
}
