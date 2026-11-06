//! Remote human takeover of an agent tab (HS-6, `docs/specs/human-step.md` §3).
//!
//! `GET /v1/tickets/{id}/takeover` upgrades to a WebSocket that streams JPEG frames of the
//! ticket's tab and forwards the controller's input to that tab only. While a takeover is
//! being opened or is pending, agent browser tools that target its tab are refused
//! ([`reserve`], [`blocked`]). Every terminal transition of the ticket calls [`finish`] before
//! it publishes the new status: that closes input admission (waiting out input already
//! admitted), stops the capture and voids the tab's captures and element references, so the
//! agent regains the tab only after the person has provably lost it.
//!
//! `GET /v1/sessions/{id}/browser/tabs/{tab_id}/control` is the same stream without a ticket:
//! a person takes one of the session's tabs from the browser sheet ([`control`]). Its lease
//! locks the agent out of that tab exactly like a takeover until the person closes the stream,
//! goes idle, is revoked or loses the tab, and it also accepts navigation and tab closing.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use axum::{
    extract::{
        ws::{rejection::WebSocketUpgradeRejection, Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Extension,
};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use openside_browser::{Browser, BrowserError, Screencast, ScreencastFrame, VIEWPORT};
use openside_core::MessageAuthorKind;
use openside_policy::{Action, Capability, Effect};
use openside_proto::{
    CapabilityScope, SessionId, TakeoverEndReason, TakeoverInput, TakeoverKeyEvent,
    TakeoverMouseButton, TakeoverMouseEvent, TakeoverServerMessage, Ticket, TicketId, TicketKind,
    TicketStatus,
};
use serde_json::{json, Value};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use crate::api::{Access, Caller, HttpError};
use crate::engine::Shared;
use crate::tools::browser::Controller;
use crate::ws_stream::{text, write_all, CONTROL_SLOTS, QUEUE};

const JPEG_QUALITY: u8 = 60;
/// Frames are acknowledged no faster than this, which caps the stream at 10 fps.
const FRAME_INTERVAL: Duration = Duration::from_millis(100);
/// A controller that sends no accepted input for this long is disconnected; it may reconnect.
const IDLE: Duration = Duration::from_secs(300);
/// Tab ownership and ticket state are rechecked this often (terminal transitions also signal).
const CHECK: Duration = Duration::from_millis(500);
/// Longest text accepted in one `text` input.
const MAX_TEXT: usize = 4096;
/// Largest client message; a `text` input at [`MAX_TEXT`] fits even fully JSON-escaped.
const MAX_MESSAGE: usize = 32 * 1024;
const MAX_KEY: usize = 32;
const MAX_KEY_TEXT: usize = 16;
/// Longest URL a control stream navigates to.
const MAX_URL: usize = 8192;
/// A person's navigation that has not loaded by then is stopped and reported as failed.
const NAVIGATE_LIMIT: Duration = Duration::from_secs(20);

static LOGIN_STARTS: LazyLock<parking_lot::Mutex<HashMap<(SessionId, String), Instant>>> =
    LazyLock::new(Default::default);

/// What a controller's lease belongs to.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Owner {
    /// A pending takeover ticket.
    Ticket(TicketId),
    /// A person's ticket-less control of one tab from the browser sheet.
    Control(SessionId, String),
}

/// The controller of one takeover ticket or controlled tab. Input dispatch and [`finish`]
/// serialize on `admit`, so once `finish` returns no input of this lease runs again.
struct Lease {
    /// The ticket's deadline; person control has none and ends on idle instead.
    expires_at: Option<DateTime<Utc>>,
    admit: tokio::sync::Mutex<bool>,
    cast: Arc<Screencast>,
    end: watch::Sender<Option<TakeoverEndReason>>,
}

impl Lease {
    async fn close(&self, reason: TakeoverEndReason) {
        let mut open = self.admit.lock().await;
        *open = false;
        self.cast.stop();
        self.end.send_if_modified(|end| {
            let first = end.is_none();
            if first {
                *end = Some(reason);
            }
            first
        });
    }
}

/// Connected controllers: one per takeover ticket and one per controlled tab.
static LEASES: LazyLock<parking_lot::Mutex<HashMap<Owner, Arc<Lease>>>> =
    LazyLock::new(Default::default);

/// Tabs whose takeover ticket is being opened, as `(session, tab)` → number of openers; each
/// opener's [`Reservation`] owns one count, so one opener ending never clears another's.
static RESERVED: LazyLock<parking_lot::Mutex<HashMap<(SessionId, String), usize>>> =
    LazyLock::new(Default::default);

/// Releases a controller's lease when its stream ends or never starts.
struct Held(Owner, Arc<Lease>);
impl Drop for Held {
    fn drop(&mut self) {
        let mut leases = LEASES.lock();
        if leases
            .get(&self.0)
            .is_some_and(|lease| Arc::ptr_eq(lease, &self.1))
        {
            leases.remove(&self.0);
        }
        drop(leases);
        self.1.cast.stop();
    }
}

/// Keeps a tab refused to agent tools from before a takeover ticket is published until the
/// ticket itself refuses them.
pub(crate) struct Reservation((SessionId, String));
impl Drop for Reservation {
    fn drop(&mut self) {
        let mut reserved = RESERVED.lock();
        if let Some(count) = reserved.get_mut(&self.0) {
            *count -= 1;
            if *count == 0 {
                reserved.remove(&self.0);
            }
        }
    }
}

fn live(ticket: &Ticket) -> bool {
    ticket.kind == TicketKind::Takeover
        && ticket.status == TicketStatus::Pending
        && ticket.expires_at > Utc::now()
}

fn tab(ticket: &Ticket) -> Option<&str> {
    ticket.target.as_ref()?.tab_id.as_deref()
}

fn matches_tab(tab_id: Option<&str>, tab: &str) -> bool {
    tab_id.map_or(true, |id| id == tab)
}

/// Pending takeover of `tab` in `session`, if any. `None` as `tab` matches any takeover of the
/// session (calls that pick a tab implicitly). A ticket past its deadline still counts until
/// its expiry is processed: that transition is what stops the person's input.
async fn active(shared: &Shared, session: &SessionId, tab_id: Option<&str>) -> Option<Ticket> {
    let state = shared.db.state(session).await.ok()?;
    state.tickets.into_values().find(|ticket| {
        ticket.kind == TicketKind::Takeover
            && ticket.status == TicketStatus::Pending
            && tab(ticket).is_some_and(|t| matches_tab(tab_id, t))
    })
}

