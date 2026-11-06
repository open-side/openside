//! The person's own browser, reached through the openside extension running in it.
//!
//! That browser has no debugging port or gateway the daemon could drive, so the extension
//! keeps a WebSocket open to the daemon and answers requests over it. Everything it reports
//! (titles, URLs, page text) is untrusted data from that browser.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::{
    extract::{
        ws::{rejection::WebSocketUpgradeRejection, Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Extension,
};
use chrono::Utc;
use openside_core::{MessageAuthorKind, SessionId};
use openside_proto::{AttachedBy, AttachedTab, PersonTab, PersonTabGroup, PersonTabList};
use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::api::{Caller, HttpError};
use crate::engine::Shared;

pub(crate) const PROTOCOL: &str = "openside.person-browser";
const VERSION: u64 = 1;
const CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// MV3 service workers are stopped after ~30 s without events; a daemon message keeps the
/// extension's socket (and so the bridge) alive.
const PING_EVERY: Duration = Duration::from_secs(20);
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// A page outline is capped by the extension well below this.
const MAX_MESSAGE: usize = 2 * 1024 * 1024;
const MAX_PENDING: usize = 64;
const MAX_ATTACHED: usize = 20;
const MAX_REFS: usize = 2000;
const MAX_TABS: usize = 2000;
const MAX_TITLE: usize = 1000;
pub(crate) const MAX_URL: usize = 8 * 1024;
const MAX_FAVICON: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BridgeError {
    NotConnected,
    Timeout,
    /// An error code the extension reported, or `bad_response` for an unreadable answer.
    Remote(String),
}

impl From<BridgeError> for HttpError {
    fn from(error: BridgeError) -> Self {
        match error {
            BridgeError::NotConnected => {
                HttpError(StatusCode::CONFLICT, "person_browser_not_connected")
            }
            BridgeError::Timeout => {
                HttpError(StatusCode::GATEWAY_TIMEOUT, "person_browser_timeout")
            }
            BridgeError::Remote(code) => match code.as_str() {
                "tab_not_found" | "not_found" => HttpError(StatusCode::NOT_FOUND, "tab_not_found"),
                "invalid_request" | "unsupported_page" => HttpError::invalid_value(),
                "site_access_required" => HttpError(StatusCode::CONFLICT, "site_access_required"),
                _ => HttpError(StatusCode::BAD_GATEWAY, "person_browser_failed"),
            },
        }
    }
}

impl BridgeError {
    /// Plain-language reason for the model.
    pub(crate) fn for_model(&self) -> String {
        match self {
            Self::NotConnected => "The person's browser is not connected to openside. Ask them to open their browser with the openside extension paired.".into(),
            Self::Timeout => "The person's browser did not answer in time.".into(),
            Self::Remote(code) => match code.as_str() {
                "tab_not_found" | "not_found" => "That tab is no longer open.".into(),
                "site_access_required" => "openside's extension does not have access to that site yet; a page asking the person to allow it opened next to the tab. Ask them to allow it, then try again.".into(),
                "stale_ref" => "That element is gone or changed; take a fresh snapshot.".into(),
                "unsupported_page" => "That page cannot be read or controlled (browser pages, extension pages and openside itself are off limits).".into(),
                "origin_changed" => "The tab moved to another site; take a fresh snapshot.".into(),
                _ => "The person's browser could not do that.".into(),
            },
        }
    }
}

/// What the agent saw for an element ref in its last snapshot of an attached tab; used to
/// classify sensitive clicks before the extension resolves the ref again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefInfo {
    pub(crate) role: String,
    pub(crate) name: String,
}

type Reply = oneshot::Sender<Result<Value, BridgeError>>;

struct Link {
    conn: u64,
    /// Paired device the extension authenticated as.
    device: String,
    out: mpsc::Sender<String>,
    pending: HashMap<u64, Reply>,
}

#[derive(Default)]
struct HubState {
    link: Option<Link>,
    /// Browser run the attachments belong to; Chrome reuses tab ids after a restart.
    run: Option<String>,
    attachments: HashMap<SessionId, Vec<AttachedTab>>,
    refs: HashMap<(SessionId, i64), HashMap<String, RefInfo>>,
}

