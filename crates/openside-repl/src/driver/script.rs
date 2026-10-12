//! Page script (ADR-022): `evaluate`, `evaluateHandle`, `$eval`/`$$eval`, `locator.evaluate*`,
//! handles, `addInitScript`, and raw `page.cdp`, run through `Runtime.*` and
//! [`Browser::page_cdp`](openside_browser::Browser::page_cdp).
//!
//! The prelude builds the page function (the script's source wrapped in a value codec) and
//! this module only resolves where it runs: the top frame, a frame, a handle, or the elements a
//! locator resolves to. Every operation is authorized as [`Gate::Script`] for the top origin
//! and, inside a frame, the frame's origin before anything reaches the page. Handles live in one
//! object group and init scripts last for one execution; [`Driver::cleanup_scripts`] undoes both.

use std::{cell::RefCell, collections::BTreeMap, sync::atomic::Ordering, time::Duration};

use openside_browser::FrameInfo;
use serde_json::{json, Value};

use super::{
    error::{browser_error, OpError, OpResult},
    locate::located,
    Driver, CHANGED,
};
use crate::{dom::Step, Frame, Gate};

/// Object group of every remote object this execution creates, released in one call at the end.
const GROUP: &str = "openside-repl";
/// Longest page exception text handed to the script.
const MAX_EXCEPTION: usize = 2000;

/// Page-script state that outlives a single operation.
#[derive(Default)]
pub(super) struct Scripts {
    init: RefCell<Vec<String>>,
    caches: RefCell<Vec<String>>,
    /// Sessions (None: the page session, else an out-of-process frame) holding objects of
    /// [`GROUP`].
    objects: RefCell<Vec<Option<String>>>,
    // Dropped operations retain their hosting session so cleanup can recover each renderer.
    running: RefCell<BTreeMap<Option<String>, u32>>,
}

/// What a page function runs against.
enum Receiver {
    /// The global object of the top frame or of the frame `args.frame` steps select; the
    /// function gets only the argument.
    Frame,
    /// A remote object from an earlier `evaluateHandle`.
    Handle(Place),
    /// Elements of a locator: `strict` (one element, auto-wait), `first`, or `all`.
    Locator(String),
}

/// Where a remote object lives: its frame (None: the top frame) and the session hosting it.
#[derive(Clone, PartialEq, Eq)]
struct Place {
    frame: Option<String>,
    host: Option<String>,
}

impl Place {
    fn of(value: &Value) -> Self {
        Self {
            frame: value["frame"].as_str().map(str::to_owned),
            host: value["host"].as_str().map(str::to_owned),
        }
    }
}

impl Driver {
    /// Handle the operations this module owns; `None` means the name belongs elsewhere.
    pub(super) async fn script_op(&self, name: &str, args: &Value) -> Option<OpResult<Value>> {
        Some(match name {
            "evaluate" => self.evaluate(args).await,
            "dispose" => self.dispose(args).await,
            "add_init_script" => self.add_init_script(args).await,
            "cdp" => self.raw_cdp(args).await,
            _ => return None,
        })
    }

    /// Undo what page script left on the tab: a page call still spinning after the execution
    /// ended, this execution's init scripts, and its remote objects.
    pub(crate) async fn cleanup_scripts(&self) {
        self.abort.store(true, Ordering::Release);
        let running = std::mem::take(&mut *self.scripts.running.borrow_mut());
        // Host count must not consume the bounded init-removal safety phase.
        futures_util::future::join_all(running.into_iter().map(|(host, count)| async move {
            if count > 0 && self.renderer_busy(host.as_deref()).await {
                self.cleanup_host(host.as_deref(), "Runtime.terminateExecution", json!({}))
                    .await;
            }
        }))
        .await;
        let init_clean = tokio::time::timeout(Duration::from_millis(750), async {
            loop {
                let init = std::mem::take(&mut *self.scripts.init.borrow_mut());
                for identifier in init {
                    if !self
                        .cleanup_host(
                            None,
                            "Page.removeScriptToEvaluateOnNewDocument",
                            json!({"identifier":identifier}),
                        )
                        .await
                    {
                        return false;
                    }
                }
                if !matches!(
                    self.browser
                        .fence_init_scripts(&self.owner, &self.tab, Duration::from_millis(500))
                        .await,
                    Ok(true)
                ) {
                    return false;
                }
                if self.scripts.init.borrow().is_empty() {
                    return true;
                }
            }
        })
        .await;
        if !matches!(init_clean, Ok(true)) {
            let _ = self
                .browser
                .quarantine_close(&self.owner, &self.tab, Duration::from_millis(750))
                .await;
        }
        let hosts = std::mem::take(&mut *self.scripts.objects.borrow_mut());
        for host in hosts {
            let release = json!({"objectGroup": GROUP});
            self.cleanup_host(host.as_deref(), "Runtime.releaseObjectGroup", release)
                .await;
        }
    }

