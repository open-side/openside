//! Native computer use in the daemon: the person turns it on in Settings (local access only),
//! approves each app an agent asks to connect, and Guard asks before every click, keystroke
//! or scroll (see [`crate::tools::computer`]). This module owns the per-chat connections, the
//! frames the agent saw (streamed to the app), and every way a connection ends: the Stop
//! route, the chat stopping, archiving or deleting, and the setting being turned off.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::sse::{Event as SseEvent, KeepAlive, Sse},
    Extension, Json,
};
use base64::Engine;
use futures_util::Stream;
use openside_computer::{Backend, Computer, Connection, Eligibility, Feature, Phase};
use openside_proto::{
    ComputerUseConnection, ComputerUseCursor, ComputerUseFeature, ComputerUseFrame,
    ComputerUsePhase, ComputerUseSessionState, ComputerUseStatus, ComputerUseStopResult,
    ComputerUseStreamEvent, Event, EventKind, SessionId,
};
use serde_json::json;
use tokio::sync::watch;

use crate::api::{Caller, HttpError};
use crate::engine::Shared;

/// Longest frame side shown to the model and the app; larger windows are halved.
pub(crate) const MAX_FRAME_EDGE: u32 = 1600;
const ELIGIBILITY_TTL: Duration = Duration::from_secs(30);

/// The frame the agent last saw of a chat's connected window.
#[derive(Clone)]
pub(crate) struct SeenFrame {
    connection: String,
    seq: u64,
    png: Arc<Vec<u8>>,
    width: u32,
    height: u32,
}

pub(crate) struct Service {
    computer: parking_lot::RwLock<Arc<Computer>>,
    eligibility: parking_lot::Mutex<Option<(Instant, Eligibility)>>,
    frames: parking_lot::Mutex<HashMap<String, SeenFrame>>,
    frame_seq: std::sync::atomic::AtomicU64,
    frame_changes: watch::Sender<u64>,
    /// Held by a connect from its setting check to its insert, and by turning the setting off
    /// from the switch through stopping every connection.
    pub(crate) gate: tokio::sync::Mutex<()>,
    viewers: parking_lot::Mutex<HashMap<String, usize>>,
}

impl Default for Service {
    fn default() -> Self {
        Self::with_backend(openside_computer::platform_backend())
    }
}

impl Service {
    fn with_backend(backend: Arc<dyn Backend>) -> Self {
        Self {
            computer: parking_lot::RwLock::new(Arc::new(Computer::new(backend))),
            eligibility: parking_lot::Mutex::new(None),
            frames: parking_lot::Mutex::default(),
            frame_seq: std::sync::atomic::AtomicU64::new(0),
            frame_changes: watch::channel(0).0,
            gate: tokio::sync::Mutex::new(()),
            viewers: parking_lot::Mutex::default(),
        }
    }

    /// Replace the desktop, for tests.
    #[cfg(test)]
    pub(crate) fn set_backend(&self, backend: Arc<dyn Backend>) {
        *self.computer.write() = Arc::new(Computer::new(backend));
        *self.eligibility.lock() = None;
        self.frames.lock().clear();
    }

    pub(crate) fn computer(&self) -> Arc<Computer> {
        self.computer.read().clone()
    }

    /// What works here, probed at most every [`ELIGIBILITY_TTL`].
    pub(crate) async fn eligibility(&self) -> Eligibility {
        if let Some((at, eligibility)) = self.eligibility.lock().as_ref() {
            if at.elapsed() < ELIGIBILITY_TTL {
                return eligibility.clone();
            }
        }
        let eligibility = self.computer().eligibility().await;
        *self.eligibility.lock() = Some((Instant::now(), eligibility.clone()));
        eligibility
    }

    /// The last probe's verdict; `None` before the first probe.
    pub(crate) fn known_eligible(&self) -> Option<bool> {
        self.eligibility
            .lock()
            .as_ref()
            .map(|(_, eligibility)| eligibility.eligible())
    }

    /// Record the frame the agent was just shown.
    pub(crate) fn seen(
        &self,
        session: &str,
        connection: &str,
        png: Vec<u8>,
        width: u32,
        height: u32,
    ) {
        let seq = self
            .frame_seq
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        self.frames.lock().insert(
            session.to_owned(),
            SeenFrame {
                connection: connection.to_owned(),
                seq,
                png: Arc::new(png),
                width,
                height,
            },
        );
        self.frame_changes.send_replace(seq);
    }

