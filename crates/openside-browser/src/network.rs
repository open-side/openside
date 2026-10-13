//! Page lifecycle and network tracking for agent tabs: which load states the tab's current
//! main-frame document reached, which requests are still in flight (for network idle), the
//! main document's response (its HTTP status, which `Page.navigate` does not report), and a
//! bounded log of requests, responses, failures and navigations for waiters.
//!
//! Tracking is opt-in per tab session ([`Browser::track_network`]): `Network.enable` makes the
//! browser stream every request of the tab over the shared websocket, which tabs that are never
//! waited on do not need. Nothing here evaluates script in the page.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use parking_lot::Mutex as SyncMutex;

use zeroize::Zeroizing;

struct RequestHeaders {
    id: String,
    url: String,
    frame: String,
    header_names: Vec<String>,
    headers: Vec<(String, Zeroizing<String>)>,
}

fn reusable_headers(value: &Value) -> Vec<(String, Zeroizing<String>)> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, value)| {
            let lower = name.to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                "cookie"
                    | "cookie2"
                    | "set-cookie"
                    | "host"
                    | "origin"
                    | "referer"
                    | "content-length"
                    | "connection"
            ) || lower.starts_with("sec-")
                || lower.starts_with(':')
            {
                return None;
            }
            Some((name.clone(), Zeroizing::new(value.as_str()?.to_owned())))
        })
        .collect()
}

fn header_names(value: &Value) -> Vec<String> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, _)| name.clone())
        .collect()
}

fn response_headers(value: &Value, requests: &VecDeque<RequestHeaders>) -> Value {
    let mut headers = value.clone();
    for (name, value) in headers.as_object_mut().into_iter().flatten() {
        if matches!(
            name.to_ascii_lowercase().as_str(),
            "set-cookie" | "set-cookie2" | "cookie" | "authorization" | "proxy-authorization"
        ) {
            *value = json!("[redacted]");
        } else if let Value::String(text) = value {
            for (_, secret) in requests.iter().flat_map(|request| &request.headers) {
                if !secret.is_empty() && text.contains(secret.as_str()) {
                    *text = text.replace(secret.as_str(), "[redacted]");
                }
            }
        }
    }
    headers
}

fn url_path(value: &str) -> Option<String> {
    Some(url::Url::parse(value).ok()?.path().to_owned())
}

fn connection_same_origin(value: &str, origin: &str) -> bool {
    url::Url::parse(value)
        .ok()
        .is_some_and(|url| url.origin().ascii_serialization() == origin)
}

const CONNECTION_FETCH_JS: &str = r#"async function(a) {
    if (location.origin !== a.origin) return {error:'connection_cross_site'};
    const u = new URL(a.url);
    if (u.origin !== a.origin) return {error:'connection_cross_site'};
    const headers = new Headers(a.headers);
    let body;
    if (a.json_body !== null && a.json_body !== undefined) {
        headers.set('content-type','application/json');
        body = JSON.stringify(a.json_body);
    }
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 15000);
    try {
        const response = await fetch(u.href, {method:a.method, headers, body,
            credentials:'include', redirect:'manual', signal:controller.signal});
        if (response.type === 'opaqueredirect' || (response.status >= 300 && response.status < 400))
            return {error:'connection_cross_site', needs_login:true};
        const reader = response.body?.getReader();
        const chunks = [];
        let count = 0;
        let truncated = false;
        if (reader) {
            while (count <= 65536) {
                const next = await reader.read();
                if (next.done) break;
                const remaining = 65536 - count;
                const chunk = next.value.subarray(0, remaining);
                chunks.push(chunk);
                count += chunk.byteLength;
                if (next.value.byteLength > remaining || count === 65536) {
                    truncated = true;
                    await reader.cancel();
                    break;
                }
            }
        }
        const bytes = new Uint8Array(count);
        let offset = 0;
        for (const chunk of chunks) { bytes.set(chunk,offset); offset += chunk.byteLength; }
        return {status:response.status, content_type:response.headers.get('content-type') || '',
            body:new TextDecoder().decode(bytes), truncated};
    } catch (_) { return {error:'connection_fetch_failed'}; }
    finally { clearTimeout(timer); }
}"#;
use super::*;

/// How many log entries a tab keeps; a waiter that falls further behind misses the oldest.
const LOG_CAP: usize = 512;
/// How many main-frame documents keep their response and stay known as "seen".
const DOCUMENT_CAP: usize = 16;

/// Which lifecycle point `goto` and `waitForLoadState` wait for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitUntil {
    /// The navigation was committed: the response headers arrived and the document replaced the
    /// old one.
    Commit,
    /// `DOMContentLoaded` of the document.
    DomContentLoaded,
    /// The `load` event of the document.
    Load,
    /// `load`, then no request in flight for the quiet period.
    NetworkIdle,
}

impl WaitUntil {
    /// Playwright's spelling: `commit`, `domcontentloaded`, `load`, `networkidle`.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "commit" => Self::Commit,
            "domcontentloaded" => Self::DomContentLoaded,
            "load" => Self::Load,
            "networkidle" => Self::NetworkIdle,
            _ => return None,
        })
    }

    /// The spelling [`WaitUntil::parse`] accepts.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Commit => "commit",
            Self::DomContentLoaded => "domcontentloaded",
            Self::Load => "load",
            Self::NetworkIdle => "networkidle",
        }
    }
}

/// Which document a load-state wait applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentWait<'a> {
    /// Whatever document is current, now or after later navigations.
    Current,
    /// The document with this loader id (`Page.navigate`'s `loaderId`), once it was committed;
    /// a later document that replaced it also satisfies the wait.
    Loader(&'a str),
    /// A document committed whose loader id is not this one: what a reload or a history move
    /// produces, given the loader id observed before the command.
    NotLoader(&'a str),
}

/// Lifecycle of a tab's current main-frame document.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadState {
    /// Loader id of the current document; `None` before tracking saw any.
    pub loader_id: Option<String>,
    /// The document was committed.
    pub commit: bool,
    /// `DOMContentLoaded` fired.
    pub dom_content_loaded: bool,
    /// `load` fired.
    pub load: bool,
}