/// Called for `request_human_step` before the tool runs. A takeover of a tab waits for the
/// agent browser operation in flight (they hold the browser gate for their whole call) and
/// marks the tab taken, so no agent operation overlaps the ticket being published.
pub(crate) async fn reserve(
    shared: &Shared,
    session: &SessionId,
    tool: &str,
    args: &Value,
) -> Option<Reservation> {
    if tool != "request_human_step" || args.get("kind").and_then(Value::as_str) != Some("takeover")
    {
        return None;
    }
    let tab = args.get("tab_id").and_then(Value::as_str)?.to_owned();
    let key = (session.clone(), tab);
    let _gate = shared.registry.browser.gate.lock().await;
    *RESERVED.lock().entry(key.clone()).or_default() += 1;
    crate::preview::yield_tab(shared, session, &key.1);
    if let Some(browser) = shared.registry.browser.tab_browser(session, &key.1).await {
        if let (Ok(url), Ok(snapshot)) = (browser.url(&key.1).await, browser.snapshot(&key.1).await)
        {
            if crate::browser_state::classify(&snapshot.nodes, &url, "").0
                == openside_proto::PageKind::Login
            {
                if let Ok(origin) = browser.origin(&key.1).await {
                    let _ =
                        crate::connections::begin_login(shared, &browser, session, &key.1, &origin)
                            .await;
                    let mut starts = LOGIN_STARTS.lock();
                    starts.retain(|_, at| at.elapsed() < Duration::from_secs(600));
                    starts.insert(key.clone(), Instant::now());
                }
            }
        }
    }
    Some(Reservation(key))
}

/// Refusal for an agent tool call that would touch a tab under human takeover, as
/// `(reason, hint)`. Every browser tool that names the tab is refused, reads included: the
/// person may be typing values the agent must not see, and the page changes under it anyway.
pub(crate) async fn blocked(
    shared: &Shared,
    session: &SessionId,
    tool: &str,
    args: &Value,
) -> Option<(&'static str, String)> {
    if !crate::tools::touches_browser(tool) {
        return None;
    }
    let tab_id = match args.get("tab_id").and_then(Value::as_str) {
        Some(tab_id) => Some(tab_id),
        // browser_repl falls back to the session's only tab; other tools without a tab do not
        // touch one (browser_tabs, browser_open_tab, vault listings).
        None if tool == "browser_repl" => None,
        None => return None,
    };
    let hint = |tab: &str, ticket: &str| {
        format!(
            "A person is controlling tab {tab} (takeover ticket {ticket}); nothing ran. Wait for the takeover to end, then take a fresh browser_snapshot."
        )
    };
    let reserved = RESERVED
        .lock()
        .keys()
        .find(|(owner, tab)| owner == session && matches_tab(tab_id, tab))
        .map(|(_, tab)| tab.clone());
    if let Some(tab) = reserved {
        return Some(("takeover_active", hint(&tab, "being opened")));
    }
    let controlled = LEASES.lock().keys().find_map(|owner| match owner {
        Owner::Control(owner, tab) if owner == session && matches_tab(tab_id, tab) => {
            Some(tab.clone())
        }
        _ => None,
    });
    if let Some(tab) = controlled {
        return Some((
            "takeover_active",
            format!("A person is controlling tab {tab} from the browser sheet; nothing ran. Wait until they hand it back, then take a fresh browser_snapshot."),
        ));
    }
    let ticket = active(shared, session, tab_id).await?;
    Some((
        "takeover_active",
        hint(tab(&ticket).unwrap_or_default(), ticket.id.as_str()),
    ))
}

/// End the person's control of a takeover ticket's tab. Every terminal transition (done,
/// reject, expiry, stop, cancellation) calls this before it publishes the new status, with
/// the ticket gate held: input admission closes after input already admitted has run, the
/// capture stops, and the tab's captures and element references are voided whether or not a
/// controller was ever connected. Other ticket kinds are a no-op.
pub(crate) async fn finish(shared: &Shared, ticket: &Ticket, reason: TakeoverEndReason) {
    if ticket.kind != TicketKind::Takeover {
        return;
    }
    let lease = LEASES
        .lock()
        .get(&Owner::Ticket(ticket.id.clone()))
        .cloned();
    if let Some(lease) = lease {
        lease.close(reason).await;
    }
    let Some(tab_id) = tab(ticket) else {
        return;
    };
    if reason != TakeoverEndReason::Done {
        LOGIN_STARTS
            .lock()
            .remove(&(ticket.session_id.clone(), tab_id.to_owned()));
    }
    shared.registry.browser.forget_captures(tab_id).await;
    if let Some(browser) = shared
        .registry
        .browser
        .tab_browser(&ticket.session_id, tab_id)
        .await
    {
        browser.invalidate_references(tab_id).await;
    }
}

/// Result delivered to the `request_human_step` call when its takeover ends with `done`: the
/// page the person left. Called after [`finish`], so no further input can change it.
pub(crate) async fn done_result(shared: &Shared, ticket: &Ticket) -> Value {
    let mut result = json!({"status":"done"});
    let Some(tab_id) = tab(ticket) else {
        return result;
    };
    result["tab_id"] = json!(tab_id);
    result["page_changed"] = json!(true);
    result["note"] = json!("A person controlled this tab; earlier snapshot ids and refs are void. Start the continuation with a fresh browser_snapshot (or browser_repl page.snapshot() when that is the exposed capture tool) of this tab and inspect whether the requested outcome actually occurred before replying. Continue any remaining steps; do not ask whether to check an outcome the person already asked you to complete. The done status means the person finished their step, not that sign-in or the task succeeded.");
    if let Some(browser) = shared
        .registry
        .browser
        .tab_browser(&ticket.session_id, tab_id)
        .await
    {
        let started = LOGIN_STARTS
            .lock()
            .remove(&(ticket.session_id.clone(), tab_id.to_owned()));
        if started.is_some_and(|at| at.elapsed() < Duration::from_secs(600)) {
            if let (Ok(url), Ok(snapshot)) =
                (browser.url(tab_id).await, browser.snapshot(tab_id).await)
            {
                let kind = crate::browser_state::classify(&snapshot.nodes, &url, "").0;
                if !matches!(
                    kind,
                    openside_proto::PageKind::Login
                        | openside_proto::PageKind::Otp
                        | openside_proto::PageKind::Error
                ) {
                    let _ = crate::connections::complete_login(
                        shared,
                        &browser,
                        &ticket.session_id,
                        tab_id,
                        openside_proto::ConnectionSource::Takeover,
                        None,
                    )
                    .await;
                }
            }
        }
        // The tab ledger's title lags behind the page; read the live document instead. Like
        // the stream's `page`, a document that is not http(s) is named only by its scheme.
        if let Ok(url) = browser.url(tab_id).await {
            result["url"] = json!(page_address(&url));
        }
        if let Ok(Value::String(title)) = browser.evaluate(tab_id, "document.title").await {
            result["title"] = json!(title.chars().take(200).collect::<String>());
        }
    }
    // URLs and titles can echo values the person typed; stored results pass the redactor.
    scrubbed(result.clone(), shared.registry.redact(result))
}

