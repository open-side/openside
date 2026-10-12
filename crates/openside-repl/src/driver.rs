//! Host operations behind the JavaScript prelude. Every page interaction is a command from
//! `openside_browser::PAGE_COMMANDS`; selector engines run here in Rust, never in the page.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use openside_browser::{Browser, BrowserError};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    dom::{
        ax_node_of, ax_nodes, ax_text, decidable, normalize, role_matches, state_matches, AxNode,
        Dom, Step, MAX_NODES, TOO_LARGE,
    },
    keys, Gate, Guard, Limits, Stop, Target, Verdict,
};

const STOPPED: &str = "execution stopped";
const CHANGED: &str = "the page navigated to another document during the operation";
const FOREIGN_FOCUS: &str =
    "keyboard focus is not on a top-frame element; the REPL only sends keys to the top frame";
/// Upper bound on `clickCount`: triple-click is the largest multi-click pages distinguish, and
/// the bound keeps one operation from becoming an unbounded event stream.
const MAX_CLICKS: u64 = 3;

type Local<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

pub(crate) struct OpError {
    name: &'static str,
    message: String,
}

impl OpError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            name: "Error",
            message: message.into(),
        }
    }
    fn timeout(message: impl Into<String>) -> Self {
        Self {
            name: "TimeoutError",
            message: message.into(),
        }
    }
}

type OpResult<T> = Result<T, OpError>;

fn browser_error(error: BrowserError) -> OpError {
    match error {
        BrowserError::StaleReference => {
            OpError::new("stale aria-ref; take a new snapshot (page.snapshot()) and use its refs")
        }
        BrowserError::OwnershipDenied(_) => OpError::new("tab is not owned by this session"),
        BrowserError::Timeout(what) => OpError::timeout(format!("browser timeout: {what}")),
        other => OpError::new(format!("browser operation failed: {other}")),
    }
}

/// Bounded console capture shared with the JavaScript `console` object.
#[derive(Default)]
pub(crate) struct Console {
    pub(crate) lines: Vec<String>,
    bytes: usize,
    truncated: bool,
}

// Each stored line costs host memory even when empty, so it is charged against the budget too.
const LINE_OVERHEAD: usize = 32;

impl Console {
    pub(crate) fn push(&mut self, line: String, limit: usize) {
        if self.truncated {
            return;
        }
        let cost = line.len() + LINE_OVERHEAD;
        if self.bytes + cost > limit {
            self.truncated = true;
            self.lines
                .push("[console output truncated; log less to see the rest]".into());
            return;
        }
        self.bytes += cost;
        self.lines.push(line);
    }
}

/// The policy stop of one execution, shared with the navigation interceptor, which records
/// stops from the browser's event task.
#[derive(Clone)]
pub(crate) struct Stops {
    stop: Arc<Mutex<Option<Stop>>>,
    abort: Arc<AtomicBool>,
}

impl Stops {
    // Policy stops are uncatchable: the interrupt handler ends the script at its next check, and
    // every later CDP command is refused.
    pub(crate) fn halt(&self, stop: Stop) {
        self.stop
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_or_insert(stop);
        self.abort.store(true, Ordering::Release);
    }

    /// Record a stop for a non-`Allow` verdict; returns whether the operation may proceed.
    pub(crate) fn allow(&self, verdict: Verdict) -> bool {
        match verdict {
            Verdict::Allow => true,
            Verdict::Deny(reason) => {
                self.halt(Stop::Denied(reason));
                false
            }
            Verdict::Approval(action) => {
                self.halt(Stop::Approval(action));
                false
            }
        }
    }

