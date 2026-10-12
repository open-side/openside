//! Keyboard input: focus handling, key chords, text insertion, and the policy check of the node
//! that holds focus when a key is sent.

use std::time::Duration;

use serde_json::{json, Value};

use super::{
    error::{OpError, OpResult},
    locate::NodeRef,
    Driver,
};
use crate::{
    dom::ax_text,
    keys::{self, hangul},
    Target,
};

pub(super) const FOREIGN_FOCUS: &str =
    "keyboard focus is not on an element of the page or its frames; the REPL only sends keys to elements it can inspect";

/// Owner elements whose content is another document; a focused one means focus is inside it.
const EMBEDDERS: &[&str] = &[
    "IFRAME",
    "FRAME",
    "OBJECT",
    "EMBED",
    "PORTAL",
    "FENCEDFRAME",
];

/// Keyboard state of one operation: the node last seen focused (a lookup hint only) and the
/// targets authorized during it with the role and name they had then (see `node_gate`).
#[derive(Default)]
pub(super) struct Keys {
    pub(super) focused: Option<NodeRef>,
    pub(super) granted: Vec<(NodeRef, String, String)>,
}

impl Driver {
    /// Handle the operations this module owns; `None` means the name belongs elsewhere.
    pub(super) async fn keyboard_op(&self, name: &str, args: &Value) -> Option<OpResult<Value>> {
        Some(match name {
            "keyboard" => self.keyboard(args).await,
            "mouse" => self.mouse(args).await,
            _ => return None,
        })
    }

    pub(super) async fn is_focused(&self, node: &NodeRef) -> OpResult<bool> {
        Ok(matches!(
            self.property(node, "focused").await?,
            Some(Value::Bool(true))
        ))
    }

    pub(super) async fn focus_on(&self, node: &NodeRef, description: &str) -> OpResult<()> {
        self.node_cmd(node, "DOM.focus", json!({})).await?;
        if self.is_focused(node).await? {
            Ok(())
        } else {
            Err(OpError::new(format!(
                "could not move keyboard focus to {description}; the page moved it elsewhere"
            )))
        }
    }

    /// The element that holds keyboard focus, in the top frame or any frame. Focus on an
    /// embedded object whose content is no inspectable document is refused rather than
    /// sent to a target the REPL cannot authorize.
    pub(super) async fn focused(&self, hint: Option<&NodeRef>) -> OpResult<NodeRef> {
        if let Some(hint) = hint {
            if self.is_focused(hint).await? && !self.embedder(hint).await? {
                return Ok(hint.clone());
            }
        }
        let top = self.frame().await?;
        let frames = self.frame_list().await?;
        for frame in &frames {
            let tree = if top["id"] == frame.id.as_str() {
                self.cmd("Accessibility.getFullAXTree", json!({})).await
            } else {
                self.host_cmd(
                    frame.host.as_deref(),
                    "Accessibility.getFullAXTree",
                    json!({"frameId": frame.id}),
                )
                .await
            };
            // A frame that went away since the listing has nothing focused.
            let Ok(tree) = tree else { continue };
            let nodes = crate::dom::ax_nodes(&tree).map_err(OpError::new)?;
            for ax in nodes.iter().rev() {
                if !matches!(ax.properties.get("focused"), Some(Value::Bool(true))) {
                    continue;
                }
                let node = NodeRef {
                    host: frame.host.clone(),
                    frame: (top["id"] != frame.id.as_str()).then(|| frame.id.clone()),
                    backend: ax.backend,
                };
                if !self.embedder(&node).await? {
                    return Ok(node);
                }
            }
        }
        Err(OpError::new(FOREIGN_FOCUS))
    }

    async fn embedder(&self, node: &NodeRef) -> OpResult<bool> {
        let described = self.node_cmd(node, "DOM.describeNode", json!({})).await?;
        let name = described["node"]["nodeName"].as_str().unwrap_or_default();
        Ok(EMBEDDERS
            .iter()
            .any(|embedder| name.eq_ignore_ascii_case(embedder)))
    }

    /// Keys reach whatever has focus when they are sent, so before every key event the focused
    /// node is looked up again and its current role and name are authorized (no per-node cache:
    /// the page may relabel a node between keys); Enter in a form field is also authorized as
    /// the submission of its form.
    pub(super) async fn key_target(&self, keys: &mut Keys, enter: bool) -> OpResult<()> {
        let node = self.focused(keys.focused.as_ref()).await?;
        self.node_gate(&node, "send keys to", &mut keys.granted)
            .await?;
        if enter {
            self.submission_gate(&node).await?;
        }
        keys.focused = Some(node);
        Ok(())
    }

    /// Implicit submission is authorized by what submitting does (the form's submit buttons and
    /// identifying attributes), not by the label of the field Enter was pressed in.
    pub(super) async fn submission_gate(&self, field: &NodeRef) -> OpResult<()> {
        let dom = self.document().await?;
        let Some(index) = dom.index_of(field) else {
            return Ok(());
        };
        if dom.nodes[index].name != "input" {
            return Ok(());
        }
        let Some(form) = dom.form_of(index) else {
            return Ok(());
        };
        let mut labels = Vec::new();
        for submitter in dom.submitters(form) {
            let node = self.gate_node(&dom.node_ref(submitter)).await?;
            labels.push(ax_text(&node["name"]));
        }
        for attribute in ["aria-label", "name", "id", "action"] {
            labels.extend(dom.nodes[form].attribute(attribute).map(str::to_owned));
        }
        labels.retain(|label| !label.is_empty());
        self.gate_node_page(
            field,
            Some(Target {
                action: "submit form",
                role: "form",
                name: &labels.join(" / "),
            }),
        )
        .await
        .map(drop)
    }