/// What a bounded load-state wait ended with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadWait {
    /// The requested state was reached.
    Reached,
    /// The deadline passed first.
    TimedOut {
        /// State at the deadline.
        state: LoadState,
        /// `METHOD url` of requests still in flight at the deadline, oldest first.
        in_flight: Vec<String>,
    },
}

/// Response of a main-frame document request.
#[derive(Clone, Debug, PartialEq)]
pub struct DocumentResponse {
    /// Loader id the document got; equals `Page.navigate`'s `loaderId`.
    pub loader_id: String,
    /// Network request id, valid for `Network.getResponseBody`.
    pub request_id: String,
    /// Final URL of the response (after redirects).
    pub url: String,
    /// HTTP status code.
    pub status: u16,
    /// HTTP status text, empty over HTTP/2.
    pub status_text: String,
    /// Response headers as the browser reported them.
    pub headers: Value,
}

/// What a [`NetworkEvent`] describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkEventKind {
    /// A request was sent.
    Request,
    /// A response was received (redirect hops included).
    Response,
    /// A request failed or was canceled.
    Failed,
    /// The main frame navigated, in the same or to a new document.
    Navigated,
}

/// One entry of a tab's network log. Fields that do not apply to the kind are empty.
#[derive(Clone, Debug, PartialEq)]
pub struct NetworkEvent {
    /// Position in the log; strictly increasing per tab session.
    pub seq: u64,
    /// When the controller saw the event, in milliseconds since the Unix epoch. Lets a waiter
    /// that started before it could read the log tell its own events from older ones.
    pub at_ms: u64,
    /// What happened.
    pub kind: NetworkEventKind,
    /// Network request id; empty for [`NetworkEventKind::Navigated`].
    pub request_id: String,
    /// Request URL, or the document URL for a navigation.
    pub url: String,
    /// Request method.
    pub method: String,
    /// CDP resource type, such as `Document`, `XHR` or `Fetch`.
    pub resource_type: String,
    /// Frame that issued the request.
    pub frame_id: String,
    /// HTTP status of a response.
    pub status: u16,
    /// HTTP status text of a response.
    pub status_text: String,
    /// Response headers; `null` otherwise.
    pub headers: Value,
    /// Chromium's error text of a failed request.
    pub error_text: String,
}

/// Log entries after a cursor.
#[derive(Clone, Debug, PartialEq)]
pub struct NetworkEvents {
    /// Entries with a sequence number above the requested cursor, oldest first.
    pub events: Vec<NetworkEvent>,
    /// Pass this as the next `after`.
    pub cursor: u64,
}

struct InFlight {
    method: String,
    url: String,
    resource_type: String,
    frame_id: String,
}

struct Page {
    main_frame: String,
    state: LoadState,
    load_at: Option<Instant>,
    recent_loaders: VecDeque<String>,
    documents: VecDeque<DocumentResponse>,
    in_flight: HashMap<String, InFlight>,
    /// Insertion order of `in_flight`, for stable diagnostics.
    order: VecDeque<String>,
    /// Last request start or end, or when tracking began.
    activity: Instant,
    log: VecDeque<NetworkEvent>,
    request_headers: VecDeque<RequestHeaders>,
    next_seq: u64,
}

impl Page {
    fn new(main_frame: &str) -> Self {
        Self {
            main_frame: main_frame.to_owned(),
            state: LoadState::default(),
            load_at: None,
            recent_loaders: VecDeque::new(),
            documents: VecDeque::new(),
            in_flight: HashMap::new(),
            order: VecDeque::new(),
            activity: Instant::now(),
            log: VecDeque::new(),
            request_headers: VecDeque::new(),
            next_seq: 1,
        }
    }

