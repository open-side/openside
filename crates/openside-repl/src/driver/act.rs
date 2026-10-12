//! `locator.<action>()`: resolve exactly one actionable element, then dispatch to the pointer,
//! keyboard, fill, or select implementation.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{
    error::{OpError, OpResult},
    keyboard::Keys,
    locate::located,
    reach::{click_count, PointerOptions},
    Driver,
};

impl Driver {
    /// Handle the operations this module owns; `None` means the name belongs elsewhere.
    pub(super) async fn act_op(&self, name: &str, args: &Value) -> Option<OpResult<Value>> {
        Some(match name {
            "act" => self.act(args).await,
            _ => return None,
        })
    }

    pub(super) async fn act(&self, args: &Value) -> OpResult<Value> {
        let action = args["action"].as_str().unwrap_or_default().to_owned();
        let value = args["value"].clone();
        let options = args["options"].clone();
        let located = located(args)?;
        let timeout = located
            .timeout
            .map(Duration::from_millis)
            .unwrap_or(self.limits.action_timeout);
        let deadline = Instant::now() + timeout;
        // `force` skips the checks that say an element can be acted on, as users can still
        // press what is covered or greyed out; policy still judges what receives the event.
        let force = options["force"] == true;
        let (need_visible, need_enabled) = match action.as_str() {
            "focus" | "scroll" => (false, false),
            "select" => (false, true),
            "click" | "dblclick" | "hover" | "fill" | "drag" | "check" if force => (false, false),
            _ => (true, true),
        };
        let resolved = self
            .single(&located, timeout, need_visible, need_enabled)
            .await?;
        let index = resolved.found[0];
        let node = resolved.dom.node_ref(index);
        let before = self.frame().await?;
        let mut keys = Keys::default();
        let mut result = Value::Null;
        match action.as_str() {
            "click" | "dblclick" | "hover" => {
                let verb = match action.as_str() {
                    "hover" => "hover",
                    "dblclick" => "double-click",
                    _ => "click",
                };
                let clicks = click_count(&action, &options)?;
                let pointer = PointerOptions::parse(&options)?;
                self.pointer(&located, resolved, deadline, verb, clicks, &pointer)
                    .await?;
            }
            "drag" => {
                self.drag(&located, resolved, &value, deadline, timeout, &options)
                    .await?;
            }
            "scroll" => {
                self.scroll_action(&node, &resolved.loader).await?;
            }
            "fill" => {
                let text = value.as_str().unwrap_or_default();
                result = self
                    .fill_value(&resolved.dom, index, &located.description, text, &mut keys)
                    .await?;
            }
            "type" | "press" => {
                self.focus_on(&node, &located.description).await?;
                keys.focused = Some(node.clone());
                let text = value.as_str().unwrap_or_default();
                let delay = options["delay"].as_u64().unwrap_or_default();
                if action == "press" {
                    self.press(text, delay, &mut keys).await?;
                } else {
                    self.type_text(text, delay, options["ime"].as_bool(), &mut keys)
                        .await?;
                }
            }
            "focus" => {
                self.node_gate(&node, "focus", &mut keys.granted).await?;
                self.node_cmd(&node, "DOM.focus", json!({})).await?;
            }
            "check" => {
                let wanted = value != false;
                if self.checked(&node).await? != wanted {
                    let verb = if wanted { "check" } else { "uncheck" };
                    let pointer = PointerOptions::parse(&options)?;
                    self.pointer(&located, resolved, deadline, verb, 1, &pointer)
                        .await?;
                    let resolved = self.single(&located, timeout, false, false).await?;
                    let node = resolved.dom.node_ref(resolved.found[0]);
                    if self.checked(&node).await? != wanted {
                        return Err(OpError::new(
                            "Clicking the checkbox did not change its state",
                        ));
                    }
                }
            }
            "select" => {
                self.focus_on(&node, &located.description).await?;
                keys.focused = Some(node.clone());
                result = self.select(&resolved.dom, index, &value, &mut keys).await?;
            }
            other => {
                return Err(OpError {
                    name: "NotSupported",
                    message: format!("locator action {other} is not supported"),
                })
            }
        }
        self.settle(before).await?;
        Ok(result)
    }
}