/// A result the redactor could not scrub keeps only the fields the daemon wrote itself: the
/// page's address and title may echo typed values, so they are dropped rather than passed on.
fn scrubbed(result: Value, redacted: crate::persistence::Result<Value>) -> Value {
    redacted.unwrap_or_else(|_| {
        let mut result = result;
        if let Some(fields) = result.as_object_mut() {
            fields.remove("url");
            fields.remove("title");
        }
        result["redaction_failed"] = json!(true);
        result
    })
}

/// An `Origin` header, which browsers always send on WebSocket handshakes, must name this
/// daemon's own host. Non-browser clients send none and authenticate by bearer token alone.
/// A reverse proxy in front of the daemon must preserve `Host` (docs/REMOTE.md).
pub(crate) fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
    let origin = origin
        .to_str()
        .ok()
        .and_then(|value| url::Url::parse(value).ok());
    match (origin, host) {
        (Some(origin), Some(host)) => {
            let authority = match (origin.host_str(), origin.port()) {
                (Some(name), Some(port)) => format!("{name}:{port}"),
                (Some(name), None) => name.to_owned(),
                _ => return false,
            };
            matches!(origin.scheme(), "http" | "https") && authority.eq_ignore_ascii_case(host)
        }
        _ => false,
    }
}

pub(crate) async fn stream(
    State(s): State<Shared>,
    Extension(access): Extension<Access>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    match open(s, access, caller, id, headers, upgrade).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

async fn open(
    s: Shared,
    access: Access,
    caller: Caller,
    id: String,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Result<Response, HttpError> {
    let id = TicketId::new(id);
    let ticket = crate::tickets::get(&s, &id)
        .await
        .map_err(|_| HttpError::internal())?
        .ok_or_else(HttpError::not_found)?;
    if !access.has(CapabilityScope::HumanStep) {
        return Err(HttpError(StatusCode::FORBIDDEN, "forbidden"));
    }
    if ticket.kind != TicketKind::Takeover {
        return Err(HttpError(StatusCode::BAD_REQUEST, "invalid_ticket_kind"));
    }
    if !live(&ticket) {
        return Err(HttpError(StatusCode::CONFLICT, "invalid_transition"));
    }
    let Some(tab_id) = tab(&ticket).map(str::to_owned) else {
        // Without a tab there is nothing to stream; the person finishes on the desktop.
        return Err(HttpError(StatusCode::CONFLICT, "needs_desktop"));
    };
    if !same_origin(&headers) {
        return Err(HttpError(StatusCode::FORBIDDEN, "origin_forbidden"));
    }
    let upgrade =
        upgrade.map_err(|_| HttpError(StatusCode::UPGRADE_REQUIRED, "websocket_required"))?;
    // Subscribe before the liveness recheck so a revocation racing this handshake is seen.
    let revoked = s.remote.revoked.subscribe();
    if let Some(device) = &caller.0 {
        if s.remote.is_revoked(device)
            || !matches!(s.db.device_exists(device.clone()).await, Ok(true))
        {
            return Err(HttpError(StatusCode::UNAUTHORIZED, "unauthorized"));
        }
    }
    let owner = Owner::Ticket(ticket.id.clone());
    if LEASES.lock().contains_key(&owner) {
        return Err(HttpError(StatusCode::CONFLICT, "takeover_in_use"));
    }
    if LEASES
        .lock()
        .contains_key(&Owner::Control(ticket.session_id.clone(), tab_id.clone()))
    {
        return Err(HttpError(StatusCode::CONFLICT, "control_in_use"));
    }
    let browser = s
        .registry
        .browser
        .tab_browser(&ticket.session_id, &tab_id)
        .await
        .ok_or(HttpError(StatusCode::CONFLICT, "tab_unavailable"))?;
    Controller::owned(&browser, &ticket.session_id, &tab_id)
        .await
        .map_err(|_| HttpError(StatusCode::CONFLICT, "tab_unavailable"))?;
    // Registration is serialized with terminal transitions: either this lease exists when
    // `finish` runs, or the ticket is no longer pending here.
    let held = {
        let _gate = s.ticket_gate.lock().await;
        match crate::tickets::get(&s, &ticket.id).await {
            Ok(Some(current)) if live(&current) => {}
            _ => return Err(HttpError(StatusCode::CONFLICT, "invalid_transition")),
        }
        if LEASES.lock().contains_key(&owner) {
            return Err(HttpError(StatusCode::CONFLICT, "takeover_in_use"));
        }
        crate::preview::yield_tab(&s, &ticket.session_id, &tab_id);
        let cast = browser
            .screencast(ticket.session_id.as_str(), &tab_id, JPEG_QUALITY)
            .await
            .map_err(|error| match error {
                BrowserError::InvalidAction(_) => {
                    HttpError(StatusCode::CONFLICT, "takeover_in_use")
                }
                _ => HttpError(StatusCode::CONFLICT, "tab_unavailable"),
            })?;
        let lease = Arc::new(Lease {
            expires_at: Some(ticket.expires_at),
            admit: tokio::sync::Mutex::new(true),
            cast: Arc::new(cast),
            end: watch::channel(None).0,
        });
        LEASES.lock().insert(owner.clone(), Arc::clone(&lease));
        Held(owner.clone(), lease)
    };
    let session = Live {
        s,
        owner,
        session: ticket.session_id,
        tab: tab_id,
        device: caller.0,
        browser,
        held,
    };
    // Web clients authenticate with a bearer subprotocol; answering with only `openside.v1`
    // completes their handshake without echoing the token.
    Ok(upgrade
        .protocols(["openside.v1"])
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| session.run(socket, revoked))
        .into_response())
}

pub(crate) async fn control(
    State(s): State<Shared>,
    Extension(access): Extension<Access>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    Path((id, tab)): Path<(String, String)>,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let request = ControlRequest {
        access,
        caller,
        author,
        session: SessionId::new(id),
        tab,
    };
    match open_control(s, request, headers, upgrade).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

struct ControlRequest {
    access: Access,
    caller: Caller,
    author: MessageAuthorKind,
    session: SessionId,
    tab: String,
}

/// Only the person may drive a tab without a ticket: their own paired device, or the local
/// socket without the MCP or channel marker. An agent client must never take its own work
/// out from under the policy and approvals that govern agent browsing.
pub(crate) fn person_only(caller: &Caller, author: MessageAuthorKind) -> Result<(), HttpError> {
    let device = caller.0.is_none() || caller.1 == openside_proto::DeviceKind::PersonDevice;
    if device && author == MessageAuthorKind::Person {
        Ok(())
    } else {
        Err(HttpError(StatusCode::FORBIDDEN, "person_device_required"))
    }
}

async fn open_control(
    s: Shared,
    request: ControlRequest,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Result<Response, HttpError> {
    let ControlRequest {
        access,
        caller,
        author,
        session,
        tab,
    } = request;
    if !access.has(CapabilityScope::Steer) {
        return Err(HttpError(StatusCode::FORBIDDEN, "forbidden"));
    }
    person_only(&caller, author)?;
    let state =
        s.db.state(&session)
            .await
            .map_err(|_| HttpError::not_found())?;
    if state.deleted {
        return Err(HttpError::not_found());
    }
    // Isolated spaces are private like incognito ones: their pages never reach another device.
    if state.incognito || state.browser_space != openside_proto::BrowserSpace::Shared {
        return Err(HttpError(StatusCode::CONFLICT, "private_session"));
    }
    if !same_origin(&headers) {
        return Err(HttpError(StatusCode::FORBIDDEN, "origin_forbidden"));
    }
    let upgrade =
        upgrade.map_err(|_| HttpError(StatusCode::UPGRADE_REQUIRED, "websocket_required"))?;
    let revoked = s.remote.revoked.subscribe();
    if let Some(device) = &caller.0 {
        if s.remote.is_revoked(device)
            || !matches!(s.db.device_exists(device.clone()).await, Ok(true))
        {
            return Err(HttpError(StatusCode::UNAUTHORIZED, "unauthorized"));
        }
    }
    let owner = Owner::Control(session.clone(), tab.clone());
    if LEASES.lock().contains_key(&owner) {
        return Err(HttpError(StatusCode::CONFLICT, "control_in_use"));
    }
    let browser = s
        .registry
        .browser
        .tab_browser(&session, &tab)
        .await
        .ok_or(HttpError(StatusCode::CONFLICT, "tab_unavailable"))?;
    Controller::owned(&browser, &session, &tab)
        .await
        .map_err(|_| HttpError(StatusCode::CONFLICT, "tab_unavailable"))?;
    // Agent browser tools hold this gate for their whole call and check `blocked` under it:
    // the lease is registered only after the agent's operation in flight ends, and every later
    // agent call on the tab sees it.
    let held = {
        let _gate = s.registry.browser.gate.lock().await;
        if active(&s, &session, Some(&tab)).await.is_some()
            || RESERVED
                .lock()
                .contains_key(&(session.clone(), tab.clone()))
        {
            return Err(HttpError(StatusCode::CONFLICT, "takeover_active"));
        }
        if LEASES.lock().contains_key(&owner) {
            return Err(HttpError(StatusCode::CONFLICT, "control_in_use"));
        }
        crate::preview::yield_tab(&s, &session, &tab);
        let cast = browser
            .screencast(session.as_str(), &tab, JPEG_QUALITY)
            .await
            .map_err(|error| match error {
                BrowserError::InvalidAction(_) => HttpError(StatusCode::CONFLICT, "control_in_use"),
                _ => HttpError(StatusCode::CONFLICT, "tab_unavailable"),
            })?;
        let lease = Arc::new(Lease {
            expires_at: None,
            admit: tokio::sync::Mutex::new(true),
            cast: Arc::new(cast),
            end: watch::channel(None).0,
        });
        LEASES.lock().insert(owner.clone(), Arc::clone(&lease));
        Held(owner.clone(), lease)
    };
    let live = Live {
        s,
        owner,
        session,
        tab,
        device: caller.0,
        browser,
        held,
    };
    Ok(upgrade
        .protocols(["openside.v1"])
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| live.run(socket, revoked))
        .into_response())
}

/// How the session's browse policy judges an address the person opens. Only an http(s) URL has
/// an origin (credentials dropped) for the policy to judge, exactly as it judges the agent's
/// navigation; any other scheme is the person's own choice and carries none. Returns the origin
/// and the policy's effect for it; an `ask` origin is the caller's to approve.
pub(crate) async fn navigable(
    s: &Shared,
    session: &SessionId,
    url: &str,
) -> Result<(Option<String>, Effect), &'static str> {
    let Address::Web(origin) = address(url)? else {
        return Ok((None, Effect::Allow));
    };
    let state = s.db.state(session).await.map_err(|_| "navigation_failed")?;
    let policy =
        crate::policy_api::effective(s, &crate::tools::ToolEnv::new(s.clone(), &state).policy);
    match policy
        .decide(&Action::new(Capability::Browse, &origin))
        .effect
    {
        Effect::Deny => Err("policy_denied"),
        effect => Ok((Some(origin), effect)),
    }
}

/// What may be recorded of an address: the origin of an http(s) URL, or only the scheme of any
/// other URL, whose remainder is page content rather than an address.
enum Address {
    Web(String),
    Other(String),
}

fn address(url: &str) -> Result<Address, &'static str> {
    let parsed = url::Url::parse(url).map_err(|_| "invalid_url")?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Ok(Address::Other(parsed.scheme().to_owned()));
    }
    openside_policy::Origin::from_url(&parsed)
        .map(|origin| Address::Web(origin.to_string()))
        .ok_or("invalid_url")
}