    pub(crate) fn current(&self) -> Option<Stop> {
        self.stop
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

struct Resolved {
    dom: Dom,
    found: Vec<usize>,
    loader: String,
}

/// Keyboard state of one operation: the node last seen focused (a lookup hint only) and the
/// targets authorized during it with the role and name they had then (see `node_gate`).
#[derive(Default)]
struct Keys {
    focused: Option<u64>,
    granted: Vec<(u64, String, String)>,
}

#[derive(Deserialize)]
struct Located {
    steps: Vec<Step>,
    #[serde(default)]
    description: String,
    timeout: Option<u64>,
}

pub(crate) struct Driver {
    browser: Arc<Browser>,
    owner: String,
    tab: String,
    guard: Arc<dyn Guard>,
    limits: Limits,
    deadline: Instant,
    cancel: Arc<AtomicBool>,
    stops: Stops,
    pub(crate) abort: Arc<AtomicBool>,
    pub(crate) console: RefCell<Console>,
    pub(crate) url: RefCell<String>,
    metrics: Cell<bool>,
    // Promise.all over page operations must not interleave focus, input, and policy checks.
    serial: tokio::sync::Mutex<()>,
}

impl Driver {
    pub(crate) fn new(
        browser: Arc<Browser>,
        owner: String,
        tab: String,
        guard: Arc<dyn Guard>,
        limits: Limits,
        deadline: Instant,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        let abort = Arc::new(AtomicBool::new(false));
        Self {
            browser,
            owner,
            tab,
            guard,
            limits,
            deadline,
            cancel,
            stops: Stops {
                stop: Arc::new(Mutex::new(None)),
                abort: Arc::clone(&abort),
            },
            abort,
            console: RefCell::default(),
            url: RefCell::new(String::new()),
            metrics: Cell::new(false),
            serial: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) fn stops(&self) -> Stops {
        self.stops.clone()
    }

    fn stopped(&self) -> bool {
        self.abort.load(Ordering::Acquire)
            || self.cancel.load(Ordering::Acquire)
            || Instant::now() >= self.deadline
    }

    pub(crate) fn log(&self, line: String) {
        self.console
            .borrow_mut()
            .push(line, self.limits.console_bytes);
    }

    pub(crate) async fn cmd(&self, method: &str, params: Value) -> OpResult<Value> {
        if self.stopped() {
            return Err(OpError::new(STOPPED));
        }
        self.browser
            .page_command(&self.owner, &self.tab, method, params)
            .await
            .map_err(browser_error)
    }

    /// Restore tab state after the script, even when it was stopped.
    pub(crate) async fn cleanup_cmd(&self, method: &str, params: Value) {
        let _ = self
            .browser
            .page_command(&self.owner, &self.tab, method, params)
            .await;
    }

    async fn frame(&self) -> OpResult<Value> {
        let mut tree = self.cmd("Page.getFrameTree", json!({})).await?;
        let frame = tree["frameTree"]["frame"].take();
        if let Some(url) = frame["url"].as_str() {
            *self.url.borrow_mut() = url.to_owned();
        }
        Ok(frame)
    }

    pub(crate) async fn refresh_url(&self) -> OpResult<String> {
        self.frame().await?;
        Ok(self.url.borrow().clone())
    }

    /// Run one prelude operation; the JSON reply always carries the latest known top-frame URL.
    pub(crate) async fn dispatch(&self, name: &str, payload: &str) -> String {
        let _serial = self.serial.lock().await;
        let result = if self.stops.current().is_some() || self.stopped() {
            Err(OpError::new(STOPPED))
        } else {
            match serde_json::from_str::<Value>(payload) {
                Ok(args) => self.op(name, args).await,
                Err(_) => Err(OpError::new("invalid operation payload")),
            }
        };
        let url = self.url.borrow().clone();
        let reply = match result {
            Ok(value) => json!({"value": value, "url": url}),
            Err(error) => json!({"error": error.message, "name": error.name, "url": url}),
        };
        let mut out = Bounded(Vec::new());
        match serde_json::to_writer(&mut out, &reply) {
            Ok(()) => String::from_utf8(out.0).unwrap_or_default(),
            Err(_) => json!({"error": TOO_LARGE, "name": "Error", "url": url}).to_string(),
        }
    }

    async fn op(&self, name: &str, args: Value) -> OpResult<Value> {
        match name {
            "count" => {
                let located = located(args)?;
                let resolved = self.resolve(&located.steps).await?;
                self.same_document(&resolved.loader).await?;
                Ok(json!(resolved.found.len()))
            }
            "act" => self.act(args).await,
            "query" => self.query(args).await,
            "texts" => {
                let inner = args["inner"] == true;
                let located = located(args)?;
                let resolved = self.resolve(&located.steps).await?;
                let texts: Vec<_> = resolved
                    .found
                    .iter()
                    .map(|&index| text_of(&resolved.dom, index, inner))
                    .collect();
                self.same_document(&resolved.loader).await?;
                Ok(json!(texts))
            }
            "wait_for" => self.wait_for(args).await,
            "goto" => {
                let url = args["url"]
                    .as_str()
                    .ok_or_else(|| OpError::new("goto requires a URL"))?;
                self.gate(Gate::Navigate { url }).await?;
                let result = self.cmd("Page.navigate", json!({"url": url})).await?;
                if let Some(error) = result["errorText"].as_str() {
                    return Err(OpError::new(format!("page.goto: {error} at {url}")));
                }
                let loader = result["loaderId"].as_str().map(str::to_owned);
                let target = url.to_owned();
                self.wait_navigation(
                    timeout_of(&args, self.limits.action_timeout),
                    move |frame| match &loader {
                        Some(loader) => frame["loaderId"] == loader.as_str(),
                        None => frame["url"].as_str().is_some_and(|url| {
                            url.starts_with(target.split('#').next().unwrap_or_default())
                        }),
                    },
                )
                .await?;
                Ok(Value::Null)
            }
            "reload" => {
                let before = self.frame().await?;
                let url = before["url"].as_str().unwrap_or_default().to_owned();
                self.gate(Gate::Navigate { url: &url }).await?;
                self.cmd("Page.reload", json!({})).await?;
                self.wait_navigation(
                    timeout_of(&args, self.limits.action_timeout),
                    move |frame| frame["loaderId"] != before["loaderId"],
                )
                .await?;
                Ok(Value::Null)
            }
            "history" => {
                let delta = args["delta"].as_i64().unwrap_or(-1);
                let history = self.cmd("Page.getNavigationHistory", json!({})).await?;
                let index = history["currentIndex"].as_i64().unwrap_or_default() + delta;
                let Some(entry) = usize::try_from(index)
                    .ok()
                    .and_then(|index| history["entries"].get(index))
                else {
                    return Ok(Value::Null);
                };
                let url = entry["url"].as_str().unwrap_or_default().to_owned();
                self.gate(Gate::Navigate { url: &url }).await?;
                let before = self.frame().await?;
                self.cmd(
                    "Page.navigateToHistoryEntry",
                    json!({"entryId": entry["id"]}),
                )
                .await?;
                self.wait_navigation(
                    timeout_of(&args, self.limits.action_timeout),
                    move |frame| {
                        frame["loaderId"] != before["loaderId"] || frame["url"] != before["url"]
                    },
                )
                .await?;
                Ok(Value::Null)
            }
            "url" => {
                self.gate_page(None).await?;
                Ok(json!(self.url.borrow().clone()))
            }
            "title" => {
                let loader = self.gate_page(None).await?;
                let dom = self.document().await?;
                let title = dom
                    .nodes
                    .iter()
                    .position(|node| node.name == "title")
                    .map(|index| normalize(dom.text_content(index)))
                    .unwrap_or_default();
                self.same_document(&loader).await?;
                Ok(json!(title))
            }
            "content" => {
                let loader = self.gate_page(None).await?;
                self.ensure_page_size().await?;
                let document = self.cmd("DOM.getDocument", json!({"depth": 0})).await?;
                let mut html = self
                    .cmd(
                        "DOM.getOuterHTML",
                        json!({"backendNodeId": document["root"]["backendNodeId"]}),
                    )
                    .await?;
                self.same_document(&loader).await?;
                Ok(html["outerHTML"].take())
            }
            "load_state" => {
                self.wait_loaded(Instant::now() + timeout_of(&args, self.limits.action_timeout))
                    .await?;
                if args["state"] == "networkidle" {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                self.gate_page(None).await?;
                Ok(Value::Null)
            }
            "sleep" => {
                let ms = args["ms"]
                    .as_u64()
                    .unwrap_or_default()
                    .min(self.limits.timeout.as_millis() as u64);
                tokio::time::sleep(Duration::from_millis(ms)).await;
                Ok(Value::Null)
            }
            "keyboard" => self.keyboard(args).await,
            "snapshot" => {
                let loader = self.gate_page(None).await?;
                self.ensure_page_size().await?;
                let snapshot = self
                    .browser
                    .snapshot(&self.tab)
                    .await
                    .map_err(browser_error)?;
                self.same_document(&loader).await?;
                Ok(json!(snapshot.text))
            }
            other => Err(OpError {
                name: "NotSupported",
                message: format!("{other} is not supported by the openside REPL"),
            }),
        }
    }

    async fn gate(&self, gate: Gate<'_>) -> OpResult<()> {
        let verdict = self.guard.check(&gate);
        let message = match &verdict {
            Verdict::Allow => return Ok(()),
            Verdict::Deny(reason) => format!("policy denied: {reason}"),
            Verdict::Approval(action) => format!("human approval required: {action}"),
        };
        self.stops.allow(verdict);
        Err(OpError {
            name: "PolicyError",
            message,
        })
    }

    /// Authorize the current top-frame document and, if given, the interaction target in it;
    /// returns the authorized document's loader id for [`Self::same_document`].
    async fn gate_page(&self, target: Option<Target<'_>>) -> OpResult<String> {
        let frame = self.frame().await?;
        let origin = frame["securityOrigin"].as_str().unwrap_or("null");
        self.gate(Gate::Page { origin, target }).await?;
        Ok(frame["loaderId"].as_str().unwrap_or_default().to_owned())
    }

    /// Page data may only leave an operation if it came from the document that was authorized.
    async fn same_document(&self, loader: &str) -> OpResult<()> {
        if self.frame().await?["loaderId"] == loader {
            Ok(())
        } else {
            Err(OpError::new(CHANGED))
        }
    }

    async fn document(&self) -> OpResult<Dom> {
        self.ensure_page_size().await?;
        let document = self.cmd("DOM.getDocument", json!({"depth": -1})).await?;
        Dom::parse(&document, &|| self.stopped()).map_err(OpError::new)
    }

    /// Authorize the current document, then evaluate `steps` in it.
    async fn resolve(&self, steps: &[Step]) -> OpResult<Resolved> {
        let loader = self.gate_page(None).await?;
        // DOM.getDocument renumbers node ids, so snapshot refs resolve before the full fetch.
        let mut references = HashMap::new();
        collect_refs(steps, &mut references);
        for (reference, backend) in references.iter_mut() {
            *backend = Some(
                self.browser
                    .reference_node(&self.owner, &self.tab, reference)
                    .await
                    .map_err(browser_error)?,
            );
        }
        let dom = self.document().await?;
        let ax = RefCell::new(None);
        let found = self.eval(&dom, &ax, &references, vec![0], steps).await?;
        Ok(Resolved { dom, found, loader })
    }

    fn eval<'a>(
        &'a self,
        dom: &'a Dom,
        ax: &'a RefCell<Option<Vec<AxNode>>>,
        references: &'a HashMap<String, Option<u64>>,
        mut current: Vec<usize>,
        steps: &'a [Step],
    ) -> Local<'a, OpResult<Vec<usize>>> {
        Box::pin(async move {
            for step in steps {
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
                            if self.visible(dom.nodes[index].backend).await? == *visible {
                                kept.push(index);
                            }
                        }
                        kept
                    }
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
            }
            Ok(current)
        })
    }

