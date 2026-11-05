//! Web Push notifications (F-2, PP-22).
//!
//! The VAPID key is created on first use at `<data_dir>/push-vapid.key` (0600).
//! Subscriptions live in `<data_dir>/push-subscriptions.json` (0600, atomic rewrite),
//! bound to the device whose token registered them; revoking the device drops them.
//! A `ticket.opened` event and a `session.settled` event of a session without pending
//! tickets each push a [`PushNotification`] to every subscription whose client has not
//! reported focus in the last [`PUSH_FOCUS_TTL_SECS`]. A settled push is titled with the
//! session title and carries a plain-text [`excerpt`] of the run's last answer, except for
//! incognito sessions. A 404/410 from the push service deletes the subscription.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use chrono::{DateTime, Utc};
use openside_proto::{
    CreatePushSubscriptionRequest, EventKind, PushNotification, PushPresenceRequest,
    PushSubscription, PushSubscriptionList, SessionId, Ticket, TicketKind, TicketStatus,
    VapidPublicKey, PUSH_FOCUS_TTL_SECS,
};
use openside_push::{Sender, Subscription, VapidKey};
use serde::{Deserialize, Serialize};

use crate::{
    api::{Access, Caller, HttpError},
    engine::Shared,
    persistence::random_id,
};

#[derive(Clone, Serialize, Deserialize)]
struct Stored {
    id: String,
    device_id: Option<String>,
    endpoint: String,
    p256dh: String,
    auth: String,
    created_at: DateTime<Utc>,
}

impl Stored {
    fn public(&self) -> PushSubscription {
        PushSubscription {
            id: self.id.clone(),
            device_id: self.device_id.clone(),
            endpoint: self.endpoint.clone(),
            created_at: self.created_at,
        }
    }
}

#[derive(Default)]
pub(crate) struct PushState {
    key: tokio::sync::OnceCell<Arc<VapidKey>>,
    subscriptions: tokio::sync::Mutex<Option<Vec<Stored>>>,
    focus: parking_lot::Mutex<HashMap<String, Instant>>,
}

fn key_path(s: &Shared) -> PathBuf {
    openside_config::data_layout::DataPath::Vapid.path(&s.config.data_dir)
}
fn subscriptions_path(s: &Shared) -> PathBuf {
    openside_config::data_layout::DataPath::Subscriptions.path(&s.config.data_dir)
}

async fn key(s: &Shared) -> Result<Arc<VapidKey>, HttpError> {
    let path = key_path(s);
    s.remote
        .push
        .key
        .get_or_try_init(|| async move {
            tokio::task::spawn_blocking(move || VapidKey::load_or_create(&path).map(Arc::new))
                .await
                .map_err(|_| HttpError::internal())?
                .map_err(|error| {
                    crate::diag!("opensided: {error}");
                    HttpError::internal()
                })
        })
        .await
        .cloned()
}

/// The owner keeps serialization through rename even if its HTTP waiter is dropped.
async fn with_subscriptions<T: Send + 'static>(
    s: &Shared,
    f: impl FnOnce(&mut Vec<Stored>) -> (T, bool) + Send + 'static,
) -> Result<T, HttpError> {
    let s = s.clone();
    tokio::spawn(async move {
        let mut guard = s.remote.push.subscriptions.lock().await;
        if guard.is_none() {
            let path = subscriptions_path(&s);
            let loaded = match tokio::fs::read(&path).await {
                Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| HttpError::internal())?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(_) => return Err(HttpError::internal()),
            };
            *guard = Some(loaded);
        }
        // Move, rather than clone, the candidate out while serialized. A failed commit
        // leaves the cache unloaded, so the next operation reloads the durable snapshot.
        let mut candidate = guard.take().ok_or_else(HttpError::internal)?;
        let (value, changed) = f(&mut candidate);
        if changed {
            let bytes = serde_json::to_vec_pretty(&candidate).map_err(|_| HttpError::internal())?;
            let path = subscriptions_path(&s);
            tokio::task::spawn_blocking(move || crate::persistence::write_private(&path, &bytes))
                .await
                .map_err(|_| HttpError::internal())?
                .map_err(|_| HttpError::internal())?;
        }
        *guard = Some(candidate);
        Ok(value)
    })
    .await
    .map_err(|_| HttpError::internal())?
}

