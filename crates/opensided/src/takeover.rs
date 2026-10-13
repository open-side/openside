//! Remote human takeover of an agent tab (HS-6, `docs/specs/human-step.md` §3).
//!
//! `GET /v1/tickets/{id}/takeover` upgrades to a WebSocket that streams JPEG frames of the
//! ticket's tab and forwards the controller's input to that tab only. While a takeover is
//! being opened or is pending, agent browser tools that target its tab are refused
//! ([`reserve`], [`blocked`]). Every terminal transition of the ticket calls [`finish`] before
//! it publishes the new status: that closes input admission (waiting out input already
//! admitted), stops the capture and voids the tab's captures and element references, so the
//! agent regains the tab only after the person has provably lost it.

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

static LOGIN_STARTS: LazyLock<parking_lot::Mutex<HashMap<(SessionId, String), Instant>>> =
    LazyLock::new(Default::default);

/// The controller of one takeover ticket. Input dispatch and [`finish`] serialize on `admit`,
/// so once `finish` returns no input of this lease runs again.
struct Lease {
    expires_at: DateTime<Utc>,
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

/// Tickets with a connected controller; one controller per ticket.
static LEASES: LazyLock<parking_lot::Mutex<HashMap<TicketId, Arc<Lease>>>> =
    LazyLock::new(Default::default);

/// Tabs whose takeover ticket is being opened, as `(session, tab)` → number of openers; each
/// opener's [`Reservation`] owns one count, so one opener ending never clears another's.
static RESERVED: LazyLock<parking_lot::Mutex<HashMap<(SessionId, String), usize>>> =
    LazyLock::new(Default::default);

/// Releases a controller's lease when its stream ends or never starts.
struct Held(TicketId, Arc<Lease>);
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
    let lease = LEASES.lock().get(&ticket.id).cloned();
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
    result["note"] = json!("A person controlled this tab; earlier snapshot ids and refs are void. Take a fresh browser_snapshot before acting.");
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
        // The tab ledger's title lags behind the page; read the live document instead.
        if let Ok(url) = browser.url(tab_id).await {
            result["url"] = json!(crate::browser::public_url(&url));
        }
        if let Ok(Value::String(title)) = browser.evaluate(tab_id, "document.title").await {
            result["title"] = json!(title.chars().take(200).collect::<String>());
        }
    }
    // URLs and titles can echo values the person typed; stored results pass the redactor.
    shared.registry.redact(result.clone()).unwrap_or(result)
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
    if LEASES.lock().contains_key(&ticket.id) {
        return Err(HttpError(StatusCode::CONFLICT, "takeover_in_use"));
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
        if LEASES.lock().contains_key(&ticket.id) {
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
            expires_at: ticket.expires_at,
            admit: tokio::sync::Mutex::new(true),
            cast: Arc::new(cast),
            end: watch::channel(None).0,
        });
        LEASES.lock().insert(ticket.id.clone(), Arc::clone(&lease));
        Held(ticket.id.clone(), lease)
    };
    let session = Live {
        s,
        ticket: ticket.id,
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

struct Live {
    s: Shared,
    ticket: TicketId,
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
        let idle_after = idle_limit(&self.ticket);
        let mut idle = Instant::now() + idle_after;
        let mut check = tokio::time::interval(CHECK);
        // The last delivered frame is acknowledged once FRAME_INTERVAL has passed.
        let mut unacked: Option<(ScreencastFrame, Instant)> = None;
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
                        Ok(()) => idle = Instant::now() + idle_after,
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
        let ticket = match crate::tickets::get(&self.s, &self.ticket).await {
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
        if !self.browser.is_connected()
            || Controller::owned(&self.browser, &self.session, &self.tab)
                .await
                .is_err()
        {
            return Some(TakeoverEndReason::TabUnavailable);
        }
        None
    }

    /// Forward one input message to the ticket's tab, inside the lease's admission boundary
    /// and this device's revocation gate. Errors carry static codes only: typed text and keys
    /// never reach logs, events or responses.
    async fn input(&self, lease: &Lease, body: &str) -> Result<(), Refused> {
        let input: TakeoverInput =
            serde_json::from_str(body).map_err(|_| Refused::Input("invalid_input"))?;
        let (method, params) = command(input).map_err(Refused::Input)?;
        let open = lease.admit.lock().await;
        if !*open || lease.expires_at <= Utc::now() {
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
        hooks::admitted(&self.ticket).await;
        self.browser
            .page_command(self.session.as_str(), &self.tab, method, params)
            .await
            .map(|_| ())
            .map_err(|_| Refused::Input("input_failed"))
    }
}

#[cfg(not(test))]
fn idle_limit(_: &TicketId) -> Duration {
    IDLE
}

#[cfg(test)]
fn idle_limit(ticket: &TicketId) -> Duration {
    hooks::IDLE.lock().get(ticket).copied().unwrap_or(IDLE)
}

/// Per-ticket test controls; keyed by ticket so parallel tests never see each other's.
#[cfg(test)]
pub(crate) mod hooks {
    use super::*;
    use tokio::sync::Notify;

    pub(crate) static IDLE: LazyLock<parking_lot::Mutex<HashMap<TicketId, Duration>>> =
        LazyLock::new(Default::default);
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
}