    /// Forget the frame of `connection` only: a reconnect racing a stop keeps its own frame.
    fn forget(&self, session: &str, connection: &str) {
        let mut frames = self.frames.lock();
        if frames
            .get(session)
            .is_some_and(|frame| frame.connection == connection)
        {
            frames.remove(session);
        }
        drop(frames);
        self.frame_changes.send_modify(|_| {});
    }

    fn frame(&self, session: &str) -> Option<SeenFrame> {
        let frame = self.frames.lock().get(session).cloned()?;
        // A frame outlives its connection only until the next stop clears it; never show one
        // from an earlier connection next to a newer one.
        let current = self.computer().connection(session)?;
        (current.id == frame.connection).then_some(frame)
    }
}

fn feature(feature: &Feature) -> ComputerUseFeature {
    ComputerUseFeature {
        available: feature.available,
        via: feature.via.clone(),
        reason: feature.reason.clone(),
    }
}

pub(crate) fn status(enabled: bool, eligibility: &Eligibility) -> ComputerUseStatus {
    let mut reasons = eligibility.reasons();
    reasons.dedup();
    ComputerUseStatus {
        enabled,
        eligible: eligibility.eligible(),
        platform: eligibility.platform.to_owned(),
        backend: eligibility.backend.map(str::to_owned),
        capture: feature(&eligibility.capture),
        pointer: feature(&eligibility.pointer),
        keyboard: feature(&eligibility.keyboard),
        reasons,
    }
}

pub(crate) fn connection(connection: &Connection) -> ComputerUseConnection {
    ComputerUseConnection {
        id: connection.id.clone(),
        generation: connection.generation,
        app: connection.window.app.clone(),
        title: connection.window.title.clone(),
        pid: connection.window.pid,
        phase: match connection.phase {
            Phase::Connected => ComputerUsePhase::Connected,
            Phase::Acting => ComputerUsePhase::Acting,
            Phase::Capturing => ComputerUsePhase::Capturing,
        },
        frame_width: connection.view.map(|view| view.width),
        frame_height: connection.view.map(|view| view.height),
        cursor: connection.cursor.map(|cursor| ComputerUseCursor {
            x: cursor.x,
            y: cursor.y,
            held: cursor.held,
        }),
    }
}

fn session_state(s: &Shared, session: &str) -> ComputerUseSessionState {
    let service = &s.computer;
    ComputerUseSessionState {
        connection: service
            .computer()
            .connection(session)
            .as_ref()
            .map(connection),
        frame_seq: service.frame(session).map(|frame| frame.seq),
    }
}

pub(crate) fn enabled(s: &Shared) -> bool {
    s.preferences.get().computer_use
}

/// End a chat's connection now and audit why.
pub(crate) async fn stop(s: &Shared, session: &str, reason: &str) -> Option<Connection> {
    let stopped = s.computer.computer().stop(session);
    if let Some(stopped) = &stopped {
        s.computer.forget(session, &stopped.id);
        crate::pairing::audit(
            s,
            "computer_use",
            "stopped",
            json!({
                "session_id": session,
                "connection": stopped.id,
                "app": stopped.window.app,
                "reason": reason,
            }),
        )
        .await;
    }
    stopped
}

/// End every connection, as when the person turns computer use off.
pub(crate) async fn stop_all(s: &Shared, reason: &str) {
    for session in s.computer.computer().sessions() {
        stop(s, &session, reason).await;
    }
}

/// A chat that stops, is archived or is deleted loses its connection with it.
pub(crate) fn committed(s: &Shared, event: &Event) {
    let reason = match event.kind {
        EventKind::SessionStopped {} => "session_stopped",
        EventKind::SessionDeleted {} => "session_deleted",
        EventKind::SessionUpdated {
            archived: Some(true),
            ..
        } => "session_archived",
        _ => return,
    };
    if s.computer
        .computer()
        .connection(event.session_id.as_str())
        .is_none()
    {
        return;
    }
    // Cancel the in-flight action synchronously; only the audit line waits.
    let stopped = s.computer.computer().stop(event.session_id.as_str());
    if let Some(stopped) = stopped {
        s.computer.forget(event.session_id.as_str(), &stopped.id);
        let s = s.clone();
        let session = event.session_id.to_string();
        tokio::spawn(async move {
            crate::pairing::audit(
                &s,
                "computer_use",
                "stopped",
                json!({"session_id": session, "connection": stopped.id, "app": stopped.window.app, "reason": reason}),
            )
            .await;
        });
    }
}

pub(crate) async fn get_status(State(s): State<Shared>) -> Json<ComputerUseStatus> {
    let eligibility = s.computer.eligibility().await;
    Json(status(enabled(&s), &eligibility))
}

