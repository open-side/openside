//! `DELETE /v1/sessions/{id}`: stop the session, write a durable `session.deleted` tombstone
//! and hide the session.
//!
//! The whole operation runs inside the session's actor (`Action::Tombstone`), so a queued
//! delivery can never reopen the runtime between stopping and the tombstone, and the actor is
//! gone when the call returns. The event log stays on disk (it is append-only); the store
//! reports the session as unknown from then on and rejects any further append to it.

use axum::{
    extract::{Path, State},
    http::StatusCode,
};
use openside_proto::SessionId;

use crate::{
    engine::{Action, Shared},
    routines::ApiFailure,
};

pub(crate) async fn delete(
    State(s): State<Shared>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiFailure> {
    let id = SessionId::new(id);
    s.db.state(&id).await.map_err(|_| ApiFailure::not_found())?;
    if s.agents
        .list()
        .map_err(|_| ApiFailure::internal())?
        .iter()
        .any(|agent| agent.home_session_id == id)
    {
        return Err(ApiFailure::new(
            StatusCode::CONFLICT,
            "agent_home_immutable",
            "Retire the teammate instead of deleting its home conversation",
        ));
    }
    if s.command(&id, Action::Tombstone).await.is_err() {
        // A concurrent delete that won the race leaves a tombstone behind: report it as gone.
        return Err(if s.db.state(&id).await.is_err() {
            ApiFailure::not_found()
        } else {
            ApiFailure::internal()
        });
    }
    on_session_deleted(&s, &id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Daemon-internal hook run once a session is gone. Per-session cleanup (routine pausing,
/// browser tabs, the task artifact directory) belongs here so every deletion path shares it.
pub(crate) async fn on_session_deleted(shared: &Shared, id: &SessionId) {
    if let Some(scheduler) = shared.routines.get() {
        if let Err(error) = scheduler.on_session_deleted(id).await {
            crate::diag!("opensided: routine cleanup after session deletion failed: {error}");
        }
    }
    shared.registry.browser.close_incognito(shared, id).await;
    shared.registry.browser.close_space(shared, id).await;
    shared.registry.browser.forget_lost_tabs(id).await;
    crate::artifacts::remove_session(shared, id).await;
}