    fn push(&mut self, kind: NetworkEventKind, fill: impl FnOnce(&mut NetworkEvent)) {
        let mut event = NetworkEvent {
            seq: self.next_seq,
            at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis() as u64),
            kind,
            request_id: String::new(),
            url: String::new(),
            method: String::new(),
            resource_type: String::new(),
            frame_id: String::new(),
            status: 0,
            status_text: String::new(),
            headers: Value::Null,
            error_text: String::new(),
        };
        fill(&mut event);
        self.next_seq += 1;
        self.log.push_back(event);
        if self.log.len() > LOG_CAP {
            self.log.pop_front();
        }
    }

    /// A main-frame document with this loader id is committed (or later). A new one resets the
    /// lifecycle and forgets the sub-resource requests of the document it replaced, whose end
    /// events may never arrive; the new document's own request stays tracked. `restored` is a
    /// commit of a loader seen before: a page brought back from the back/forward cache keeps
    /// its loader id and fires no load events again, so it counts as fully loaded.
    fn document(&mut self, loader: &str, committed: bool) {
        if self.state.loader_id.as_deref() == Some(loader) {
            self.state.commit = true;
            return;
        }
        let restored = self.recent_loaders.iter().any(|seen| seen == loader);
        if restored && !committed {
            // A late lifecycle event of a document that was already replaced.
            return;
        }
        self.state = LoadState {
            loader_id: Some(loader.to_owned()),
            commit: true,
            dom_content_loaded: restored,
            load: restored,
        };
        self.load_at = restored.then(Instant::now);
        let main = self.main_frame.clone();
        self.in_flight
            .retain(|_, request| request.resource_type == "Document" && request.frame_id == main);
        let kept = &self.in_flight;
        self.order.retain(|id| kept.contains_key(id));
        self.activity = Instant::now();
        if !restored {
            self.recent_loaders.push_back(loader.to_owned());
            if self.recent_loaders.len() > DOCUMENT_CAP {
                self.recent_loaders.pop_front();
            }
        }
    }

    fn started(&mut self, params: &Value) {
        let id = params["requestId"].as_str().unwrap_or_default();
        let url = params["request"]["url"].as_str().unwrap_or_default();
        if url.starts_with("data:") {
            return;
        }
        let method = params["request"]["method"].as_str().unwrap_or_default();
        let resource_type = params["type"].as_str().unwrap_or_default();
        let frame = params["frameId"].as_str().unwrap_or_default();
        self.request_headers.retain(|request| request.id != id);
        self.request_headers.push_back(RequestHeaders {
            id: id.into(),
            url: url.into(),
            frame: frame.into(),
            header_names: header_names(&params["request"]["headers"]),
            headers: reusable_headers(&params["request"]["headers"]),
        });
        if self.request_headers.len() > LOG_CAP {
            self.request_headers.pop_front();
        }
        if params.get("redirectResponse").is_some() {
            let hop = &params["redirectResponse"];
            let headers = response_headers(&hop["headers"], &self.request_headers);
            self.push(NetworkEventKind::Response, |event| {
                event.request_id = id.to_owned();
                event.url = hop["url"].as_str().unwrap_or_default().to_owned();
                event.method = method.to_owned();
                event.resource_type = resource_type.to_owned();
                event.frame_id = frame.to_owned();
                event.status = hop["status"].as_u64().unwrap_or_default() as u16;
                event.status_text = hop["statusText"].as_str().unwrap_or_default().to_owned();
                event.headers = headers;
            });
        }
        if self
            .in_flight
            .insert(
                id.to_owned(),
                InFlight {
                    method: method.to_owned(),
                    url: url.to_owned(),
                    resource_type: resource_type.to_owned(),
                    frame_id: frame.to_owned(),
                },
            )
            .is_none()
        {
            self.order.push_back(id.to_owned());
        }
        self.activity = Instant::now();
        self.push(NetworkEventKind::Request, |event| {
            event.request_id = id.to_owned();
            event.url = url.to_owned();
            event.method = method.to_owned();
            event.resource_type = resource_type.to_owned();
            event.frame_id = frame.to_owned();
        });
    }

    fn responded(&mut self, params: &Value) {
        let id = params["requestId"].as_str().unwrap_or_default();
        let response = &params["response"];
        let url = response["url"].as_str().unwrap_or_default();
        if url.starts_with("data:") {
            return;
        }
        let method = self
            .in_flight
            .get(id)
            .map(|request| request.method.clone())
            .unwrap_or_default();
        let resource_type = params["type"].as_str().unwrap_or_default();
        let frame = params["frameId"].as_str().unwrap_or_default();
        let status = response["status"].as_u64().unwrap_or_default() as u16;
        let status_text = response["statusText"].as_str().unwrap_or_default();
        let headers = response_headers(&response["headers"], &self.request_headers);
        if resource_type == "Document" && frame == self.main_frame {
            if let Some(loader) = params["loaderId"].as_str() {
                self.documents.push_back(DocumentResponse {
                    loader_id: loader.to_owned(),
                    request_id: id.to_owned(),
                    url: url.to_owned(),
                    status,
                    status_text: status_text.to_owned(),
                    headers: headers.clone(),
                });
                if self.documents.len() > DOCUMENT_CAP {
                    self.documents.pop_front();
                }
            }
        }
        self.push(NetworkEventKind::Response, |event| {
            event.request_id = id.to_owned();
            event.url = url.to_owned();
            event.method = method;
            event.resource_type = resource_type.to_owned();
            event.frame_id = frame.to_owned();
            event.status = status;
            event.status_text = status_text.to_owned();
            event.headers = headers;
        });
    }

    fn finished(&mut self, params: &Value, failed: bool) {
        let id = params["requestId"].as_str().unwrap_or_default();
        let request = self.in_flight.remove(id);
        if request.is_some() {
            self.order.retain(|queued| queued != id);
            self.activity = Instant::now();
        }
        if failed {
            let request = request.unwrap_or(InFlight {
                method: String::new(),
                url: String::new(),
                resource_type: params["type"].as_str().unwrap_or_default().to_owned(),
                frame_id: String::new(),
            });
            self.push(NetworkEventKind::Failed, |event| {
                event.request_id = id.to_owned();
                event.url = request.url;
                event.method = request.method;
                event.resource_type = request.resource_type;
                event.frame_id = request.frame_id;
                event.error_text = params["errorText"].as_str().unwrap_or_default().to_owned();
            });
        }
    }

    fn navigated(&mut self, url: &str) {
        self.push(NetworkEventKind::Navigated, |event| {
            event.url = url.to_owned();
        });
    }

    fn observe(&mut self, method: &str, params: &Value) {
        match method {
            "Page.lifecycleEvent" if params["frameId"] == self.main_frame.as_str() => {
                let Some(loader) = params["loaderId"]
                    .as_str()
                    .filter(|loader| !loader.is_empty())
                else {
                    return;
                };
                // `init` precedes the commit and carries the loader of a navigation that may
                // still fail, so it says nothing about the current document.
                if params["name"] == "init" {
                    return;
                }
                self.document(loader, false);
                match params["name"].as_str() {
                    Some("DOMContentLoaded") => self.state.dom_content_loaded = true,
                    Some("load") => {
                        self.state.dom_content_loaded = true;
                        self.state.load = true;
                        self.load_at = Some(Instant::now());
                    }
                    _ => {}
                }
            }
            "Page.frameNavigated"
                if params["frame"]["id"] == self.main_frame.as_str()
                    && params["frame"].get("parentId").is_none() =>
            {
                if let Some(loader) = params["frame"]["loaderId"].as_str() {
                    self.document(loader, true);
                }
                self.navigated(params["frame"]["url"].as_str().unwrap_or_default());
            }
            "Page.navigatedWithinDocument" if params["frameId"] == self.main_frame.as_str() => {
                self.navigated(params["url"].as_str().unwrap_or_default());
            }
            "Network.requestWillBeSent" => self.started(params),
            "Network.requestWillBeSentExtraInfo" => {
                if let Some(request) = self
                    .request_headers
                    .iter_mut()
                    .find(|request| request.id == params["requestId"])
                {
                    request.headers = reusable_headers(&params["headers"]);
                    request.header_names = header_names(&params["headers"]);
                }
            }
            "Network.responseReceived" => self.responded(params),
            "Network.loadingFinished" => self.finished(params, false),
            "Network.loadingFailed" => self.finished(params, true),
            _ => {}
        }
    }

    fn reached(&self, until: WaitUntil, document: DocumentWait<'_>, quiet: Duration) -> bool {
        let committed = match document {
            DocumentWait::Current => self.state.commit,
            DocumentWait::Loader(loader) => self.recent_loaders.iter().any(|seen| seen == loader),
            DocumentWait::NotLoader(loader) => {
                self.state.commit && self.state.loader_id.as_deref() != Some(loader)
            }
        };
        if !committed {
            return false;
        }
        match until {
            WaitUntil::Commit => true,
            WaitUntil::DomContentLoaded => self.state.dom_content_loaded,
            WaitUntil::Load => self.state.load,
            WaitUntil::NetworkIdle => {
                let since = self
                    .load_at
                    .map_or(self.activity, |load| load.max(self.activity));
                self.state.load && self.in_flight.is_empty() && since.elapsed() >= quiet
            }
        }
    }

    fn pending(&self) -> Vec<String> {
        self.order
            .iter()
            .filter_map(|id| self.in_flight.get(id))
            .map(|request| format!("{} {}", request.method, request.url))
            .collect()
    }
}

