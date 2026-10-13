//! Nodes the daemon finds and acts on itself, rather than through a snapshot reference: a
//! login form's fields and buttons, or the options of a choice the page asks for. Each node is
//! pinned as a [`NodeHandle`] when it is found, and every later write or click checks that the
//! tab still shows the same document at the same origin.

use std::sync::Arc;

use serde_json::{json, Value};

use super::{required_string, Browser, BrowserError, NodeHandle};

impl Browser {
    /// Run `script` in an isolated world of the owned tab. It must evaluate (or resolve) to
    /// `{data, nodes}`: `data` is returned by value, and each element of the `nodes` array is
    /// pinned in order. Fails with `StaleReference` when the document changed meanwhile.
    pub async fn locate(
        &self,
        owner: &str,
        tab_id: &str,
        script: &str,
    ) -> Result<(Value, Vec<NodeHandle>), BrowserError> {
        let _interaction = self.inner.interaction_lock.lock().await;
        let session = self.owned_session(owner, tab_id).await?;
        let before = self.document_identity(&session).await?;
        let context = self.isolated_world(&session).await?;
        let result = self
            .inner
            .cdp
            .call(
                Some(&session),
                "Runtime.evaluate",
                json!({"expression":script,"contextId":context,"returnByValue":false,
                    "awaitPromise":true}),
            )
            .await?;
        if result.get("exceptionDetails").is_some() {
            return Err(BrowserError::InvalidAction("isolated evaluation failed"));
        }
        let object = required_string(&result["result"], "objectId")?.to_owned();
        let mut objects = vec![object.clone()];
        let located = async {
            let data = self
                .inner
                .cdp
                .call(
                    Some(&session),
                    "Runtime.callFunctionOn",
                    json!({"objectId":object,"functionDeclaration":"function() { return this.data; }",
                        "returnByValue":true}),
                )
                .await?;
            let list = self
                .inner
                .cdp
                .call(
                    Some(&session),
                    "Runtime.callFunctionOn",
                    json!({"objectId":object,
                        "functionDeclaration":"function() { return Array.isArray(this.nodes) ? this.nodes : []; }",
                        "returnByValue":false}),
                )
                .await?;
            let list = required_string(&list["result"], "objectId")?.to_owned();
            objects.push(list.clone());
            let properties = self
                .inner
                .cdp
                .call(
                    Some(&session),
                    "Runtime.getProperties",
                    json!({"objectId":list,"ownProperties":true}),
                )
                .await?;
            let mut nodes = Vec::new();
            for property in properties["result"].as_array().into_iter().flatten() {
                let Some(index) = property["name"]
                    .as_str()
                    .and_then(|name| name.parse::<usize>().ok())
                else {
                    continue;
                };
                let element = required_string(&property["value"], "objectId")?.to_owned();
                objects.push(element.clone());
                let described = self
                    .inner
                    .cdp
                    .call(
                        Some(&session),
                        "DOM.describeNode",
                        json!({"objectId":element}),
                    )
                    .await?;
                let backend = described["node"]["backendNodeId"]
                    .as_u64()
                    .ok_or(BrowserError::InvalidResponse("located backend node"))?;
                nodes.push((index, backend));
            }
            nodes.sort_unstable();
            if nodes.iter().enumerate().any(|(at, (index, _))| at != *index) {
                return Err(BrowserError::InvalidResponse("located node list"));
            }
            Ok((data["result"]["value"].clone(), nodes))
        }
        .await;
        for object in objects {
            let _ = self
                .inner
                .cdp
                .call(
                    Some(&session),
                    "Runtime.releaseObject",
                    json!({"objectId":object}),
                )
                .await;
        }
        let (data, nodes) = located?;
        let after = self.document_identity(&session).await?;
        if before != after {
            return Err(BrowserError::StaleReference);
        }
        let (document_id, origin) = after;
        if !origin.contains("://") {
            return Err(BrowserError::InvalidResponse("top-frame origin is opaque"));
        }
        Ok((
            data,
            nodes
                .into_iter()
                .map(|(_, backend_node_id)| NodeHandle {
                    backend_node_id,
                    document_id: document_id.clone(),
                    origin: origin.clone(),
                    frame: None,
                })
                .collect(),
        ))
    }

    /// Call `function` (a JS function declaration) on a pinned node in an isolated world and
    /// return its value. `StaleReference` or `OriginMismatch` when the tab moved on.
    pub async fn inspect_node(
        &self,
        owner: &str,
        tab_id: &str,
        node: &NodeHandle,
        function: &str,
    ) -> Result<Value, BrowserError> {
        let _interaction = self.inner.interaction_lock.lock().await;
        let session = self.owned_session(owner, tab_id).await?;
        self.check_node_document(&session, node).await?;
        let context = self.isolated_world(&session).await?;
        self.call_on_node_in(
            &session,
            node.backend_node_id,
            Some(context),
            function,
            json!([]),
        )
        .await
    }