    async fn find(
        &self,
        dom: &Dom,
        ax: &RefCell<Option<Vec<AxNode>>>,
        references: &HashMap<String, Option<u64>>,
        scope: usize,
        step: &Step,
    ) -> OpResult<Vec<usize>> {
        Ok(match step {
            Step::Css { selector } => {
                let result = self
                    .cmd(
                        "DOM.querySelectorAll",
                        json!({"nodeId": dom.nodes[scope].node_id, "selector": selector}),
                    )
                    .await
                    .map_err(|_| OpError::new(format!("invalid CSS selector {selector:?}")))?;
                result["nodeIds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|id| dom.by_node_id.get(&id.as_i64()?).copied())
                    .collect()
            }
            Step::Ref { reference } => {
                let index = references
                    .get(reference)
                    .copied()
                    .flatten()
                    .and_then(|backend| dom.by_backend.get(&backend).copied())
                    .ok_or_else(|| browser_error(BrowserError::StaleReference))?;
                if scope == 0 || dom.contains(scope, index) {
                    vec![index]
                } else {
                    Vec::new()
                }
            }
            Step::Role { role, name, states } => {
                if ax.borrow().is_none() {
                    *ax.borrow_mut() = Some(self.full_ax().await?);
                }
                let name = name
                    .as_ref()
                    .map(|name| name.compile())
                    .transpose()
                    .map_err(OpError::new)?;
                let nodes = ax.borrow();
                nodes
                    .iter()
                    .flatten()
                    .filter(|node| {
                        role_matches(role, &node.role)
                            && name.as_ref().is_none_or(|name| name.matches(&node.name))
                            && states.iter().all(|(state, expected)| {
                                state_matches(state, expected, &node.properties)
                            })
                    })
                    .filter_map(|node| dom.by_backend.get(&node.backend).copied())
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
            Step::Nth { .. } | Step::HasText { .. } | Step::Has { .. } | Step::Visible { .. } => {
                Vec::new()
            }
        })
    }

    async fn ax_tree(&self, backend: u64) -> OpResult<Value> {
        self.cmd(
            "Accessibility.getPartialAXTree",
            json!({"backendNodeId": backend, "fetchRelatives": false}),
        )
        .await
    }

    async fn ax_node(&self, backend: u64) -> OpResult<Value> {
        Ok(ax_node_of(&self.ax_tree(backend).await?, backend).unwrap_or(Value::Null))
    }

    /// The accessibility node a policy decision is made on; see [`decidable`].
    async fn gate_node(&self, backend: u64) -> OpResult<Value> {
        decidable(ax_node_of(&self.ax_tree(backend).await?, backend)).map_err(OpError::new)
    }

    async fn visible(&self, backend: u64) -> OpResult<bool> {
        let Ok(model) = self
            .cmd("DOM.getBoxModel", json!({"backendNodeId": backend}))
            .await
        else {
            return Ok(false);
        };
        if model["model"]["width"].as_f64().unwrap_or_default() <= 0.0
            || model["model"]["height"].as_f64().unwrap_or_default() <= 0.0
        {
            return Ok(false);
        }
        // `visibility: hidden` keeps a layout box; Chromium marks such nodes notVisible in AX.
        let node = self.ax_node(backend).await?;
        let hidden = node["ignored"] == true
            && node["ignoredReasons"].as_array().is_some_and(|reasons| {
                reasons.iter().any(|reason| {
                    matches!(reason["name"].as_str(), Some("notVisible" | "notRendered"))
                })
            });
        Ok(!hidden)
    }

    async fn property(&self, backend: u64, name: &str) -> OpResult<Option<Value>> {
        let node = self.ax_node(backend).await?;
        Ok(node["properties"].as_array().and_then(|properties| {
            properties
                .iter()
                .find(|property| property["name"] == name)
                .map(|property| property["value"]["value"].clone())
        }))
    }

    async fn enabled(&self, dom: &Dom, index: usize) -> OpResult<bool> {
        if dom.nodes[index].attribute("disabled").is_some() {
            return Ok(false);
        }
        let disabled = self.property(dom.nodes[index].backend, "disabled").await?;
        Ok(!matches!(disabled, Some(Value::Bool(true))))
    }

    async fn checked(&self, backend: u64) -> OpResult<bool> {
        match self.property(backend, "checked").await? {
            Some(Value::String(state)) => Ok(state == "true"),
            Some(Value::Bool(state)) => Ok(state),
            _ => Err(OpError::new("Not a checkbox or radio button")),
        }
    }

    /// Wait until the locator resolves, within one authorized document, to exactly one element
    /// that is visible and enabled as required.
    async fn single(
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

    async fn try_single(
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
                let backend = resolved.dom.nodes[index].backend;
                let ready = (!need_visible || self.visible(backend).await?)
                    && (!need_enabled || self.enabled(&resolved.dom, index).await?);
                self.same_document(&resolved.loader).await?;
                Ok(ready.then_some(resolved))
            }
            count => Err(OpError::new(format!(
                "strict mode violation: {} resolved to {count} elements",
                located.description
            ))),
        }
    }

    async fn box_center(&self, backend: u64) -> OpResult<(f64, f64)> {
        let model = self
            .cmd("DOM.getBoxModel", json!({"backendNodeId": backend}))
            .await?;
        let quad = model["model"]["content"]
            .as_array()
            .filter(|quad| quad.len() == 8)
            .ok_or_else(|| OpError::new("element has no box"))?;
        let (mut x, mut y) = (0.0, 0.0);
        for pair in quad.chunks_exact(2) {
            x += pair[0].as_f64().unwrap_or_default();
            y += pair[1].as_f64().unwrap_or_default();
        }
        Ok(((x / 4.0).round(), (y / 4.0).round()))
    }

    /// The point a pointer action on `index` would use, provided the event would reach that
    /// element: its box must hold still across two reads and the top-frame hit target at the
    /// point must be the element or a descendant, so an overlay or a frame cannot take the event.
    async fn hit_point(
        &self,
        dom: &Dom,
        index: usize,
    ) -> OpResult<Result<(f64, f64), &'static str>> {
        let backend = dom.nodes[index].backend;
        if self
            .cmd(
                "DOM.scrollIntoViewIfNeeded",
                json!({"backendNodeId": backend}),
            )
            .await
            .is_err()
        {
            return Ok(Err("is not attached"));
        }
        let Ok(first) = self.box_center(backend).await else {
            return Ok(Err("has no box"));
        };
        tokio::time::sleep(Duration::from_millis(40)).await;
        let Ok(point) = self.box_center(backend).await else {
            return Ok(Err("has no box"));
        };
        if point != first {
            return Ok(Err("is not stable"));
        }
        let Ok(hit) = self
            .cmd(
                "DOM.getNodeForLocation",
                json!({"x": point.0 as i64, "y": point.1 as i64, "includeUserAgentShadowDOM": false, "ignorePointerEventsNone": false}),
            )
            .await
        else {
            return Ok(Err("is outside the viewport"));
        };
        let top = self.frame().await?;
        if hit["frameId"] != top["id"] {
            return Ok(Err("is covered by a frame"));
        }
        let receiver = hit["backendNodeId"].as_u64();
        let inside = receiver == Some(backend)
            || receiver
                .and_then(|receiver| dom.by_backend.get(&receiver))
                .is_some_and(|&receiver| dom.contains(index, receiver));
        Ok(if inside {
            Ok(point)
        } else {
            Err("is covered by another element that would receive the pointer event")
        })
    }

