//! Locator resolution and page reads: selector evaluation over the CDP DOM and accessibility
//! trees of every frame of the tab, ref lookup, strict single-match resolution with candidate
//! diagnostics, frame-qualified node addressing, and the read-only page operations.

use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap},
    future::Future,
    pin::Pin,
    time::{Duration, Instant},
};

use openside_browser::{BrowserError, FrameInfo, RefTarget};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    error::{browser_error, OpError, OpResult},
    wait::metric,
    Driver, CHANGED, STOPPED,
};
pub(super) use crate::dom::NodeRef;
use crate::{
    dom::{
        ax_node_of, ax_nodes, ax_text, decidable, normalize, role_matches, state_matches, AxNode,
        Dom, Step, MAX_NODES, TOO_LARGE,
    },
    Frame, Gate, Target, Verdict,
};

type Local<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Accessibility nodes per frame index of a [`Dom`], fetched when a role step first needs them.
type AxCache = RefCell<HashMap<usize, Vec<AxNode>>>;

pub(super) struct Resolved {
    pub(super) dom: Dom,
    pub(super) found: Vec<usize>,
    /// Loader id of the authorized top-frame document.
    pub(super) loader: String,
    /// Child frames holding found nodes, authorized with their loader ids then.
    pub(super) frames: Vec<(String, String)>,
}

#[derive(Deserialize)]
pub(super) struct Located {
    pub(super) steps: Vec<Step>,
    #[serde(default)]
    pub(super) description: String,
    pub(super) timeout: Option<u64>,
}

/// What a top-viewport point hits, after descending into out-of-process iframes.
pub(super) struct Hit {
    pub(super) node: NodeRef,
}

/// One frame chosen by frame steps (`page.frame`, `frameLocator`, `contentFrame`), authorized.
pub(super) struct FrameTarget {
    pub(super) id: String,
}

/// In-page XPath evaluation; it runs in an isolated world so page script cannot redefine
/// `document.evaluate`, and only element results are kept.
pub(super) const XPATH: &str = "function (selector, limit) {
  const doc = this.nodeType === 9 ? this : this.ownerDocument;
  const result = doc.evaluate(selector, this, null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null);
  const out = [];
  for (let i = 0; i < result.snapshotLength && out.length < limit; i++) {
    const node = result.snapshotItem(i);
    if (node.nodeType === 1) out.push(node);
  }
  return out;
}";
const XPATH_LIMIT: usize = 10_000;
const OBJECT_GROUP: &str = "openside-locate";

/// Candidates listed in a strict-mode error before the rest are only counted.
const LISTED_CANDIDATES: usize = 10;

impl Driver {
    /// Handle the operations this module owns; `None` means the name belongs elsewhere.
    pub(super) async fn locate_op(&self, name: &str, args: &Value) -> Option<OpResult<Value>> {
        Some(match name {
            "count" => self.count(args).await,
            "texts" => self.texts(args).await,
            "title" => self.title(args).await,
            "content" => self.content(args).await,
            "snapshot" => self.snapshot().await,
            "query" => self.query(args).await,
            "frames" => self.list_frames().await,
            _ => return None,
        })
    }

    pub(super) async fn frame_list(&self) -> OpResult<Vec<FrameInfo>> {
        if self.stopped() {
            return Err(OpError::new(STOPPED));
        }
        self.browser
            .frames(&self.owner, &self.tab)
            .await
            .map_err(browser_error)
    }

    /// Every document of the tab: the top document with its same-process frames and open shadow
    /// roots, plus one document per out-of-process iframe.
    pub(super) async fn document(&self) -> OpResult<Dom> {
        self.ensure_page_size().await?;
        let frames = self.frame_list().await?;
        let fetch = json!({"depth": -1, "pierce": true});
        let mut documents = vec![(None, self.cmd("DOM.getDocument", fetch.clone()).await?)];
        for frame in &frames {
            if frame.host.as_deref() != Some(frame.id.as_str()) {
                continue;
            }
            // A frame that detached since the listing has no document to add.
            if let Ok(document) = self
                .host_cmd(Some(&frame.id), "DOM.getDocument", fetch.clone())
                .await
            {
                documents.push((frame.host.clone(), document));
            }
        }
        Dom::parse(&documents, &frames, &|| self.stopped()).map_err(OpError::new)
    }

