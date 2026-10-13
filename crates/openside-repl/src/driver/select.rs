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
    /// Native `<select>` changes use trusted keyboard input, authorized for every possible
    /// intermediate option, and return only after the selected value is read back.
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
        #[cfg(not(target_os = "macos"))]
        let passed: Vec<usize> = options[..=target]
            .iter()
            .copied()
            .filter(|&option| !disabled(option))
            .collect();
        #[cfg(target_os = "macos")]
        let label = label_of(options[target]);
        #[cfg(target_os = "macos")]
        let passed = {
            let label = label.trim();
            if label.is_empty() {
                return Err(needs_takeover("The target option has no label to type."));
            }
            if label.chars().any(char::is_control) {
                return Err(needs_takeover(
                    "The target option's label contains nonprintable keys.",
                ));
            }
            let key: String = typeahead_label_chars(label).collect();
            let first = key
                .chars()
                .next()
                .expect("a nonempty label has a first key");
            let mut matches = 0;
            let mut passed = Vec::new();
            for &option in &options {
                if disabled(option) {
                    continue;
                }
                let candidate = label_of(option);
                if typeahead_label_chars(&candidate).eq(key.chars()) {
                    matches += 1;
                }
                if typeahead_can_visit(&candidate, first) {
                    passed.push(option);
                }
            }
            if matches != 1 {
                return Err(needs_takeover(
                    "More than one enabled option has the target label.",
                ));
            }
            passed
        };
        // A key can commit an intermediate choice, so authorize every possible choice
        // before the first event rather than checking only the requested endpoint.
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
        #[cfg(not(target_os = "macos"))]
        {
            let steps = passed.len() - 1;
            self.press("Home", 0, keys).await?;
            for _ in 0..steps {
                self.press("ArrowDown", 0, keys).await?;
            }
        }
        #[cfg(target_os = "macos")]
        {
            // Refocusing the same select does not clear Chrome's one-second typeahead buffer.
            tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
            let mut utf8 = [0; 4];
            for character in label.trim().chars() {
                self.key_target(keys, false).await?;
                if keys.focused.as_ref() != Some(&select) {
                    return Err(needs_takeover(
                        "The page moved focus while choosing an option.",
                    ));
                }
                let text = character.encode_utf8(&mut utf8);
                self.cmd(
                    "Input.dispatchKeyEvent",
                    json!({"type":"char","key":text,"text":text,"unmodifiedText":text,"modifiers":0}),
                ).await?;
            }
        }
        let current = self.document().await?;
        let current_index = current
            .index_of(&select)
            .ok_or_else(|| needs_takeover("The select was replaced while choosing an option."))?;
        let value = self.input_value(&current, current_index).await?;
        let committed = match dom.nodes[options[target]].attribute("value") {
            Some(expected) => value == expected,
            None => value == normalize(dom.text_content(options[target])),
        };
        if !committed {
            return Err(needs_takeover(
                "The browser did not commit the requested option.",
            ));
        }
        Ok(json!([value]))
    }
}

fn needs_takeover(reason: &str) -> OpError {
    OpError {
        name: "select_needs_takeover",
        message: format!("{reason} Take over this tab and choose the option manually."),
    }
}

#[cfg(any(target_os = "macos", test))]
fn typeahead_label_chars(label: &str) -> impl Iterator<Item = char> + '_ {
    label
        .trim()
        .chars()
        .flat_map(char::to_lowercase)
        .flat_map(char::to_uppercase)
}

#[cfg(any(target_os = "macos", test))]
fn typeahead_can_visit(label: &str, first: char) -> bool {
    // Native case folding can expand Unicode labels; include them conservatively rather
    // than accidentally sending a key to an option that was never authorized.
    label
        .trim_start()
        .chars()
        .next()
        .is_some_and(|candidate| !candidate.is_ascii() || candidate.eq_ignore_ascii_case(&first))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typeahead_ambiguity_checks_cover_case_and_unicode_expansions() {
        for (left, right) in [(" Rename ", "rename"), ("ß", "ẞ"), ("Σ", "ς"), ("ﬃ", "FFI")] {
            assert!(typeahead_label_chars(left).eq(typeahead_label_chars(right)));
        }
    }

    #[test]
    fn typeahead_authorizes_first_character_candidates_before_partial_selection() {
        assert!(typeahead_can_visit("Rename", 'R'));
        assert!(typeahead_can_visit("remove account", 'R'));
        assert!(typeahead_can_visit("ßensitive", 'S'));
        assert!(typeahead_can_visit("Σ choice", 'R'));
        assert!(!typeahead_can_visit("Keep", 'R'));
        assert!(!typeahead_can_visit("", 'R'));
    }
}
