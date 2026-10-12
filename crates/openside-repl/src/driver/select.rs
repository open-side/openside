//! `selectOption`: option matching and selection through trusted input, authorized per option.

use serde_json::{json, Value};

use super::{
    error::{OpError, OpResult},
    keyboard::Keys,
    Driver,
};
use crate::{
    dom::{ax_text, normalize, Dom},
    Target,
};

impl Driver {
    /// Native `<select>` changes go through the keyboard (Home, then ArrowDown) so the page sees
    /// trusted input/change events without any script; disabled options are skipped like users do.
    pub(super) async fn select(
        &self,
        dom: &Dom,
        index: usize,
        wanted: &Value,
        keys: &mut Keys,
    ) -> OpResult<Value> {
        let node = &dom.nodes[index];
        if node.name != "select" {
            return Err(OpError::new("Element is not a <select> element"));
        }
        if node.attribute("multiple").is_some() {
            return Err(OpError {
                name: "NotSupported",
                message: "selectOption on <select multiple> is not supported".into(),
            });
        }
        let options: Vec<usize> = (index + 1..dom.nodes.len())
            .take_while(|&option| dom.contains(index, option))
            .filter(|&option| dom.nodes[option].name == "option")
            .collect();
        let wanted = match wanted {
            Value::Array(values) if values.len() == 1 => &values[0],
            Value::Array(values) if values.is_empty() => {
                return Err(OpError::new("selectOption requires one option"))
            }
            Value::Array(_) => {
                return Err(OpError {
                    name: "NotSupported",
                    message: "selecting several options is not supported".into(),
                })
            }
            other => other,
        };
        let value_of = |option: usize| {
            dom.nodes[option]
                .attribute("value")
                .map(str::to_owned)
                .unwrap_or_else(|| normalize(dom.text_content(option)))
        };
        let label_of = |option: usize| {
            dom.nodes[option]
                .attribute("label")
                .map(str::to_owned)
                .unwrap_or_else(|| normalize(dom.text_content(option)))
        };
        let target = options.iter().position(|&option| match wanted {
            Value::String(text) => value_of(option) == *text || label_of(option) == *text,
            Value::Object(object) => {
                object
                    .get("value")
                    .and_then(Value::as_str)
                    .is_none_or(|value| value_of(option) == value)
                    && object
                        .get("label")
                        .and_then(Value::as_str)
                        .is_none_or(|label| label_of(option) == label)
                    && object
                        .get("index")
                        .and_then(Value::as_u64)
                        .is_none_or(|wanted| {
                            options.iter().position(|&other| other == option)
                                == usize::try_from(wanted).ok()
                        })
            }
            _ => false,
        });
        let target = target.ok_or_else(|| OpError::new(format!("no option matches {wanted}")))?;
        let disabled = |option: usize| {
            dom.nodes[option].attribute("disabled").is_some()
                || dom.nodes[option].parent.is_some_and(|parent| {
                    dom.nodes[parent].name == "optgroup"
                        && dom.nodes[parent].attribute("disabled").is_some()
                })
        };
        if disabled(options[target]) {
            return Err(OpError::new("option is disabled"));
        }
        let passed: Vec<usize> = options[..=target]
            .iter()
            .copied()
            .filter(|&option| !disabled(option))
            .collect();
        // Home and each ArrowDown select (and fire change for) every enabled option up to the
        // target, so each of them is authorized, named together with the select's own name.
        let select = dom.node_ref(index);
        let field = ax_text(&self.gate_node(&select).await?["name"]);
        for &option in &passed {
            let name = format!("{field}: {}", label_of(option));
            self.gate_node_page(
                &select,
                Some(Target {
                    action: "select option",
                    role: "option",
                    name: name.trim_start_matches(": "),
                }),
            )
            .await?;
        }
        let steps = passed.len() - 1;
        self.press("Home", 0, keys).await?;
        for _ in 0..steps {
            self.press("ArrowDown", 0, keys).await?;
        }
        Ok(json!([value_of(options[target])]))
    }
}