    /// `Runtime.terminateExecution` also cancels the *next* script when none is running (an
    /// abandoned call may only be awaiting a promise), so terminate only a renderer that does
    /// not answer a trivial evaluation.
    async fn renderer_busy(&self, host: Option<&str>) -> bool {
        let probe = self.browser.host_command(
            &self.owner,
            &self.tab,
            host,
            "Runtime.evaluate",
            json!({"expression": "0"}),
        );
        tokio::time::timeout(Duration::from_millis(250), probe)
            .await
            .is_err()
    }
    async fn cleanup_host(&self, host: Option<&str>, method: &str, params: Value) -> bool {
        matches!(
            tokio::time::timeout(
                Duration::from_millis(250),
                self.browser
                    .host_command(&self.owner, &self.tab, host, method, params),
            )
            .await,
            Ok(Ok(_))
        )
    }

    fn holds_objects(&self, host: &Option<String>) {
        let mut objects = self.scripts.objects.borrow_mut();
        if !objects.contains(host) {
            objects.push(host.clone());
        }
    }

    /// The top frame and frame `frame` (the top frame again when None) as they are now.
    async fn script_frames(&self, frame: Option<&str>) -> OpResult<(FrameInfo, FrameInfo)> {
        let frames = self.frame_list().await?;
        let top = frames
            .first()
            .cloned()
            .ok_or_else(|| OpError::new("the tab has no document"))?;
        *self.url.borrow_mut() = top.url.clone();
        let target = match frame {
            None => top.clone(),
            Some(id) => frames
                .into_iter()
                .find(|candidate| candidate.id == id)
                .ok_or_else(|| OpError::new(format!("frame {id} was detached")))?,
        };
        Ok((top, target))
    }

    /// Authorize `api` in the top document or in frame `frame`; returns that frame as checked.
    async fn gate_script(&self, frame: Option<&str>, api: &str) -> OpResult<FrameInfo> {
        let (top, target) = self.script_frames(frame).await?;
        let frame = frame.map(|_| Frame {
            id: &target.id,
            origin: &target.origin,
        });
        self.gate(Gate::Script {
            origin: &top.origin,
            frame,
            api,
        })
        .await?;
        Ok(target)
    }

    /// The object a frame-level page function is called on, in the frame's main world: the
    /// global object of a session's own top frame, else the document of a frame inside it.
    async fn frame_object(&self, frame: &FrameInfo, top: bool) -> OpResult<String> {
        let host = frame.host.as_deref();
        self.holds_objects(&frame.host);
        let object = if top || host == Some(frame.id.as_str()) {
            let mut reply = self
                .page_call(
                    host,
                    "Runtime.evaluate",
                    json!({"expression": "globalThis", "objectGroup": GROUP}),
                )
                .await?;
            reply["result"].take()
        } else {
            let detached = || OpError::new(format!("frame {} was detached", frame.id));
            let owner = self
                .host_cmd(host, "DOM.getFrameOwner", json!({"frameId": frame.id}))
                .await
                .map_err(|_| detached())?;
            let described = self
                .host_cmd(
                    host,
                    "DOM.describeNode",
                    json!({"backendNodeId": owner["backendNodeId"], "depth": 0}),
                )
                .await?;
            let document = described["node"]["contentDocument"]["backendNodeId"]
                .as_u64()
                .ok_or_else(detached)?;
            let mut reply = self
                .host_cmd(
                    host,
                    "DOM.resolveNode",
                    json!({"backendNodeId": document, "objectGroup": GROUP}),
                )
                .await?;
            reply["object"].take()
        };
        object["objectId"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| OpError::new("the page has no script context"))
    }