    /// Click a pinned node with trusted mouse input at its centre, under focus emulation:
    /// the tab is never brought to the front and the person's window keeps focus. Nothing is
    /// clicked when the node is gone or the tab shows another document.
    pub async fn click_node(
        &self,
        owner: &str,
        tab_id: &str,
        node: &NodeHandle,
    ) -> Result<(), BrowserError> {
        let _interaction = self.inner.interaction_lock.lock().await;
        let session = self.owned_session(owner, tab_id).await?;
        self.pinned_connected(&session, node).await?;
        let cleanup = Arc::clone(&self.inner).begin_focus(session.clone()).await?;
        let clicked = self.pointer_at(&session, node.backend_node_id, true).await;
        let reset = cleanup.reset().await;
        clicked?;
        reset
    }

    /// Bring a pinned checkbox to `checked`. Custom-styled boxes are often zero-sized or
    /// covered by their label, so the element's own `click()` toggles it, firing the page's
    /// input and change handlers; a box already in that state is left alone.
    pub async fn set_checked_node(
        &self,
        owner: &str,
        tab_id: &str,
        node: &NodeHandle,
        checked: bool,
    ) -> Result<(), BrowserError> {
        let _interaction = self.inner.interaction_lock.lock().await;
        let session = self.owned_session(owner, tab_id).await?;
        self.pinned_connected(&session, node).await?;
        let context = self.isolated_world(&session).await?;
        let state = self
            .call_on_node_in(
                &session,
                node.backend_node_id,
                Some(context),
                "function(want) {
                    if (this.type !== 'checkbox' && this.getAttribute('role') !== 'checkbox') return 'not_checkbox';
                    const now = () => this.type === 'checkbox' ? this.checked : this.getAttribute('aria-checked') === 'true';
                    if (now() !== want) this.click();
                    return now() === want ? 'ok' : 'unchanged';
                }",
                json!([{"value":checked}]),
            )
            .await?;
        match state.as_str() {
            Some("ok") => Ok(()),
            Some("not_checkbox") => Err(BrowserError::InvalidAction("target is not a checkbox")),
            _ => Err(BrowserError::InvalidAction("checkbox did not change")),
        }
    }

    /// Press Enter in a pinned field (a form without a usable submit control), under focus
    /// emulation like [`Self::click_node`].
    pub async fn press_enter_node(
        &self,
        owner: &str,
        tab_id: &str,
        node: &NodeHandle,
    ) -> Result<(), BrowserError> {
        let _interaction = self.inner.interaction_lock.lock().await;
        let session = self.owned_session(owner, tab_id).await?;
        self.pinned_connected(&session, node).await?;
        let cleanup = Arc::clone(&self.inner).begin_focus(session.clone()).await?;
        let pressed = async {
            self.focus_node(&session, node.backend_node_id).await?;
            self.check_node_document(&session, node).await?;
            self.press_key(&session, "Enter").await
        }
        .await;
        let reset = cleanup.reset().await;
        pressed?;
        reset
    }

    async fn pinned_connected(&self, session: &str, node: &NodeHandle) -> Result<(), BrowserError> {
        self.check_node_document(session, node).await?;
        let connected = self
            .call_on_node(
                session,
                node.backend_node_id,
                "function() { return this.isConnected; }",
                json!([]),
            )
            .await?;
        if connected != true {
            return Err(BrowserError::StaleReference);
        }
        Ok(())
    }

    /// Move the mouse to the centre of a node's content box (scrolled into view first) and,
    /// with `click`, press and release the left button there. The caller holds the
    /// interaction lock and focus emulation.
    pub(super) async fn pointer_at(
        &self,
        session: &str,
        backend: u64,
        click: bool,
    ) -> Result<(), BrowserError> {
        self.inner
            .cdp
            .call(
                Some(session),
                "DOM.scrollIntoViewIfNeeded",
                json!({"backendNodeId":backend}),
            )
            .await?;
        let box_model = self
            .inner
            .cdp
            .call(
                Some(session),
                "DOM.getBoxModel",
                json!({"backendNodeId":backend}),
            )
            .await?;
        let quad = box_model["model"]["content"]
            .as_array()
            .filter(|quad| quad.len() == 8)
            .ok_or(BrowserError::InvalidResponse("target box model"))?;
        let mut x = 0.0;
        let mut y = 0.0;
        for pair in quad.chunks_exact(2) {
            x += pair[0]
                .as_f64()
                .ok_or(BrowserError::InvalidResponse("box x"))?;
            y += pair[1]
                .as_f64()
                .ok_or(BrowserError::InvalidResponse("box y"))?;
        }
        x /= 4.0;
        y /= 4.0;
        self.inner
            .cdp
            .call(
                Some(session),
                "Input.dispatchMouseEvent",
                json!({"type":"mouseMoved","x":x,"y":y}),
            )
            .await?;
        if click {
            for kind in ["mousePressed", "mouseReleased"] {
                self.inner
                    .cdp
                    .call(
                        Some(session),
                        "Input.dispatchMouseEvent",
                        json!({"type":kind,"x":x,"y":y,"button":"left","clickCount":1}),
                    )
                    .await?;
            }
        }
        Ok(())
    }
}
