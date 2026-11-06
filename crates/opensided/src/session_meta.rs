//! Session list filtering, durable list metadata (title, pinned, archived), a chat's own
//! options (model, effort, permission, confirmation) and its read marker.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use openside_proto::{
    normalize_title, EventKind, MessageAuthorKind, Session, SessionId, SessionList, Settings,
    ThinkingEffort, UpdateSessionRequest, MAX_SESSION_TITLE_CHARS,
};
use serde::Deserialize;

use crate::{
    api::{Caller, HttpError},
    engine::Shared,
};

#[derive(Deserialize)]
pub(crate) struct ListQuery {
    q: Option<String>,
    agent: Option<String>,
    /// `none` for chats outside every project, or a project id.
    project: Option<String>,
    #[serde(default)]
    archived: bool,
    /// Only the first N after ordering, for a "recent chats" view.
    limit: Option<usize>,
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
    let sessions: Vec<Session> =
        s.db.store(move |db| {
            let mut listed: Vec<_> = db
                .list_sessions()
                .into_iter()
                .filter(|session| session.archived == archived)
                .filter(|session| match query.agent.as_deref() {
                    None => true,
                    Some("none") => session.agent_id.is_none(),
                    Some(id) => session.agent_id.as_deref() == Some(id),
                })
                .filter(|session| match query.project.as_deref() {
                    None => true,
                    Some("none") => session.project_id.is_none(),
                    Some(id) => session.project_id.as_deref() == Some(id),
                })
                .filter_map(|session| {
                    let state = db.session(&session.id)?;
                    if let Some(needle) = &needle {
                        let matches = session.title.to_lowercase().contains(needle)
                            || state
                                .first_message
                                .as_deref()
                                .is_some_and(|text| text.to_lowercase().contains(needle));
                        if !matches {
                            return None;
                        }
                    }
                    Some((state.last_activity, session))
                })
                .collect();
            // Pinned first, then the most recently active; the store's identifier order is random,
            // so without this the list would reshuffle with every restart.
            listed.sort_by(|(a_at, a), (b_at, b)| {
                b.pinned.cmp(&a.pinned).then_with(|| b_at.cmp(a_at))
            });
            let limit = query.limit.unwrap_or(usize::MAX);
            Ok(listed
                .into_iter()
                .take(limit)
                .map(|(_, session)| session)
                .collect())
        })
        .await?;
    Ok(Json(SessionList { sessions }))
}

/// Display form of a client-supplied title, or `invalid_title` when none remains or it is too long.
pub(crate) fn valid_title(raw: &str) -> Result<String, HttpError> {
    normalize_title(raw)
        .filter(|title| title.chars().count() <= MAX_SESSION_TITLE_CHARS)
        .ok_or(HttpError(StatusCode::BAD_REQUEST, "invalid_title"))
}

pub(crate) async fn update(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(author): Extension<MessageAuthorKind>,
    Path(id): Path<String>,
    Json(request): Json<UpdateSessionRequest>,
) -> Result<Json<Session>, HttpError> {
    let id = SessionId::new(id);
    let current = s.db.state(&id).await.map_err(|_| HttpError::not_found())?;
    if request.is_empty() {
        return Err(HttpError::invalid_value());
    }
    let title = match request.title {
        Some(title) => Some(valid_title(&title)?),
        None => None,
    };
    let model = match request.model.as_deref().map(str::trim) {
        Some("") => return Err(HttpError::invalid_value()),
        other => other.map(str::to_owned),
    };
    // An agent's home conversation runs on its agent's model and ceiling.
    if model.is_some() || request.mode.is_some() {
        let home = crate::agents::for_session(&s, &current)?
            .is_some_and(|agent| agent.home_session_id == current.id);
        if home {
            return Err(HttpError(StatusCode::CONFLICT, "agent_home"));
        }
    }
    // The same limits a paired device has when it creates a chat.
    let costly = matches!(
        request.effort,
        Some(ThinkingEffort::Xhigh | ThinkingEffort::Max)
    ) && caller.0.is_some()
        && caller.1 == openside_proto::DeviceKind::AgentClient;
    if caller.widens(&s, request.mode, None) || costly {
        return Err(HttpError(StatusCode::FORBIDDEN, "local_socket_required"));
    }
    // A rename also claims an unchanged auto title, so a pending model proposal cannot win.
    let title = title.filter(|title| *title != current.title || !current.title_locked);
    let pinned = request.pinned.filter(|pinned| *pinned != current.pinned);
    let archived = request
        .archived
        .filter(|archived| *archived != current.archived);
    if title.is_some() || pinned.is_some() || archived.is_some() {
        s.emit(
            &id,
            None,
            EventKind::SessionUpdated {
                mode: None,
                title,
                pinned,
                archived,
            },
        )
        .await?;
    }
    let model = model.filter(|model| *model != current.model);
    let effort = request
        .effort
        .filter(|effort| Some(*effort) != current.effort);
    let mode = request.mode.filter(|mode| *mode != current.mode);
    let confirmation = request
        .confirmation
        .filter(|confirmation| *confirmation != current.confirmation);
    if model.is_some() || effort.is_some() || mode.is_some() || confirmation.is_some() {
        s.emit(
            &id,
            None,
            EventKind::SessionOptionsUpdated {
                model: model.clone(),
                effort,
                mode,
                confirmation,
            },
        )
        .await?;
    }
    let unread = current.session().unread;
    if let Some(read) = request.read.filter(|read| *read == unread) {
        s.emit(&id, None, EventKind::SessionRead { read }).await?;
    }
    // The model a person last picked for a chat becomes the default for the next one; an agent
    // or MCP client (including `openside mcp` on the local socket) changes only its own chat.
    if let Some(model) = model.filter(|_| author == MessageAuthorKind::Person) {
        let s = s.clone();
        tokio::spawn(async move { crate::models_api::remember_default(&s, &model).await });
    }
    if archived == Some(true) {
        s.registry.browser.cleanup_warm(&s, &id, None).await;
    }
    // Archiving ends an isolated space: its tabs close and its cookies and storage are wiped.
    if archived == Some(true) && current.browser_space == openside_proto::BrowserSpace::Isolated {
        s.registry.browser.close_space(&s, &id).await;
    }
    Ok(Json(s.db.state(&id).await?.session()))
}

pub(crate) async fn settings(State(s): State<Shared>) -> Json<Settings> {
    let models = s.models.models();
    Json(Settings {
        default_model: models.default,
        fallback_models: models.fallback,
        policy_mode: s.policy.default_mode(),
    })
}