#[derive(Default)]
pub(crate) struct Hub {
    state: Mutex<HubState>,
    next: AtomicU64,
}

impl Hub {
    /// Install a new extension connection. The same paired device replaces (and closes) its
    /// older connection; a different device is refused while that one is still live, so another
    /// person-device token cannot take over the browser link by forging the extension Origin.
    pub(crate) fn connect(
        &self,
        run: String,
        device: String,
    ) -> Option<(u64, mpsc::Receiver<String>)> {
        let mut state = self.state.lock();
        if state
            .link
            .as_ref()
            .is_some_and(|link| link.device != device && !link.out.is_closed())
        {
            return None;
        }
        let conn = self.next.fetch_add(1, Ordering::Relaxed);
        let (out, receiver) = mpsc::channel(MAX_PENDING);
        if let Some(old) = state.link.take() {
            fail_pending(old.pending);
        }
        if state.run.as_deref() != Some(run.as_str()) {
            state.attachments.clear();
            state.refs.clear();
            state.run = Some(run);
        }
        state.link = Some(Link {
            conn,
            device,
            out,
            pending: HashMap::new(),
        });
        Some((conn, receiver))
    }

    pub(crate) fn disconnect(&self, conn: u64) {
        let mut state = self.state.lock();
        if state.link.as_ref().is_some_and(|link| link.conn == conn) {
            if let Some(link) = state.link.take() {
                fail_pending(link.pending);
            }
        }
    }

    /// Route one extension frame to its waiting call; anything else is ignored.
    pub(crate) fn deliver(&self, conn: u64, frame: &str) {
        #[derive(Deserialize)]
        struct Answer {
            id: String,
            #[serde(default)]
            result: Option<Value>,
            #[serde(default)]
            error: Option<RemoteError>,
        }
        #[derive(Deserialize)]
        struct RemoteError {
            code: String,
        }
        let Ok(answer) = serde_json::from_str::<Answer>(frame) else {
            return;
        };
        let Ok(id) = answer.id.parse::<u64>() else {
            return;
        };
        let reply = {
            let mut state = self.state.lock();
            match state.link.as_mut() {
                Some(link) if link.conn == conn => link.pending.remove(&id),
                _ => None,
            }
        };
        let Some(reply) = reply else {
            return;
        };
        let outcome = match (answer.error, answer.result) {
            (Some(error), _) => Err(BridgeError::Remote(error_code(&error.code))),
            (None, Some(result)) => Ok(result),
            (None, None) => Ok(Value::Null),
        };
        let _ = reply.send(outcome);
    }