    /// `method` on the session that hosts `host`'s nodes (None: the tab's page session).
    pub(super) async fn host_cmd(
        &self,
        host: Option<&str>,
        method: &str,
        params: Value,
    ) -> OpResult<Value> {
        if self.stopped() {
            return Err(OpError::new(STOPPED));
        }
        self.browser
            .host_command(&self.owner, &self.tab, host, method, params)
            .await
            .map_err(browser_error)
    }

    /// `method` on the session hosting `node`, with `backendNodeId` inserted into `params`.
    pub(super) async fn node_cmd(
        &self,
        node: &NodeRef,
        method: &str,
        mut params: Value,
    ) -> OpResult<Value> {
        params["backendNodeId"] = json!(node.backend);
        self.host_cmd(node.host.as_deref(), method, params).await
    }

    /// Top-viewport position of the origin of `host`'s coordinate space; add it to box and
    /// quad coordinates of that host's nodes. (0, 0) for the page session, whose same-process
    /// frames already report top-viewport coordinates.
    pub(super) async fn host_offset(&self, host: Option<&str>) -> OpResult<(f64, f64)> {
        if host.is_none() {
            return Ok((0.0, 0.0));
        }
        if self.stopped() {
            return Err(OpError::new(STOPPED));
        }
        self.browser
            .frame_offset(&self.owner, &self.tab, host)
            .await
            .map_err(browser_error)
    }

    /// The node a top-viewport point hits, descending into out-of-process iframes the way
    /// pointer input is routed; None when nothing is there.
    pub(super) async fn hit_at(&self, x: f64, y: f64) -> OpResult<Option<Hit>> {
        if self.stopped() {
            return Err(OpError::new(STOPPED));
        }
        let Some(hit) = self
            .browser
            .frame_hit(&self.owner, &self.tab, x, y)
            .await
            .map_err(browser_error)?
        else {
            return Ok(None);
        };
        let top = self.frame().await?;
        Ok(Some(Hit {
            node: NodeRef {
                host: hit.host,
                frame: (top["id"] != hit.frame.as_str()).then_some(hit.frame),
                backend: hit.backend,
            },
        }))
    }

    /// Authorize the document holding `node` and, if given, the interaction target in it:
    /// the top-frame gate for top-frame nodes, else the top origin plus the node's frame
    /// origin. Returns the authorized document's loader id.
    pub(super) async fn gate_node_page(
        &self,
        node: &NodeRef,
        target: Option<Target<'_>>,
    ) -> OpResult<String> {
        let Some(frame) = node.frame.as_deref() else {
            return self.gate_page(target).await;
        };
        let top = self.frame().await?;
        let frames = self.frame_list().await?;
        let info = frames
            .iter()
            .find(|info| info.id == frame)
            .ok_or_else(|| OpError::new(CHANGED))?;
        self.gate(Gate::Page {
            origin: top["securityOrigin"].as_str().unwrap_or("null"),
            frame: Some(Frame {
                id: &info.id,
                origin: &info.origin,
            }),
            target,
        })
        .await?;
        Ok(info.loader.clone())
    }

    /// Authorize each child frame that holds a found node; returns (frame id, loader id).
    async fn gate_frames(&self, dom: &Dom, found: &[usize]) -> OpResult<Vec<(String, String)>> {
        let frames: BTreeSet<usize> = found
            .iter()
            .map(|&index| dom.nodes[index].frame)
            .filter(|&frame| frame != 0)
            .collect();
        let mut gated = Vec::new();
        for frame in frames {
            let info = &dom.frames[frame];
            self.gate(Gate::Page {
                origin: &dom.frames[0].origin,
                frame: Some(Frame {
                    id: &info.id,
                    origin: &info.origin,
                }),
                target: None,
            })
            .await?;
            gated.push((info.id.clone(), info.loader.clone()));
        }
        Ok(gated)
    }

