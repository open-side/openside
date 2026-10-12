//! Web Push notifications (F-2, PP-22).
//!
//! The VAPID key is created on first use at `<data_dir>/push-vapid.key` (0600).
//! Subscriptions live in `<data_dir>/push-subscriptions.json` (0600, atomic rewrite),
//! bound to the device whose token registered them; revoking the device drops them.
//! A `ticket.opened` event and a `session.settled` event of a session without pending
//! tickets each push a [`PushNotification`] to every subscription whose client has not
//! reported focus in the last [`PUSH_FOCUS_TTL_SECS`]. A 404/410 from the push service
//! deletes the subscription.

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
    PushSubscription, PushSubscriptionList, Ticket, TicketKind, TicketStatus, VapidPublicKey,
    PUSH_FOCUS_TTL_SECS,
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
        loop {
            let event = tokio::select! {
                _ = shutdown.recv() => break,
                event = events.recv() => event,
            };
            let event = match event {
                Ok(event) => event,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            match event.kind {
                EventKind::TicketOpened { ticket } if ticket.status == TicketStatus::Pending => {
                    let ttl = (ticket.expires_at - Utc::now())
                        .num_seconds()
                        .clamp(60, 86_400);
                    tokio::spawn(notify(s.clone(), ticket_notification(&ticket), ttl as u32));
                }
                EventKind::SessionSettled {} => {
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
                    let notification = PushNotification {
                        kind: "settled".into(),
                        session_id: event.session_id.as_str().to_owned(),
                        ticket_id: None,
                        title: "Task finished".into(),
                        body: truncate(&state.title),
                        url: format!("/sessions/{}", event.session_id.as_str()),
                    };
                    tokio::spawn(notify(s.clone(), notification, 3600));
                }
                _ => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::known_push_service;

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
}