    pub(crate) async fn call(&self, method: &str, params: Value) -> Result<Value, BridgeError> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (reply, answer) = oneshot::channel();
        let tab_id = params.get("tab_id").and_then(Value::as_i64);
        let frame = json!({"id": id.to_string(), "method": method, "params": params}).to_string();
        let conn = {
            let mut state = self.state.lock();
            let link = state.link.as_mut().ok_or(BridgeError::NotConnected)?;
            if link.pending.len() >= MAX_PENDING {
                return Err(BridgeError::Remote("busy".into()));
            }
            link.pending.insert(id, reply);
            if link.out.try_send(frame).is_err() {
                link.pending.remove(&id);
                return Err(BridgeError::Remote("busy".into()));
            }
            link.conn
        };
        // A caller that gives up (timeout, or a dropped HTTP request) frees its slot, so
        // abandoned calls cannot fill MAX_PENDING and starve later ones.
        struct Slot<'a> {
            hub: &'a Hub,
            conn: u64,
            id: u64,
        }
        impl Drop for Slot<'_> {
            fn drop(&mut self) {
                let mut state = self.hub.state.lock();
                if let Some(link) = state.link.as_mut().filter(|link| link.conn == self.conn) {
                    link.pending.remove(&self.id);
                }
            }
        }
        let _slot = Slot {
            hub: self,
            conn,
            id,
        };
        let outcome = match tokio::time::timeout(CALL_TIMEOUT, answer).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => Err(BridgeError::NotConnected),
            Err(_) => Err(BridgeError::Timeout),
        };
        if let (Err(BridgeError::Remote(code)), Some(tab_id)) = (&outcome, tab_id) {
            if code == "tab_not_found" {
                self.prune(|id| id != tab_id);
            }
        }
        outcome
    }

    /// Drop every chat's attachment (and refs) to a tab the browser no longer has open.
    fn prune(&self, open: impl Fn(i64) -> bool) {
        let mut state = self.state.lock();
        state.refs.retain(|(_, tab_id), _| open(*tab_id));
        state.attachments.retain(|_, tabs| {
            tabs.retain(|attached| open(attached.tab_id));
            !tabs.is_empty()
        });
    }

    pub(crate) async fn request<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> Result<T, BridgeError> {
        let value = self.call(method, params).await?;
        serde_json::from_value(value).map_err(|_| BridgeError::Remote("bad_response".into()))
    }

    /// The person's open, non-incognito tabs, re-checked here so a faulty extension cannot
    /// leak incognito tabs, and with untrusted strings bounded.
    pub(crate) async fn tabs(&self) -> Result<PersonTabList, BridgeError> {
        #[derive(Deserialize)]
        struct WireTab {
            #[serde(flatten)]
            tab: PersonTab,
            #[serde(default)]
            incognito: bool,
        }
        #[derive(Deserialize)]
        struct WireTabs {
            #[serde(default)]
            focused_window_id: Option<i64>,
            tabs: Vec<WireTab>,
            #[serde(default)]
            groups: Vec<PersonTabGroup>,
        }
        let wire: WireTabs = self.request("tabs.list", json!({})).await?;
        // Closed tabs leave every chat; the full id set is used, not the bounded list.
        let open: std::collections::HashSet<i64> = wire
            .tabs
            .iter()
            .filter(|tab| !tab.incognito)
            .map(|tab| tab.tab.tab_id)
            .collect();
        self.prune(|id| open.contains(&id));
        let tabs: Vec<PersonTab> = wire
            .tabs
            .into_iter()
            .filter(|tab| !tab.incognito)
            .take(MAX_TABS)
            .map(|tab| sanitize_tab(tab.tab))
            .collect();
        let groups = wire
            .groups
            .into_iter()
            .filter(|group| tabs.iter().any(|tab| tab.window_id == group.window_id))
            .map(|mut group| {
                group.title = bounded(group.title, MAX_TITLE);
                group
            })
            .collect();
        Ok(PersonTabList {
            connected: true,
            focused_window_id: wire.focused_window_id,
            tabs,
            groups,
        })
    }

    /// The tab with `tab_id`, or the selected tab of the focused window when `None`.
    pub(crate) async fn find_tab(&self, tab_id: Option<i64>) -> Result<PersonTab, BridgeError> {
        let list = self.tabs().await?;
        let found = match tab_id {
            Some(id) => list.tabs.into_iter().find(|tab| tab.tab_id == id),
            None => {
                let focused = list.focused_window_id;
                let mut active = list.tabs.into_iter().filter(|tab| tab.active);
                match focused {
                    Some(window) => active.find(|tab| tab.window_id == window),
                    None => active.next(),
                }
            }
        };
        found.ok_or_else(|| BridgeError::Remote("tab_not_found".into()))
    }

    pub(crate) fn attach(
        &self,
        session: &SessionId,
        tab: &PersonTab,
        by: AttachedBy,
    ) -> AttachedTab {
        let mut state = self.state.lock();
        let tabs = state.attachments.entry(session.clone()).or_default();
        if let Some(existing) = tabs.iter().find(|attached| attached.tab_id == tab.tab_id) {
            return existing.clone();
        }
        let attached = AttachedTab {
            tab_id: tab.tab_id,
            title: tab.title.clone(),
            url: tab.url.clone(),
            attached_by: by,
            attached_at: Utc::now(),
        };
        tabs.push(attached.clone());
        // A chat keeps its most recent attachments; the oldest one (and its refs) goes first.
        if tabs.len() > MAX_ATTACHED {
            let oldest = tabs.remove(0).tab_id;
            state.refs.remove(&(session.clone(), oldest));
        }
        attached
    }

    pub(crate) fn detach(&self, session: &SessionId, tab_id: i64) -> bool {
        let mut state = self.state.lock();
        state.refs.remove(&(session.clone(), tab_id));
        let Some(tabs) = state.attachments.get_mut(session) else {
            return false;
        };
        let before = tabs.len();
        tabs.retain(|attached| attached.tab_id != tab_id);
        let removed = tabs.len() != before;
        if tabs.is_empty() {
            state.attachments.remove(session);
        }
        removed
    }

    pub(crate) fn attached(&self, session: &SessionId) -> Vec<AttachedTab> {
        self.state
            .lock()
            .attachments
            .get(session)
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn is_attached(&self, session: &SessionId, tab_id: i64) -> bool {
        self.state
            .lock()
            .attachments
            .get(session)
            .is_some_and(|tabs| tabs.iter().any(|attached| attached.tab_id == tab_id))
    }

    pub(crate) fn set_refs(
        &self,
        session: &SessionId,
        tab_id: i64,
        mut refs: HashMap<String, RefInfo>,
    ) {
        if refs.len() > MAX_REFS {
            let keep: std::collections::HashSet<String> =
                (1..=MAX_REFS).map(|index| format!("u{index}")).collect();
            refs.retain(|element, _| keep.contains(element));
        }
        self.state
            .lock()
            .refs
            .insert((session.clone(), tab_id), refs);
    }

    pub(crate) fn ref_info(
        &self,
        session: &SessionId,
        tab_id: i64,
        element: &str,
    ) -> Option<RefInfo> {
        self.state
            .lock()
            .refs
            .get(&(session.clone(), tab_id))
            .and_then(|refs| refs.get(element).cloned())
    }

    /// Forget a deleted chat's attachments.
    pub(crate) fn forget(&self, session: &SessionId) {
        let mut state = self.state.lock();
        state.attachments.remove(session);
        state.refs.retain(|(owner, _), _| owner != session);
    }
}