    /// Like `same_document`, for the top document and every frame a result came from.
    pub(super) async fn same_documents(&self, resolved: &Resolved) -> OpResult<()> {
        self.same_document(&resolved.loader).await?;
        if resolved.frames.is_empty() {
            return Ok(());
        }
        let frames = self.frame_list().await?;
        for (id, loader) in &resolved.frames {
            if !frames
                .iter()
                .any(|frame| &frame.id == id && &frame.loader == loader)
            {
                return Err(OpError::new(CHANGED));
            }
        }
        Ok(())
    }

    /// Authorize the current document, then evaluate `steps` in it; frames that hold results
    /// are authorized too.
    pub(super) async fn resolve(&self, steps: &[Step]) -> OpResult<Resolved> {
        let loader = self.gate_page(None).await?;
        // DOM.getDocument renumbers node ids, so snapshot refs resolve before the full fetch.
        let mut references = HashMap::new();
        collect_refs(steps, &mut references);
        for (reference, target) in references.iter_mut() {
            *target = Some(
                self.browser
                    .reference_node(&self.owner, &self.tab, reference)
                    .await
                    .map_err(browser_error)?,
            );
        }
        let dom = self.document().await?;
        let ax = AxCache::default();
        let found = self.eval(&dom, &ax, &references, vec![0], steps).await?;
        let frames = self.gate_frames(&dom, &found).await?;
        Ok(Resolved {
            dom,
            found,
            loader,
            frames,
        })
    }