    /// Run one page function. `declaration` is built by the prelude; `spec` and `arg` are its
    /// first two arguments, followed by the argument handles and, for `all`, the elements.
    async fn evaluate(&self, args: &Value) -> OpResult<Value> {
        let api = args["api"].as_str().unwrap_or("page.evaluate");
        let by_value = args["returnHandle"] != true;
        let gate_api = if by_value {
            "evaluate"
        } else {
            "evaluateHandle"
        };
        let declaration = args["declaration"]
            .as_str()
            .ok_or_else(|| OpError::new("invalid page function"))?;
        let receiver = match args["target"]["kind"].as_str() {
            Some("handle") => Receiver::Handle(Place::of(&args["target"])),
            Some("locator") => Receiver::Locator(
                args["target"]["mode"]
                    .as_str()
                    .unwrap_or("strict")
                    .to_owned(),
            ),
            _ => Receiver::Frame,
        };
        let handles: Vec<(String, Place)> = args["handles"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|handle| {
                handle["object"]
                    .as_str()
                    .map(|object| (object.to_owned(), Place::of(handle)))
                    .ok_or_else(|| OpError::new("invalid handle"))
            })
            .collect::<OpResult<_>>()?;
        let mut elements = Vec::new();
        let (object, place) = match receiver {
            Receiver::Frame => {
                let steps: Vec<Step> = match &args["frame"] {
                    Value::Null => Vec::new(),
                    steps => serde_json::from_value(steps.clone())
                        .map_err(|_| OpError::new("invalid frame"))?,
                };
                let frame = if steps.is_empty() {
                    None
                } else {
                    Some(self.frame_target(&steps).await?.id)
                };
                let target = self.gate_script(frame.as_deref(), gate_api).await?;
                let object = self.frame_object(&target, frame.is_none()).await?;
                // The global must belong to the document that was authorized.
                let (_, now) = self.script_frames(frame.as_deref()).await?;
                if now.loader != target.loader {
                    return Err(OpError::new(CHANGED));
                }
                let host = target.host;
                (object, Place { frame, host })
            }
            Receiver::Handle(place) => {
                self.gate_script(place.frame.as_deref(), gate_api).await?;
                let object = args["target"]["object"]
                    .as_str()
                    .ok_or_else(|| OpError::new("invalid handle"))?
                    .to_owned();
                (object, place)
            }
            Receiver::Locator(mode) => {
                let (objects, place) = self
                    .locator_elements(&args["target"], &mode, api, gate_api)
                    .await?;
                if mode == "all" {
                    elements = objects;
                    let (_, frame) = self.script_frames(place.frame.as_deref()).await?;
                    let object = self.frame_object(&frame, place.frame.is_none()).await?;
                    (object, place)
                } else {
                    (objects.into_iter().next().unwrap_or_default(), place)
                }
            }
        };
        let mut arguments = vec![
            json!({"value": args["spec"]}),
            json!({"value": args["arg"]}),
        ];
        for (handle, from) in handles {
            // A remote object only means something in the context that made it.
            if from != place {
                return Err(OpError::new(format!(
                    "{api}: a JSHandle from another frame cannot be passed here"
                )));
            }
            arguments.push(json!({"objectId": handle}));
        }
        for element in elements {
            arguments.push(json!({"objectId": element}));
        }
        self.holds_objects(&place.host);
        let reply = self
            .page_call(
                place.host.as_deref(),
                "Runtime.callFunctionOn",
                json!({
                    "functionDeclaration": declaration,
                    "objectId": object,
                    "arguments": arguments,
                    "returnByValue": by_value,
                    "awaitPromise": true,
                    "userGesture": true,
                    "objectGroup": GROUP,
                }),
            )
            .await?;
        page_result(api, reply, by_value, &place)
    }