fn fail_pending(pending: HashMap<u64, Reply>) {
    for (_, reply) in pending {
        let _ = reply.send(Err(BridgeError::NotConnected));
    }
}

/// Extension error codes become fixed daemon labels; an unknown code is never echoed.
fn error_code(code: &str) -> String {
    match code {
        "tab_not_found"
        | "not_found"
        | "invalid_request"
        | "unsupported_page"
        | "unsupported"
        | "site_access_required"
        | "stale_ref"
        | "origin_changed"
        | "busy" => code.into(),
        _ => "failed".into(),
    }
}

pub(crate) fn bounded(mut value: String, max_chars: usize) -> String {
    if let Some((end, _)) = value.char_indices().nth(max_chars) {
        value.truncate(end);
    }
    value
}

pub(crate) fn sanitize_tab(mut tab: PersonTab) -> PersonTab {
    tab.title = bounded(tab.title, MAX_TITLE);
    if tab.url.len() > MAX_URL {
        tab.url = bounded(tab.url, MAX_URL);
    }
    // The favicon is rendered by the web shell; only image sources it can safely load pass.
    tab.favicon_url = tab.favicon_url.filter(|icon| {
        icon.len() <= MAX_FAVICON
            && (icon.starts_with("https://")
                || icon.starts_with("http://")
                || icon.starts_with("data:image/"))
    });
    tab
}

/// Pages the person-browser tools never read or drive: only ordinary web pages qualify, and
/// never openside's own UI under any loopback alias (it holds the person's approvals).
pub(crate) fn web_page(s: &Shared, url: &str) -> bool {
    let Ok(url) = url::Url::parse(url) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let loopback = match url.host() {
        Some(url::Host::Domain(host)) => {
            let host = host.trim_end_matches('.').to_ascii_lowercase();
            host == "localhost" || host.ends_with(".localhost")
        }
        Some(url::Host::Ipv4(ip)) => ip.is_loopback() || ip.is_unspecified(),
        Some(url::Host::Ipv6(ip)) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip
                    .to_ipv4_mapped()
                    .is_some_and(|ip| ip.is_loopback() || ip.is_unspecified())
        }
        None => false,
    };
    !(loopback
        && url.port_or_known_default().is_some()
        && url.port_or_known_default() == daemon_port(s))
}