struct ConnectionFence {
    frame: String,
    origin: String,
    urls: Vec<String>,
}

/// Tracked tab sessions, fed by the controller's event task.
#[derive(Default)]
pub(super) struct Tracker {
    pages: SyncMutex<HashMap<String, Page>>,
    /// Out-of-process iframe session → its tab's page session, whose requests it adds to.
    frames: SyncMutex<HashMap<String, String>>,
    connection_fences: SyncMutex<HashMap<String, ConnectionFence>>,
    pub(super) service_worker_bypass: SyncMutex<HashMap<String, bool>>,
    pub(super) document_policy: std::sync::RwLock<Option<DocumentCheck>>,
    pub(super) document_denials: SyncMutex<HashMap<String, DocumentPermission>>,
    pub(super) connection_restrictions: std::sync::RwLock<Option<ConnectionRestrictions>>,
    pub(super) request_modes: SyncMutex<HashMap<String, bool>>,
    pub(super) request_epochs: SyncMutex<HashMap<String, requests::DocumentEpoch>>,
    pub(super) worker_request_targets: SyncMutex<HashMap<String, String>>,
    pub(super) worker_request_order: SyncMutex<VecDeque<String>>,
    pub(super) worker_fence_active: std::sync::atomic::AtomicBool,
    pub(super) worker_fence_gate: tokio::sync::Mutex<()>,
}

impl Tracker {
    pub(super) fn has_document_policy(&self) -> bool {
        self.document_policy
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }

    pub(super) fn document_permission(&self, request: &DocumentRequest) -> DocumentPermission {
        let policy = self
            .document_policy
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        policy.map_or(DocumentPermission::Allowed, |policy| policy(request))
    }

    pub(super) fn document_denial(&self, tab: &str) -> Option<BrowserError> {
        self.document_denials
            .lock()
            .get(tab)?
            .clone()
            .into_error(tab.into())
    }

    pub(super) fn take_document_denial(&self, tab: &str) -> Option<BrowserError> {
        self.document_denials
            .lock()
            .remove(tab)?
            .into_error(tab.into())
    }
    pub(super) fn connection_request_allowed(
        &self,
        session: Option<&str>,
        frame: &str,
        params: &Value,
    ) -> Option<bool> {
        let mut fences = self.connection_fences.lock();
        let fence = if let Some(session) = session.filter(|session| fences.contains_key(*session)) {
            fences.get_mut(session)?
        } else {
            fences.values_mut().find(|fence| fence.frame == frame)?
        };
        let url = params["request"]["url"].as_str().unwrap_or_default();
        if !connection_same_origin(url, &fence.origin) {
            return Some(false);
        }
        if fence.urls.len() < LOG_CAP && !fence.urls.iter().any(|known| known == url) {
            fence.urls.push(url.into());
        }
        if params["responseStatusCode"]
            .as_u64()
            .is_some_and(|status| (300..400).contains(&status))
        {
            if let Some(location) = params["responseHeaders"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|header| {
                    header["name"]
                        .as_str()
                        .is_some_and(|name| name.eq_ignore_ascii_case("location"))
                })
                .and_then(|header| header["value"].as_str())
                .and_then(|location| url::Url::parse(url).ok()?.join(location).ok())
                .filter(|target| target.origin().ascii_serialization() == fence.origin)
            {
                let location = location.to_string();
                if fence.urls.len() < LOG_CAP && !fence.urls.contains(&location) {
                    fence.urls.push(location);
                }
            }
        }
        Some(true)
    }

    pub(super) fn set_service_worker_bypass(&self, session: &str, bypass: bool) {
        self.service_worker_bypass
            .lock()
            .insert(session.into(), bypass);
    }
    fn tracked(&self, session: &str) -> bool {
        self.pages.lock().contains_key(session)
    }

    /// Start tracking before the domains are enabled, so replayed lifecycle events are seen.
    fn begin(&self, session: &str, main_frame: &str) -> bool {
        let mut pages = self.pages.lock();
        if pages.contains_key(session) {
            return false;
        }
        pages.insert(session.to_owned(), Page::new(main_frame));
        true
    }

