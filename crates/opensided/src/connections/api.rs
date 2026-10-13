use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use openside_core::{HostToolHandler, ToolCall, ToolOutcome};
use serde_json::json;

use super::*;
use crate::{
    api::{Caller, HttpError},
    engine::Shared,
};

type ApiResult<T> = std::result::Result<T, HttpError>;
fn internal(_: impl std::fmt::Display) -> HttpError {
    HttpError::internal()
}
fn unavailable() -> HttpError {
    HttpError(
        StatusCode::SERVICE_UNAVAILABLE,
        "connection_browser_unavailable",
    )
}
fn missing() -> HttpError {
    HttpError(StatusCode::NOT_FOUND, "connection_not_found")
}
fn denied() -> HttpError {
    HttpError(StatusCode::FORBIDDEN, "connection_denied")
}
fn document_error(error: openside_browser::BrowserError) -> HttpError {
    match error {
        openside_browser::BrowserError::ConnectionDenied => denied(),
        openside_browser::BrowserError::ConnectionGrantRequired { .. } => {
            HttpError(StatusCode::FORBIDDEN, "connection_grant_required")
        }
        _ => unavailable(),
    }
}

pub(crate) async fn list(
    State(shared): State<Shared>,
    Query(query): Query<ConnectionListQuery>,
) -> ApiResult<Json<ConnectionList>> {
    refresh(&shared).await.map_err(internal)?;
    let rows = shared.connections.list().map_err(internal)?;
    let mut response = ConnectionList {
        browser_available: shared.registry.browser.available(),
        connections: Vec::new(),
        candidates: Vec::new(),
    };
    for row in rows {
        if query.status.is_some_and(|status| row.status != status) {
            continue;
        }
        if let Some(agent) = &query.agent {
            if row
                .grants
                .iter()
                .any(|grant| &grant.agent_id == agent && grant.access == ConnectionAccess::Denied)
            {
                continue;
            }
        }
        if row.status == ConnectionStatus::Candidate {
            response.candidates.push(row);
        } else {
            response.connections.push(row);
        }
    }
    Ok(Json(response))
}

pub(crate) async fn get(
    State(shared): State<Shared>,
    Path(id): Path<String>,
) -> ApiResult<Json<Connection>> {
    Ok(Json(
        shared
            .connections
            .get(&id)
            .map_err(internal)?
            .ok_or_else(missing)?,
    ))
}

pub(crate) async fn update(
    State(shared): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    Json(mut request): Json<UpdateConnectionRequest>,
) -> ApiResult<Json<Connection>> {
    spaces::person(&caller)?;
    let _gate = shared.connections.gate.lock().await;
    let row = shared
        .connections
        .get(&id)
        .map_err(internal)?
        .ok_or_else(missing)?;
    if let Some(home) = &request.home_url {
        let (site, safe) = site_url(home).ok_or_else(HttpError::invalid_value)?;
        if site != row.site {
            return Err(HttpError(StatusCode::BAD_REQUEST, "connection_cross_site"));
        }
        request.home_url = Some(safe);
    }
    if request.grants.as_ref().is_some_and(|grants| {
        grants.len() > 256
            || grants
                .iter()
                .any(|grant| grant.agent_id.is_empty() || grant.agent_id.len() > 128)
    }) {
        return Err(HttpError::invalid_value());
    }
    shared.connections.update(&id, request).map_err(internal)?;
    Ok(Json(
        shared
            .connections
            .get(&id)
            .map_err(internal)?
            .ok_or_else(missing)?,
    ))
}