/// What one stream message asks for.
enum Step {
    Cdp(&'static str, Value),
    Navigate(String),
    History(i64),
    Reload,
    Close,
}

impl Step {
    fn audit_name(&self) -> &'static str {
        match self {
            Self::Cdp(..) => "input",
            Self::Navigate(_) => "navigate",
            Self::History(delta) if *delta < 0 => "back",
            Self::History(_) => "forward",
            Self::Reload => "reload",
            Self::Close => "close_tab",
        }
    }
}

/// What an accepted message changed.
enum Applied {
    Input,
    Page,
    Closed,
}

struct Live {
    s: Shared,
    owner: Owner,
    session: SessionId,
    tab: String,
    device: Option<String>,
    browser: Arc<Browser>,
    held: Held,
}

impl Live {
    async fn run(self, socket: WebSocket, mut revoked: tokio::sync::broadcast::Receiver<String>) {
        let (sink, mut source) = socket.split();
        let (out, queue) = mpsc::channel(QUEUE);
        let writer = tokio::spawn(write_all(sink, queue, |message| {
            std::future::ready(Some(message))
        }));
        let lease = Arc::clone(&self.held.1);
        let mut end = lease.end.subscribe();
        let control = matches!(self.owner, Owner::Control(..));
        let idle_after = idle_limit(&self.owner);
        let mut idle = Instant::now() + idle_after;
        let mut check = tokio::time::interval(CHECK);
        // The last delivered frame is acknowledged once FRAME_INTERVAL has passed.
        let mut unacked: Option<(ScreencastFrame, Instant)> = None;
        let mut page = None;
        let mut inputs = 0u64;
        if control {
            self.audit("control_started", json!({})).await;
            self.refresh(&mut page, &out).await;
        }
        let reason = loop {
            if Instant::now() >= idle {
                break Some(TakeoverEndReason::Idle);
            }
            let ack_at = unacked.as_ref().map_or_else(Instant::now, |(_, at)| *at);
            tokio::select! {
                biased;
                _ = end.changed() => break *end.borrow(),
                () = crate::pairing::until_revoked(&self.s, &mut revoked, self.device.as_deref().unwrap_or_default()), if self.device.is_some() => {
                    break Some(TakeoverEndReason::Revoked);
                }
                () = out.closed() => break None,
                // Deadlines and liveness come before data: a peer that keeps input ready
                // (even rejected input) must not hold the controller past them.
                () = tokio::time::sleep_until(idle) => break Some(TakeoverEndReason::Idle),
                _ = check.tick() => {
                    if let Some(reason) = self.ended().await {
                        break Some(reason);
                    }
                    if control {
                        self.refresh(&mut page, &out).await;
                    }
                }
                frame = lease.cast.next(), if unacked.is_none() => {
                    let message = if self.withheld().await {
                        TakeoverServerMessage::Withheld { code: "screenshot_unavailable_after_secret_fill".into() }
                    } else {
                        TakeoverServerMessage::Frame { data: frame.data.clone(), width: frame.viewport.0, height: frame.viewport.1 }
                    };
                    unacked = Some((frame, Instant::now() + FRAME_INTERVAL));
                    // A peer that is behind loses this frame rather than queueing it; the
                    // reserved slots keep room for input errors and `ended`.
                    if out.capacity() > CONTROL_SLOTS {
                        let _ = out.try_send(text(&message));
                    }
                }
                () = tokio::time::sleep_until(ack_at), if unacked.is_some() => {
                    if let Some((frame, _)) = unacked.take() {
                        lease.cast.ack(&frame);
                    }
                }
                message = source.next() => match message {
                    Some(Ok(Message::Text(body))) => match self.input(&lease, body.as_str()).await {
                        Ok(Applied::Input) => {
                            idle = Instant::now() + idle_after;
                            inputs += 1;
                        }
                        Ok(Applied::Page) => {
                            idle = Instant::now() + idle_after;
                            self.refresh(&mut page, &out).await;
                        }
                        Ok(Applied::Closed) => break Some(TakeoverEndReason::TabUnavailable),
                        Err(Refused::Ended) => {}
                        Err(Refused::Input(code)) => {
                            let _ = out.try_send(text(&TakeoverServerMessage::InputError { code: code.into() }));
                        }
                    },
                    Some(Ok(Message::Binary(_))) => {
                        let _ = out.try_send(text(&TakeoverServerMessage::InputError { code: "invalid_input".into() }));
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_)) | Err(_)) | None => break None,
                },
            }
        };
        // Release the tab before talking to a peer that may never read again: input admission
        // closes, the capture stops and the ticket accepts a new controller.
        *lease.admit.lock().await = false;
        if control {
            // A takeover's `finish` voids what the agent knew about the tab; person control has
            // no ticket, so its end does the same before the agent may use the tab again.
            lease.cast.stop();
            self.s.registry.browser.forget_captures(&self.tab).await;
            self.browser.invalidate_references(&self.tab).await;
            let ended = reason.map_or(json!("released"), |reason| json!(reason));
            self.audit("control_ended", json!({"reason":ended,"inputs":inputs}))
                .await;
        }
        drop(self.held);
        crate::ws_stream::close(
            out,
            writer,
            source,
            reason.map(|reason| text(&TakeoverServerMessage::Ended { reason })),
            Message::Close(None),
        )
        .await;
    }

    fn device_revoked(&self) -> bool {
        self.device
            .as_deref()
            .is_some_and(|device| self.s.remote.is_revoked(device))
    }

    /// Any secret fill in this tab's history withholds every frame. A buffered JPEG carries no
    /// document identity, so a frame painted before a navigation cannot be told apart from one
    /// painted after it; tab-wide taint only ever grows, so no tainted pixels slip through.
    async fn withheld(&self) -> bool {
        self.s.registry.browser.tab_tainted(&self.tab).await
    }

    async fn ended(&self) -> Option<TakeoverEndReason> {
        match &self.owner {
            Owner::Ticket(id) => {
                let ticket = match crate::tickets::get(&self.s, id).await {
                    Ok(Some(ticket)) => ticket,
                    _ => return Some(TakeoverEndReason::Rejected),
                };
                match ticket.status {
                    TicketStatus::Resolved => return Some(TakeoverEndReason::Done),
                    TicketStatus::Expired => return Some(TakeoverEndReason::Expired),
                    TicketStatus::Rejected | TicketStatus::NeedsDesktop => {
                        return Some(TakeoverEndReason::Rejected)
                    }
                    TicketStatus::Pending => {}
                }
            }
            Owner::Control(..) => match self.s.db.state(&self.session).await {
                Ok(state) if !state.deleted => {}
                _ => return Some(TakeoverEndReason::TabUnavailable),
            },
        }
        if !self.browser.is_connected()
            || Controller::owned(&self.browser, &self.session, &self.tab)
                .await
                .is_err()
        {
            return Some(TakeoverEndReason::TabUnavailable);
        }
        None
    }

    /// Send the tab's address, title and history position when they changed since `last`.
    /// A full queue skips the update; the next check retries it.
    async fn refresh(&self, last: &mut Option<TakeoverServerMessage>, out: &mpsc::Sender<Message>) {
        let Ok(history) = self
            .browser
            .page_command(
                self.session.as_str(),
                &self.tab,
                "Page.getNavigationHistory",
                json!({}),
            )
            .await
        else {
            return;
        };
        let entries = history["entries"].as_array().map_or(&[][..], Vec::as_slice);
        let Some(index) = history["currentIndex"]
            .as_u64()
            .and_then(|index| usize::try_from(index).ok())
            .filter(|index| *index < entries.len())
        else {
            return;
        };
        let entry = &entries[index];
        let message = json!({
            "type": "page",
            "url": page_address(entry["url"].as_str().unwrap_or_default()),
            "title": entry["title"].as_str().unwrap_or_default().chars().take(200).collect::<String>(),
            "can_go_back": index > 0,
            "can_go_forward": index + 1 < entries.len(),
        });
        // Addresses and titles can echo filled secrets; they pass the redactor like results.
        let Ok(value) = self.s.registry.redact(message) else {
            return;
        };
        let Ok(message) = serde_json::from_value::<TakeoverServerMessage>(value) else {
            return;
        };
        if last.as_ref() != Some(&message)
            && out.capacity() > CONTROL_SLOTS
            && out.try_send(text(&message)).is_ok()
        {
            *last = Some(message);
        }
    }

    /// One audit record of person control: ids, origins and outcome codes, never typed values.
    async fn audit(&self, decision: &str, fields: Value) {
        let mut record = json!({"ts":Utc::now(),"session":self.session,"tool":"browser_control",
            "decision":decision,"device":self.device.as_deref().unwrap_or("local"),"tab_id":self.tab});
        if let (Some(record), Some(fields)) = (record.as_object_mut(), fields.as_object()) {
            record.extend(fields.clone());
        }
        let _ = crate::tickets::audit_record(&self.s, record).await;
    }

    /// Forward one input message to the stream's tab, inside the lease's admission boundary
    /// and this device's revocation gate. Errors carry static codes only: typed text and keys
    /// never reach logs, events or responses.
    async fn input(&self, lease: &Lease, body: &str) -> Result<Applied, Refused> {
        let input: TakeoverInput =
            serde_json::from_str(body).map_err(|_| Refused::Input("invalid_input"))?;
        let step = match (input, &self.owner) {
            (TakeoverInput::Navigate { url }, Owner::Control(..)) => Step::Navigate(url),
            (TakeoverInput::Back, Owner::Control(..)) => Step::History(-1),
            (TakeoverInput::Forward, Owner::Control(..)) => Step::History(1),
            (TakeoverInput::Reload, Owner::Control(..)) => Step::Reload,
            (TakeoverInput::CloseTab, Owner::Control(..)) => Step::Close,
            (input, _) => {
                let (method, params) = command(input).map_err(Refused::Input)?;
                Step::Cdp(method, params)
            }
        };
        let open = lease.admit.lock().await;
        if !*open || lease.expires_at.is_some_and(|at| at <= Utc::now()) {
            return Err(Refused::Ended);
        }
        // Revocation marks the device, then takes this gate for writing before it deletes the
        // token and returns: input admitted under the read guard finishes first, and input
        // after the mark is refused here.
        let _device = match &self.device {
            Some(device) => {
                let gate = self.s.remote.device_gate(device).read_owned().await;
                if self.device_revoked() {
                    return Err(Refused::Ended);
                }
                Some(gate)
            }
            None => None,
        };
        #[cfg(test)]
        if let Owner::Ticket(ticket) = &self.owner {
            hooks::admitted(ticket).await;
        }
        let step = match step {
            Step::Cdp(method, params) => {
                return self
                    .browser
                    .page_command(self.session.as_str(), &self.tab, method, params)
                    .await
                    .map(|_| Applied::Input)
                    .map_err(|_| Refused::Input("input_failed"));
            }
            step => step,
        };
        let name = step.audit_name();
        let (origin, scheme) = match &step {
            Step::Navigate(url) => match address(url) {
                Ok(Address::Web(origin)) => (Some(origin), None),
                Ok(Address::Other(scheme)) => (None, Some(scheme)),
                Err(_) => (None, None),
            },
            _ => (None, None),
        };
        let mut approved = false;
        let result = self.navigate(step, &mut approved).await;
        let outcome = result.as_ref().err().copied().unwrap_or("ok");
        let mut fields = json!({"origin":origin,"outcome":outcome,"person_approved":approved});
        if let Some(scheme) = scheme {
            fields["scheme"] = json!(scheme);
        }
        self.audit(name, fields).await;
        result.map_err(Refused::Input)
    }

    /// Navigation and tab closing hold the browser gate, like the agent's own browser tools,
    /// so they stay serialized with every other daemon navigation and fill. `approved` is set
    /// when a typed address approved an `ask` origin for its own navigation.
    async fn navigate(&self, step: Step, approved: &mut bool) -> Result<Applied, &'static str> {
        let owner = self.session.as_str();
        let _gate = self.s.registry.browser.gate.lock().await;
        match step {
            Step::Cdp(..) => return Err("invalid_input"),
            Step::Navigate(url) => {
                if url.len() > MAX_URL {
                    return Err("invalid_url");
                }
                let (origin, effect) = navigable(&self.s, &self.session, &url).await?;
                *approved = effect == Effect::Ask;
                // Typing the address is the person's approval of that origin, for this
                // navigation and its same-origin redirects only: the grant ends with the load.
                let _grant = origin.map(|origin| {
                    self.s
                        .registry
                        .browser
                        .grant_navigation(&self.session, vec![origin])
                });
                // A `javascript:` address runs in the current document and starts no load, so
                // Chromium answers `net::ERR_ABORTED` even though the script ran. A script that
                // never yields is bounded like any other navigation.
                if matches!(address(&url), Ok(Address::Other(scheme)) if scheme == "javascript") {
                    let command = self.browser.page_command(
                        owner,
                        &self.tab,
                        "Page.navigate",
                        json!({"url":url}),
                    );
                    tokio::time::timeout(NAVIGATE_LIMIT, command)
                        .await
                        .map_err(|_| "navigation_failed")?
                        .map_err(|_| "navigation_failed")?;
                } else {
                    self.browser
                        .navigate_cancellable(
                            owner,
                            &self.tab,
                            &url,
                            tokio::time::sleep(NAVIGATE_LIMIT),
                        )
                        .await
                        .map_err(|error| match error {
                            BrowserError::ConnectionDenied
                            | BrowserError::ConnectionGrantRequired { .. } => "connection_denied",
                            _ => "navigation_failed",
                        })?;
                }
            }
            Step::History(delta) => {
                let id = self.history_entry(delta).await?;
                self.browser
                    .page_command(
                        owner,
                        &self.tab,
                        "Page.navigateToHistoryEntry",
                        json!({"entryId":id}),
                    )
                    .await
                    .map_err(|_| "navigation_failed")?;
            }
            Step::Reload => {
                self.history_entry(0).await?;
                self.browser
                    .page_command(owner, &self.tab, "Page.reload", json!({}))
                    .await
                    .map_err(|_| "navigation_failed")?;
            }
            Step::Close => {
                self.browser
                    .close_agent_tab(owner, &self.tab)
                    .await
                    .map_err(|_| "input_failed")?;
                let _ = self.s.registry.browser.persist(&self.s).await;
                return Ok(Applied::Closed);
            }
        }
        let _ = self.s.registry.browser.persist(&self.s).await;
        Ok(Applied::Page)
    }

    /// The id of the history entry `delta` steps from the current one, when the person may load
    /// it. Back, forward and reload send no address of their own, so an `ask` origin is refused
    /// with `approval_required`: retyping the address is how the person approves it.
    async fn history_entry(&self, delta: i64) -> Result<i64, &'static str> {
        let history = self
            .browser
            .page_command(
                self.session.as_str(),
                &self.tab,
                "Page.getNavigationHistory",
                json!({}),
            )
            .await
            .map_err(|_| "input_failed")?;
        let entry = history["currentIndex"]
            .as_i64()
            .map(|index| index + delta)
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| history["entries"].get(index))
            .ok_or("no_history")?;
        let url = entry["url"].as_str().unwrap_or_default();
        if navigable(&self.s, &self.session, url).await?.1 == Effect::Ask {
            return Err("approval_required");
        }
        entry["id"].as_i64().ok_or("input_failed")
    }
}

