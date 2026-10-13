use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use openside_proto::{
    CreateRoutineRequest, Routine, RoutineAction, RoutineDraft, RoutineList, RoutineSuggestionList,
    RunList, RunRecord, UpdateRoutineRequest,
};
use openside_sched::{UserMessage, DEFAULT_RUN_LIMIT};
use serde::Deserialize;

use super::{ApiFailure, Sched};
use crate::{api::Caller, engine::Shared};

type Result<T, E = ApiFailure> = std::result::Result<T, E>;

fn person(caller: &Caller) -> Result<()> {
    if caller.0.is_some() && caller.1 != openside_proto::DeviceKind::PersonDevice {
        return Err(ApiFailure::new(
            StatusCode::FORBIDDEN,
            "person_device_required",
            "Use propose_routine and ask the person to approve its confirmation",
        ));
    }
    Ok(())
}

fn scheduler(shared: &Shared) -> Result<Arc<Sched>> {
    shared.routines.get().ok_or_else(ApiFailure::unavailable)
}

pub(crate) async fn list(State(s): State<Shared>) -> Result<Json<RoutineList>> {
    Ok(Json(RoutineList {
        routines: scheduler(&s)?.list()?,
    }))
}

/// Routines run later with the daemon's authority, so a device may not use one to get past
/// the limits that `POST /v1/sessions` applies to it.
fn ensure_not_widening(s: &Shared, caller: &Caller, action: &RoutineAction) -> Result<()> {
    if let RoutineAction::NewSession {
        mode, workspace, ..
    } = action
    {
        if caller.widens(s, *mode, workspace.as_deref()) {
            return Err(ApiFailure::new(
                StatusCode::FORBIDDEN,
                "local_socket_required",
                "local socket required",
            ));
        }
    }
    Ok(())
}

pub(crate) async fn create(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<CreateRoutineRequest>,
) -> Result<(StatusCode, Json<Routine>)> {
    person(&caller)?;
    ensure_not_widening(&s, &caller, &request.action)?;
    Ok((
        StatusCode::CREATED,
        Json(scheduler(&s)?.create(request).await?),
    ))
}

pub(crate) async fn get(State(s): State<Shared>, Path(id): Path<String>) -> Result<Json<Routine>> {
    Ok(Json(scheduler(&s)?.get(&id)?))
}

pub(crate) async fn patch(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    Json(request): Json<UpdateRoutineRequest>,
) -> Result<Json<Routine>> {
    if request.name.is_some()
        || request.schedule.is_some()
        || request.action.is_some()
        || request.catch_up.is_some()
        || request.enabled == Some(true)
    {
        person(&caller)?;
    }
    if let Some(action) = &request.action {
        ensure_not_widening(&s, &caller, action)?;
    }
    Ok(Json(scheduler(&s)?.update(&id, request).await?))
}

pub(crate) async fn delete(
    State(s): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Routine>> {
    Ok(Json(scheduler(&s)?.delete(&id).await?))
}

pub(crate) async fn run(
    State(s): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<RunRecord>> {
    Ok(Json(scheduler(&s)?.run_now(&id).await?))
}

#[derive(Deserialize)]
pub(crate) struct RunsQuery {
    limit: Option<usize>,
}

pub(crate) async fn runs(
    State(s): State<Shared>,
    Path(id): Path<String>,
    Query(query): Query<RunsQuery>,
) -> Result<Json<RunList>> {
    let runs = scheduler(&s)?
        .runs(&id, query.limit.unwrap_or(DEFAULT_RUN_LIMIT))
        .await?;
    Ok(Json(RunList { runs }))
}

pub(crate) async fn suggestions(State(s): State<Shared>) -> Result<Json<RoutineSuggestionList>> {
    let scheduler = scheduler(&s)?;
    let queued =
        s.db.store(|store| Ok(store.queued_messages(openside_sched::suggest::MAX_MESSAGES)?))
            .await
            .map_err(|_| ApiFailure::internal())?;
    let messages: Vec<UserMessage> = queued
        .into_iter()
        .filter(|message| {
            message.author_kind == openside_proto::MessageAuthorKind::Person
                && message.from_agent.is_none()
        })
        .map(|message| UserMessage {
            session_id: message.session_id,
            message_id: message.message_id.to_string(),
            text: message.text,
            at: message.at,
        })
        .collect();
    let suggestions: Vec<RoutineDraft> = scheduler.suggestions(&messages)?;
    Ok(Json(RoutineSuggestionList { suggestions }))
}