    /// Remote objects of the elements a locator target resolves to, after authorizing `gate_api`
    /// in the one frame that holds them.
    async fn locator_elements(
        &self,
        target: &Value,
        mode: &str,
        api: &str,
        gate_api: &str,
    ) -> OpResult<(Vec<String>, Place)> {
        let located = located(target)?;
        let resolved = if mode == "strict" {
            let timeout = located
                .timeout
                .map(Duration::from_millis)
                .unwrap_or(self.limits.action_timeout);
            self.single(&located, timeout, false, false).await?
        } else {
            self.resolve(&located.steps).await?
        };
        let found = match mode {
            "first" => match resolved.found.first() {
                Some(&index) => vec![index],
                None => {
                    return Err(OpError::new(format!(
                        "{api}: failed to find element matching {}",
                        located.description
                    )))
                }
            },
            _ => resolved.found.clone(),
        };
        let nodes: Vec<_> = found
            .iter()
            .map(|&index| resolved.dom.node_ref(index))
            .collect();
        let place = match nodes.first() {
            Some(node) => Place {
                frame: node.frame.clone(),
                host: node.host.clone(),
            },
            None => Place {
                frame: None,
                host: None,
            },
        };
        if nodes.iter().any(|node| node.frame != place.frame) {
            return Err(OpError::new(format!(
                "{api}: the matched elements are in different frames"
            )));
        }
        let checked = self.gate_script(place.frame.as_deref(), gate_api).await?;
        // The elements must come from the documents that were authorized.
        let (top, _) = self.script_frames(None).await?;
        let resolved_loader = match &place.frame {
            None => Some(&resolved.loader),
            Some(id) => resolved
                .frames
                .iter()
                .find(|(frame, _)| frame == id)
                .map(|(_, loader)| loader),
        };
        if top.loader != resolved.loader || resolved_loader != Some(&checked.loader) {
            return Err(OpError::new(CHANGED));
        }
        self.holds_objects(&place.host);
        let mut objects = Vec::with_capacity(nodes.len());
        for node in &nodes {
            let reply = self
                .node_cmd(node, "DOM.resolveNode", json!({"objectGroup": GROUP}))
                .await
                .map_err(|_| OpError::new(CHANGED))?;
            objects.push(
                reply["object"]["objectId"]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| OpError::new(CHANGED))?,
            );
        }
        Ok((objects, place))
    }

    /// A page call that may run page code for an unbounded time; see [`Scripts::running`].
    async fn page_call(&self, host: Option<&str>, method: &str, params: Value) -> OpResult<Value> {
        let host_key = host.map(str::to_owned);
        *self
            .scripts
            .running
            .borrow_mut()
            .entry(host_key.clone())
            .or_default() += 1;
        let reply = self.host_cmd(host, method, params).await;
        // A protocol timeout is not proof that the page stopped executing.
        if reply.is_ok() {
            if let Some(count) = self.scripts.running.borrow_mut().get_mut(&host_key) {
                *count -= 1;
            }
        }
        reply
    }

    async fn dispose(&self, args: &Value) -> OpResult<Value> {
        let object = args["object"]
            .as_str()
            .ok_or_else(|| OpError::new("invalid handle"))?;
        // Releasing an object another navigation already discarded is not an error.
        let _ = self
            .host_cmd(
                args["host"].as_str(),
                "Runtime.releaseObject",
                json!({"objectId": object}),
            )
            .await;
        Ok(Value::Null)
    }

    async fn add_init_script(&self, args: &Value) -> OpResult<Value> {
        let source = args["source"]
            .as_str()
            .ok_or_else(|| OpError::new("invalid init script"))?;
        let source = self.guarded_init_source(source, "addInitScript").await?;
        let reply = self
            .cmd(
                "Page.addScriptToEvaluateOnNewDocument",
                json!({"source": source}),
            )
            .await?;
        if let Some(identifier) = reply["identifier"].as_str() {
            self.scripts.init.borrow_mut().push(identifier.to_owned());
        }
        Ok(Value::Null)
    }
    async fn guarded_init_source(&self, source: &str, api: &str) -> OpResult<String> {
        let top = self.gate_script(None, api).await?;
        let mut origins = vec![top.origin.clone()];
        for frame in self.frame_list().await? {
            if matches!(
                self.guard.check(&Gate::Script {
                    origin: &top.origin,
                    frame: frame.parent.as_ref().map(|_| Frame {
                        id: &frame.id,
                        origin: &frame.origin
                    }),
                    api,
                }),
                crate::Verdict::Allow
            ) && !origins.contains(&frame.origin)
            {
                origins.push(frame.origin);
            }
        }
        // Opaque documents all report "null"; that is not an origin-specific authorization.
        origins.retain(|origin| origin != "null" && !origin.is_empty());
        let origins =
            serde_json::to_string(&origins).map_err(|_| OpError::new("invalid script origins"))?;
        let source =
            serde_json::to_string(source).map_err(|_| OpError::new("invalid init script"))?;
        Ok(format!(
            "(() => {{ if ({origins}.includes(location.origin)) (0, eval)({source}); }})();"
        ))
    }

    /// Raw methods retain the same file, frame and receiver boundaries as wrapper operations.
    async fn raw_cdp(&self, args: &Value) -> OpResult<Value> {
        let method = args["method"]
            .as_str()
            .filter(|method| !method.is_empty())
            .ok_or_else(|| OpError::new("page.cdp: method must be a non-empty string"))?;
        let mut params = match &args["params"] {
            Value::Null => json!({}),
            params @ Value::Object(_) => params.clone(),
            _ => return Err(OpError::new("page.cdp: params must be an object")),
        };
        let api = format!("cdp:{method}");
        self.gate_script(None, &api).await?;
        if method == "DOM.setFileInputFiles" {
            return self.raw_upload(&params, &api).await;
        }
        openside_browser::Browser::validate_page_cdp(method).map_err(browser_error)?;
        self.admit_raw_params(&params, &api).await?;
        if let Some(cache) = params["cacheId"].as_str() {
            if !self
                .scripts
                .caches
                .borrow()
                .iter()
                .any(|known| known == cache)
            {
                return Err(OpError::new("cache id was not admitted for this tab"));
            }
        }
        if method == "Network.getCookies" {
            let frames = self.frame_list().await?;
            if let Some(urls) = params["urls"].as_array() {
                for url in urls {
                    let url = url
                        .as_str()
                        .ok_or_else(|| OpError::new("invalid cookie URL"))?;
                    let origin = super::events::origin_of(url);
                    if origin == "null" {
                        return Err(OpError::new("cookie URL must have an HTTP(S) origin"));
                    }
                    let frame = frames
                        .iter()
                        .find(|frame| frame.origin == origin)
                        .ok_or_else(|| OpError::new("cookie URL does not belong to this tab"))?;
                    self.gate_script(Some(&frame.id), &api).await?;
                }
            } else {
                for frame in &frames {
                    self.gate_script(Some(&frame.id), &api).await?;
                }
            }
        }
        if method == "Page.navigate" {
            let url = params["url"]
                .as_str()
                .ok_or_else(|| OpError::new("Page.navigate requires a URL"))?;
            self.gate(Gate::Navigate { url }).await?;
        }
        if method == "Page.addScriptToEvaluateOnNewDocument" {
            if params["runImmediately"] == true {
                return Err(OpError::new(
                    "init scripts are confined to future documents",
                ));
            }
            let source = params["source"]
                .as_str()
                .ok_or_else(|| OpError::new("invalid init script"))?;
            params["source"] = json!(self.guarded_init_source(source, &api).await?);
        } else if method.starts_with("Input.") {
            self.admit_raw_input(&params, &api).await?;
            if method == "Input.dispatchDragEvent" {
                if let Some(files) = params["data"]["files"].as_array() {
                    let x = params["x"]
                        .as_f64()
                        .ok_or_else(|| OpError::new("invalid drag point"))?;
                    let y = params["y"]
                        .as_f64()
                        .ok_or_else(|| OpError::new("invalid drag point"))?;
                    let node = self
                        .hit_at(x, y)
                        .await?
                        .ok_or_else(|| OpError::new("drag has no receiver"))?
                        .node;
                    let checked = self.gate_script(node.frame.as_deref(), &api).await?;
                    let frame = checked
                        .parent
                        .as_ref()
                        .map(|_| (checked.id.clone(), checked.origin.clone()));
                    let mut paths = Vec::with_capacity(files.len());
                    for file in files {
                        paths.push(
                            self.upload_path(
                                file.as_str()
                                    .ok_or_else(|| OpError::new("invalid drag file"))?,
                                frame.as_ref(),
                            )
                            .await?,
                        );
                    }
                    params["data"]["files"] = json!(paths);
                }
            }
        } else if !matches!(
            method,
            "Runtime.evaluate"
                | "Runtime.callFunctionOn"
                | "Runtime.compileScript"
                | "Runtime.runScript"
                | "Page.navigate"
                | "Page.close"
                | "Page.crash"
        ) {
            // These methods can inspect renderer-wide state or address opaque node/object ids.
            self.gate_raw_renderer(&api).await?;
        }
        if params.get("objectId").is_some()
            || params["arguments"].as_array().is_some_and(|arguments| {
                arguments
                    .iter()
                    .any(|argument| argument.get("objectId").is_some())
            })
            || params.get("callFrameId").is_some()
            || params.get("scriptId").is_some()
        {
            self.gate_raw_renderer(&api).await?;
        }
        if matches!(
            method,
            "Runtime.evaluate" | "Runtime.callFunctionOn" | "Runtime.runScript" | "DOM.resolveNode"
        ) {
            params["objectGroup"] = json!(GROUP);
        }
        self.holds_objects(&None);
        *self.scripts.running.borrow_mut().entry(None).or_default() += 1;
        let reply = self
            .browser
            .page_cdp(&self.owner, &self.tab, method, params)
            .await;
        if reply.is_ok() {
            if let Some(count) = self.scripts.running.borrow_mut().get_mut(&None) {
                *count -= 1;
            }
        }
        let reply = reply.map_err(|error| {
            OpError::new(format!(
                "page.cdp({method}): {}",
                browser_error(error).message
            ))
        })?;
        if method == "Page.addScriptToEvaluateOnNewDocument" {
            if let Some(identifier) = reply["identifier"].as_str() {
                self.scripts.init.borrow_mut().push(identifier.to_owned());
            }
        }
        if method == "CacheStorage.requestCacheNames" {
            for cache in reply["caches"].as_array().into_iter().flatten() {
                if let Some(id) = cache["cacheId"].as_str() {
                    self.scripts.caches.borrow_mut().push(id.to_owned());
                }
            }
        }
        Ok(reply)
    }
    async fn gate_raw_renderer(&self, api: &str) -> OpResult<()> {
        for frame in self.frame_list().await? {
            if frame.parent.is_some() && frame.host.is_none() {
                self.gate_script(Some(&frame.id), api).await?;
            }
        }
        Ok(())
    }

    async fn admit_raw_params(&self, params: &Value, api: &str) -> OpResult<()> {
        let mut pending = vec![params];
        while let Some(value) = pending.pop() {
            match value {
                Value::Object(fields) => {
                    for (key, value) in fields {
                        match key.as_str() {
                            "targetId" | "browserContextId" | "sessionId" => {
                                return Err(OpError::new(
                                    "cdp target/session authority is reserved by the host",
                                ))
                            }
                            "arguments" if api == "cdp:Runtime.callFunctionOn" => continue,
                            "url" => {
                                let url = value
                                    .as_str()
                                    .ok_or_else(|| OpError::new("invalid CDP URL"))?;
                                if url
                                    .trim_start()
                                    .get(..5)
                                    .is_some_and(|scheme| scheme.eq_ignore_ascii_case("file:"))
                                {
                                    return Err(OpError::new(
                                        "local file URLs are reserved by the host",
                                    ));
                                }
                                if api != "cdp:Page.navigate" {
                                    let origin = super::events::origin_of(url);
                                    if origin == "null" {
                                        return Err(OpError::new(
                                            "CDP URL must have an HTTP(S) origin",
                                        ));
                                    }
                                    let frames = self.frame_list().await?;
                                    let frame = frames
                                        .iter()
                                        .find(|frame| frame.origin == origin)
                                        .ok_or_else(|| {
                                            OpError::new("cdp URL does not belong to this tab")
                                        })?;
                                    self.gate_script(Some(&frame.id), api).await?;
                                }
                            }
                            "frameId" => {
                                let id = value
                                    .as_str()
                                    .ok_or_else(|| OpError::new("invalid frame id"))?;
                                self.gate_script(Some(id), api).await?;
                            }
                            "executionContextId" | "contextId" | "uniqueContextId" => {
                                let (id, unique) = if key == "uniqueContextId" {
                                    (
                                        None,
                                        Some(value.as_str().ok_or_else(|| {
                                            OpError::new("invalid execution context")
                                        })?),
                                    )
                                } else {
                                    (
                                        Some(value.as_i64().ok_or_else(|| {
                                            OpError::new("invalid execution context")
                                        })?),
                                        None,
                                    )
                                };
                                let frame = self
                                    .browser
                                    .page_context_frame(&self.owner, &self.tab, id, unique)
                                    .await
                                    .map_err(browser_error)?;
                                self.gate_script(Some(&frame), api).await?;
                            }
                            "storageKey" => {
                                let key = value
                                    .as_str()
                                    .ok_or_else(|| OpError::new("invalid storage key"))?;
                                let frame = self
                                    .browser
                                    .page_storage_key_frame(&self.owner, &self.tab, key)
                                    .await
                                    .map_err(browser_error)?;
                                self.gate_script(Some(&frame), api).await?;
                            }
                            "securityOrigin" | "origin" => {
                                let value = value
                                    .as_str()
                                    .ok_or_else(|| OpError::new("invalid storage origin"))?;
                                let origin = value.split('^').next().unwrap_or_default();
                                let frames = self.frame_list().await?;
                                let frame = frames
                                    .iter()
                                    .find(|frame| frame.origin == origin)
                                    .ok_or_else(|| {
                                        OpError::new(
                                            "cdp storage origin does not belong to this tab",
                                        )
                                    })?;
                                self.gate_script(Some(&frame.id), api).await?;
                            }
                            _ => {}
                        }
                        pending.push(value);
                    }
                }
                Value::Array(values) => pending.extend(values),
                _ => {}
            }
        }
        Ok(())
    }

    async fn admit_raw_input(&self, params: &Value, api: &str) -> OpResult<()> {
        if let (Some(x), Some(y)) = (params["x"].as_f64(), params["y"].as_f64()) {
            self.require_inside(x, y).await?;
            return self.gate_pointer_origin(x, y).await;
        }
        if let Some(points) = params["touchPoints"].as_array() {
            if points.is_empty() {
                // A release has no coordinates but still reaches the active touch receivers.
                for frame in self.frame_list().await? {
                    self.gate_script(Some(&frame.id), api).await?;
                }
                return Ok(());
            }
            for point in points {
                let x = point["x"]
                    .as_f64()
                    .ok_or_else(|| OpError::new("invalid touch point"))?;
                let y = point["y"]
                    .as_f64()
                    .ok_or_else(|| OpError::new("invalid touch point"))?;
                self.require_inside(x, y).await?;
                self.gate_pointer_origin(x, y).await?;
            }
            return Ok(());
        }
        let node = self.focused(None).await?;
        self.gate_node_page(&node, None).await?;
        Ok(())
    }

    async fn raw_upload(&self, params: &Value, api: &str) -> OpResult<Value> {
        self.admit_raw_params(params, api).await?;
        let mut identity = json!({});
        for key in ["backendNodeId", "nodeId", "objectId"] {
            if let Some(value) = params.get(key) {
                identity[key] = value.clone();
            }
        }
        let (frame, backend) = self
            .browser
            .page_node_frame(&self.owner, &self.tab, identity)
            .await
            .map_err(browser_error)?;
        let checked = self.gate_script(Some(&frame), api).await?;
        let frame_gate = checked
            .parent
            .as_ref()
            .map(|_| (checked.id.clone(), checked.origin.clone()));
        let files = params["files"]
            .as_array()
            .ok_or_else(|| OpError::new("invalid upload files"))?;
        let mut paths = Vec::with_capacity(files.len());
        for file in files {
            paths.push(
                self.upload_path(
                    file.as_str()
                        .ok_or_else(|| OpError::new("invalid file path"))?,
                    frame_gate.as_ref(),
                )
                .await?,
            );
        }
        self.host_cmd(
            checked.host.as_deref(),
            "DOM.setFileInputFiles",
            json!({"backendNodeId": backend, "files": paths}),
        )
        .await
    }
}