/// The address a control stream shows for a page. For a document that is not http(s)
/// (`data:`, `blob:`, `about:`, `file:`, …), whether typed or reached by the page itself, only
/// the scheme is reported, since the rest of such a URL is page content, not an address.
fn page_address(url: &str) -> String {
    if url == "about:blank" {
        return url.to_owned();
    }
    match url::Url::parse(url) {
        Ok(parsed) if matches!(parsed.scheme(), "http" | "https") => {
            crate::browser::public_url(url)
        }
        Ok(parsed) => format!("{}:", parsed.scheme()),
        Err(_) => String::new(),
    }
}

#[cfg(not(test))]
fn idle_limit(_: &Owner) -> Duration {
    IDLE
}

#[cfg(test)]
fn idle_limit(owner: &Owner) -> Duration {
    match owner {
        Owner::Ticket(ticket) => hooks::IDLE.lock().get(ticket).copied(),
        Owner::Control(session, tab) => hooks::CONTROL_IDLE
            .lock()
            .get(&(session.clone(), tab.clone()))
            .copied(),
    }
    .unwrap_or(IDLE)
}

/// Per-ticket (and per controlled tab) test controls, so parallel tests never see each other's.
#[cfg(test)]
pub(crate) mod hooks {
    use super::*;
    use tokio::sync::Notify;