pub(crate) async fn dismiss(
    State(shared): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    spaces::person(&caller)?;
    let _gate = shared.connections.gate.lock().await;
    let row = shared
        .connections
        .get(&id)
        .map_err(internal)?
        .ok_or_else(missing)?;
    if row.status != ConnectionStatus::Candidate {
        return Err(HttpError::invalid_value());
    }
    shared
        .connections
        .remove(&row, false, true)
        .map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn audit(
    State(shared): State<Shared>,
    Query(query): Query<ConnectionAuditQuery>,
) -> ApiResult<Json<ConnectionAuditList>> {
    use std::io::BufRead;
    let path = DataPath::ConnectionsAudit.path(&shared.config.data_dir);
    let service = shared.connections.clone();
    let records = tokio::task::spawn_blocking(move || -> Result<Vec<ConnectionAuditRecord>> {
        let _store = service.store.lock();
        let file = private_options().read(true).open(path)?;
        let mut records = std::collections::VecDeque::new();
        let limit = query.limit.unwrap_or(100).clamp(1, 1000);
        for line in std::io::BufReader::new(file).lines() {
            let record: ConnectionAuditRecord = serde_json::from_str(&line?)?;
            if query
                .connection
                .as_ref()
                .is_some_and(|id| id != &record.connection_id)
                || query
                    .agent
                    .as_ref()
                    .is_some_and(|id| id != &record.agent_id)
                || query.action.is_some_and(|action| action != record.action)
            {
                continue;
            }
            records.push_back(record);
            if records.len() > limit {
                records.pop_front();
            }
        }
        Ok(records.into_iter().rev().collect())
    })
    .await
    .map_err(internal)?
    .map_err(internal)?;
    Ok(Json(ConnectionAuditList { records }))
}

async fn home_session(shared: &Shared, requested: Option<SessionId>) -> ApiResult<SessionId> {
    let id = match requested {
        Some(id) => id,
        None => {
            shared
                .agents
                .get("chief")
                .map_err(internal)?
                .ok_or_else(HttpError::internal)?
                .home_session_id
        }
    };
    let state = shared.db.state(&id).await.map_err(internal)?;
    if state.deleted || state.incognito || state.browser_space != BrowserSpace::Shared {
        return Err(denied());
    }
    Ok(id)
}

async fn browse(shared: &Shared, session: &SessionId, url: &str) -> ApiResult<()> {
    let state = shared.db.state(session).await.map_err(internal)?;
    let env = crate::tools::ToolEnv::new(shared.clone(), &state);
    let policy = crate::policy_api::effective(shared, &env.policy);
    let action = openside_policy::Action::new(openside_policy::Capability::Browse, url);
    if policy.decide(&action).effect != openside_policy::Effect::Allow {
        return Err(denied());
    }
    Ok(())
}

pub(crate) async fn check(
    State(shared): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> ApiResult<Json<Connection>> {
    spaces::person(&caller)?;
    let _browser_gate = shared.registry.browser.gate.lock().await;
    let _gate = shared.connections.gate.lock().await;
    let row = shared
        .connections
        .get(&id)
        .map_err(internal)?
        .ok_or_else(missing)?;
    let session = home_session(&shared, None).await?;
    let home = row
        .home_url
        .clone()
        .unwrap_or_else(|| format!("https://{}/", row.site));
    browse(&shared, &session, &home).await?;
    let browser = match row.account_space.as_deref() {
        Some(space) => {
            shared
                .registry
                .browser
                .account_browser(&shared, &row.site, space)
                .await
        }
        None => shared.registry.browser.get(&shared).await,
    }
    .map_err(|_| unavailable())?;
    Ok(Json(
        check_in_session(&shared, &row, &session, &browser, None, None).await?,
    ))
}

/// Browser and connection gates and Browse admission belong to the caller.
pub(crate) async fn check_in_session(
    shared: &Shared,
    row: &Connection,
    session: &SessionId,
    browser: &openside_browser::Browser,
    authorization: Option<(&str, &[String])>,
    task: Option<&str>,
) -> ApiResult<Connection> {
    let id = &row.id;
    let home = row
        .home_url
        .clone()
        .unwrap_or_else(|| format!("https://{}/", row.site));
    let state = shared.db.state(session).await.map_err(internal)?;
    let waiting: Vec<_> = state
        .tickets
        .values()
        .filter(|ticket| {
            crate::tools::connections::is_grant_ticket(ticket)
                && ticket.expires_at > chrono::Utc::now()
                && matches!(
                    ticket.status,
                    TicketStatus::Pending | TicketStatus::Resolved
                )
                && ticket.tool.as_ref().is_some_and(|tool| {
                    tool.args["connection_ids"].as_array().is_some_and(|ids| {
                        ids.iter().any(|id| id.as_str() == Some(row.id.as_str()))
                    })
                })
        })
        .filter_map(|ticket| {
            ticket.tool.as_ref()?.args["navigation_tab_id"]
                .as_str()
                .map(str::to_owned)
        })
        .collect();
    let reusable = browser.tabs().await.into_iter().find(|tab| {
        tab.owner_session_id.as_deref() == Some(session.as_str())
            && waiting.contains(&tab.id)
            && shared
                .registry
                .browser
                .task_tabs
                .holder(session, &tab.id)
                .as_deref()
                == task
    });
    let tab = match reusable {
        Some(tab) => tab,
        None => browser
            .create_agent_tab(session.as_str(), "about:blank")
            .await
            .map_err(|_| unavailable())?,
    };
    let _authorization = authorization
        .map(|(agent, ids)| {
            ids.iter()
                .map(|id| {
                    shared
                        .connections
                        .navigation
                        .authorize(session, &tab.id, id, agent)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let result: ApiResult<Connection> = async {
        browser
            .network_events(session.as_str(), &tab.id, 0)
            .await
            .map_err(|_| unavailable())?;
        browser
            .navigate(session.as_str(), &tab.id, &home)
            .await
            .map_err(document_error)?;
        let snapshot = browser.snapshot(&tab.id).await.map_err(|_| unavailable())?;
        let current = browser
            .tabs()
            .await
            .into_iter()
            .find(|item| item.id == tab.id)
            .ok_or_else(unavailable)?;
        let (kind, _) =
            crate::browser_state::classify(&snapshot.nodes, &current.url, &current.title);
        let frames = browser
            .frames(session.as_str(), &tab.id)
            .await
            .map_err(|_| unavailable())?;
        let mut unauthorized = false;
        if let Some(frame) = frames.iter().find(|frame| frame.parent.is_none()) {
            unauthorized = browser
                .document_response(session.as_str(), &tab.id, &frame.loader)
                .await
                .map_err(|_| unavailable())?
                .is_some_and(|response| matches!(response.status, 401 | 403));
        }
        let failed = unauthorized
            || matches!(kind, PageKind::Login | PageKind::Otp)
            || site_url(&current.url).map_or(true, |(site, _)| site != row.site);
        let probe = if failed {
            None
        } else {
            Some(
                probe(browser, session.as_str(), &tab.id)
                    .await
                    .map_err(internal)?,
            )
        };
        if failed {
            shared
                .connections
                .mark(row, ConnectionStatus::NeedsLogin)
                .map_err(internal)?;
        } else if probe.as_ref().is_some_and(|probe| probe.signed_in) {
            shared
                .connections
                .mark(row, ConnectionStatus::Active)
                .map_err(internal)?;
            if let Some(probe) = probe {
                shared
                    .connections
                    .identify(row, &probe.identities)
                    .map_err(internal)?;
            }
        }
        shared
            .connections
            .get(id)
            .map_err(internal)?
            .ok_or_else(missing)
    }
    .await;
    if matches!(&result,Err(error) if error.1=="connection_grant_required") {
        if let Some(task) = task {
            shared
                .registry
                .browser
                .task_tabs
                .claim(session, &tab.id, task);
        }
    } else {
        browser
            .close_agent_tab(session.as_str(), &tab.id)
            .await
            .map_err(|_| unavailable())?;
    }
    shared
        .registry
        .browser
        .persist(shared)
        .await
        .map_err(internal)?;
    result
}

pub(crate) async fn relogin(
    State(shared): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    Json(request): Json<ReloginConnectionRequest>,
) -> ApiResult<Json<ReloginResult>> {
    spaces::person(&caller)?;
    let row = shared
        .connections
        .get(&id)
        .map_err(internal)?
        .ok_or_else(missing)?;
    let session = home_session(&shared, request.session_id).await?;
    let home = row
        .home_url
        .unwrap_or_else(|| format!("https://{}/", row.site));
    browse(&shared, &session, &home).await?;
    let tab = {
        let _gate = shared.registry.browser.gate.lock().await;
        if shared.connections.get(&id).map_err(internal)?.is_none() {
            return Err(missing());
        }
        let browser = match row.account_space.as_deref() {
            Some(space) => {
                shared
                    .registry
                    .browser
                    .account_browser(&shared, &row.site, space)
                    .await
            }
            None => shared.registry.browser.get(&shared).await,
        }
        .map_err(|_| unavailable())?;
        let tab = browser
            .create_agent_tab(session.as_str(), &home)
            .await
            .map_err(|_| unavailable())?;
        browser
            .navigate(session.as_str(), &tab.id, &home)
            .await
            .map_err(|_| unavailable())?;
        shared
            .registry
            .browser
            .persist(&shared)
            .await
            .map_err(internal)?;
        shared
            .connections
            .store
            .lock()
            .audit(
                &id,
                &row.site,
                Some(&session),
                "chief",
                ConnectionAuditAction::Relogin,
            )
            .map_err(internal)?;
        tab
    };
    let state = shared.db.state(&session).await.map_err(internal)?;
    let host = crate::host::Host::for_api(shared.clone(), &state, &caller);
    let outcome = host
        .call(ToolCall {
            tool_call_id: format!("api-tool-{}", crate::persistence::random_id()),
            name: "request_login".into(),
            arguments_json: json!({"tab_id":tab.id}).to_string(),
            subagent: None,
            approval_ticket: None,
        })
        .await;
    let next = match outcome {
        ToolOutcome::Deferred { ticket, .. } => {
            return Ok(Json(ReloginResult::NeedsYou {
                ticket: TicketRef {
                    session_id: session,
                    ticket_id: ticket,
                },
            }))
        }
        ToolOutcome::Done { for_model_json } => {
            let result: serde_json::Value =
                serde_json::from_str(&for_model_json).map_err(internal)?;
            if result["next"] == "done" {
                return Ok(Json(ReloginResult::SignedIn));
            }
            result["next"].as_str().unwrap_or("unknown").to_owned()
        }
        _ => "unknown".into(),
    };
    let continuation = host.relogin_continuation(&tab.id, &next).await;
    let continuation = if matches!(&continuation, ToolOutcome::Denied { reason, .. } if reason == "connection_takeover_required")
    {
        host.call(ToolCall {
            tool_call_id: format!("api-tool-{}", crate::persistence::random_id()),
            name: "request_human_step".into(),
            arguments_json: json!({"kind":"takeover","summary":"Finish signing in","tab_id":tab.id}).to_string(),
            subagent: None,
            approval_ticket: None,
        }).await
    } else {
        continuation
    };
    match continuation {
        ToolOutcome::Deferred { ticket, .. } => Ok(Json(ReloginResult::NeedsYou {
            ticket: TicketRef {
                session_id: session,
                ticket_id: ticket,
            },
        })),
        ToolOutcome::Denied { .. } => Err(denied()),
        _ => Err(unavailable()),
    }
}

pub(crate) async fn revoke(
    State(shared): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    Json(request): Json<RevokeConnectionRequest>,
) -> ApiResult<StatusCode> {
    spaces::person(&caller)?;
    let _browser_gate = shared.registry.browser.gate.lock().await;
    let _gate = shared.connections.gate.lock().await;
    let row = shared
        .connections
        .get(&id)
        .map_err(internal)?
        .ok_or_else(missing)?;
    if request.delete_vault_item {
        if let Some(item) = &row.vault_item_id {
            crate::vault_api::delete(
                State(shared.clone()),
                Extension(caller.clone()),
                Path(item.clone()),
            )
            .await?;
        }
    }
    if let Some(space) = row.account_space.as_deref() {
        let root = DataPath::AccountSpaces.path(&shared.config.data_dir);
        shared
            .registry
            .browser
            .accounts
            .revoke(&root, space)
            .await
            .map_err(|_| unavailable())?;
    } else if request.sign_out {
        let browser = shared
            .registry
            .browser
            .get(&shared)
            .await
            .map_err(|_| unavailable())?;
        let keys: Vec<_> = browser
            .site_cookie_keys()
            .await
            .map_err(|_| unavailable())?
            .into_iter()
            .filter(|key| site(&key.domain).as_deref() == Some(row.site.as_str()))
            .collect();
        let mut origins: std::collections::BTreeSet<_> = shared
            .connections
            .origins(&row.site)
            .map_err(internal)?
            .into_iter()
            .collect();
        let profile = browser.endpoint().profile_dir.join("Default");
        match tokio::task::spawn_blocking(move || {
            openside_import::profile_storage_origins(&profile)
        })
        .await
        {
            Ok(Ok(stored)) => origins.extend(
                stored
                    .into_iter()
                    .filter(|origin| site_url(origin).is_some_and(|(site, _)| site == row.site)),
            ),
            _ => eprintln!(
                "opensided: storage origin inventory unavailable; revoking recorded origins"
            ),
        }
        for key in &keys {
            for scheme in ["http", "https"] {
                origins.insert(format!("{scheme}://{}", key.domain.trim_start_matches('.')));
            }
        }
        if let Some(home) = &row.home_url {
            if let Ok(url) = url::Url::parse(home) {
                origins.insert(url.origin().ascii_serialization());
            }
        }
        origins.extend(
            browser
                .quiesce_storage_origins(|url| {
                    site_url(url).is_some_and(|(site, _)| site == row.site)
                })
                .await
                .map_err(|_| unavailable())?,
        );
        browser
            .sign_out_site(&keys, &origins.into_iter().collect::<Vec<_>>())
            .await
            .map_err(|_| unavailable())?;
        for other in shared
            .connections
            .list()
            .map_err(internal)?
            .into_iter()
            .filter(|other| {
                other.site == row.site && other.id != row.id && other.account_space.is_none()
            })
        {
            shared
                .connections
                .mark(&other, ConnectionStatus::NeedsLogin)
                .map_err(internal)?;
        }
        shared
            .registry
            .browser
            .persist(&shared)
            .await
            .map_err(internal)?;
    }
    shared
        .connections
        .remove(&row, request.sign_out, false)
        .map_err(internal)?;
    shared
        .connections
        .pending
        .lock()
        .await
        .retain(|_, (_, snapshot)| snapshot.site != row.site);
    Ok(StatusCode::NO_CONTENT)
}