    fn end(&self, session: &str) {
        if let Some(page) = self.pages.lock().remove(session) {
            self.document_denials.lock().remove(&page.main_frame);
        }
        self.connection_fences.lock().remove(session);
        self.service_worker_bypass.lock().remove(session);
        self.request_modes.lock().remove(session);
        self.frames
            .lock()
            .retain(|child, page| child != session && page != session);
    }

    /// Count requests of `child`, auto-attached by `parent`, toward the tab `parent` belongs to
    /// (`parent` is a tracked page session or an iframe already counted for one).
    pub(super) fn adopt(&self, child: &str, parent: &str) -> bool {
        let pages = self.pages.lock();
        let mut frames = self.frames.lock();
        let page = if pages.contains_key(parent) {
            parent.to_owned()
        } else if let Some(page) = frames.get(parent) {
            page.clone()
        } else {
            return false;
        };
        frames.insert(child.to_owned(), page);
        true
    }

    fn with<T>(&self, session: &str, read: impl FnOnce(&Page) -> T) -> Result<T, BrowserError> {
        self.pages
            .lock()
            .get(session)
            .map(read)
            .ok_or_else(|| BrowserError::TabNotReady("network tracking ended".to_owned()))
    }

    pub(super) fn loaded(&self, session: &str, loader: Option<&str>) -> Result<bool, BrowserError> {
        self.with(session, |page| {
            page.state.load && page.state.loader_id.as_deref() == loader
        })
    }

    pub(super) fn document_loader(&self, session: &str) -> Result<Option<String>, BrowserError> {
        self.with(session, |page| page.state.loader_id.clone())
    }

    pub(super) fn document_is(
        &self,
        session: &str,
        loader: Option<&str>,
    ) -> Result<bool, BrowserError> {
        self.with(session, |page| page.state.loader_id.as_deref() == loader)
    }

    /// Feed one CDP event; cheap and non-blocking, called for every event the browser emits.
    pub(super) fn observe(&self, event: &Value) {
        let method = event["method"].as_str().unwrap_or_default();
        if method == "Target.detachedFromTarget" {
            if let Some(session) = event["params"]["sessionId"].as_str() {
                self.end(session);
            }
            return;
        }
        if !(method.starts_with("Network.") || method.starts_with("Page.")) {
            return;
        }
        let Some(session) = event["sessionId"].as_str() else {
            return;
        };
        let mut pages = self.pages.lock();
        if let Some(page) = pages.get_mut(session) {
            page.observe(method, &event["params"]);
        } else if method.starts_with("Network.") {
            // An iframe's lifecycle events describe its own document, so only its requests count.
            if let Some(page) = self
                .frames
                .lock()
                .get(session)
                .and_then(|page| pages.get_mut(page))
            {
                page.observe(method, &event["params"]);
            }
        }
    }
}

const POLL: Duration = Duration::from_millis(20);

impl Browser {
    /// Start tracking lifecycle and network events of an owned tab (idempotent). Requests that
    /// were already in flight when tracking began are unknown, so a network-idle wait is only
    /// exact from the first tracked navigation on.
    pub async fn track_network(&self, owner: &str, tab_id: &str) -> Result<(), BrowserError> {
        let session = self.owned_session(owner, tab_id).await?;
        self.track_session(&session).await
    }