    pub(crate) static IDLE: LazyLock<parking_lot::Mutex<HashMap<TicketId, Duration>>> =
        LazyLock::new(Default::default);
    /// Idle limits of person control, keyed by `(session, tab)`.
    pub(crate) static CONTROL_IDLE: LazyLock<
        parking_lot::Mutex<HashMap<(SessionId, String), Duration>>,
    > = LazyLock::new(Default::default);
    /// `(reached, release)` for one paused input.
    type Pause = (Arc<Notify>, Arc<Notify>);
    /// The next admitted input of the ticket signals `reached` after its authorization checks
    /// and waits for `release` before dispatching.
    pub(crate) static PAUSE: LazyLock<parking_lot::Mutex<HashMap<TicketId, Pause>>> =
        LazyLock::new(Default::default);

    pub(super) async fn admitted(ticket: &TicketId) {
        let pause = PAUSE.lock().remove(ticket);
        if let Some((reached, release)) = pause {
            reached.notify_one();
            release.notified().await;
        }
    }
}

enum Refused {
    /// The lease or the device ended; the stream is about to close.
    Ended,
    Input(&'static str),
}

fn point(x: f64, y: f64) -> Result<(f64, f64), &'static str> {
    let (width, height) = (f64::from(VIEWPORT.0), f64::from(VIEWPORT.1));
    if x.is_finite() && y.is_finite() && (0.0..width).contains(&x) && (0.0..height).contains(&y) {
        Ok((x, y))
    } else {
        Err("out_of_bounds")
    }
}