    fn eval<'a>(
        &'a self,
        dom: &'a Dom,
        ax: &'a AxCache,
        references: &'a HashMap<String, Option<RefTarget>>,
        mut current: Vec<usize>,
        steps: &'a [Step],
    ) -> Local<'a, OpResult<Vec<usize>>> {
        Box::pin(async move {
            for step in steps {
                self.gate_frames(dom, &current).await?;
                current = match step {
                    Step::Nth { index } => {
                        let length = current.len() as i64;
                        let index = if *index < 0 { length + index } else { *index };
                        usize::try_from(index)
                            .ok()
                            .and_then(|index| current.get(index).copied())
                            .into_iter()
                            .collect()
                    }
                    Step::HasText { matcher, negate } => {
                        let matcher = matcher.compile().map_err(OpError::new)?;
                        current
                            .into_iter()
                            .filter(|&index| matcher.matches(dom.text(index)) != *negate)
                            .collect()
                    }
                    Step::Has { steps, negate } => {
                        let mut kept = Vec::new();
                        for index in current {
                            let inner = self.eval(dom, ax, references, vec![index], steps).await?;
                            if inner.is_empty() == *negate {
                                kept.push(index);
                            }
                        }
                        kept
                    }
                    Step::Visible { visible } => {
                        let mut kept = Vec::new();
                        for index in current {
                            if self.visible(&dom.node_ref(index)).await? == *visible {
                                kept.push(index);
                            }
                        }
                        kept
                    }
                    Step::Frame => {
                        let mut documents: Vec<usize> = current
                            .into_iter()
                            .filter_map(|index| dom.frame_document(index))
                            .collect();
                        documents.sort_unstable();
                        documents.dedup();
                        documents
                    }
                    Step::FrameMatch { name, url } => {
                        let url = url
                            .as_ref()
                            .map(|url| url.compile())
                            .transpose()
                            .map_err(OpError::new)?;
                        dom.frames
                            .iter()
                            .filter(|frame| name.as_ref().is_none_or(|name| &frame.name == name))
                            .filter(|frame| url.as_ref().is_none_or(|url| url.matches(&frame.url)))
                            .filter_map(|frame| frame.document)
                            .collect()
                    }
                    Step::FrameId { id } => dom
                        .frames
                        .iter()
                        .filter(|frame| &frame.id == id)
                        .filter_map(|frame| frame.document)
                        .collect(),
                    _ => {
                        let mut found = Vec::new();
                        for scope in current {
                            found.extend(self.find(dom, ax, references, scope, step).await?);
                        }
                        found.sort_unstable();
                        found.dedup();
                        found
                    }
                };
                self.gate_frames(dom, &current).await?;
            }
            Ok(current)
        })
    }

    async fn find(
        &self,
        dom: &Dom,
        ax: &AxCache,
        references: &HashMap<String, Option<RefTarget>>,
        scope: usize,
        step: &Step,
    ) -> OpResult<Vec<usize>> {
        Ok(match step {
            Step::Css { selector } => self.css(dom, scope, selector).await?,
            Step::Xpath { selector } => self.xpath(dom, scope, selector).await?,
            Step::Ref { reference } => {
                let index = references
                    .get(reference)
                    .cloned()
                    .flatten()
                    .and_then(|target| {
                        dom.index_of(&NodeRef {
                            host: target.host,
                            frame: target.frame,
                            backend: target.backend,
                        })
                    })
                    .ok_or_else(|| browser_error(BrowserError::StaleReference))?;
                // Snapshot refs name nodes in any frame, so the page itself scopes them all.
                if scope == 0 || dom.contains(scope, index) {
                    vec![index]
                } else {
                    Vec::new()
                }
            }
            Step::Role { role, name, states } => {
                let frame = dom.nodes[scope].frame;
                if !ax.borrow().contains_key(&frame) {
                    let nodes = self.frame_ax(dom, frame).await?;
                    ax.borrow_mut().insert(frame, nodes);
                }
                let name = name
                    .as_ref()
                    .map(|name| name.compile())
                    .transpose()
                    .map_err(OpError::new)?;
                let host = dom.nodes[scope].host;
                let cache = ax.borrow();
                cache
                    .get(&frame)
                    .into_iter()
                    .flatten()
                    .filter(|node| {
                        role_matches(role, &node.role)
                            && name.as_ref().is_none_or(|name| name.matches(&node.name))
                            && states.iter().all(|(state, expected)| {
                                state_matches(state, expected, &node.properties)
                            })
                    })
                    .filter_map(|node| dom.index_in(host, node.backend))
                    .filter(|&index| dom.nodes[index].element() && dom.contains(scope, index))
                    .collect()
            }
            Step::Text { matcher } => {
                dom.text_matches(scope, &matcher.compile().map_err(OpError::new)?)
            }
            Step::Label { matcher } => {
                dom.label_matches(scope, &matcher.compile().map_err(OpError::new)?)
            }
            Step::Attr { name, matcher } => {
                dom.attribute_matches(scope, name, &matcher.compile().map_err(OpError::new)?)
            }
            Step::Nth { .. }
            | Step::HasText { .. }
            | Step::Has { .. }
            | Step::Visible { .. }
            | Step::Frame
            | Step::FrameMatch { .. }
            | Step::FrameId { .. } => Vec::new(),
        })
    }

    /// CSS inside `scope`, piercing open shadow roots: each tree scope is queried on its own,
    /// as `querySelectorAll` never crosses a shadow boundary.
    async fn css(&self, dom: &Dom, scope: usize, selector: &str) -> OpResult<Vec<usize>> {
        let host = dom.nodes[scope].host;
        let mut found = Vec::new();
        for root in std::iter::once(scope).chain(dom.shadow_roots(scope)) {
            let result = self
                .host_cmd(
                    dom.hosts[host].as_deref(),
                    "DOM.querySelectorAll",
                    json!({"nodeId": dom.nodes[root].node_id, "selector": selector}),
                )
                .await
                .map_err(|error| {
                    if error.message == STOPPED {
                        error
                    } else {
                        OpError::new(format!("invalid CSS selector {selector:?}"))
                    }
                })?;
            found.extend(
                result["nodeIds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|id| dom.by_node_id.get(&(host, id.as_i64()?)).copied()),
            );
        }
        found.sort_unstable();
        found.dedup();
        Ok(found)
    }

    /// XPath evaluated with `scope` as the context node, keeping elements inside `scope`.
    async fn xpath(&self, dom: &Dom, scope: usize, selector: &str) -> OpResult<Vec<usize>> {
        let node = dom.node_ref(scope);
        let host = node.host.as_deref();
        let frame = &dom.frames[dom.nodes[scope].frame].id;
        let world = self
            .host_cmd(
                host,
                "Page.createIsolatedWorld",
                json!({"frameId": frame, "worldName": "openside"}),
            )
            .await?;
        let object = self
            .node_cmd(
                &node,
                "DOM.resolveNode",
                json!({"executionContextId": world["executionContextId"], "objectGroup": OBJECT_GROUP}),
            )
            .await?;
        let found = self.xpath_in(dom, scope, host, &object, selector).await;
        let _ = self
            .host_cmd(
                host,
                "Runtime.releaseObjectGroup",
                json!({"objectGroup": OBJECT_GROUP}),
            )
            .await;
        found
    }

    async fn xpath_in(
        &self,
        dom: &Dom,
        scope: usize,
        host: Option<&str>,
        object: &Value,
        selector: &str,
    ) -> OpResult<Vec<usize>> {
        let result = self
            .host_cmd(
                host,
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object["object"]["objectId"],
                    "functionDeclaration": XPATH,
                    "arguments": [{"value": selector}, {"value": XPATH_LIMIT}],
                    "objectGroup": OBJECT_GROUP,
                }),
            )
            .await?;
        if result.get("exceptionDetails").is_some() {
            return Err(OpError::new(format!("invalid XPath selector {selector:?}")));
        }
        let properties = self
            .host_cmd(
                host,
                "Runtime.getProperties",
                json!({"objectId": result["result"]["objectId"], "ownProperties": true}),
            )
            .await?;
        let mut items: Vec<(usize, &Value)> = properties["result"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|property| {
                Some((
                    property["name"].as_str()?.parse().ok()?,
                    &property["value"]["objectId"],
                ))
            })
            .collect();
        items.sort_unstable_by_key(|(position, _)| *position);
        let host_index = dom.nodes[scope].host;
        let mut found = Vec::new();
        for (_, object) in items {
            let described = self
                .host_cmd(host, "DOM.describeNode", json!({"objectId": object}))
                .await?;
            if let Some(index) = described["node"]["backendNodeId"]
                .as_u64()
                .and_then(|backend| dom.index_in(host_index, backend))
                .filter(|&index| dom.contains(scope, index))
            {
                found.push(index);
            }
        }
        found.sort_unstable();
        found.dedup();
        Ok(found)
    }

    /// Accessibility nodes of one frame's document, from the session that hosts it.
    async fn frame_ax(&self, dom: &Dom, frame: usize) -> OpResult<Vec<AxNode>> {
        if frame == 0 {
            return self.full_ax().await;
        }
        let Some(document) = dom.frames[frame].document else {
            return Ok(Vec::new());
        };
        let tree = self
            .host_cmd(
                dom.hosts[dom.nodes[document].host].as_deref(),
                "Accessibility.getFullAXTree",
                json!({"frameId": dom.frames[frame].id}),
            )
            .await?;
        ax_nodes(&tree).map_err(OpError::new)
    }

    pub(super) async fn ax_tree(&self, node: &NodeRef) -> OpResult<Value> {
        self.node_cmd(
            node,
            "Accessibility.getPartialAXTree",
            json!({"fetchRelatives": false}),
        )
        .await
    }

    pub(super) async fn ax_node(&self, node: &NodeRef) -> OpResult<Value> {
        Ok(ax_node_of(&self.ax_tree(node).await?, node.backend).unwrap_or(Value::Null))
    }

    /// The accessibility node a policy decision is made on; see [`decidable`].
    pub(super) async fn gate_node(&self, node: &NodeRef) -> OpResult<Value> {
        decidable(ax_node_of(&self.ax_tree(node).await?, node.backend)).map_err(OpError::new)
    }

    /// Wait until the locator resolves, within one authorized document, to exactly one element
    /// that is visible and enabled as required.
    pub(super) async fn single(
        &self,
        located: &Located,
        timeout: Duration,
        need_visible: bool,
        need_enabled: bool,
    ) -> OpResult<Resolved> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.try_single(located, need_visible, need_enabled).await {
                Ok(Some(resolved)) => return Ok(resolved),
                Ok(None) => {}
                Err(error) if error.message == CHANGED => {}
                Err(error) => return Err(error),
            }
            if Instant::now() >= deadline {
                let wanted = match (need_visible, need_enabled) {
                    (true, true) => "visible, enabled and stable",
                    (true, false) => "visible",
                    _ => "attached",
                };
                return Err(OpError::timeout(format!(
                    "Timeout {}ms exceeded waiting for {} to be {wanted}",
                    timeout.as_millis(),
                    located.description
                )));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub(super) async fn try_single(
        &self,
        located: &Located,
        need_visible: bool,
        need_enabled: bool,
    ) -> OpResult<Option<Resolved>> {
        let resolved = self.resolve(&located.steps).await?;
        match resolved.found.len() {
            0 => Ok(None),
            1 => {
                let index = resolved.found[0];
                let node = resolved.dom.node_ref(index);
                let ready = (!need_visible || self.visible(&node).await?)
                    && (!need_enabled || self.enabled(&resolved.dom, index).await?);
                self.same_documents(&resolved).await?;
                Ok(ready.then_some(resolved))
            }
            _ => Err(strict_violation(&located.description, &resolved)),
        }
    }

    pub(super) async fn full_ax(&self) -> OpResult<Vec<AxNode>> {
        self.ensure_page_size().await?;
        let tree = self.cmd("Accessibility.getFullAXTree", json!({})).await?;
        ax_nodes(&tree).map_err(OpError::new)
    }

    /// Refuse before the browser serializes a whole document or accessibility tree into one
    /// reply: the renderer's live node count bounds the reply's node count up front.
    pub(super) async fn ensure_page_size(&self) -> OpResult<()> {
        let metrics = self.metrics().await?;
        let nodes = metric(&metrics, "Nodes");
        if nodes > MAX_NODES as f64 {
            return Err(OpError::new(TOO_LARGE));
        }
        Ok(())
    }

    pub(super) async fn query(&self, args: &Value) -> OpResult<Value> {
        let what = args["what"].as_str().unwrap_or_default().to_owned();
        let name = args["name"].as_str().map(str::to_owned);
        let located = located(args)?;
        self.gate_page(None).await?;
        let timeout = located
            .timeout
            .map(Duration::from_millis)
            .unwrap_or(self.limits.action_timeout);
        if matches!(what.as_str(), "isVisible" | "isHidden") {
            let resolved = self.resolve(&located.steps).await?;
            let visible = match resolved.found.as_slice() {
                [] => false,
                [index] => self.visible(&resolved.dom.node_ref(*index)).await?,
                _ => return Err(strict_violation(&located.description, &resolved)),
            };
            self.same_documents(&resolved).await?;
            return Ok(json!(visible == (what == "isVisible")));
        }
        let resolved = self.single(&located, timeout, false, false).await?;
        let dom = &resolved.dom;
        let index = resolved.found[0];
        let node = dom.node_ref(index);
        let value = match what.as_str() {
            "textContent" => json!(dom.text_content(index)),
            "innerText" => json!(text_of(dom, index, true)),
            "getAttribute" => {
                json!(dom.nodes[index].attribute(name.as_deref().unwrap_or_default()))
            }
            "inputValue" => json!(self.input_value(dom, index).await?),
            "isChecked" => json!(self.checked(&node).await?),
            "isEnabled" => json!(self.enabled(dom, index).await?),
            "isDisabled" => json!(!self.enabled(dom, index).await?),
            "isEditable" => json!(
                self.enabled(dom, index).await? && dom.nodes[index].attribute("readonly").is_none()
            ),
            "isFocused" => json!(matches!(
                self.property(&node, "focused").await?,
                Some(Value::Bool(true))
            )),
            "boundingBox" => match self.node_cmd(&node, "DOM.getBoxModel", json!({})).await {
                Ok(model) => {
                    let (left, top) = self.host_offset(node.host.as_deref()).await?;
                    let border = &model["model"]["border"];
                    json!({
                        "x": border[0].as_f64().unwrap_or_default() + left,
                        "y": border[1].as_f64().unwrap_or_default() + top,
                        "width": model["model"]["width"],
                        "height": model["model"]["height"],
                    })
                }
                Err(_) => Value::Null,
            },
            other => {
                return Err(OpError {
                    name: "NotSupported",
                    message: format!("locator.{other} is not supported"),
                })
            }
        };
        self.same_documents(&resolved).await?;
        Ok(value)
    }

    pub(super) async fn input_value(&self, dom: &Dom, index: usize) -> OpResult<String> {
        let node = &dom.nodes[index];
        match node.name.as_str() {
            "input" | "textarea" => {
                let ax = self.ax_node(&dom.node_ref(index)).await?;
                Ok(ax_text(&ax["value"]))
            }
            "select" => {
                let nodes = self.frame_ax(dom, node.frame).await?;
                let selected = nodes.into_iter().find_map(|ax| {
                    let option = dom.index_in(node.host, ax.backend)?;
                    (dom.nodes[option].name == "option"
                        && dom.contains(index, option)
                        && matches!(ax.properties.get("selected"), Some(Value::Bool(true))))
                    .then_some(option)
                });
                Ok(selected
                    .map(|option| {
                        dom.nodes[option]
                            .attribute("value")
                            .map(str::to_owned)
                            .unwrap_or_else(|| normalize(dom.text_content(option)))
                    })
                    .unwrap_or_default())
            }
            _ => Err(OpError::new(
                "Node is not an <input>, <textarea> or <select> element",
            )),
        }
    }

    pub(super) async fn count(&self, args: &Value) -> OpResult<Value> {
        let located = located(args)?;
        let resolved = self.resolve(&located.steps).await?;
        self.same_documents(&resolved).await?;
        Ok(json!(resolved.found.len()))
    }

    pub(super) async fn texts(&self, args: &Value) -> OpResult<Value> {
        let inner = args["inner"] == true;
        let located = located(args)?;
        let resolved = self.resolve(&located.steps).await?;
        let texts: Vec<_> = resolved
            .found
            .iter()
            .map(|&index| text_of(&resolved.dom, index, inner))
            .collect();
        self.same_documents(&resolved).await?;
        Ok(json!(texts))
    }

    /// The one frame `steps` select (frame steps from `page.frame`, `frameLocator`,
    /// `contentFrame` or `page.frames()`; none for the main frame), authorized for the top
    /// origin and its own.
    pub(super) async fn frame_target(&self, steps: &[Step]) -> OpResult<FrameTarget> {
        let (resolved, document) = self.frame_of(steps).await?;
        let frame = &resolved.dom.frames[resolved.dom.nodes[document].frame];
        Ok(FrameTarget {
            id: frame.id.clone(),
        })
    }

    /// The resolution of frame steps and the `#document` node of the one frame they select.
    async fn frame_of(&self, steps: &[Step]) -> OpResult<(Resolved, usize)> {
        let resolved = self.resolve(steps).await?;
        let documents: Vec<usize> = resolved
            .found
            .iter()
            .copied()
            .filter(|&index| resolved.dom.nodes[index].node_type == 9)
            .collect();
        match documents.as_slice() {
            [document] => {
                let document = *document;
                Ok((resolved, document))
            }
            [] => Err(OpError::new("the frame is not attached")),
            _ => Err(strict_violation("the frame selector", &resolved)),
        }
    }

    async fn frame_document(&self, args: &Value) -> OpResult<(Resolved, usize)> {
        self.frame_of(&frame_steps(args)?).await
    }

    pub(super) async fn title(&self, args: &Value) -> OpResult<Value> {
        let (resolved, document) = self.frame_document(args).await?;
        let dom = &resolved.dom;
        let title = (document..dom.nodes.len())
            .take_while(|&index| index == document || dom.contains(document, index))
            .find(|&index| dom.nodes[index].name == "title")
            .map(|index| normalize(dom.text_content(index)))
            .unwrap_or_default();
        self.same_documents(&resolved).await?;
        Ok(json!(title))
    }

    pub(super) async fn content(&self, args: &Value) -> OpResult<Value> {
        let (resolved, document) = self.frame_document(args).await?;
        let mut html = self
            .node_cmd(
                &resolved.dom.node_ref(document),
                "DOM.getOuterHTML",
                json!({}),
            )
            .await?;
        self.same_documents(&resolved).await?;
        Ok(html["outerHTML"].take())
    }

    /// The tab's frames, top first: id, parent id, name and URL.
    async fn list_frames(&self) -> OpResult<Value> {
        let loader = self.gate_page(None).await?;
        let frames = self.frame_list().await?;
        self.same_document(&loader).await?;
        Ok(Value::Array(
            frames
                .into_iter()
                .map(|frame| {
                    json!({"id": frame.id, "parentId": frame.parent, "name": frame.name, "url": frame.url})
                })
                .collect(),
        ))
    }

    /// Snapshot with the child frames whose origin policy allows; a denied frame shows only
    /// as its owner element, without content.
    pub(super) async fn snapshot(&self) -> OpResult<Value> {
        let loader = self.gate_page(None).await?;
        self.ensure_page_size().await?;
        let guard = &self.guard;
        let keep = |top: &FrameInfo, frame: &FrameInfo| {
            matches!(
                guard.check(&Gate::Page {
                    origin: &top.origin,
                    frame: Some(Frame {
                        id: &frame.id,
                        origin: &frame.origin,
                    }),
                    target: None,
                }),
                Verdict::Allow
            )
        };
        let snapshot = self
            .browser
            .snapshot_frames(&self.owner, &self.tab, false, &keep)
            .await
            .map_err(browser_error)?;
        self.same_document(&loader).await?;
        Ok(json!(snapshot.text))
    }
}