/// The reply of `Runtime.callFunctionOn`: the codec tree of the value, or a handle descriptor.
fn page_result(api: &str, mut reply: Value, by_value: bool, place: &Place) -> OpResult<Value> {
    if let Some(details) = reply.get("exceptionDetails") {
        let text = details["exception"]["description"]
            .as_str()
            .or_else(|| details["text"].as_str())
            .unwrap_or("page script failed");
        let mut end = text.len().min(MAX_EXCEPTION);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        return Err(OpError::new(format!("{api}: {}", &text[..end])));
    }
    let mut result = reply["result"].take();
    if by_value {
        return Ok(result["value"].take());
    }
    let mut handle = json!({
        "type": result["type"],
        "subtype": result["subtype"],
        "description": result["description"],
        "frame": place.frame,
        "host": place.host,
    });
    match result["objectId"].take() {
        Value::String(object) => handle["object"] = json!(object),
        _ => handle["value"] = primitive(&mut result),
    }
    Ok(handle)
}

/// The codec tree (see the prelude's `codec`) of a primitive `RemoteObject`.
fn primitive(result: &mut Value) -> Value {
    if let Some(special) = result["unserializableValue"].as_str() {
        return match special.strip_suffix('n') {
            Some(digits) => json!({"$b": digits}),
            None => json!({"$v": special}),
        };
    }
    if result["type"] == "undefined" {
        return json!({"$v": "undefined"});
    }
    result["value"].take()
}
