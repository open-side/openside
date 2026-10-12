use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{
        ws::{rejection::WebSocketUpgradeRejection, Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Extension,
};
use futures_util::StreamExt;
use openside_browser::Screencast;
use openside_proto::{
    CapabilityScope, Event, EventKind, PreviewEndReason, PreviewPauseReason, PreviewServerMessage,
    SessionId, SessionStatus,
};
use serde_json::json;
use tokio::sync::{mpsc, watch, Notify};
use tokio::time::Instant;

use crate::api::{Access, Caller, HttpError};
use crate::engine::Shared;
use crate::ws_stream::{text, CONTROL_SLOTS, QUEUE};

const INTERVAL: Duration = Duration::from_millis(500);
const MAX_VIEWERS: usize = 8;
const SIZE: (u32, u32) = (720, 450);

struct Item {
    message: PreviewServerMessage,
    generation: u64,
}
impl Item {
    fn control(message: PreviewServerMessage) -> Arc<Self> {
        Arc::new(Self {
            message,
            generation: 0,
        })
    }
}
struct Feed {
    messages: watch::Sender<Arc<Item>>,
    end: parking_lot::Mutex<Option<PreviewEndReason>>,
    finished: Notify,
}
impl Feed {
    fn publish(&self, message: Arc<Item>) {
        if self.end.lock().is_none() {
            self.messages.send_replace(message);
        }
    }
    fn finish(&self, reason: PreviewEndReason) {
        let mut end = self.end.lock();
        if end.is_none() {
            *end = Some(reason.clone());
            self.messages
                .send_replace(Item::control(PreviewServerMessage::Ended { reason }));
            self.finished.notify_one();
        }
    }
}
#[derive(Default)]
struct Generation {
    value: u64,
    tainted: bool,
}

struct Thumbnail {
    id: String,
    session: SessionId,
    tab: String,
    document: String,
    generation: u64,
    created: Instant,
    jpeg: axum::body::Bytes,
}

const THUMBNAIL_TTL: Duration = Duration::from_secs(600);
const MAX_THUMBNAILS: usize = 64;
const MAX_THUMBNAIL_BYTES: usize = 128 * 1024;
#[derive(Default)]
pub(crate) struct Registry {
    feeds: parking_lot::Mutex<HashMap<SessionId, Arc<Feed>>>,
    captures: parking_lot::Mutex<HashMap<(SessionId, String), Arc<Screencast>>>,
    generations: parking_lot::Mutex<HashMap<String, Generation>>,
    thumbnails: parking_lot::Mutex<VecDeque<Thumbnail>>,
}
impl Registry {
    fn invalidate(&self, tab: &str, tainted: bool) {
        {
            let mut generations = self.generations.lock();
            let generation = generations.entry(tab.to_owned()).or_default();
            generation.value = generation.value.wrapping_add(1);
            generation.tainted |= tainted;
        }
        self.thumbnails.lock().retain(|image| image.tab != tab);
        self.captures.lock().retain(|(_, captured), cast| {
            if captured == tab {
                cast.stop();
                false
            } else {
                true
            }
        });
    }
    fn generation(&self, tab: &str) -> (u64, bool) {
        let mut generations = self.generations.lock();
        let generation = generations.entry(tab.to_owned()).or_default();
        (generation.value, generation.tainted)
    }
    fn finish(&self, session: &SessionId, feed: &Arc<Feed>, reason: PreviewEndReason) {
        let mut feeds = self.feeds.lock();
        feed.finish(reason);
        if feeds
            .get(session)
            .is_some_and(|current| Arc::ptr_eq(current, feed))
        {
            feeds.remove(session);
            self.captures.lock().retain(|(owner, _), cast| {
                if owner == session {
                    cast.stop();
                    false
                } else {
                    true
                }
            });
        }
    }
}

pub(crate) fn committed(s: &Shared, event: &Event) {
    if matches!(
        event.kind,
        EventKind::SessionUpdated {
            archived: Some(true),
            ..
        }
    ) {
        s.previews
            .thumbnails
            .lock()
            .retain(|image| image.session != event.session_id);
    }
    let reason = match event.kind {
        EventKind::SessionStopped {} => PreviewEndReason::SessionStopped,
        EventKind::SessionDeleted {} => PreviewEndReason::SessionDeleted,
        _ => return,
    };
    s.previews
        .thumbnails
        .lock()
        .retain(|image| image.session != event.session_id);
    let feed = s.previews.feeds.lock().get(&event.session_id).cloned();
    if let Some(feed) = feed {
        s.previews.finish(&event.session_id, &feed, reason);
    }
}
pub(crate) fn yield_tab(s: &Shared, _: &SessionId, tab: &str) {
    s.previews.invalidate(tab, false);
}
pub(crate) fn withhold_tab(s: &Shared, tab: &str) {
    s.previews.invalidate(tab, true);
}

pub(crate) async fn capture_thumbnail(
    s: &Shared,
    session: &SessionId,
    tab: &str,
) -> Option<String> {
    let _gate = s.registry.browser.gate.lock().await;
    let state = s.db.state(session).await.ok()?;
    if state.incognito
        || state.deleted
        || state.archived
        || state.status == SessionStatus::Stopped
        || s.registry.browser.tab_tainted(tab).await
        || paused(s, session, tab).await
    {
        return None;
    }
    let browser = s.registry.browser.session_browser(session).await?;
    let document = browser.document_lifetime_id(tab).await.ok()?;
    let (generation, tainted) = s.previews.generation(tab);
    if tainted {
        return None;
    }
    let jpeg = tokio::time::timeout(
        Duration::from_secs(2),
        browser.thumbnail(session.as_str(), tab),
    )
    .await
    .ok()?
    .ok()?;
    if jpeg.len() > MAX_THUMBNAIL_BYTES
        || !jpeg.starts_with(&[0xff, 0xd8])
        || browser.document_lifetime_id(tab).await.ok().as_deref() != Some(&document)
        || s.registry.browser.tab_tainted(tab).await
    {
        return None;
    }
    let generations = s.previews.generations.lock();
    if !generations
        .get(tab)
        .is_some_and(|current| current.value == generation && !current.tainted)
    {
        return None;
    }
    let id = crate::persistence::random_id();
    let mut images = s.previews.thumbnails.lock();
    images.retain(|image| image.created.elapsed() < THUMBNAIL_TTL);
    if images.len() >= MAX_THUMBNAILS {
        images.pop_front();
    }
    images.push_back(Thumbnail {
        id: id.clone(),
        session: session.clone(),
        tab: tab.to_owned(),
        document,
        generation,
        created: Instant::now(),
        jpeg: jpeg.into(),
    });
    Some(id)
}

pub(crate) async fn thumbnail(
    State(s): State<Shared>,
    Path((id, thumbnail_id)): Path<(String, String)>,
) -> Result<Response, HttpError> {
    let session = SessionId::new(id);
    let state =
        s.db.state(&session)
            .await
            .map_err(|_| HttpError::not_found())?;
    if state.incognito || state.deleted || state.archived || state.status == SessionStatus::Stopped
    {
        return Err(HttpError::not_found());
    }
    let image = s
        .previews
        .thumbnails
        .lock()
        .iter()
        .find(|image| {
            image.id == thumbnail_id
                && image.session == session
                && image.created.elapsed() < THUMBNAIL_TTL
        })
        .map(|image| {
            (
                image.tab.clone(),
                image.document.clone(),
                image.generation,
                image.jpeg.clone(),
            )
        })
        .ok_or_else(HttpError::not_found)?;
    let _gate = s.registry.browser.gate.lock().await;
    let browser = s
        .registry
        .browser
        .session_browser(&session)
        .await
        .ok_or_else(HttpError::not_found)?;
    if s.registry.browser.tab_tainted(&image.0).await
        || paused(&s, &session, &image.0).await
        || browser.document_lifetime_id(&image.0).await.ok().as_deref() != Some(&image.1)
    {
        return Err(HttpError::not_found());
    }
    let generations = s.previews.generations.lock();
    if !generations
        .get(&image.0)
        .is_some_and(|current| current.value == image.2 && !current.tainted)
    {
        return Err(HttpError::not_found());
    }
    Ok((
        [
            ("Content-Type", "image/jpeg"),
            ("Cache-Control", "no-store"),
            ("X-Content-Type-Options", "nosniff"),
        ],
        image.3,
    )
        .into_response())
}

struct Capture {
    session: SessionId,
    tab: String,
    generation: u64,
    cast: Arc<Screencast>,
    registry: Arc<Registry>,
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.cast.stop();
        let key = (self.session.clone(), self.tab.clone());
        let mut casts = self.registry.captures.lock();
        if casts
            .get(&key)
            .is_some_and(|cast| Arc::ptr_eq(cast, &self.cast))
        {
            casts.remove(&key);
        }
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
    match open(s, access, caller, SessionId::new(id), headers, upgrade).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}
async fn open(
    s: Shared,
    access: Access,
    caller: Caller,
    session: SessionId,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Result<Response, HttpError> {
    if !access.has(CapabilityScope::Browse) {
        return Err(HttpError(StatusCode::FORBIDDEN, "forbidden"));
    }
    let state =
        s.db.state(&session)
            .await
            .map_err(|_| HttpError::not_found())?;
    if state.deleted {
        return Err(HttpError::not_found());
    }
    if state.incognito {
        return Err(HttpError(StatusCode::CONFLICT, "private_session"));
    }
    if !crate::takeover::same_origin(&headers) {
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
    let (feed, receiver) = {
        let mut feeds = s.previews.feeds.lock();
        if let Some(feed) = feeds.get(&session) {
            if feed.messages.receiver_count() >= MAX_VIEWERS {
                return Err(HttpError(StatusCode::TOO_MANY_REQUESTS, "too_many_viewers"));
            }
            (Arc::clone(feed), feed.messages.subscribe())
        } else {
            let (messages, receiver) = watch::channel(Item::control(PreviewServerMessage::NoTab));
            let feed = Arc::new(Feed {
                messages,
                end: Default::default(),
                finished: Notify::new(),
            });
            feeds.insert(session.clone(), Arc::clone(&feed));
            tokio::spawn(produce(s.clone(), session.clone(), Arc::clone(&feed)));
            (feed, receiver)
        }
    };
    // The durable replay closes the gap between the first state read and feed registration.
    // Later terminal commits synchronously latch the registered feed before stop returns.
    let reason = if state.status == SessionStatus::Stopped {
        Some(PreviewEndReason::SessionStopped)
    } else {
        terminal_since(&s, &session, state.last_seq).await?
    };
    if let Some(reason) = reason {
        s.previews.finish(&session, &feed, reason);
    }
    Ok(upgrade
        .protocols(["openside.v1"])
        .max_message_size(1024)
        .max_frame_size(1024)
        .on_upgrade(move |socket| run(s, session, caller.0, socket, feed, receiver, revoked))
        .into_response())
}
async fn terminal_since(
    s: &Shared,
    session: &SessionId,
    mut after: u64,
) -> Result<Option<PreviewEndReason>, HttpError> {
    loop {
        let events =
            s.db.events(session, after)
                .await
                .map_err(|_| HttpError::internal())?;
        if events.is_empty() {
            return Ok(None);
        }
        for event in events {
            after = event.seq;
            match event.kind {
                EventKind::SessionStopped {} => return Ok(Some(PreviewEndReason::SessionStopped)),
                EventKind::SessionDeleted {} => return Ok(Some(PreviewEndReason::SessionDeleted)),
                _ => {}
            }
        }
    }
}
async fn ended(s: &Shared, session: &SessionId) -> Option<PreviewEndReason> {
    match s.db.state(session).await {
        Ok(state) if state.deleted => Some(PreviewEndReason::SessionDeleted),
        Ok(state) if state.status == SessionStatus::Stopped => {
            Some(PreviewEndReason::SessionStopped)
        }
        Err(_) => Some(PreviewEndReason::SessionDeleted),
        _ => None,
    }
}
async fn paused(s: &Shared, session: &SessionId, tab: &str) -> bool {
    crate::takeover::blocked(s, session, "browser_snapshot", &json!({"tab_id":tab}))
        .await
        .is_some()
}
async fn produce(s: Shared, session: SessionId, feed: Arc<Feed>) {
    let mut capture: Option<Capture> = None;
    let mut check = tokio::time::interval(INTERVAL);
    check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut next_frame = Instant::now() + INTERVAL;
    loop {
        if feed.end.lock().is_some() {
            break;
        }
        let cast = capture.as_ref().map(|capture| Arc::clone(&capture.cast));
        tokio::select! {
            biased;
            () = feed.finished.notified() => break,
            () = feed.messages.closed() => {
                let mut feeds = s.previews.feeds.lock();
                if feed.messages.receiver_count() != 0 { continue; }
                if feeds.get(&session).is_some_and(|current| Arc::ptr_eq(current, &feed)) { feeds.remove(&session); }
                drop(capture.take());
                break;
            },
            _ = check.tick() => {
                if let Some(reason) = ended(&s, &session).await { s.previews.finish(&session, &feed, reason); break; }
                let _gate = tokio::select! {
                    () = feed.finished.notified() => break,
                    () = feed.messages.closed() => continue,
                    gate = s.registry.browser.gate.lock() => gate,
                };
                let Some((browser, tab)) = s.registry.browser.preview_tab(&session).await else {
                    capture = None;
                    feed.publish(Item::control(PreviewServerMessage::NoTab));
                    continue;
                };
                let (generation, tainted) = s.previews.generation(&tab);
                let status = if paused(&s, &session, &tab).await {
                    Some(PreviewServerMessage::Paused { reason: PreviewPauseReason::Takeover, tab_id: tab.clone() })
                } else if tainted || s.registry.browser.tab_tainted(&tab).await {
                    Some(PreviewServerMessage::Withheld { code: "screenshot_unavailable_after_secret_fill".into(), tab_id: tab.clone() })
                } else { None };
                if let Some(status) = status { capture = None; feed.publish(Item::control(status)); continue; }
                if capture.as_ref().is_some_and(|capture| capture.tab == tab && capture.generation == generation
                    && s.previews.captures.lock().get(&(session.clone(), tab.clone())).is_some_and(|cast| Arc::ptr_eq(cast, &capture.cast))) { continue; }
                capture = None;
                if let Ok(cast) = browser.screencast_scaled(session.as_str(), &tab, 40, SIZE).await {
                    let cast = Arc::new(cast);
                    s.previews.captures.lock().insert((session.clone(), tab.clone()), Arc::clone(&cast));
                    capture = Some(Capture { session: session.clone(), tab, generation, cast, registry: Arc::clone(&s.previews) });
                    next_frame = Instant::now() + INTERVAL;
                } else { feed.publish(Item::control(PreviewServerMessage::NoTab)); }
            }
            frame = async {
                tokio::time::sleep_until(next_frame).await;
                match &cast { Some(cast) => cast.next().await, None => std::future::pending().await }
            } => {
                let Some(current) = capture.as_ref() else { continue; };
                current.cast.ack(&frame);
                if !s.registry.browser.tab_tainted(&current.tab).await && !paused(&s, &session, &current.tab).await {
                    feed.publish(Arc::new(Item { generation: current.generation, message: PreviewServerMessage::Frame {
                        data: frame.data, width: SIZE.0, height: SIZE.1, tab_id: current.tab.clone(),
                    } }));
                }
                next_frame = Instant::now() + INTERVAL;
            }
        }
    }
    drop(capture);
    let mut feeds = s.previews.feeds.lock();
    if feeds
        .get(&session)
        .is_some_and(|current| Arc::ptr_eq(current, &feed))
    {
        feeds.remove(&session);
    }
}
enum Outgoing {
    Preview(Arc<Item>),
    Close,
}
async fn admit(
    s: Shared,
    session: SessionId,
    device: Option<String>,
    feed: Arc<Feed>,
    message: Outgoing,
) -> Option<Message> {
    let Outgoing::Preview(item) = message else {
        return Some(Message::Close(None));
    };
    if let PreviewServerMessage::Frame { tab_id, .. } = &item.message {
        if ended(&s, &session).await.is_some()
            || s.registry.browser.tab_tainted(tab_id).await
            || paused(&s, &session, tab_id).await
            || !s
                .registry
                .browser
                .preview_tab(&session)
                .await
                .is_some_and(|(_, tab)| tab == *tab_id)
        {
            return None;
        }
        #[cfg(test)]
        hooks::wait(device.as_deref(), hooks::Stage::Before).await;
        let message = {
            // This synchronous check is the frame admission boundary. Transition invalidation
            // shares the generation lock; a single already admitted send may finish afterward.
            let generations = s.previews.generations.lock();
            if !generations.get(tab_id).is_some_and(|generation| {
                generation.value == item.generation && !generation.tainted
            }) || feed.end.lock().is_some()
                || device
                    .as_deref()
                    .is_some_and(|device| s.remote.is_revoked(device))
            {
                return None;
            }
            text(&item.message)
        };
        #[cfg(test)]
        hooks::wait(device.as_deref(), hooks::Stage::After).await;
        return Some(message);
    }
    Some(text(&item.message))
}
async fn run(
    s: Shared,
    session: SessionId,
    device: Option<String>,
    socket: WebSocket,
    feed: Arc<Feed>,
    mut receiver: watch::Receiver<Arc<Item>>,
    mut revoked: tokio::sync::broadcast::Receiver<String>,
) {
    let (sink, mut source) = socket.split();
    let (out, queue) = mpsc::channel(QUEUE);
    let writer_s = s.clone();
    let writer_session = session.clone();
    let writer_device = device.clone();
    let writer_feed = Arc::clone(&feed);
    let writer = tokio::spawn(crate::ws_stream::write_all(sink, queue, move |message| {
        admit(
            writer_s.clone(),
            writer_session.clone(),
            writer_device.clone(),
            Arc::clone(&writer_feed),
            message,
        )
    }));
    let initial = Arc::clone(&receiver.borrow_and_update());
    let mut pending = if matches!(initial.message, PreviewServerMessage::Frame { .. }) {
        Some(initial)
    } else {
        let _ = out.try_send(Outgoing::Preview(initial));
        None
    };
    let mut send_at = Instant::now();
    let reason = loop {
        if let Some(reason) = feed.end.lock().clone() {
            break Some(reason);
        }
        tokio::select! {
            biased;
            () = crate::pairing::until_revoked(&s, &mut revoked, device.as_deref().unwrap_or_default()), if device.is_some() => break Some(PreviewEndReason::Revoked),
            () = out.closed() => break None,
            changed = receiver.changed() => {
                if changed.is_err() { break ended(&s, &session).await; }
                let item = Arc::clone(&receiver.borrow_and_update());
                if let PreviewServerMessage::Ended { reason } = &item.message { break Some(reason.clone()); }
                if matches!(item.message, PreviewServerMessage::Frame { .. }) { pending = Some(item); }
                else { pending = None; let _ = out.try_send(Outgoing::Preview(item)); }
            }
            () = tokio::time::sleep_until(send_at), if pending.is_some() => {
                if out.capacity() > CONTROL_SLOTS {
                    if let Some(frame) = pending.take() { let _ = out.try_send(Outgoing::Preview(frame)); }
                }
                else { pending = None; }
                send_at = Instant::now() + INTERVAL;
            }
            message = source.next() => match message {
                Some(Ok(Message::Close(_)) | Err(_)) | None => break None,
                Some(Ok(_)) => {}
            },
        }
    };
    drop(receiver);
    crate::ws_stream::close(
        out,
        writer,
        source,
        reason
            .map(|reason| Outgoing::Preview(Item::control(PreviewServerMessage::Ended { reason }))),
        Outgoing::Close,
    )
    .await;
}

#[cfg(test)]
pub(crate) mod hooks {
    use super::*;
    use std::sync::LazyLock;
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Stage {
        Before,
        After,
    }
    #[derive(Default)]
    pub(crate) struct Barrier {
        pub(crate) reached: Notify,
        pub(crate) release: Notify,
    }
    type Barriers = parking_lot::Mutex<HashMap<String, (Stage, Arc<Barrier>)>>;
    pub(crate) static BARRIERS: LazyLock<Barriers> = LazyLock::new(Default::default);
    pub(super) async fn wait(device: Option<&str>, stage: Stage) {
        let barrier = device.and_then(|device| {
            let mut barriers = BARRIERS.lock();
            if barriers
                .get(device)
                .is_some_and(|(expected, _)| *expected == stage)
            {
                barriers.remove(device).map(|(_, barrier)| barrier)
            } else {
                None
            }
        });
        if let Some(barrier) = barrier {
            barrier.reached.notify_one();
            barrier.release.notified().await;
        }
    }
}