/// Strict-mode error listing the candidates, so the script can pick a narrower locator.
fn strict_violation(description: &str, resolved: &Resolved) -> OpError {
    let dom = &resolved.dom;
    let mut message = format!(
        "strict mode violation: {description} resolved to {} elements:",
        resolved.found.len()
    );
    for (position, &index) in resolved.found.iter().take(LISTED_CANDIDATES).enumerate() {
        message.push_str(&format!("\n    {}) {}", position + 1, preview(dom, index)));
    }
    if resolved.found.len() > LISTED_CANDIDATES {
        message.push_str(&format!(
            "\n    ...and {} more",
            resolved.found.len() - LISTED_CANDIDATES
        ));
    }
    OpError::new(message)
}

/// A short HTML-like rendering of a candidate: tag, identifying attributes, text, and the
/// frame it is in when that is not the top frame.
fn preview(dom: &Dom, index: usize) -> String {
    const ATTRIBUTES: &[&str] = &[
        "id",
        "name",
        "type",
        "role",
        "aria-label",
        "data-testid",
        "href",
        "class",
    ];
    const TEXT: usize = 40;
    let node = &dom.nodes[index];
    if node.node_type == 9 {
        let frame = &dom.frames[node.frame];
        return format!("<frame name={:?} url={:?}>", frame.name, frame.url);
    }
    let mut out = format!("<{}", node.name);
    for name in ATTRIBUTES {
        if let Some(value) = node.attribute(name) {
            let value: String = value.chars().take(TEXT).collect();
            out.push_str(&format!(" {name}={value:?}"));
        }
    }
    out.push('>');
    let text = normalize(dom.text(index));
    let mut short: String = text.chars().take(TEXT).collect();
    if text.chars().count() > TEXT {
        short.push('…');
    }
    out.push_str(&short);
    out.push_str(&format!("</{}>", node.name));
    if node.frame != 0 {
        let frame = &dom.frames[node.frame];
        let label = if frame.name.is_empty() {
            &frame.url
        } else {
            &frame.name
        };
        out.push_str(&format!(" in frame {label:?}"));
    }
    out
}

pub(super) fn located(args: &Value) -> OpResult<Located> {
    Located::deserialize(args).map_err(|_| OpError::new("invalid locator"))
}

/// The frame steps in `args.frame`; none (the main frame) when absent.
fn frame_steps(args: &Value) -> OpResult<Vec<Step>> {
    match args.get("frame") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(frame) => Vec::<Step>::deserialize(frame).map_err(|_| OpError::new("invalid frame")),
    }
}

pub(super) fn text_of(dom: &Dom, index: usize, inner: bool) -> String {
    if inner {
        normalize(dom.text(index))
    } else {
        dom.text_content(index).to_owned()
    }
}

fn collect_refs(steps: &[Step], references: &mut HashMap<String, Option<RefTarget>>) {
    for step in steps {
        match step {
            Step::Ref { reference } => {
                references.insert(reference.clone(), None);
            }
            Step::Has { steps, .. } => collect_refs(steps, references),
            _ => {}
        }
    }
}