    pub(super) async fn track_session(&self, session: &str) -> Result<(), BrowserError> {
        if self.inner.network.tracked(session) {
            return Ok(());
        }
        let frame = self.top_frame(session).await?;
        if !self
            .inner
            .network
            .begin(session, required_string(&frame, "id")?)
        {
            return Ok(());
        }
        let enabled = async {
            self.inner
                .cdp
                .call(Some(session), "Network.enable", json!({}))
                .await?;
            self.inner
                .cdp
                .call(
                    Some(session),
                    "Page.setLifecycleEventsEnabled",
                    json!({"enabled": true}),
                )
                .await?;
            // Out-of-process iframes are held before their first request until their own
            // network tracking is on (see `frames::adopt`), so networkidle misses none of them.
            self.inner
                .cdp
                .call(
                    Some(session),
                    "Target.setAutoAttach",
                    json!({"autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true}),
                )
                .await
        }
        .await;
        if let Err(error) = enabled {
            self.inner.network.end(session);
            return Err(error);
        }
        Ok(())
    }

    /// Lifecycle state of an owned tab's current main-frame document.
    pub async fn load_state(&self, owner: &str, tab_id: &str) -> Result<LoadState, BrowserError> {
        let session = self.owned_session(owner, tab_id).await?;
        self.track_session(&session).await?;
        self.inner.network.with(&session, |page| page.state.clone())
    }

    /// Wait until the document selected by `document` reached `until`, or `deadline` passes.
    /// `quiet` is the idle window of [`WaitUntil::NetworkIdle`]. No browser-wide lock is held:
    /// the wait only reads tracked state, so a page that never settles blocks nobody else.
    pub async fn wait_load_state(
        &self,
        owner: &str,
        tab_id: &str,
        until: WaitUntil,
        document: DocumentWait<'_>,
        quiet: Duration,
        deadline: Instant,
    ) -> Result<LoadWait, BrowserError> {
        let session = self.owned_session(owner, tab_id).await?;
        self.track_session(&session).await?;
        loop {
            let reached = self
                .inner
                .network
                .with(&session, |page| page.reached(until, document, quiet))?;
            if reached {
                return Ok(LoadWait::Reached);
            }
            if Instant::now() >= deadline {
                return self
                    .inner
                    .network
                    .with(&session, |page| LoadWait::TimedOut {
                        state: page.state.clone(),
                        in_flight: page.pending(),
                    });
            }
            tokio::time::sleep(POLL).await;
        }
    }

    /// Response of the main-frame document that got `loader_id`, if tracking saw it.
    pub async fn document_response(
        &self,
        owner: &str,
        tab_id: &str,
        loader_id: &str,
    ) -> Result<Option<DocumentResponse>, BrowserError> {
        let session = self.owned_session(owner, tab_id).await?;
        self.inner.network.with(&session, |page| {
            page.documents
                .iter()
                .rev()
                .find(|document| document.loader_id == loader_id)
                .cloned()
        })
    }

    /// Log entries after `after` (0 for all kept entries). Starts tracking when needed; the
    /// returned cursor of a first call is where a later call should continue.
    pub async fn network_events(
        &self,
        owner: &str,
        tab_id: &str,
        after: u64,
    ) -> Result<NetworkEvents, BrowserError> {
        let session = self.owned_session(owner, tab_id).await?;
        self.track_session(&session).await?;
        self.inner.network.with(&session, |page| NetworkEvents {
            events: page
                .log
                .iter()
                .filter(|event| event.seq > after)
                .cloned()
                .collect(),
            cursor: page.next_seq - 1,
        })
    }

    /// Metadata only; the private header cache is never part of NetworkEvent or diagnostics.
    pub async fn connection_requests(
        &self,
        owner: &str,
        tab: &str,
    ) -> Result<Vec<Value>, BrowserError> {
        let session = self.owned_session(owner, tab).await?;
        self.track_session(&session).await?;
        self.inner.network.with(&session, |page| {
            page.request_headers.iter().filter_map(|request| {
                let event = page.log.iter().find(|event| event.request_id == request.id
                    && event.kind == NetworkEventKind::Request && matches!(event.resource_type.as_str(), "XHR" | "Fetch"))?;
                let response = page.log.iter().rev().find(|event| event.request_id == request.id && event.kind == NetworkEventKind::Response);
                let path = url_path(&request.url)?;
                Some(json!({"request_id":request.id, "method":event.method, "path":path,
                    "status":response.map(|event| event.status),
                    "content_type":response.and_then(|event| event.headers.as_object()?.iter()
                        .find(|(name, _)| name.eq_ignore_ascii_case("content-type")).and_then(|(_, value)| value.as_str())),
                    "header_names":request.header_names}))
            }).collect()
        })
    }

    /// Isolated globals and a pre-request origin fence keep injected headers away from page
    /// overrides and service workers. Cookie guards use exact request and observed hop URLs.
    pub async fn connection_fetch(
        &self,
        owner: &str,
        tab: &str,
        origin: &str,
        mut request: Value,
        reuse: Option<&str>,
        mut redact: impl FnMut(&str, bool),
    ) -> Result<Value, BrowserError> {
        let _interaction = self.inner.interaction_lock.lock().await;
        let session = self.owned_session(owner, tab).await?;
        let frame = self.top_frame(&session).await?;
        if !connection_same_origin(frame["url"].as_str().unwrap_or_default(), origin) {
            return Err(BrowserError::OriginMismatch);
        }
        let mut target = url::Url::parse(origin)
            .ok()
            .and_then(|base| base.join(request["path"].as_str()?).ok())
            .filter(|target| target.origin().ascii_serialization() == origin)
            .ok_or(BrowserError::OriginMismatch)?;
        if let Some(query) = request["query"].as_object() {
            for (key, value) in query {
                target.query_pairs_mut().append_pair(
                    key,
                    value
                        .as_str()
                        .ok_or(BrowserError::InvalidAction("invalid connection query"))?,
                );
            }
        }
        let target = target.to_string();
        let headers = if let Some(id) = reuse {
            self.inner
                .network
                .with(&session, |page| {
                    page.request_headers
                        .iter()
                        .find(|request| {
                            request.id == id
                                && request.frame == page.main_frame
                                && connection_same_origin(&request.url, origin)
                        })
                        .map(|request| {
                            request
                                .headers
                                .iter()
                                .map(|(name, value)| (name.clone(), value.clone()))
                                .collect::<Vec<_>>()
                        })
                })?
                .ok_or(BrowserError::InvalidAction("unknown same-origin request"))?
        } else {
            Vec::new()
        };
        for (_, value) in &headers {
            redact(value, false);
        }
        {
            let mut fences = self.inner.network.connection_fences.lock();
            if fences.contains_key(&session) {
                return Err(BrowserError::InvalidAction(
                    "connection request fence unavailable",
                ));
            }
            fences.insert(
                session.clone(),
                ConnectionFence {
                    frame: required_string(&frame, "id")?.into(),
                    origin: origin.into(),
                    urls: vec![target.clone()],
                },
            );
        }
        let bypass = self.inner.network.effective_bypass(&session);
        let result = async {
            self.inner.cdp.call(Some(&session), "Network.setBypassServiceWorker", json!({"bypass":true})).await?;
            self.inner.cdp.call(Some(&session), "Fetch.enable", json!({"patterns":[
                {"urlPattern":"*","requestStage":"Request"},
                {"urlPattern":"*","requestStage":"Response"},
            ]})).await?;
            let world = self.inner.cdp.call(Some(&session), "Page.createIsolatedWorld",
                json!({"frameId":frame["id"],"worldName":format!("openside-connection-{:016x}",rand::random::<u64>()),"grantUniveralAccess":false})).await?;
            let context = world["executionContextId"].as_i64().ok_or(BrowserError::InvalidResponse("connection isolated world unavailable"))?;
            self.connection_cookie_redactors(&session, std::slice::from_ref(&target), &mut |value|redact(value,true)).await?;
            request["url"] = json!(target);
            request["origin"] = json!(origin);
            request["headers"] = json!(headers.iter().map(|(name, value)| (name, value.as_str())).collect::<HashMap<_, _>>());
            let expression = Zeroizing::new(format!("({CONNECTION_FETCH_JS})({})", request));
            request["headers"] = Value::Null;
            let result = self.inner.cdp.call_sensitive(Some(&session), "Runtime.evaluate",
                json!({"expression":expression.as_str(),"contextId":context,"returnByValue":true,"awaitPromise":true})).await;
            let urls = self.inner.network.connection_fences.lock().get(&session)
                .map(|fence| fence.urls.clone()).ok_or(BrowserError::InvalidResponse("connection request fence ended"))?;
            self.connection_cookie_redactors(&session, &urls, &mut |value|redact(value,true)).await?;
            let result = result.map_err(|_| BrowserError::InvalidResponse("connection fetch unavailable"))?;
            if result.get("exceptionDetails").is_some() {
                return Err(BrowserError::InvalidResponse("connection fetch failed"));
            }
            Ok(result["result"]["value"].clone())
        }.await;
        let disabled = self.restore_connection_requests(&session).await;
        let restored = self
            .inner
            .cdp
            .call(
                Some(&session),
                "Network.setBypassServiceWorker",
                json!({"bypass":bypass}),
            )
            .await;
        // A failed cleanup keeps the origin fence fail-closed until the tab detaches.
        if disabled.is_ok() {
            self.inner.network.connection_fences.lock().remove(&session);
        }
        if disabled.is_err() || restored.is_err() {
            return Err(BrowserError::InvalidResponse(
                "connection request cleanup unavailable",
            ));
        }
        result
    }

    async fn connection_cookie_redactors(
        &self,
        session: &str,
        urls: &[String],
        redact: &mut impl FnMut(&str),
    ) -> Result<(), BrowserError> {
        let mut cookies = self
            .inner
            .cdp
            .call_sensitive(Some(session), "Network.getCookies", json!({"urls":urls}))
            .await
            .map_err(|_| BrowserError::InvalidResponse("connection cookie guard unavailable"))?;
        for cookie in cookies["cookies"].as_array_mut().into_iter().flatten() {
            if let Value::String(value) = cookie["value"].take() {
                let value = Zeroizing::new(value);
                redact(&value);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lifecycle(page: &mut Page, loader: &str, name: &str) {
        page.observe(
            "Page.lifecycleEvent",
            &json!({"frameId": "main", "loaderId": loader, "name": name}),
        );
    }

    fn navigated(page: &mut Page, loader: &str) {
        page.observe(
            "Page.frameNavigated",
            &json!({"frame": {"id": "main", "loaderId": loader, "url": "http://x/"}}),
        );
    }

    fn request(page: &mut Page, id: &str, frame: &str, kind: &str) {
        page.observe(
            "Network.requestWillBeSent",
            &json!({"requestId": id, "frameId": frame, "type": kind,
                    "request": {"url": format!("http://x/{id}"), "method": "GET"}}),
        );
    }

    #[test]
    fn captured_document_identity_tracks_top_frame_navigation_only() {
        let tracker = Tracker::default();
        assert!(tracker.begin("session", "main"));
        assert!(tracker.document_is("session", None).unwrap());
        assert_eq!(tracker.document_loader("session").unwrap(), None);
        for loader in ["a", "b"] {
            tracker.observe(&json!({
                "method": "Page.frameNavigated", "sessionId": "session",
                "params": {"frame": {"id": "main", "loaderId": loader, "url": "http://x/"}}
            }));
            assert!(tracker.document_is("session", Some(loader)).unwrap());
            assert_eq!(
                tracker.document_loader("session").unwrap().as_deref(),
                Some(loader)
            );
            if loader == "b" {
                assert!(!tracker.document_is("session", Some("a")).unwrap());
            }
            tracker.observe(&json!({
                "method": "Page.navigatedWithinDocument", "sessionId": "session",
                "params": {"frameId": "main", "url": "http://x/#history"}
            }));
            tracker.observe(&json!({
                "method": "Page.frameNavigated", "sessionId": "session",
                "params": {"frame": {"id": "child", "parentId": "main", "loaderId": "child"}}
            }));
            assert!(tracker.document_is("session", Some(loader)).unwrap());
        }
        tracker.end("session");
        assert!(tracker.document_is("session", Some("b")).is_err());
        assert!(tracker.document_loader("session").is_err());
    }

    #[test]
    fn a_new_document_forgets_the_sub_resources_of_the_old_one() {
        let mut page = Page::new("main");
        navigated(&mut page, "a");
        request(&mut page, "img", "main", "Image");
        request(&mut page, "doc", "main", "Document");
        navigated(&mut page, "b");
        assert_eq!(page.pending(), ["GET http://x/doc"]);
        assert!(page.state.commit && !page.state.load);
    }

    #[test]
    fn init_events_do_not_replace_the_current_document() {
        let mut page = Page::new("main");
        navigated(&mut page, "a");
        lifecycle(&mut page, "a", "load");
        lifecycle(&mut page, "b", "init");
        assert_eq!(page.state.loader_id.as_deref(), Some("a"));
        assert!(page.state.load);
    }

    #[test]
    fn a_restored_document_counts_as_loaded_and_late_events_of_old_ones_are_ignored() {
        let mut page = Page::new("main");
        navigated(&mut page, "a");
        lifecycle(&mut page, "a", "load");
        navigated(&mut page, "b");
        lifecycle(&mut page, "a", "networkIdle");
        assert_eq!(page.state.loader_id.as_deref(), Some("b"));
        navigated(&mut page, "a");
        assert!(page.state.load && page.state.dom_content_loaded);
        assert!(page.reached(WaitUntil::Load, DocumentWait::Loader("a"), Duration::ZERO));
    }

    #[test]
    fn network_idle_needs_load_no_requests_and_a_quiet_window() {
        let quiet = Duration::from_millis(50);
        let mut page = Page::new("main");
        navigated(&mut page, "a");
        request(&mut page, "xhr", "main", "XHR");
        lifecycle(&mut page, "a", "load");
        assert!(!page.reached(WaitUntil::NetworkIdle, DocumentWait::Current, quiet));
        page.observe("Network.loadingFinished", &json!({"requestId": "xhr"}));
        assert!(!page.reached(WaitUntil::NetworkIdle, DocumentWait::Current, quiet));
        std::thread::sleep(Duration::from_millis(70));
        assert!(page.reached(WaitUntil::NetworkIdle, DocumentWait::Current, quiet));
    }

    #[test]
    fn waits_for_other_documents_follow_the_loader() {
        let mut page = Page::new("main");
        navigated(&mut page, "a");
        lifecycle(&mut page, "a", "load");
        assert!(!page.reached(WaitUntil::Load, DocumentWait::Loader("b"), Duration::ZERO));
        assert!(!page.reached(
            WaitUntil::Load,
            DocumentWait::NotLoader("a"),
            Duration::ZERO
        ));
        navigated(&mut page, "b");
        assert!(page.reached(WaitUntil::Commit, DocumentWait::Loader("b"), Duration::ZERO));
        assert!(!page.reached(
            WaitUntil::Load,
            DocumentWait::NotLoader("a"),
            Duration::ZERO
        ));
        lifecycle(&mut page, "b", "load");
        assert!(page.reached(
            WaitUntil::Load,
            DocumentWait::NotLoader("a"),
            Duration::ZERO
        ));
    }

    #[test]
    fn the_log_is_bounded_and_cursors_only_move_forward() {
        let mut page = Page::new("main");
        for n in 0..(LOG_CAP + 10) {
            request(&mut page, &format!("r{n}"), "main", "XHR");
        }
        assert_eq!(page.log.len(), LOG_CAP);
        assert_eq!(page.log.front().map(|event| event.seq), Some(11));
        assert_eq!(page.next_seq, (LOG_CAP + 11) as u64);
    }

    #[test]
    fn a_response_of_the_main_document_is_kept_by_loader() {
        let mut page = Page::new("main");
        request(&mut page, "doc", "main", "Document");
        page.observe(
            "Network.responseReceived",
            &json!({"requestId": "doc", "frameId": "main", "loaderId": "a", "type": "Document",
                    "response": {"url": "http://x/doc", "status": 404, "statusText": "Not Found",
                                 "headers": {"X-A": "1"}}}),
        );
        let document = page.documents.back().unwrap();
        assert_eq!((document.loader_id.as_str(), document.status), ("a", 404));
    }

    #[test]
    fn connection_header_cache_is_private_bounded_and_drops_cookie_values() {
        let mut page = Page::new("main");
        for index in 0..=LOG_CAP {
            page.started(&json!({"requestId":index.to_string(),"frameId":"main","type":"Fetch",
                "request":{"url":"https://example.com/request?private=query","method":"GET",
                    "headers":{"x-csrf":"HEADER_CANARY_PRIVATE","Cookie":"session=COOKIE_CANARY_PRIVATE"}}}));
        }
        assert_eq!(page.request_headers.len(), LOG_CAP);
        assert!(!page.request_headers.iter().any(|request| request.id == "0"));
        assert_eq!(page.request_headers.back().unwrap().headers.len(), 1);
        assert!(page
            .request_headers
            .back()
            .unwrap()
            .header_names
            .iter()
            .any(|name| name == "Cookie"));
        assert!(!format!("{:?}", page.log).contains("HEADER_CANARY_PRIVATE"));
        assert!(!format!("{:?}", page.log).contains("COOKIE_CANARY_PRIVATE"));
        page.responded(&json!({"requestId":LOG_CAP.to_string(),"frameId":"main","type":"Fetch",
            "response":{"url":"https://example.com/request","status":200,"headers":{"Set-Cookie":"session=COOKIE_CANARY_PRIVATE","x-echoed-csrf":"HEADER_CANARY_PRIVATE","content-type":"application/json"}}}));
        assert!(!format!("{:?}", page.log).contains("COOKIE_CANARY_PRIVATE"));
        assert!(!format!("{:?}", page.log).contains("HEADER_CANARY_PRIVATE"));
        assert_eq!(
            url_path("https://example.com/request?private=query#secret").as_deref(),
            Some("/request")
        );
    }

    #[test]
    fn connection_fence_checks_exact_origin_before_requests_and_records_same_origin_hops() {
        let tracker = Tracker::default();
        tracker.connection_fences.lock().insert(
            "page".into(),
            ConnectionFence {
                frame: "main".into(),
                origin: "https://app.example.com".into(),
                urls: vec!["https://app.example.com/api/start?q=one".into()],
            },
        );
        let request = json!({"request":{"url":"https://app.example.com/api/start?q=one"}});
        assert_eq!(
            tracker.connection_request_allowed(Some("page"), "main", &request),
            Some(true)
        );
        assert_eq!(
            tracker.connection_request_allowed(None, "main", &request),
            Some(true)
        );
        for target in [
            "https://attacker.example/api",
            "http://app.example.com/api",
            "https://app.example.com:444/api",
            "https://other.example.com/api",
        ] {
            assert_eq!(
                tracker.connection_request_allowed(
                    Some("page"),
                    "main",
                    &json!({"request":{"url":target}})
                ),
                Some(false)
            );
        }
        let response = json!({"request":{"url":"https://app.example.com/api/start?q=one"},
            "responseStatusCode":302,"responseHeaders":[{"name":"Location","value":"/api/hop?q=two"}]});
        assert_eq!(
            tracker.connection_request_allowed(Some("page"), "main", &response),
            Some(true)
        );
        let fences = tracker.connection_fences.lock();
        assert!(fences["page"]
            .urls
            .contains(&"https://app.example.com/api/hop?q=two".into()));
        assert_eq!(fences["page"].urls.len(), 2);
    }
}
