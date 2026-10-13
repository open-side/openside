use serde_json::{json, Value};

use super::{Browser, BrowserError, FormFieldBinding, NodeHandle};

impl Browser {
    /// Locate daemon-selected fields in a verified frame, including an OOPIF.
    pub async fn locate_form_fields(
        &self,
        owner: &str,
        tab: &str,
        frame_id: &str,
        script: &str,
    ) -> Result<(Value, Vec<NodeHandle>), BrowserError> {
        let _lock = self.inner.interaction_lock.lock().await;
        let page = self.owned_session(owner, tab).await?;
        let top = self.document_identity(&page).await?;
        let frames = self.frames_of(&page, tab).await?;
        let frame = frames
            .iter()
            .find(|f| f.id == frame_id)
            .ok_or(BrowserError::StaleReference)?;
        if !frame.origin.starts_with("https://") && !frame.origin.starts_with("http://") {
            return Err(BrowserError::OriginMismatch);
        }
        let mut ancestor = frame;
        while let Some(parent_id) = ancestor.parent.as_deref() {
            let parent = frames
                .iter()
                .find(|f| f.id == parent_id)
                .ok_or(BrowserError::StaleReference)?;
            let owner_node = self
                .host_command(
                    owner,
                    tab,
                    parent.host.as_deref(),
                    "DOM.getFrameOwner",
                    json!({"frameId":ancestor.id}),
                )
                .await?;
            let world = self
                .host_command(
                    owner,
                    tab,
                    parent.host.as_deref(),
                    "Page.createIsolatedWorld",
                    json!({"frameId":parent.id,"worldName":"openside-form"}),
                )
                .await?;
            let resolved = self.host_command(owner, tab, parent.host.as_deref(), "DOM.resolveNode",
                json!({"backendNodeId":owner_node["backendNodeId"],"executionContextId":world["executionContextId"]})).await?;
            let object = resolved["object"]["objectId"]
                .as_str()
                .ok_or(BrowserError::StaleReference)?;
            let visible = self.host_command(owner, tab, parent.host.as_deref(), "Runtime.callFunctionOn",
                json!({"objectId":object,"returnByValue":true,"functionDeclaration":"function(){const r=this.getBoundingClientRect(),s=getComputedStyle(this);return this.isConnected&&r.width>0&&r.height>0&&s.visibility==='visible'&&s.display!=='none';}"})).await;
            let _ = self
                .host_command(
                    owner,
                    tab,
                    parent.host.as_deref(),
                    "Runtime.releaseObject",
                    json!({"objectId":object}),
                )
                .await;
            if visible?["result"]["value"] != true {
                return Ok((json!([]), Vec::new()));
            }
            ancestor = parent;
        }
        let host = frame.host.as_deref();
        let world = self
            .host_command(
                owner,
                tab,
                host,
                "Page.createIsolatedWorld",
                json!({"frameId":frame.id,"worldName":"openside-form"}),
            )
            .await?;
        let context = world["executionContextId"]
            .as_i64()
            .ok_or(BrowserError::StaleReference)?;
        let result = async {
            let result = self.host_command(owner, tab, host, "Runtime.evaluate", json!({
                "expression":script,"contextId":context,"objectGroup":"openside-form",
                "returnByValue":false,"awaitPromise":true
            })).await?;
            if result.get("exceptionDetails").is_some() { return Err(BrowserError::InvalidAction("form discovery failed")); }
            let object = result["result"]["objectId"].as_str().ok_or(BrowserError::StaleReference)?;
            let data = self.host_command(owner, tab, host, "Runtime.callFunctionOn", json!({
                "objectId":object,"functionDeclaration":"function(){return this.data}","returnByValue":true
            })).await?;
            let count = data["result"]["value"].as_array().ok_or(BrowserError::InvalidResponse("form metadata"))?.len();
            if count > 64 { return Err(BrowserError::InvalidAction("too many form fields")); }
            let mut pins = Vec::with_capacity(count);
            for index in 0..count {
                let node = self.host_command(owner, tab, host, "Runtime.callFunctionOn", json!({
                    "objectId":object,"functionDeclaration":"function(i){return this.nodes[i]}",
                    "arguments":[{"value":index}],"returnByValue":false,"objectGroup":"openside-form"
                })).await?;
                let object = node["result"]["objectId"].as_str().ok_or(BrowserError::StaleReference)?;
                let described = self.host_command(owner, tab, host, "DOM.describeNode", json!({"objectId":object})).await?;
                pins.push(NodeHandle {
                    backend_node_id: described["node"]["backendNodeId"].as_u64().ok_or(BrowserError::StaleReference)?,
                    document_id: top.0.clone(), origin: top.1.clone(),
                    frame: Some(FormFieldBinding { frame_id:frame.id.clone(), loader_id:frame.loader.clone(), host:frame.host.clone(), origin:frame.origin.clone() }),
                });
            }
            self.validate_form_fields_inner(owner, tab, &pins).await?;
            Ok((data["result"]["value"].clone(), pins))
        }.await;
        let _ = self
            .host_command(
                owner,
                tab,
                host,
                "Runtime.releaseObjectGroup",
                json!({"objectGroup":"openside-form"}),
            )
            .await;
        result
    }