async fn existing(
    s: &Shared,
    session: &SessionId,
) -> Result<openside_sessions::SessionState, HttpError> {
    let state =
        s.db.state(session)
            .await
            .map_err(|_| HttpError::not_found())?;
    if state.deleted {
        return Err(HttpError::not_found());
    }
    Ok(state)
}

/// The chat's connection: app, title, pid, phase and pointer, which only the person may read,
/// like the frames.
pub(crate) async fn get_session(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<openside_core::MessageAuthorKind>,
    Path(id): Path<String>,
) -> Result<Json<ComputerUseSessionState>, HttpError> {
    crate::takeover::person_only(&caller, author)?;
    let session = SessionId::new(id);
    let state = existing(&s, &session).await?;
    if state.incognito {
        return Err(HttpError(StatusCode::CONFLICT, "private_session"));
    }
    Ok(Json(session_state(&s, session.as_str())))
}

pub(crate) async fn post_stop(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> Result<Json<ComputerUseStopResult>, HttpError> {
    let session = SessionId::new(id);
    existing(&s, &session).await?;
    let reason = if caller.0.is_some() {
        "person_stop_remote"
    } else {
        "person_stop"
    };
    let stopped = stop(&s, session.as_str(), reason).await;
    Ok(Json(ComputerUseStopResult {
        stopped: stopped.is_some(),
        connection: stopped.as_ref().map(connection),
    }))
}

fn sse(event: &ComputerUseStreamEvent) -> Option<SseEvent> {
    let name = match event {
        ComputerUseStreamEvent::State(_) => "state",
        ComputerUseStreamEvent::Frame(_) => "frame",
    };
    let data = serde_json::to_string(event).ok()?;
    Some(SseEvent::default().event(name).data(data))
}

/// The desktop frames of one chat, for the person only: the local app or one of their own
/// paired devices (a phone may watch on purpose), never an agent client.
pub(crate) async fn frames(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<openside_core::MessageAuthorKind>,
    Path(id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>, HttpError> {
    crate::takeover::person_only(&caller, author)?;
    let session = SessionId::new(id);
    let state = existing(&s, &session).await?;
    if state.incognito {
        return Err(HttpError(StatusCode::CONFLICT, "private_session"));
    }
    let viewer = Viewer::join(&s, session.as_str())
        .ok_or(HttpError(StatusCode::TOO_MANY_REQUESTS, "too_many_viewers"))?;
    let mut changes = s.computer.computer().changes();
    let mut frames = s.computer.frame_changes.subscribe();
    let mut events = s.events.subscribe();
    let stream = async_stream::stream! {
        let _viewer = viewer;
        let mut last_state = None;
        let mut last_frame = None;
        loop {
            let current = session_state(&s, session.as_str());
            if last_state.as_ref() != Some(&current) {
                if let Some(event) = sse(&ComputerUseStreamEvent::State(current.clone())) {
                    yield Ok(event);
                }
                last_state = Some(current);
            }
            if let Some(frame) = s.computer.frame(session.as_str()) {
                if last_frame != Some(frame.seq) {
                    last_frame = Some(frame.seq);
                    let event = ComputerUseStreamEvent::Frame(ComputerUseFrame {
                        connection: frame.connection.clone(),
                        seq: frame.seq,
                        width: frame.width,
                        height: frame.height,
                        mime: "image/png".into(),
                        data: base64::engine::general_purpose::STANDARD.encode(frame.png.as_slice()),
                    });
                    if let Some(event) = sse(&event) {
                        yield Ok(event);
                    }
                }
            }
            tokio::select! {
                changed = changes.changed() => if changed.is_err() { break },
                changed = frames.changed() => if changed.is_err() { break },
                event = events.recv() => match event {
                    Ok(event) if event.session_id == session
                        && matches!(event.kind, EventKind::SessionDeleted {}) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    _ => {}
                },
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// Open frame streams of one chat; each re-encodes every frame, so they are bounded.
const MAX_VIEWERS: usize = 4;

struct Viewer {
    shared: Shared,
    session: String,
}

impl Viewer {
    fn join(s: &Shared, session: &str) -> Option<Self> {
        let mut viewers = s.computer.viewers.lock();
        let count = viewers.entry(session.to_owned()).or_insert(0);
        if *count >= MAX_VIEWERS {
            return None;
        }
        *count += 1;
        Some(Self {
            shared: s.clone(),
            session: session.to_owned(),
        })
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        let mut viewers = self.shared.computer.viewers.lock();
        if let Some(count) = viewers.get_mut(&self.session) {
            *count -= 1;
            if *count == 0 {
                viewers.remove(&self.session);
            }
        }
    }
}
