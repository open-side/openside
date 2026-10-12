//! Routine scheduling inside the daemon: the [`DaemonHost`] that performs run side effects,
//! the one-second tick loop, and the `/v1/routines` HTTP handlers.
//!
//! A routine run reaches a session through the same engine path as
//! `POST /v1/sessions/{id}/messages`: a `message.queued` event (queue lane) followed by a
//! `Deliver` command. Routine-sent messages carry the `routine-` id prefix so repetition
//! analysis can tell them from user messages.

mod api;

use std::{sync::Arc, time::Duration};

use axum::{http::StatusCode, response::IntoResponse, response::Response, Json};
use openside_core::{MessageId, MessageStatus, QueueMode};
use openside_proto::{ApiError, EventKind, MessageOutcome, SessionId};
use openside_sched::{
    Dispatch, HostError, NewSessionSpec, Progress, RoutineHost, Scheduler, Store, SystemClock,
};
use parking_lot::RwLock;

pub(crate) use api::{create, delete, get, list, patch, run, runs, suggestions};

use crate::{
    engine::{Action, Shared},
    persistence::Result,
};

pub(crate) type Sched = Scheduler<DaemonHost>;

/// Holder that lets `Shared` reference the scheduler without a reference cycle: the tick
/// loop clears it on shutdown, releasing the host's clone of `Shared`.
#[derive(Default)]
pub(crate) struct Slot(RwLock<Option<Arc<Sched>>>);

impl Slot {
    pub(crate) fn get(&self) -> Option<Arc<Sched>> {
        self.0.read().clone()
    }
    fn set(&self, scheduler: Arc<Sched>) {
        *self.0.write() = Some(scheduler);
    }
    fn clear(&self) {
        *self.0.write() = None;
    }
}

/// Side effects of routine runs, executed through the daemon's session engine.
pub(crate) struct DaemonHost {
    shared: Shared,
}

impl DaemonHost {
    /// Queue the run's prompt under the message id the scheduler already stored. Appending is
    /// idempotent per message id, so repeating a dispatch can never queue the prompt twice.
    async fn submit(
        &self,
        dispatch: &Dispatch,
        prompt: &str,
    ) -> std::result::Result<(), HostError> {
        let shared = &self.shared;
        let session = &dispatch.session_id;
        // A session deleted while we were working is the target's absence, not a failure.
        let classify = |reason: &'static str| async move {
            if shared.db.state(session).await.is_err() {
                HostError::TargetMissing
            } else {
                HostError::Failed(reason.into())
            }
        };
        if shared
            .auth_failed(session)
            .await
            .map_err(|_| HostError::TargetMissing)?
        {
            return Err(HostError::Failed(
                "session authentication is unavailable".into(),
            ));
        }
        if shared
            .emit(
                session,
                None,
                EventKind::MessageQueued {
                    message_id: dispatch.message_id.clone(),
                    text: prompt.to_owned(),
                    mode: QueueMode::Queue,
                    attachments: Vec::new(),
                },
            )
            .await
            .is_err()
        {
            return Err(classify("session rejected the prompt").await);
        }
        if shared.command(session, Action::Deliver).await.is_err() {
            return Err(classify("could not deliver the prompt to the runtime").await);
        }
        Ok(())
    }
}

impl RoutineHost for DaemonHost {
    async fn start_session(
        &self,
        spec: &NewSessionSpec,
        dispatch: &Dispatch,
    ) -> std::result::Result<(), HostError> {
        let shared = &self.shared;
        let model = spec
            .model
            .clone()
            .or_else(|| shared.config.loaded.config.models.default.clone())
            .filter(|model| !model.trim().is_empty())
            .ok_or_else(|| HostError::Failed("model_not_configured".into()))?;
        let mode = spec.mode.unwrap_or_else(|| shared.policy.default_mode());
        let workspace = match &spec.workspace {
            Some(path) => {
                let canonical = tokio::fs::canonicalize(path)
                    .await
                    .ok()
                    .filter(|path| path.is_dir())
                    .ok_or_else(|| HostError::Failed("invalid_workspace".into()))?;
                Some(canonical.to_string_lossy().into_owned())
            }
            None => None,
        };
        let id = dispatch.session_id.clone();
        let title = spec.title.clone();
        let created = shared
            .db
            .store(move |db| Ok(db.create_session(id, title, model, mode, workspace)?))
            .await
            .map_err(|_| HostError::Failed("could not create the session".into()))?;
        let _ = shared.events.send(created);
        self.submit(dispatch, &spec.prompt).await
    }