    /// Preflight every receiving document before recording a durable fill intent.
    pub async fn validate_form_fields(
        &self,
        owner: &str,
        tab: &str,
        nodes: &[NodeHandle],
    ) -> Result<(), BrowserError> {
        let _lock = self.inner.interaction_lock.lock().await;
        self.validate_form_fields_inner(owner, tab, nodes).await
    }

    async fn validate_form_fields_inner(
        &self,
        owner: &str,
        tab: &str,
        nodes: &[NodeHandle],
    ) -> Result<(), BrowserError> {
        let page = self.owned_session(owner, tab).await?;
        let top = self.document_identity(&page).await?;
        let frames = self.frames_of(&page, tab).await?;
        for node in nodes {
            if node.document_id != top.0 {
                return Err(BrowserError::StaleReference);
            }
            if node.origin != top.1 {
                return Err(BrowserError::OriginMismatch);
            }
            let pin = node.frame.as_ref().ok_or(BrowserError::StaleReference)?;
            let frame = frames
                .iter()
                .find(|f| f.id == pin.frame_id)
                .ok_or(BrowserError::StaleReference)?;
            if frame.loader.is_empty() || frame.loader != pin.loader_id || frame.host != pin.host {
                return Err(BrowserError::StaleReference);
            }
            if frame.origin != pin.origin {
                return Err(BrowserError::OriginMismatch);
            }
        }
        Ok(())
    }

    /// Values are never returned; no click, keypress, or submit operation is used.
    pub async fn fill_form_fields(
        &self,
        owner: &str,
        tab: &str,
        nodes: &[NodeHandle],
        values: &[Value],
    ) -> Result<(), BrowserError> {
        let _lock = self.inner.interaction_lock.lock().await;
        if nodes.len() != values.len() {
            return Err(BrowserError::InvalidAction("form value count"));
        }
        self.validate_form_fields_inner(owner, tab, nodes).await?;
        for (node, value) in nodes.iter().zip(values) {
            // Recheck every frame before each write: page handlers may navigate another frame.
            self.validate_form_fields_inner(owner, tab, nodes).await?;
            let pin = node.frame.as_ref().ok_or(BrowserError::StaleReference)?;
            let world = self
                .host_command(
                    owner,
                    tab,
                    pin.host.as_deref(),
                    "Page.createIsolatedWorld",
                    json!({"frameId":pin.frame_id,"worldName":"openside-form"}),
                )
                .await?;
            let context = world["executionContextId"]
                .as_i64()
                .ok_or(BrowserError::StaleReference)?;
            let resolved = self
                .host_command(
                    owner,
                    tab,
                    pin.host.as_deref(),
                    "DOM.resolveNode",
                    json!({"backendNodeId":node.backend_node_id,"executionContextId":context}),
                )
                .await?;
            let object = resolved["object"]["objectId"]
                .as_str()
                .ok_or(BrowserError::StaleReference)?;
            let session = self.host_session(owner, tab, pin.host.as_deref()).await?;
            let filled = self.inner.cdp.call_sensitive(Some(&session), "Runtime.callFunctionOn", json!({
                "objectId":object,"returnByValue":true,"arguments":[{"value":value}],
                "functionDeclaration":"function(value){if(!this.isConnected||this.disabled||this.readOnly)return false; const checkbox=this instanceof HTMLInputElement&&this.type==='checkbox'; if(checkbox?typeof value!=='boolean':typeof value!=='string')return false; const proto=this instanceof HTMLTextAreaElement?HTMLTextAreaElement.prototype:HTMLInputElement.prototype; const setter=Object.getOwnPropertyDescriptor(proto,checkbox?'checked':'value')?.set; if(!setter)return false; setter.call(this,value); this.dispatchEvent(new Event('input',{bubbles:true})); this.dispatchEvent(new Event('change',{bubbles:true})); return checkbox?this.checked===value:this.value===value;}"
            })).await;
            let _ = self
                .host_command(
                    owner,
                    tab,
                    pin.host.as_deref(),
                    "Runtime.releaseObject",
                    json!({"objectId":object}),
                )
                .await;
            // CDP/page diagnostics can include the argument. Never propagate them.
            if !matches!(&filled, Ok(result) if result.get("exceptionDetails").is_none() && result["result"]["value"] == true)
            {
                return Err(BrowserError::InvalidAction("form fill indeterminate"));
            }
        }
        Ok(())
    }
}