fn daemon_port(s: &Shared) -> Option<u16> {
    match s.config.listen.map(|address| address.port()) {
        Some(port) if port != 0 => Some(port),
        // Port 0 picks a free port at start-up; the daemon records the one it got.
        _ => std::fs::read_to_string(
            openside_config::data_layout::DataPath::Port.path(&s.config.data_dir),
        )
        .ok()?
        .trim()
        .parse()
        .ok(),
    }
}

fn hello_run(frame: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Hello {
        kind: String,
        protocol: String,
        version: u64,
        run: String,
    }
    let hello: Hello = serde_json::from_str(frame).ok()?;
    let valid_run = !hello.run.is_empty()
        && hello.run.len() <= 64
        && hello
            .run
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    (hello.kind == "hello" && hello.protocol == PROTOCOL && hello.version == VERSION && valid_run)
        .then_some(hello.run)
}

/// `GET /v1/browser/person/bridge`: the extension's WebSocket.
pub(crate) async fn bridge(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let upgrade = match upgrade {
        Ok(upgrade) => upgrade,
        Err(rejection) => return rejection.into_response(),
    };
    let extension = headers
        .get(header::ORIGIN)
        .and_then(|origin| origin.to_str().ok())
        == Some(crate::pairing::EXTENSION_ORIGIN);
    let device = match (&caller.0, crate::takeover::person_only(&caller, author)) {
        (Some(device), Ok(())) if extension => device.clone(),
        _ => return HttpError(StatusCode::FORBIDDEN, "person_device_required").into_response(),
    };
    let revoked = s.remote.revoked.subscribe();
    if s.remote.is_revoked(&device) || !matches!(s.db.device_exists(device.clone()).await, Ok(true))
    {
        return HttpError(StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    // The extension authenticates with a bearer subprotocol; answering with only `openside.v1`
    // completes the handshake without echoing the token.
    upgrade
        .protocols(["openside.v1"])
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| run(s, device, socket, revoked))
        .into_response()
}

async fn run(
    s: Shared,
    device: String,
    mut socket: WebSocket,
    mut revoked: broadcast::Receiver<String>,
) {
    let hello = tokio::time::timeout(HELLO_TIMEOUT, async {
        loop {
            match socket.recv().await {
                Some(Ok(Message::Text(text))) => return hello_run(text.as_str()),
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                _ => return None,
            }
        }
    })
    .await;
    let Some(run) = hello.ok().flatten() else {
        let _ = socket.send(Message::Close(None)).await;
        return;
    };
    if s.remote.is_revoked(&device) {
        return;
    }
    let hub = &s.registry.person_browser;
    let Some((conn, mut out)) = hub.connect(run, device.clone()) else {
        crate::pairing::audit(
            &s,
            "person_browser.bridge",
            "refused_in_use",
            json!({"device": device}),
        )
        .await;
        let _ = socket.send(Message::Close(None)).await;
        return;
    };
    crate::pairing::audit(
        &s,
        "person_browser.bridge",
        "connected",
        json!({"device": device}),
    )
    .await;
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    loop {
        tokio::select! {
            frame = out.recv() => match frame {
                Some(frame) => {
                    if socket.send(Message::Text(frame.into())).await.is_err() {
                        break;
                    }
                }
                // Replaced by a newer connection.
                None => break,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(text))) => hub.deliver(conn, text.as_str()),
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            _ = ping.tick() => {
                if socket.send(Message::Text(r#"{"kind":"ping"}"#.into())).await.is_err() {
                    break;
                }
            }
            () = crate::pairing::until_revoked(&s, &mut revoked, &device) => break,
        }
    }
    hub.disconnect(conn);
    let _ = socket.send(Message::Close(None)).await;
}