    async fn follow_up(
        &self,
        dispatch: &Dispatch,
        prompt: &str,
    ) -> std::result::Result<(), HostError> {
        if self.shared.db.state(&dispatch.session_id).await.is_err() {
            return Err(HostError::TargetMissing);
        }
        self.submit(dispatch, prompt).await
    }

    async fn progress(&self, session: &SessionId, message: &MessageId) -> Progress {
        let Ok(state) = self.shared.db.state(session).await else {
            return Progress::Missing;
        };
        match state.queue.status(message) {
            None => Progress::NotDispatched,
            Some(MessageStatus::Queued(_)) => Progress::InProgress,
            Some(MessageStatus::Dropped(_)) => Progress::Failed("message was dropped".into()),
            Some(MessageStatus::Delivered) => match state.completions.get(message) {
                None | Some(MessageOutcome::Deferred) => Progress::InProgress,
                Some(MessageOutcome::Completed) => Progress::Completed,
                Some(MessageOutcome::Aborted) => Progress::Failed("run was aborted".into()),
                Some(MessageOutcome::Failed) => Progress::Failed("run failed".into()),
            },
        }
    }

    async fn session_exists(&self, session: &SessionId) -> bool {
        self.shared.db.state(session).await.is_ok()
    }
}

/// Open `routines.db`, reconcile what happened while the daemon was down, and start ticking.
pub(crate) async fn start(shared: &Shared) -> Result<()> {
    let path = openside_config::data_layout::DataPath::Routines.path(&shared.config.data_dir);
    let store = tokio::task::spawn_blocking(move || Store::open(path)).await??;
    let scheduler = Arc::new(Scheduler::new(
        store,
        Arc::new(SystemClock),
        DaemonHost {
            shared: shared.clone(),
        },
    ));
    shared.routines.set(scheduler.clone());
    if let Err(error) = scheduler.recover().await {
        crate::diag!("opensided: routine recovery failed: {error}");
    }
    let slot = shared.routines.clone();
    let mut shutdown = shared.shutdown.subscribe();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_error = String::new();
        loop {
            tokio::select! {
                _ = shutdown.recv() => break,
                _ = tick.tick() => {
                    match scheduler.tick().await {
                        Ok(_) => last_error.clear(),
                        Err(error) => {
                            let error = error.to_string();
                            if error != last_error {
                                crate::diag!("opensided: routine scheduling failed: {error}");
                                last_error = error;
                            }
                        }
                    }
                }
            }
        }
        slot.clear();
    });
    Ok(())
}

/// Error body shared by the routine and session-deletion handlers.
pub(crate) struct ApiFailure(StatusCode, String, String);

impl ApiFailure {
    pub(crate) fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self(status, code.to_owned(), message.into())
    }
    pub(crate) fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "not found")
    }
    pub(crate) fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "internal error",
        )
    }
    pub(crate) fn unavailable() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "routines_unavailable",
            "routine scheduler is not running",
        )
    }
}

impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(ApiError {
                code: self.1,
                message: self.2,
            }),
        )
            .into_response()
    }
}

impl From<openside_sched::Error> for ApiFailure {
    fn from(error: openside_sched::Error) -> Self {
        match error {
            openside_sched::Error::UnknownRoutine(_) => Self::not_found(),
            openside_sched::Error::Invalid(invalid) => {
                Self::new(StatusCode::BAD_REQUEST, invalid.code(), invalid.to_string())
            }
            _ => Self::internal(),
        }
    }
}