fn visible(stored: &Stored, access: &Access, caller: &Caller) -> bool {
    access.has(openside_proto::CapabilityScope::Admin) || stored.device_id == caller.0
}

pub(crate) async fn vapid(State(s): State<Shared>) -> Result<Json<VapidPublicKey>, HttpError> {
    Ok(Json(VapidPublicKey {
        public_key: key(&s).await?.public_key(),
    }))
}

pub(crate) async fn list(
    State(s): State<Shared>,
    Extension(access): Extension<Access>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<PushSubscriptionList>, HttpError> {
    let subscriptions = with_subscriptions(&s, move |list| {
        let visible = list
            .iter()
            .filter(|stored| visible(stored, &access, &caller))
            .map(Stored::public)
            .collect();
        (visible, false)
    })
    .await?;
    Ok(Json(PushSubscriptionList { subscriptions }))
}

/// Push services browsers subscribe to. Anything else must be listed in `network.allow`.
const PUSH_SERVICES: &[&str] = &[
    "fcm.googleapis.com",
    "android.googleapis.com",
    "*.push.services.mozilla.com",
    "*.push.apple.com",
    "*.notify.windows.com",
];

fn known_push_service(host: &str) -> bool {
    // Chrome 152 hands out `https://jmt<N>.google.com/fcm/send/…` rather than fcm.googleapis.com
    // (observed with the real service); only the numbered shard hosts are accepted, not all of google.com.
    let chrome_shard = host
        .strip_suffix(".google.com")
        .and_then(|label| label.strip_prefix("jmt"))
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    chrome_shard
        || PUSH_SERVICES
            .iter()
            .any(|pattern| match pattern.strip_prefix("*.") {
                Some(domain) => host
                    .strip_suffix(domain)
                    .is_some_and(|rest| rest.len() > 1 && rest.ends_with('.')),
                None => host == *pattern,
            })
}

/// Globally routable unicast; everything else is this host, its LAN or special-purpose.
pub(crate) fn public_ip(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(ip) => {
            let [a, b, ..] = ip.octets();
            !(ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.is_multicast()
                || a == 0
                || (a == 100 && (64..128).contains(&b))
                || (a == 198 && (b == 18 || b == 19))
                || a >= 240)
        }
        IpAddr::V6(ip) => {
            let first = ip.segments()[0];
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
                || first == 0x2001 && ip.segments()[1] == 0x0db8)
        }
    }
}

struct Destination {
    host: String,
    port: u16,
    /// Named in `network.allow`: the user chose it, so its address may be local.
    explicit: bool,
}

/// Endpoint policy: an https endpoint of a known browser push service, or a
/// destination the user listed in `network.allow`. The endpoint is chosen by a paired
/// device, so loopback and LAN addresses are not implied the way they are for
/// daemon-configured services.
fn destination(s: &Shared, endpoint: &str) -> Option<Destination> {
    let url = url::Url::parse(endpoint).ok()?;
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let host = url
        .host_str()?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    let port = url.port_or_known_default()?;
    let explicit = openside_policy::Egress::new(&s.config.loaded.config.network.allow)
        .is_ok_and(|egress| egress.allows_explicitly(url.scheme(), &host, port));
    let known = url.scheme() == "https" && known_push_service(&host);
    if !(explicit || known) {
        return None;
    }
    Some(Destination {
        host,
        port,
        explicit,
    })
}

/// Re-vet right before a send and pick the address to connect to, so a DNS answer that
/// changed since registration cannot point the daemon at an internal service.
async fn vet(s: &Shared, endpoint: &str) -> Option<SocketAddr> {
    let destination = destination(s, endpoint)?;
    let addresses: Vec<SocketAddr> =
        tokio::net::lookup_host((destination.host.as_str(), destination.port))
            .await
            .ok()?
            .collect();
    if addresses.is_empty()
        || !(destination.explicit || addresses.iter().all(|address| public_ip(address.ip())))
    {
        crate::diag!("opensided: push endpoint resolves to a non-public address; not sending");
        return None;
    }
    addresses.first().copied()
}

