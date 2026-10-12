//! Session list filtering and durable list metadata (title, pinned, archived).

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use openside_proto::{
    EventKind, Session, SessionId, SessionList, Settings, UpdateSessionRequest,
    MAX_SESSION_TITLE_CHARS,
};
use serde::Deserialize;

use crate::{api::HttpError, engine::Shared};

#[derive(Deserialize)]
pub(crate) struct ListQuery {
    q: Option<String>,
    #[serde(default)]
    archived: bool,
}

pub(crate) async fn list(
    State(s): State<Shared>,
    Query(query): Query<ListQuery>,
) -> Result<Json<SessionList>, HttpError> {
    let needle = query
        .q
        .map(|q| q.trim().to_lowercase())
        .filter(|q| !q.is_empty());
    let archived = query.archived;
    let mut sessions: Vec<Session> =
        s.db.store(move |db| {
            Ok(db
                .list_sessions()
                .into_iter()
                .filter(|session| session.archived == archived)
                .filter(|session| {
                    let Some(needle) = &needle else { return true };
                    session.title.to_lowercase().contains(needle)
                        || db
                            .session(&session.id)
                            .and_then(|state| state.first_message.as_deref())
                            .is_some_and(|text| text.to_lowercase().contains(needle))
                })
                .collect())
        })
        .await?;
    // Stable sort keeps the store's identifier order within each group.
    sessions.sort_by_key(|session| !session.pinned);
    Ok(Json(SessionList { sessions }))
}

pub(crate) async fn update(
    State(s): State<Shared>,
    Path(id): Path<String>,
    Json(request): Json<UpdateSessionRequest>,
) -> Result<Json<Session>, HttpError> {
    let id = SessionId::new(id);
    let current = s.db.state(&id).await.map_err(|_| HttpError::not_found())?;
    if request.is_empty() {
        return Err(HttpError::invalid_value());
    }
    let title = match request.title {
        Some(title) => {
            let title = title.trim();
            if title.is_empty() || title.chars().count() > MAX_SESSION_TITLE_CHARS {
                return Err(HttpError(StatusCode::BAD_REQUEST, "invalid_title"));
            }
            Some(title.to_owned())
        }
        None => None,
    };
    // Unchanged fields are dropped so the log only records real transitions.
    let title = title.filter(|title| *title != current.title);
    let pinned = request.pinned.filter(|pinned| *pinned != current.pinned);
    let archived = request
        .archived
        .filter(|archived| *archived != current.archived);
    if title.is_some() || pinned.is_some() || archived.is_some() {
        s.emit(
            &id,
            None,
            EventKind::SessionUpdated {
                title,
                pinned,
                archived,
            },
        )
        .await?;
    }
    Ok(Json(s.db.state(&id).await?.session()))
}

pub(crate) async fn settings(State(s): State<Shared>) -> Json<Settings> {
    let config = &s.config.loaded.config;
    Json(Settings {
        default_model: config.models.default.clone(),
        fallback_models: config.models.fallback.clone(),
        policy_mode: s.policy.default_mode(),
    })
}
