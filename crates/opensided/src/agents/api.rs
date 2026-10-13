use crate::{
    api::{Access, Caller, HttpError},
    engine::Shared,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use openside_proto::*;

fn person(caller: &Caller) -> Result<(), HttpError> {
    if caller.0.is_some() && caller.1 != DeviceKind::PersonDevice {
        return Err(HttpError(StatusCode::FORBIDDEN, "person_device_required"));
    }
    Ok(())
}
pub(crate) fn validate_channel(s: &Shared, agent: &Agent) -> Result<(), HttpError> {
    if let Some(channel) = &agent.channel {
        if crate::channels_api::binding_owner(s, &channel.kind, &channel.account_id)?
            .is_some_and(|owner| owner != agent.id)
            || s.agents
                .channel_owner(&channel.kind, &channel.account_id)?
                .is_some_and(|owner| owner != agent.id)
        {
            return Err(HttpError(StatusCode::CONFLICT, "channel_binding_conflict"));
        }
    }
    Ok(())
}
pub(crate) async fn list(State(s): State<Shared>) -> Result<Json<AgentList>, HttpError> {
    let _gate = s.agents.gate.lock().await;
    let mut agents = Vec::new();
    for agent in s.agents.list()? {
        let state = s.db.state(&agent.home_session_id).await?;
        let session = state.session();
        let preview = state.last_message_preview.clone();
        agents.push(AgentSummary {
            agent,
            session_status: session.status,
            activity: session.activity,
            needs_you: session.status == SessionStatus::WaitingHuman,
            last_seq: state.last_seq,
            last_activity: preview,
            last_message_at: state.last_activity,
        });
    }
    agents.sort_by(|a, b| {
        (b.agent.kind == AgentKind::Chief)
            .cmp(&(a.agent.kind == AgentKind::Chief))
            .then_with(|| b.last_message_at.cmp(&a.last_message_at))
    });
    Ok(Json(AgentList { agents }))
}
pub(crate) async fn get(
    State(s): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Agent>, HttpError> {
    let _gate = s.agents.gate.lock().await;
    Ok(Json(s.agents.get(&id)?.ok_or(HttpError(
        StatusCode::NOT_FOUND,
        "agent_not_found",
    ))?))
}
pub(crate) async fn create(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Json(draft): Json<CreateAgentRequest>,
) -> Result<(StatusCode, Json<Agent>), HttpError> {
    person(&caller)?;
    let agent = crate::team::hire(
        &s,
        &draft,
        &format!("manual:{}", crate::persistence::random_id()),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(agent)))
}
pub(crate) async fn update(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Extension(access): Extension<Access>,
    Path(id): Path<String>,
    Json(patch): Json<UpdateAgentRequest>,
) -> Result<Json<Agent>, HttpError> {
    let _settings = crate::settings_api::WRITE.lock().await;
    let _gate = s.agents.gate.lock().await;
    let mut agent = s
        .agents
        .get(&id)?
        .ok_or(HttpError(StatusCode::NOT_FOUND, "agent_not_found"))?;
    if crate::team::lifecycle_busy(&s, &id)? {
        return Err(HttpError(StatusCode::CONFLICT, "transfer_in_progress"));
    }
    let privileged = patch.skills.is_some()
        || patch.model.is_some()
        || patch.account_pool.is_some()
        || patch.channel.is_some()
        || patch
            .mode
            .is_some_and(|mode| super::rank(mode) > super::rank(agent.mode));
    if privileged {
        person(&caller)?;
        if !access.has(CapabilityScope::Admin) {
            return Err(HttpError(StatusCode::FORBIDDEN, "forbidden"));
        }
    }
    let credentials_changed = patch
        .model
        .as_ref()
        .is_some_and(|value| value != &agent.model)
        || patch
            .account_pool
            .as_ref()
            .is_some_and(|value| value != &agent.account_pool);
    let channel_changed = patch
        .channel
        .as_ref()
        .is_some_and(|binding| binding != &agent.channel);
    let previous_channel = if channel_changed {
        agent.channel.clone()
    } else {
        None
    };
    if let Some(value) = patch.name {
        agent.name = value;
    }
    if let Some(value) = patch.initials {
        agent.initials = value;
    }
    if let Some(value) = patch.color {
        agent.color = value;
    }
    if let Some(value) = patch.persona {
        agent.persona = value;
    }
    if let Some(value) = patch.skills {
        agent.skills = value;
    }
    if let Some(value) = patch.model {
        agent.model = value;
    }
    if let Some(value) = patch.account_pool {
        agent.account_pool = value;
    }
    if let Some(value) = patch.channel {
        agent.channel = value;
    }
    if let Some(value) = patch.hidden {
        agent.hidden = value;
    }
    if let Some(value) = patch.pinned {
        agent.pinned = value;
    }
    if let Some(value) = patch.mode {
        let chief = s.agents.get("chief")?.ok_or_else(HttpError::internal)?;
        if super::rank(value) > super::rank(s.policy.default_mode())
            || agent.kind != AgentKind::Chief && super::rank(value) > super::rank(chief.mode)
        {
            return Err(HttpError(StatusCode::FORBIDDEN, "forbidden"));
        }
        agent.mode = value;
    }
    super::validate(&agent).map_err(|code| HttpError(StatusCode::BAD_REQUEST, code))?;
    let normalized_name = agent.name.to_lowercase();
    if s.agents
        .list()?
        .iter()
        .any(|other| other.id != agent.id && other.name.to_lowercase() == normalized_name)
    {
        return Err(HttpError(StatusCode::CONFLICT, "name_taken"));
    }
    validate_channel(&s, &agent)?;
    agent.persona_version = agent
        .persona_version
        .checked_add(1)
        .ok_or_else(HttpError::internal)?;
    let credential_change = if credentials_changed {
        Some(s.models.change_credentials().await)
    } else {
        None
    };
    s.agents.put(&agent, "updated")?;
    drop(credential_change);
    let home_result = super::ensure_home(&s, &agent).await;
    drop(_gate);
    if credentials_changed {
        s.models.invalidate(&s).await;
    }
    home_result?;
    if channel_changed {
        let mut failure = None;
        // Both identities must observe the commit even when one adapter cannot restart.
        for binding in previous_channel.iter().chain(agent.channel.iter()) {
            if let Err(error) =
                crate::channels_api::binding_changed(&s, &binding.kind, &binding.account_id).await
            {
                if failure.is_none() {
                    failure = Some(error);
                }
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
    }
    Ok(Json(agent))
}

pub(crate) async fn retire(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    Json(request): Json<RetireAgentRequest>,
) -> Result<Json<Agent>, HttpError> {
    person(&caller)?;
    Ok(Json(
        crate::team::retire(
            &s,
            &id,
            &request,
            &format!("retire:{id}:{}", request.revert_hire),
        )
        .await?,
    ))
}

pub(crate) async fn restore(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> Result<Json<Agent>, HttpError> {
    person(&caller)?;
    Ok(Json(
        crate::team::restore(&s, &id, &format!("restore:{id}")).await?,
    ))
}

pub(crate) async fn suppressed_proposals(
    State(s): State<Shared>,
) -> Result<Json<Vec<SuppressedProposal>>, HttpError> {
    Ok(Json(s.agents.suppressed_proposals()?))
}

pub(crate) async fn clear_proposal(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(key): Path<String>,
) -> Result<StatusCode, HttpError> {
    person(&caller)?;
    let _gate = s.ticket_gate.lock().await;
    s.agents.clear_proposal(&key)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn transfer(
    State(s): State<Shared>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    Json(request): Json<TransferRequest>,
) -> Result<Json<Agent>, HttpError> {
    use sha2::{Digest, Sha256};
    person(&caller)?;
    let encoded = serde_json::to_vec(&request).map_err(|_| HttpError::internal())?;
    let key = format!("api:{id}:{:x}", Sha256::digest(encoded));
    Ok(Json(crate::team::transfer(&s, &id, &request, &key).await?))
}

pub(crate) async fn handoffs(
    State(s): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<HandoffList>, HttpError> {
    s.agents
        .get(&id)?
        .ok_or(HttpError(StatusCode::NOT_FOUND, "agent_not_found"))?;
    let handoffs = s
        .agents
        .list_handoffs()?
        .into_iter()
        .filter(|handoff| handoff.from_agent == id || handoff.to_agent == id)
        .collect();
    Ok(Json(HandoffList { handoffs }))
}

pub(crate) async fn handoff(
    State(s): State<Shared>,
    Path(id): Path<String>,
    Json(request): Json<CreateHandoffRequest>,
) -> Result<Json<Handoff>, HttpError> {
    Ok(Json(
        crate::team::delegate(&s, "chief", &id, &request).await?,
    ))
}