pub(crate) async fn create(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<CreatePushSubscriptionRequest>,
) -> Result<(StatusCode, Json<PushSubscription>), HttpError> {
    if destination(&s, &request.endpoint).is_none()
        || openside_push::validate_keys(&request.keys.p256dh, &request.keys.auth).is_err()
    {
        return Err(HttpError(StatusCode::BAD_REQUEST, "invalid_subscription"));
    }
    // The client needs the key to subscribe; make sure it exists before any send.
    key(&s).await?;
    let stored = Stored {
        id: random_id(),
        device_id: caller.0.clone(),
        endpoint: request.endpoint,
        p256dh: request.keys.p256dh,
        auth: request.keys.auth,
        created_at: Utc::now(),
    };
    let public = stored.public();
    // Commit section: revoke marks the device before it clears its subscriptions under this
    // same lock, so either the mark is seen here or the entry is cleared after.
    let remote = s.remote.clone();
    let admitted = with_subscriptions(&s, move |list| {
        if stored
            .device_id
            .as_deref()
            .is_some_and(|device| remote.is_revoked(device))
        {
            return (false, false);
        }
        // A browser re-subscribing reports the same endpoint; keep one entry.
        list.retain(|existing| existing.endpoint != stored.endpoint);
        list.push(stored);
        (true, true)
    })
    .await?;
    if !admitted {
        return Err(HttpError(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    Ok((StatusCode::CREATED, Json(public)))
}

pub(crate) async fn delete(
    State(s): State<Shared>,
    Extension(access): Extension<Access>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> Result<StatusCode, HttpError> {
    let (removed, id) = with_subscriptions(&s, move |list| {
        let before = list.len();
        list.retain(|stored| !(stored.id == id && visible(stored, &access, &caller)));
        let removed = list.len() != before;
        ((removed, id), removed)
    })
    .await?;
    if !removed {
        return Err(HttpError(StatusCode::NOT_FOUND, "unknown_subscription"));
    }
    s.remote.push.focus.lock().remove(&id);
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn presence(
    State(s): State<Shared>,
    Extension(access): Extension<Access>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<PushPresenceRequest>,
) -> Result<StatusCode, HttpError> {
    let PushPresenceRequest {
        subscription_id,
        focused,
    } = request;
    let (known, subscription_id) = with_subscriptions(&s, move |list| {
        let known = list
            .iter()
            .any(|stored| stored.id == subscription_id && visible(stored, &access, &caller));
        ((known, subscription_id), false)
    })
    .await?;
    if !known {
        return Err(HttpError(StatusCode::NOT_FOUND, "unknown_subscription"));
    }
    let mut focus = s.remote.push.focus.lock();
    if focused {
        focus.insert(
            subscription_id,
            s.remote.instant() + Duration::from_secs(PUSH_FOCUS_TTL_SECS.unsigned_abs()),
        );
    } else {
        focus.remove(&subscription_id);
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn forget_device(s: &Shared, device_id: &str) -> Result<(), HttpError> {
    let device_id = device_id.to_owned();
    let removed = with_subscriptions(s, move |list| {
        let removed: Vec<String> = list
            .iter()
            .filter(|stored| stored.device_id.as_deref() == Some(device_id.as_str()))
            .map(|stored| stored.id.clone())
            .collect();
        list.retain(|stored| stored.device_id.as_deref() != Some(device_id.as_str()));
        let changed = !removed.is_empty();
        (removed, changed)
    })
    .await?;
    let mut focus = s.remote.push.focus.lock();
    for id in removed {
        focus.remove(&id);
    }
    Ok(())
}

fn truncate(text: &str) -> String {
    let mut out: String = text.chars().take(200).collect();
    if out.len() < text.len() {
        out.push('…');
    }
    out
}

const EXCERPT_CHARS: usize = 140;
const SETTLED: &str = "Task finished";

/// Lock-screen text of an answer: Markdown syntax, fenced code, images, citation links
/// and `[citation needed]` markers removed, whitespace collapsed, at most
/// [`EXCERPT_CHARS`] characters. `None` when nothing readable is left.
fn excerpt(answer: &str) -> Option<String> {
    let mut plain = String::new();
    let mut fence: Option<&str> = None;
    for line in answer.lines() {
        let line = line.trim();
        if let Some(marker) = fence {
            if line.starts_with(marker) {
                fence = None;
            }
            continue;
        }
        if let Some(marker) = ["```", "~~~"].into_iter().find(|m| line.starts_with(m)) {
            fence = Some(marker);
            continue;
        }
        let line = block_text(line);
        if !line.is_empty() {
            inline_text(line, &mut plain);
            plain.push(' ');
        }
    }
    let mut out = String::new();
    for word in plain.split_whitespace() {
        // Removed citations leave "fact ." behind.
        if !out.is_empty() && !word.starts_with(['.', ',', ';', ':', '!', '?']) {
            out.push(' ');
        }
        out.push_str(word);
    }
    if out.is_empty() {
        return None;
    }
    if out.chars().count() > EXCERPT_CHARS {
        out = out.chars().take(EXCERPT_CHARS - 1).collect();
        out.truncate(out.trim_end().len());
        out.push('…');
    }
    Some(out)
}

/// A line without its block markup: quote, heading and list markers, table pipes;
/// horizontal rules and table separators become empty.
fn block_text(mut line: &str) -> &str {
    while let Some(rest) = line.strip_prefix('>') {
        line = rest.trim_start();
    }
    if line.len() >= 3 && line.chars().all(|c| matches!(c, '-' | '*' | '_' | ' ')) {
        return "";
    }
    if line.starts_with('|') && line.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
        return "";
    }
    let hashes = line.len() - line.trim_start_matches('#').len();
    if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
        return line[hashes..].trim_start();
    }
    for bullet in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(bullet) {
            line = rest.trim_start();
            return line
                .strip_prefix("[ ] ")
                .or_else(|| line.strip_prefix("[x] "))
                .unwrap_or(line);
        }
    }
    let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits > 0 {
        if let Some(rest) = line[digits..]
            .strip_prefix(". ")
            .or_else(|| line[digits..].strip_prefix(") "))
        {
            return rest.trim_start();
        }
    }
    line
}

/// Inline Markdown to plain text: links keep their label unless it is only a citation,
/// images, autolinks and HTML tags go, emphasis and code marks go.
fn inline_text(text: &str, out: &mut String) {
    const MARKER: &str = "[citation needed]";
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if rest.starts_with(MARKER) {
            rest = &rest[MARKER.len()..];
            continue;
        }
        let image = c == '!' && rest[1..].starts_with('[');
        let at = usize::from(image);
        if c == '[' || image {
            if let Some((label, after)) = link(&rest[at..]) {
                if !image && !citation_label(label) {
                    inline_text(label, out);
                }
                rest = after;
                continue;
            }
        }
        if c == '<' {
            if let Some(end) = rest.find('>') {
                let tag = &rest[1..end];
                if tag.starts_with(|c: char| c.is_ascii_alphabetic() || c == '/' || c == '!')
                    && !tag.contains('<')
                {
                    rest = &rest[end + 1..];
                    continue;
                }
            }
        }
        match c {
            '*' | '`' | '~' => {}
            '|' => out.push(' '),
            // `snake_case` keeps its underscores; only emphasis at a word edge goes.
            '_' if out.ends_with(|p: char| p.is_alphanumeric())
                && rest[1..].starts_with(|n: char| n.is_alphanumeric()) =>
            {
                out.push('_')
            }
            '_' => {}
            '\\' if rest[1..].starts_with(|n: char| n.is_ascii_punctuation()) => {
                rest = &rest[1..];
                let escaped = rest.chars().next().unwrap_or_default();
                out.push(escaped);
                rest = &rest[escaped.len_utf8()..];
                continue;
            }
            c => out.push(c),
        }
        rest = &rest[c.len_utf8()..];
    }
}

/// `[label](target)` at the start of `text`: the label and what follows the link.
fn link(text: &str) -> Option<(&str, &str)> {
    let close = text.find("](")?;
    let label = &text[1..close];
    if label.contains(['[', ']']) {
        return None;
    }
    let target = &text[close + 2..];
    let end = target.find(')')?;
    Some((label, &target[end + 1..]))
}

/// Research answers cite every fact as `[source](url)`; the label carries nothing to read.
fn citation_label(label: &str) -> bool {
    let label = label.trim();
    label.is_empty()
        || label.eq_ignore_ascii_case("source")
        || label.eq_ignore_ascii_case("sources")
        || label.chars().all(|c| c.is_ascii_digit())
        || label.starts_with("http://")
        || label.starts_with("https://")
}

fn settled_notification(
    session_id: &SessionId,
    title: &str,
    incognito: bool,
    excerpt: Option<String>,
) -> PushNotification {
    // Incognito answers stay off the lock screen.
    let body = excerpt
        .filter(|_| !incognito)
        .unwrap_or_else(|| SETTLED.into());
    let title = if title.trim().is_empty() {
        SETTLED.into()
    } else {
        truncate(title)
    };
    PushNotification {
        kind: "settled".into(),
        session_id: session_id.as_str().to_owned(),
        ticket_id: None,
        title,
        body,
        url: format!("/sessions/{}", session_id.as_str()),
    }
}

const MAX_ANSWERS: usize = 256;

/// Excerpt of the latest answer per session since its run started; the event log has no
/// cheap backwards read, and the bus already carries every final answer. Bounded because
/// a session can answer and never settle.
#[derive(Default)]
struct Answers {
    next: u64,
    by_session: HashMap<SessionId, (u64, Option<String>)>,
}

impl Answers {
    fn record(&mut self, session: SessionId, answer: &str) {
        self.next += 1;
        self.by_session
            .insert(session, (self.next, excerpt(answer)));
        if self.by_session.len() > MAX_ANSWERS {
            let oldest = self
                .by_session
                .iter()
                .min_by_key(|(_, (order, _))| *order)
                .map(|(session, _)| session.clone());
            if let Some(oldest) = oldest {
                self.by_session.remove(&oldest);
            }
        }
    }

    fn take(&mut self, session: &SessionId) -> Option<String> {
        self.by_session.remove(session).and_then(|(_, excerpt)| excerpt)
    }
}

fn ticket_notification(ticket: &Ticket) -> PushNotification {
    let title = match ticket.kind {
        TicketKind::Confirm => "Approval needed",
        TicketKind::Info => "Question from the agent",
        TicketKind::SecretInput => "Code needed",
        TicketKind::DeviceApproval if ticket.approval.is_some() => "Approve on your phone",
        TicketKind::DeviceApproval => "Approve on your device",
        TicketKind::Takeover => "Action needed",
        TicketKind::Login => "Sign-in needed",
        TicketKind::Choice => "Choice needed",
        TicketKind::Hire => "Teammate proposal",
    };
    PushNotification {
        kind: "ticket".into(),
        session_id: ticket.session_id.as_str().to_owned(),
        ticket_id: Some(ticket.id.as_str().to_owned()),
        title: title.into(),
        body: truncate(&ticket.summary),
        url: format!(
            "/sessions/{}?ticket={}",
            ticket.session_id.as_str(),
            ticket.id.as_str()
        ),
    }
}

fn contact(s: &Shared) -> String {
    let remote = &s.config.loaded.config.remote;
    remote
        .push_contact
        .clone()
        .or_else(|| {
            remote
                .public_url
                .clone()
                .filter(|url| url.starts_with("https://"))
        })
        .unwrap_or_else(|| "mailto:openside@localhost".into())
}

async fn notify(s: Shared, notification: PushNotification, ttl_secs: u32) {
    let now = s.remote.instant();
    let targets = match with_subscriptions(&s, |list| (list.clone(), false)).await {
        Ok(list) => list,
        Err(_) => return,
    };
    let targets: Vec<Stored> = {
        let focus = s.remote.push.focus.lock();
        targets
            .into_iter()
            .filter(|stored| focus.get(&stored.id).map_or(true, |until| *until <= now))
            .collect()
    };
    if targets.is_empty() {
        return;
    }
    let (Ok(key), Ok(sender), Ok(payload)) = (
        key(&s).await,
        Sender::new(),
        serde_json::to_vec(&notification),
    ) else {
        return;
    };
    let subject = contact(&s);
    let mut gone = Vec::new();
    for stored in targets {
        // Subscribed before the liveness check, so a revoke after it aborts the send
        // (DNS or HTTP) instead of waiting for it.
        let mut revocations = s.remote.revoked.subscribe();
        if let Some(device) = &stored.device_id {
            if s.remote.is_revoked(device)
                || !matches!(s.db.device_exists(device.clone()).await, Ok(true))
            {
                continue;
            }
        }
        let subscription = Subscription {
            endpoint: &stored.endpoint,
            p256dh: &stored.p256dh,
            auth: &stored.auth,
        };
        let attempt = async {
            let address = vet(&s, &stored.endpoint).await?;
            Some(
                sender
                    .send(
                        &key,
                        &subscription,
                        &payload,
                        ttl_secs,
                        &subject,
                        Some(address),
                    )
                    .await,
            )
        };
        let outcome = match stored.device_id.as_deref() {
            Some(device) => tokio::select! {
                outcome = attempt => outcome,
                () = crate::pairing::until_revoked(&s, &mut revocations, device) => None,
            },
            None => attempt.await,
        };
        let Some(outcome) = outcome else {
            continue;
        };
        match outcome {
            Ok(404 | 410) => gone.push(stored.id),
            Ok(status) if !(200..300).contains(&status) => {
                crate::diag!("opensided: push service answered {status}");
            }
            Ok(_) => {}
            Err(error) => crate::diag!("opensided: {error}"),
        }
    }
    if !gone.is_empty() {
        let remote = s.remote.clone();
        let _ = with_subscriptions(&s, move |list| {
            // Cleanup for a revoked device is owned by revoke and must be written last.
            list.retain(|stored| {
                !gone.contains(&stored.id)
                    || stored
                        .device_id
                        .as_deref()
                        .is_some_and(|id| remote.is_revoked(id))
            });
            ((), true)
        })
        .await;
    }
}

/// Follow the event bus and push ticket and settlement notifications.
pub(crate) fn start(shared: &Shared) {
    let s = shared.clone();
    let mut events = shared.events.subscribe();
    let mut shutdown = shared.shutdown.subscribe();
    tokio::spawn(async move {
        let mut answers = Answers::default();
        loop {
            let event = tokio::select! {
                _ = shutdown.recv() => break,
                event = events.recv() => event,
            };
            let event = match event {
                Ok(event) => event,
                // A missed event may have been a newer answer or a turn start: never
                // attach an answer that might be stale.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    answers.by_session.clear();
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            match event.kind {
                EventKind::TicketOpened { ticket } if ticket.status == TicketStatus::Pending => {
                    let ttl = (ticket.expires_at - Utc::now())
                        .num_seconds()
                        .clamp(60, 86_400);
                    tokio::spawn(notify(s.clone(), ticket_notification(&ticket), ttl as u32));
                }
                EventKind::AssistantFinal { text, .. } => {
                    answers.record(event.session_id, &text);
                }
                EventKind::TurnStarted {}
                | EventKind::SessionStopped {}
                | EventKind::SessionDeleted {} => {
                    answers.take(&event.session_id);
                }
                EventKind::SessionSettled {} => {
                    let excerpt = answers.take(&event.session_id);
                    let Ok(state) = s.db.state(&event.session_id).await else {
                        continue;
                    };
                    // A run that ended waiting for a human already pushed its ticket.
                    if state
                        .tickets
                        .values()
                        .any(|ticket| ticket.status == TicketStatus::Pending)
                    {
                        continue;
                    }
                    let notification = settled_notification(
                        &event.session_id,
                        &state.title,
                        state.incognito,
                        excerpt,
                    );
                    let s = s.clone();
                    let session = event.session_id;
                    tokio::spawn(async move {
                        // A session deleted after the state read must not be announced.
                        if s.db.state(&session).await.is_ok() {
                            notify(s, notification, 3600).await;
                        }
                    });
                }
                _ => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use openside_proto::SessionId;

    use super::{excerpt, known_push_service, settled_notification, Answers, MAX_ANSWERS};

    #[test]
    fn chrome_numbered_shards_are_known_but_other_google_hosts_are_not() {
        for host in ["fcm.googleapis.com", "jmt17.google.com", "jmt0.google.com"] {
            assert!(known_push_service(host), "{host}");
        }
        for host in [
            "google.com",
            "sites.google.com",
            "jmt.google.com",
            "jmt1x.google.com",
            "a.jmt17.google.com",
            "jmt17.google.com.evil.test",
        ] {
            assert!(!known_push_service(host), "{host}");
        }
    }

    #[test]
    fn excerpt_strips_markdown_and_citations() {
        let answer = "## Flights\n\n\
            Found **3** matching flights [source](https://a.test/1).\n\
            - The cheapest is `UA124` at 08:30 [1](https://b.test) [citation needed].\n\
            > See the [fare table](https://c.test/t) and ![chart](https://c.test/c.png)\n\
            ```json\n{\"secret\": \"code block\"}\n```\n\
            | a | b |\n|---|---|\n| x_y | <b>z</b> |";
        assert_eq!(
            excerpt(answer).as_deref(),
            Some(
                "Flights Found 3 matching flights. The cheapest is UA124 at 08:30. \
                 See the fare table and a b x_y z"
            )
        );
        assert_eq!(excerpt("```\nonly code\n```\n---\n"), None);
        assert_eq!(excerpt("   "), None);
        assert_eq!(excerpt("1. step one\n2) step _two_"), Some("step one step two".into()));
        assert_eq!(excerpt(r"a \*literal\* star"), Some("a *literal* star".into()));
    }

    #[test]
    fn excerpt_is_bounded_on_char_boundaries() {
        let long = "가나다 ".repeat(100);
        let out = excerpt(&long).unwrap();
        assert_eq!(out.chars().count(), 140);
        assert!(out.ends_with('…'));
        assert!(!out.trim_end_matches('…').ends_with(' '));
        let exact = "é".repeat(140);
        assert_eq!(excerpt(&exact), Some(exact));
    }

    #[test]
    fn settled_notification_is_titled_by_session_and_hides_incognito_answers() {
        let id = SessionId::new("s1");
        let shown = settled_notification(&id, "Find flights", false, excerpt("**Found** it."));
        assert_eq!(
            (shown.kind.as_str(), shown.title.as_str(), shown.body.as_str()),
            ("settled", "Find flights", "Found it.")
        );
        assert_eq!(shown.url, "/sessions/s1");
        assert_eq!(shown.ticket_id, None);
        let private = settled_notification(&id, "Find flights", true, excerpt("Found it."));
        assert_eq!(private.body, "Task finished");
        let silent = settled_notification(&id, "", false, excerpt("```\ncode\n```"));
        assert_eq!((silent.title.as_str(), silent.body.as_str()), ("Task finished", "Task finished"));
        assert_eq!(settled_notification(&id, "t", false, None).body, "Task finished");
    }

    #[test]
    fn answers_keep_excerpts_and_evict_the_oldest_session() {
        let mut answers = Answers::default();
        answers.record(SessionId::new("old"), &"word ".repeat(1000));
        let kept = answers.by_session[&SessionId::new("old")].1.as_deref().unwrap();
        assert_eq!(kept.chars().count(), 140);
        for n in 0..MAX_ANSWERS {
            answers.record(SessionId::new(&format!("s{n}")), "answer");
        }
        assert_eq!(answers.by_session.len(), MAX_ANSWERS);
        assert_eq!(answers.take(&SessionId::new("old")), None);
        assert_eq!(answers.take(&SessionId::new("s0")).as_deref(), Some("answer"));
        assert_eq!(answers.take(&SessionId::new("s0")), None);
    }
}