/// Map one input message to the CDP `Input.*` command that performs it.
fn command(input: TakeoverInput) -> Result<(&'static str, Value), &'static str> {
    Ok(match input {
        TakeoverInput::Mouse {
            event,
            x,
            y,
            button,
            click_count,
            modifiers,
        } => {
            let (x, y) = point(x, y)?;
            let (kind, default_button, default_clicks) = match event {
                TakeoverMouseEvent::Down => ("mousePressed", TakeoverMouseButton::Left, 1),
                TakeoverMouseEvent::Up => ("mouseReleased", TakeoverMouseButton::Left, 1),
                TakeoverMouseEvent::Move => ("mouseMoved", TakeoverMouseButton::None, 0),
            };
            let button = match button.unwrap_or(default_button) {
                TakeoverMouseButton::None => "none",
                TakeoverMouseButton::Left => "left",
                TakeoverMouseButton::Middle => "middle",
                TakeoverMouseButton::Right => "right",
            };
            (
                "Input.dispatchMouseEvent",
                json!({"type":kind,"x":x,"y":y,"button":button,
                    "clickCount":click_count.unwrap_or(default_clicks).min(3),
                    "modifiers":modifiers.unwrap_or(0) & 15}),
            )
        }
        TakeoverInput::Wheel {
            x,
            y,
            delta_x,
            delta_y,
        } => {
            let (x, y) = point(x, y)?;
            if !delta_x.is_finite() || !delta_y.is_finite() {
                return Err("invalid_input");
            }
            (
                "Input.dispatchMouseEvent",
                json!({"type":"mouseWheel","x":x,"y":y,"deltaX":delta_x,"deltaY":delta_y}),
            )
        }
        TakeoverInput::Key {
            event,
            key,
            code,
            text,
            key_code,
            modifiers,
        } => {
            if key.is_empty()
                || key.len() > MAX_KEY
                || code.as_ref().is_some_and(|code| code.len() > MAX_KEY)
                || text.as_ref().is_some_and(|text| text.len() > MAX_KEY_TEXT)
            {
                return Err("invalid_input");
            }
            let mut params = json!({"key":key,"modifiers":modifiers.unwrap_or(0) & 15});
            if let Some(code) = code {
                params["code"] = json!(code);
            }
            if let Some(key_code) = key_code {
                params["windowsVirtualKeyCode"] = json!(key_code);
            }
            let kind = match (event, text) {
                (TakeoverKeyEvent::Down, Some(text)) if !text.is_empty() => {
                    params["text"] = json!(text);
                    params["unmodifiedText"] = json!(text);
                    "keyDown"
                }
                (TakeoverKeyEvent::Down, _) => "rawKeyDown",
                (TakeoverKeyEvent::Up, _) => "keyUp",
            };
            params["type"] = json!(kind);
            ("Input.dispatchKeyEvent", params)
        }
        TakeoverInput::Text { text } => {
            if text.is_empty() || text.len() > MAX_TEXT {
                return Err("invalid_input");
            }
            ("Input.insertText", json!({"text":text}))
        }
        // Control streams handle these before reaching here; a takeover ticket does not.
        TakeoverInput::Navigate { .. }
        | TakeoverInput::Back
        | TakeoverInput::Forward
        | TakeoverInput::Reload
        | TakeoverInput::CloseTab => return Err("invalid_input"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_maps_to_tab_scoped_cdp_commands_within_the_viewport() {
        let click = command(TakeoverInput::Mouse {
            event: TakeoverMouseEvent::Down,
            x: 10.0,
            y: 899.0,
            button: None,
            click_count: None,
            modifiers: Some(0xff),
        })
        .unwrap();
        assert_eq!(click.0, "Input.dispatchMouseEvent");
        assert_eq!(
            click.1,
            json!({"type":"mousePressed","x":10.0,"y":899.0,"button":"left","clickCount":1,"modifiers":15})
        );
        for (x, y) in [(1440.0, 10.0), (-1.0, 10.0), (10.0, 900.0), (f64::NAN, 1.0)] {
            assert_eq!(
                command(TakeoverInput::Wheel {
                    x,
                    y,
                    delta_x: 0.0,
                    delta_y: 1.0
                })
                .unwrap_err(),
                "out_of_bounds"
            );
        }
        let key = command(TakeoverInput::Key {
            event: TakeoverKeyEvent::Down,
            key: "a".into(),
            code: Some("KeyA".into()),
            text: Some("a".into()),
            key_code: Some(65),
            modifiers: None,
        })
        .unwrap();
        assert_eq!(key.1["type"], "keyDown");
        assert_eq!(key.1["text"], "a");
        let raw = command(TakeoverInput::Key {
            event: TakeoverKeyEvent::Down,
            key: "Enter".into(),
            code: None,
            text: None,
            key_code: Some(13),
            modifiers: None,
        })
        .unwrap();
        assert_eq!(raw.1["type"], "rawKeyDown");
        assert_eq!(
            command(TakeoverInput::Text {
                text: "x".repeat(MAX_TEXT + 1)
            })
            .unwrap_err(),
            "invalid_input"
        );
        let key = |code: &str, text: &str| TakeoverInput::Key {
            event: TakeoverKeyEvent::Down,
            key: "a".into(),
            code: Some(code.into()),
            text: Some(text.into()),
            key_code: None,
            modifiers: None,
        };
        assert!(command(key(&"K".repeat(MAX_KEY), &"a".repeat(MAX_KEY_TEXT))).is_ok());
        assert_eq!(
            command(key(&"K".repeat(MAX_KEY + 1), "a")).unwrap_err(),
            "invalid_input"
        );
        assert_eq!(
            command(key("KeyA", &"a".repeat(MAX_KEY_TEXT + 1))).unwrap_err(),
            "invalid_input"
        );
    }

    #[test]
    fn browser_origins_must_name_the_daemon_host() {
        let headers = |origin: Option<&str>| {
            let mut headers = HeaderMap::new();
            headers.insert(header::HOST, "127.0.0.1:4000".parse().unwrap());
            if let Some(origin) = origin {
                headers.insert(header::ORIGIN, origin.parse().unwrap());
            }
            headers
        };
        assert!(same_origin(&headers(None)));
        assert!(same_origin(&headers(Some("http://127.0.0.1:4000"))));
        assert!(!same_origin(&headers(Some("http://evil.example"))));
        assert!(!same_origin(&headers(Some("http://127.0.0.1:4001"))));
        assert!(!same_origin(&headers(Some("null"))));
    }

    #[test]
    fn page_addresses_of_non_network_documents_name_only_their_scheme() {
        assert_eq!(
            page_address("https://user:pw@example.com/a?q=1#f"),
            "https://example.com/a"
        );
        assert_eq!(page_address("about:blank"), "about:blank");
        for (url, shown) in [
            ("about:blank?leak#x", "about:"),
            ("data:text/html,<b>secret</b>", "data:"),
            ("javascript:alert(1)", "javascript:"),
            ("blob:https://example.com/0d3c", "blob:"),
            ("file:///etc/passwd", "file:"),
            ("not a url", ""),
        ] {
            assert_eq!(page_address(url), shown, "{url}");
        }
    }

    #[test]
    fn a_done_result_the_redactor_cannot_scrub_drops_the_page_fields() {
        let result = json!({"status":"done","tab_id":"t1","url":"https://x.test/typed",
            "title":"typed title","page_changed":true});
        assert_eq!(scrubbed(result.clone(), Ok(result.clone())), result);
        assert_eq!(
            scrubbed(result, Err("redactor unavailable".into())),
            json!({"status":"done","tab_id":"t1","page_changed":true,"redaction_failed":true})
        );
    }

    #[test]
    fn only_a_person_may_take_control_and_takeover_refuses_control_commands() {
        use openside_proto::DeviceKind;
        let phone = Caller(Some("d1".into()), DeviceKind::PersonDevice);
        let agent = Caller(Some("d2".into()), DeviceKind::AgentClient);
        // The socket's caller kind is AgentClient; only its author marker tells a person apart.
        let socket = Caller(None, DeviceKind::AgentClient);
        assert!(person_only(&phone, MessageAuthorKind::Person).is_ok());
        assert!(person_only(&socket, MessageAuthorKind::Person).is_ok());
        for (caller, author) in [
            (&agent, MessageAuthorKind::AgentClient),
            (&agent, MessageAuthorKind::Person),
            (&socket, MessageAuthorKind::AgentClient),
            (&socket, MessageAuthorKind::Channel),
            (&phone, MessageAuthorKind::AgentClient),
        ] {
            let refused = person_only(caller, author).unwrap_err();
            assert_eq!(
                (refused.0, refused.1),
                (StatusCode::FORBIDDEN, "person_device_required")
            );
        }
        for input in [
            TakeoverInput::Navigate {
                url: "https://example.com".into(),
            },
            TakeoverInput::Back,
            TakeoverInput::Forward,
            TakeoverInput::Reload,
            TakeoverInput::CloseTab,
        ] {
            assert_eq!(command(input).unwrap_err(), "invalid_input");
        }
    }
}