    /// Hit-test, then dispatch each pointer event only after re-verifying, right before it, that
    /// the authorized document is current, the top-frame hit target at the point is still the
    /// element or a descendant, and the element's current role and name pass policy. An event
    /// can change the page, so a target that stops verifying ends the sequence without further
    /// events. Before the first event a failed hit test re-resolves and retries until `deadline`.
    async fn pointer(
        &self,
        located: &Located,
        mut resolved: Resolved,
        deadline: Instant,
        verb: &str,
        button: &str,
        clicks: u64,
    ) -> OpResult<()> {
        let (x, y) = loop {
            let index = resolved.found[0];
            match self.hit_point(&resolved.dom, index).await? {
                Ok(point) => break point,
                Err(reason) if Instant::now() >= deadline => {
                    return Err(OpError::timeout(format!(
                        "Timeout exceeded waiting for {} to receive pointer events: it {reason}",
                        located.description
                    )))
                }
                Err(_) => {}
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            resolved = self
                .single(
                    located,
                    deadline.saturating_duration_since(Instant::now()),
                    true,
                    true,
                )
                .await?;
        };
        let backend = resolved.dom.nodes[resolved.found[0]].backend;
        let mut granted = Vec::new();
        // Built one at a time, so nothing is prepared for events a failed check never sends.
        let mut event = Some(json!({"type":"mouseMoved","x":x,"y":y}));
        let (mut count, mut pressed) = (0, true);
        while let Some(next) = event.take() {
            self.same_document(&resolved.loader).await?;
            if !self.receives(backend, x, y).await? {
                return Err(OpError::new(format!(
                    "{} no longer receives pointer events at its point; stopped before the next input event",
                    located.description
                )));
            }
            self.node_gate(backend, verb, &mut granted).await?;
            self.cmd("Input.dispatchMouseEvent", next).await?;
            if pressed && count < clicks {
                count += 1;
                pressed = false;
                event = Some(
                    json!({"type":"mousePressed","x":x,"y":y,"button":button,"clickCount":count}),
                );
            } else if !pressed {
                pressed = true;
                event = Some(
                    json!({"type":"mouseReleased","x":x,"y":y,"button":button,"clickCount":count}),
                );
            }
        }
        Ok(())
    }

    /// Whether the top-frame hit target at `(x, y)` is `backend` or one of its descendants now.
    async fn receives(&self, backend: u64, x: f64, y: f64) -> OpResult<bool> {
        let Ok(hit) = self
            .cmd(
                "DOM.getNodeForLocation",
                json!({"x": x as i64, "y": y as i64, "includeUserAgentShadowDOM": false, "ignorePointerEventsNone": false}),
            )
            .await
        else {
            return Ok(false);
        };
        if hit["frameId"] != self.frame().await?["id"] {
            return Ok(false);
        }
        let Some(receiver) = hit["backendNodeId"].as_u64() else {
            return Ok(false);
        };
        if receiver == backend {
            return Ok(true);
        }
        let dom = self.document().await?;
        Ok(
            match (dom.by_backend.get(&backend), dom.by_backend.get(&receiver)) {
                (Some(&target), Some(&receiver)) => dom.contains(target, receiver),
                _ => false,
            },
        )
    }

    /// Authorize interacting with `backend` as `action`, by its current accessibility role and
    /// name, which are read again on every call. Within one operation a target whose role and
    /// name are unchanged since it was authorized is not decided again, so one approval covers
    /// all events of one operation; a relabelled target is decided anew. The page's origin is
    /// checked every time.
    async fn node_gate(
        &self,
        backend: u64,
        action: &str,
        granted: &mut Vec<(u64, String, String)>,
    ) -> OpResult<()> {
        let node = self.gate_node(backend).await?;
        let current = (backend, ax_text(&node["role"]), ax_text(&node["name"]));
        if granted.contains(&current) {
            return self.gate_page(None).await.map(drop);
        }
        self.gate_page(Some(Target {
            action,
            role: &current.1,
            name: &current.2,
        }))
        .await?;
        granted.push(current);
        Ok(())
    }

    async fn is_focused(&self, backend: u64) -> OpResult<bool> {
        Ok(matches!(
            self.property(backend, "focused").await?,
            Some(Value::Bool(true))
        ))
    }

    async fn focus_on(&self, backend: u64, description: &str) -> OpResult<()> {
        self.cmd("DOM.focus", json!({"backendNodeId": backend}))
            .await?;
        if self.is_focused(backend).await? {
            Ok(())
        } else {
            Err(OpError::new(format!(
                "could not move keyboard focus to {description}; the page moved it elsewhere"
            )))
        }
    }

    /// The top-frame node that holds keyboard focus. Keys aimed at a node inside another frame
    /// (the top frame then reports the frame element, or nothing, as focused) are refused
    /// rather than sent to a target the REPL cannot inspect.
    async fn focused(&self, hint: Option<u64>) -> OpResult<u64> {
        let backend = match hint {
            Some(backend) if self.is_focused(backend).await? => backend,
            _ => self
                .full_ax()
                .await?
                .into_iter()
                .rfind(|node| matches!(node.properties.get("focused"), Some(Value::Bool(true))))
                .map(|node| node.backend)
                .ok_or_else(|| OpError::new(FOREIGN_FOCUS))?,
        };
        let node = self
            .cmd("DOM.describeNode", json!({"backendNodeId": backend}))
            .await?;
        let name = node["node"]["nodeName"].as_str().unwrap_or_default();
        if [
            "IFRAME",
            "FRAME",
            "OBJECT",
            "EMBED",
            "PORTAL",
            "FENCEDFRAME",
        ]
        .iter()
        .any(|frame| name.eq_ignore_ascii_case(frame))
        {
            return Err(OpError::new(FOREIGN_FOCUS));
        }
        Ok(backend)
    }

    /// Keys reach whatever has focus when they are sent, so before every key event the focused
    /// node is looked up again and its current role and name are authorized (no per-node cache:
    /// the page may relabel a node between keys); Enter in a form field is also authorized as
    /// the submission of its form.
    async fn key_target(&self, keys: &mut Keys, enter: bool) -> OpResult<()> {
        let backend = self.focused(keys.focused).await?;
        keys.focused = Some(backend);
        self.node_gate(backend, "send keys to", &mut keys.granted)
            .await?;
        if enter {
            self.submission_gate(backend).await?;
        }
        Ok(())
    }

    /// Implicit submission is authorized by what submitting does (the form's submit buttons and
    /// identifying attributes), not by the label of the field Enter was pressed in.
    async fn submission_gate(&self, backend: u64) -> OpResult<()> {
        let dom = self.document().await?;
        let Some(&index) = dom.by_backend.get(&backend) else {
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
            let node = self.gate_node(dom.nodes[submitter].backend).await?;
            labels.push(ax_text(&node["name"]));
        }
        for attribute in ["aria-label", "name", "id", "action"] {
            labels.extend(dom.nodes[form].attribute(attribute).map(str::to_owned));
        }
        labels.retain(|label| !label.is_empty());
        self.gate_page(Some(Target {
            action: "submit form",
            role: "form",
            name: &labels.join(" / "),
        }))
        .await
        .map(drop)
    }

    async fn press(&self, spec: &str, keys: &mut Keys) -> OpResult<()> {
        let events = keys::chord(spec).map_err(OpError::new)?;
        let key = spec.rsplit('+').next().unwrap_or(spec);
        let enter = matches!(key, "Enter" | "NumpadEnter");
        for event in events {
            // Implicit submission is triggered by the Enter key-down, not modifiers or key-up.
            let submits = enter && event["key"] == "Enter" && event["type"] != "keyUp";
            self.key_target(keys, submits).await?;
            self.cmd("Input.dispatchKeyEvent", event).await?;
        }
        Ok(())
    }

    async fn insert(&self, text: &str, keys: &mut Keys) -> OpResult<()> {
        self.key_target(keys, false).await?;
        self.cmd("Input.insertText", json!({"text": text})).await?;
        Ok(())
    }

    async fn type_text(&self, text: &str, delay: u64, keys: &mut Keys) -> OpResult<()> {
        for character in text.chars() {
            let spec = match character {
                '\n' | '\r' => "Enter".to_owned(),
                '\t' => "Tab".to_owned(),
                other => other.to_string(),
            };
            if character.is_ascii() && keys::chord(&spec).is_ok() {
                self.press(&spec, keys).await?;
            } else {
                self.insert(&character.to_string(), keys).await?;
            }
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
        }
        Ok(())
    }

    async fn act(&self, args: Value) -> OpResult<Value> {
        let action = args["action"].as_str().unwrap_or_default().to_owned();
        let value = args["value"].clone();
        let options = args["options"].clone();
        let located = located(args)?;
        let timeout = located
            .timeout
            .map(Duration::from_millis)
            .unwrap_or(self.limits.action_timeout);
        let deadline = Instant::now() + timeout;
        let (need_visible, need_enabled) = match action.as_str() {
            "focus" => (false, false),
            "select" => (false, true),
            _ => (true, true),
        };
        let resolved = self
            .single(&located, timeout, need_visible, need_enabled)
            .await?;
        let index = resolved.found[0];
        let backend = resolved.dom.nodes[index].backend;
        let before = self.frame().await?;
        let mut keys = Keys::default();
        let mut result = Value::Null;
        match action.as_str() {
            "click" | "dblclick" | "hover" => {
                let button = options["button"].as_str().unwrap_or("left");
                let (verb, clicks) = match action.as_str() {
                    "hover" => ("hover", 0),
                    "dblclick" => ("double-click", 2),
                    _ => {
                        let clicks = options["clickCount"].as_u64().unwrap_or(1);
                        if !(1..=MAX_CLICKS).contains(&clicks) {
                            return Err(OpError::new(format!(
                                "clickCount must be between 1 and {MAX_CLICKS}"
                            )));
                        }
                        ("click", clicks)
                    }
                };
                self.pointer(&located, resolved, deadline, verb, button, clicks)
                    .await?;
            }
            "fill" => {
                self.editable(&resolved.dom, index).await?;
                self.focus_on(backend, &located.description).await?;
                self.node_gate(backend, "fill", &mut keys.granted).await?;
                keys.focused = Some(backend);
                self.press("ControlOrMeta+a", &mut keys).await?;
                match value.as_str().unwrap_or_default() {
                    "" => self.press("Delete", &mut keys).await?,
                    text => self.insert(text, &mut keys).await?,
                }
            }
            "type" | "press" => {
                self.focus_on(backend, &located.description).await?;
                keys.focused = Some(backend);
                let text = value.as_str().unwrap_or_default();
                if action == "press" {
                    self.press(text, &mut keys).await?;
                } else {
                    let delay = options["delay"].as_u64().unwrap_or_default();
                    self.type_text(text, delay, &mut keys).await?;
                }
            }
            "focus" => {
                self.node_gate(backend, "focus", &mut keys.granted).await?;
                self.cmd("DOM.focus", json!({"backendNodeId": backend}))
                    .await?;
            }
            "check" => {
                let wanted = value != false;
                if self.checked(backend).await? != wanted {
                    let verb = if wanted { "check" } else { "uncheck" };
                    self.pointer(&located, resolved, deadline, verb, "left", 1)
                        .await?;
                    let resolved = self.single(&located, timeout, false, false).await?;
                    let backend = resolved.dom.nodes[resolved.found[0]].backend;
                    if self.checked(backend).await? != wanted {
                        return Err(OpError::new(
                            "Clicking the checkbox did not change its state",
                        ));
                    }
                }
            }
            "select" => {
                self.focus_on(backend, &located.description).await?;
                keys.focused = Some(backend);
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

    async fn editable(&self, dom: &Dom, index: usize) -> OpResult<()> {
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

    /// Native `<select>` changes go through the keyboard (Home, then ArrowDown) so the page sees
    /// trusted input/change events without any script; disabled options are skipped like users do.
    async fn select(
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
        let field = ax_text(&self.gate_node(node.backend).await?["name"]);
        for &option in &passed {
            let name = format!("{field}: {}", label_of(option));
            self.gate_page(Some(Target {
                action: "select option",
                role: "option",
                name: name.trim_start_matches(": "),
            }))
            .await?;
        }
        let steps = passed.len() - 1;
        self.press("Home", keys).await?;
        for _ in 0..steps {
            self.press("ArrowDown", keys).await?;
        }
        Ok(json!([value_of(options[target])]))
    }

    /// A CDP reply can precede the navigation an input handler starts; give it a moment.
    async fn settle(&self, before: Value) -> OpResult<()> {
        let grace = Instant::now() + Duration::from_millis(250);
        loop {
            let frame = self.frame().await?;
            if frame["loaderId"] != before["loaderId"] {
                return self
                    .wait_loaded(Instant::now() + self.limits.action_timeout)
                    .await;
            }
            if Instant::now() >= grace {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    async fn wait_navigation(
        &self,
        timeout: Duration,
        committed: impl Fn(&Value) -> bool,
    ) -> OpResult<()> {
        let deadline = Instant::now() + timeout;
        loop {
            let frame = self.frame().await?;
            if committed(&frame) {
                break;
            }
            if Instant::now() >= deadline {
                return Err(OpError::timeout(format!(
                    "navigation timeout of {}ms exceeded",
                    timeout.as_millis()
                )));
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        self.wait_loaded(deadline).await
    }

    async fn metrics(&self) -> OpResult<Value> {
        if !self.metrics.get() {
            self.cmd("Performance.enable", json!({})).await?;
            self.metrics.set(true);
        }
        self.cmd("Performance.getMetrics", json!({})).await
    }

    /// Refuse before the browser serializes a whole document or accessibility tree into one
    /// reply: the renderer's live node count bounds the reply's node count up front.
    async fn ensure_page_size(&self) -> OpResult<()> {
        let metrics = self.metrics().await?;
        let nodes = metric(&metrics, "Nodes");
        if nodes > MAX_NODES as f64 {
            return Err(OpError::new(TOO_LARGE));
        }
        Ok(())
    }

    async fn full_ax(&self) -> OpResult<Vec<AxNode>> {
        self.ensure_page_size().await?;
        let tree = self.cmd("Accessibility.getFullAXTree", json!({})).await?;
        ax_nodes(&tree).map_err(OpError::new)
    }

    /// DOMContentLoaded of the current document, read from `Performance.getMetrics` instead of
    /// `document.readyState` so that no expression is evaluated in the page.
    async fn wait_loaded(&self, deadline: Instant) -> OpResult<()> {
        loop {
            let metrics = self.metrics().await?;
            let loaded = metric(&metrics, "DomContentLoaded");
            if loaded > 0.0 && loaded >= metric(&metrics, "NavigationStart") {
                self.frame().await?;
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(OpError::timeout("timeout waiting for DOMContentLoaded"));
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    async fn query(&self, args: Value) -> OpResult<Value> {
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
                [index] => self.visible(resolved.dom.nodes[*index].backend).await?,
                found => {
                    return Err(OpError::new(format!(
                        "strict mode violation: {} resolved to {} elements",
                        located.description,
                        found.len()
                    )))
                }
            };
            return Ok(json!(visible == (what == "isVisible")));
        }
        let resolved = self.single(&located, timeout, false, false).await?;
        let dom = &resolved.dom;
        let index = resolved.found[0];
        let backend = dom.nodes[index].backend;
        Ok(match what.as_str() {
            "textContent" => json!(dom.text_content(index)),
            "innerText" => json!(text_of(dom, index, true)),
            "getAttribute" => {
                json!(dom.nodes[index].attribute(name.as_deref().unwrap_or_default()))
            }
            "inputValue" => json!(self.input_value(dom, index).await?),
            "isChecked" => json!(self.checked(backend).await?),
            "isEnabled" => json!(self.enabled(dom, index).await?),
            "isDisabled" => json!(!self.enabled(dom, index).await?),
            "isEditable" => json!(
                self.enabled(dom, index).await? && dom.nodes[index].attribute("readonly").is_none()
            ),
            "isFocused" => json!(matches!(
                self.property(backend, "focused").await?,
                Some(Value::Bool(true))
            )),
            "boundingBox" => match self
                .cmd("DOM.getBoxModel", json!({"backendNodeId": backend}))
                .await
            {
                Ok(model) => {
                    let border = &model["model"]["border"];
                    json!({"x": border[0], "y": border[1], "width": model["model"]["width"], "height": model["model"]["height"]})
                }
                Err(_) => Value::Null,
            },
            other => {
                return Err(OpError {
                    name: "NotSupported",
                    message: format!("locator.{other} is not supported"),
                })
            }
        })
    }

    async fn input_value(&self, dom: &Dom, index: usize) -> OpResult<String> {
        let node = &dom.nodes[index];
        match node.name.as_str() {
            "input" | "textarea" => {
                let ax = self.ax_node(node.backend).await?;
                Ok(ax_text(&ax["value"]))
            }
            "select" => {
                let selected = self.full_ax().await?.into_iter().find_map(|ax| {
                    let option = *dom.by_backend.get(&ax.backend)?;
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

    async fn wait_for(&self, args: Value) -> OpResult<Value> {
        let state = args["state"].as_str().unwrap_or("visible").to_owned();
        let located = located(args)?;
        self.gate_page(None).await?;
        let timeout = located
            .timeout
            .map(Duration::from_millis)
            .unwrap_or(self.limits.action_timeout);
        let deadline = Instant::now() + timeout;
        loop {
            let resolved = self.resolve(&located.steps).await?;
            let done = match (state.as_str(), resolved.found.as_slice()) {
                ("attached", found) => !found.is_empty(),
                ("detached", found) => found.is_empty(),
                ("hidden", []) => true,
                ("hidden", [index]) => !self.visible(resolved.dom.nodes[*index].backend).await?,
                (_, [index]) => self.visible(resolved.dom.nodes[*index].backend).await?,
                (_, []) => false,
                (_, found) => {
                    return Err(OpError::new(format!(
                        "strict mode violation: {} resolved to {} elements",
                        located.description,
                        found.len()
                    )))
                }
            };
            if done {
                return Ok(Value::Null);
            }
            if Instant::now() >= deadline {
                return Err(OpError::timeout(format!(
                    "Timeout {}ms exceeded waiting for {} to be {state}",
                    timeout.as_millis(),
                    located.description
                )));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// `page.keyboard`: every key is authorized against the node focused when it is sent.
    async fn keyboard(&self, args: Value) -> OpResult<Value> {
        let before = self.frame().await?;
        let mut keys = Keys::default();
        let text = args["text"].as_str().unwrap_or_default();
        match args["action"].as_str() {
            Some("press") => self.press(text, &mut keys).await?,
            Some("type") => {
                let delay = args["delay"].as_u64().unwrap_or_default();
                self.type_text(text, delay, &mut keys).await?
            }
            Some("insertText") => self.insert(text, &mut keys).await?,
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

fn located(args: Value) -> OpResult<Located> {
    serde_json::from_value(args).map_err(|_| OpError::new("invalid locator"))
}

fn timeout_of(args: &Value, default: Duration) -> Duration {
    args["timeout"]
        .as_u64()
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis)
        .unwrap_or(default)
}

fn metric(metrics: &Value, name: &str) -> f64 {
    metrics["metrics"]
        .as_array()
        .and_then(|metrics| metrics.iter().find(|metric| metric["name"] == name))
        .and_then(|metric| metric["value"].as_f64())
        .unwrap_or_default()
}

/// Largest operation reply handed to the script; it is copied into the QuickJS heap next.
const MAX_REPLY_BYTES: usize = 4 * 1024 * 1024;

/// Serialization sink that fails instead of growing past [`MAX_REPLY_BYTES`].
struct Bounded(Vec<u8>);

impl std::io::Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len() + bytes.len() > MAX_REPLY_BYTES {
            return Err(std::io::ErrorKind::OutOfMemory.into());
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn text_of(dom: &Dom, index: usize, inner: bool) -> String {
    if inner {
        normalize(dom.text(index))
    } else {
        dom.text_content(index).to_owned()
    }
}

fn collect_refs(steps: &[Step], references: &mut HashMap<String, Option<u64>>) {
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