    /// Send one key transition, authorized against the node focused at that moment. Holding a
    /// key (no matching release yet) turns a repeated `down` into an auto-repeat.
    pub(super) async fn key(&self, name: &str, down: bool, keys: &mut Keys) -> OpResult<()> {
        let event = self.held.borrow().event(name, down).map_err(OpError::new)?;
        // Implicit submission is triggered by the Enter key-down, not modifiers or key-up.
        let submits = down && event["key"] == "Enter";
        self.key_target(keys, submits).await?;
        self.cmd("Input.dispatchKeyEvent", event).await?;
        self.held.borrow_mut().apply(name, down);
        Ok(())
    }

    /// Press a chord (`Control+Shift+T`, `a`, `Enter`): modifiers down, the key down and up,
    /// modifiers up in reverse. Keys already held by `keyboard.down` stay held.
    pub(super) async fn press(&self, spec: &str, delay: u64, keys: &mut Keys) -> OpResult<()> {
        let (modifiers, main) = keys::split_chord(spec).map_err(OpError::new)?;
        // Resolve every name before the first event so a typo sends nothing.
        for name in modifiers.iter().chain([&main]) {
            self.held.borrow().event(name, true).map_err(OpError::new)?;
        }
        for modifier in &modifiers {
            self.key(modifier, true, keys).await?;
        }
        self.key(main, true, keys).await?;
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
        self.key(main, false, keys).await?;
        for modifier in modifiers.iter().rev() {
            self.key(modifier, false, keys).await?;
        }
        Ok(())
    }

    pub(super) async fn insert(&self, text: &str, keys: &mut Keys) -> OpResult<()> {
        self.key_target(keys, false).await?;
        self.cmd("Input.insertText", json!({"text": text})).await?;
        Ok(())
    }

    /// Replace the composition string (the underlined preedit of an input method). Offsets are
    /// UTF-16 units into `text` and default to its end, like a caret after the composition.
    async fn compose(
        &self,
        text: &str,
        start: Option<u64>,
        end: Option<u64>,
        keys: &mut Keys,
    ) -> OpResult<()> {
        self.key_target(keys, false).await?;
        let length = text.encode_utf16().count() as u64;
        self.cmd(
            "Input.imeSetComposition",
            json!({
                "text": text,
                "selectionStart": start.unwrap_or(length),
                "selectionEnd": end.unwrap_or(length),
            }),
        )
        .await?;
        Ok(())
    }

    /// A run of Hangul as an input method composes it: each keystroke grows the preedit and a
    /// finished syllable is committed. Text the composer cannot reproduce is inserted whole.
    async fn type_hangul(&self, run: &str, delay: u64, keys: &mut Keys) -> OpResult<()> {
        let Some(updates) = hangul::compose(run) else {
            return self.insert(run, keys).await;
        };
        for update in updates {
            // Committing replaces the composition string in place.
            if !update.commit.is_empty() {
                self.insert(&update.commit, keys).await?;
            }
            if !update.preedit.is_empty() {
                self.compose(&update.preedit, None, None, keys).await?;
            }
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
        }
        Ok(())
    }

    /// Type `text`: characters on the layout are real key presses, Hangul is composed (unless
    /// `ime` is `Some(false)`; `Some(true)` composes even without a Hangul check), anything else
    /// is inserted.
    pub(super) async fn type_text(
        &self,
        text: &str,
        delay: u64,
        ime: Option<bool>,
        keys: &mut Keys,
    ) -> OpResult<()> {
        let compose = ime.unwrap_or(true);
        let characters: Vec<char> = text.chars().collect();
        let mut at = 0;
        while at < characters.len() {
            let character = characters[at];
            if compose && hangul::is_hangul(character) {
                let end = (at..characters.len())
                    .find(|&i| !hangul::is_hangul(characters[i]))
                    .unwrap_or(characters.len());
                let run: String = characters[at..end].iter().collect();
                self.type_hangul(&run, delay, keys).await?;
                at = end;
                continue;
            }
            at += 1;
            let spec = match character {
                '\n' | '\r' => "Enter".to_owned(),
                '\t' => "Tab".to_owned(),
                other => other.to_string(),
            };
            if keys::on_layout(&spec) {
                self.press(&spec, 0, keys).await?;
            } else {
                self.insert(&character.to_string(), keys).await?;
            }
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
        }
        Ok(())
    }

    /// `page.keyboard`: every key is authorized against the node focused when it is sent.
    pub(super) async fn keyboard(&self, args: &Value) -> OpResult<Value> {
        let before = self.frame().await?;
        let mut keys = Keys::default();
        let text = args["text"].as_str().unwrap_or_default();
        let delay = args["delay"].as_u64().unwrap_or_default();
        match args["action"].as_str() {
            Some("press") => self.press(text, delay, &mut keys).await?,
            Some("down") => self.key(text, true, &mut keys).await?,
            Some("up") => self.key(text, false, &mut keys).await?,
            Some("type") => {
                self.type_text(text, delay, args["ime"].as_bool(), &mut keys)
                    .await?
            }
            Some("insertText") => self.insert(text, &mut keys).await?,
            Some("imeSetComposition") => {
                let offset = |name: &str| args[name].as_u64();
                self.compose(
                    text,
                    offset("selectionStart"),
                    offset("selectionEnd"),
                    &mut keys,
                )
                .await?
            }
            other => {
                return Err(OpError {
                    name: "NotSupported",
                    message: format!("keyboard.{} is not supported", other.unwrap_or_default()),
                })
            }
        }
        self.settle(before).await?;
        Ok(Value::Null)
    }
}
