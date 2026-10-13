use crate::{agents, api::HttpError, engine::Shared};
use axum::http::StatusCode;
use chrono::Utc;
use openside_proto::*;
use serde::{Deserialize, Serialize};

type Result<T, E = HttpError> = std::result::Result<T, E>;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Journal {
    pub source: String,
    pub result: String,
    pub operation: String,
    pub steps: Vec<Step>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) enum Step {
    Create {
        draft: HireDraft,
        id: String,
    },
    Agent {
        before: Box<Agent>,
        after: Box<Agent>,
    },
    Routine {
        id: String,
        before: RoutineAction,
        after: RoutineAction,
        enabled: bool,
        pause: bool,
        restore: bool,
    },
    Memory {
        id: String,
        before: Option<String>,
        after: Option<String>,
    },
    Archive {
        agent: String,
        archived: bool,
    },
    Event {
        agent: String,
        after_seq: u64,
        event: EventKind,
    },
    Queue {
        agent: String,
        sender: String,
        id: String,
        text: String,
    },
}

pub(super) fn missing() -> HttpError {
    HttpError(StatusCode::NOT_FOUND, "agent_not_found")
}
pub(super) fn conflict(code: &'static str) -> HttpError {
    HttpError(StatusCode::CONFLICT, code)
}
pub(super) fn active(s: &Shared, id: &str) -> Result<Agent> {
    let agent = s.agents.get(id)?.ok_or_else(missing)?;
    if agent.status != AgentStatus::Active {
        return Err(conflict("agent_retired"));
    }
    Ok(agent)
}

fn equivalent(a: &EventKind, b: &EventKind) -> bool {
    match (super::event_identity(a), super::event_identity(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

impl Step {
    async fn apply(&self, s: &Shared) -> Result<()> {
        match self {
            Self::Create { draft, id } => {
                agents::create(s, draft, id).await?;
            }
            Self::Agent { before, after } => {
                let current = s.agents.get(&before.id)?.ok_or_else(missing)?;
                if current != **after {
                    if current != **before {
                        return Err(conflict("ownership_changed"));
                    }
                    let credential_change = if before.status != after.status
                        || before.model != after.model
                        || before.account_pool != after.account_pool
                    {
                        Some(s.models.change_credentials().await)
                    } else {
                        None
                    };
                    s.agents.put(after, "team_lifecycle")?;
                    drop(credential_change);
                }
                if after.status == AgentStatus::Retired {
                    crate::engine::cancel_agent_runs(s, &after.id)
                        .await
                        .map_err(|_| HttpError::internal())?;
                    for mut handoff in s.agents.list_handoffs()?.into_iter().filter(|handoff| {
                        handoff.to_agent == after.id
                            && matches!(
                                handoff.status,
                                HandoffStatus::Open | HandoffStatus::Cancelled
                            )
                    }) {
                        if handoff.status == HandoffStatus::Open {
                            handoff.status = HandoffStatus::Cancelled;
                            handoff.ended_at = Some(Utc::now());
                            s.agents.put_handoff(&handoff)?;
                        }
                        if matches!(
                            s.db.state(&after.home_session_id)
                                .await?
                                .queue
                                .status(&handoff.message_id),
                            Some(openside_core::MessageStatus::Queued(_))
                        ) {
                            s.emit(
                                &after.home_session_id,
                                None,
                                EventKind::MessageDropped {
                                    message_id: handoff.message_id,
                                    reason: openside_core::DropReason::Removed,
                                },
                            )
                            .await?;
                        }
                    }
                }
            }
            Self::Routine {
                id,
                before,
                after,
                enabled,
                pause,
                restore,
            } => {
                s.routines
                    .get()
                    .ok_or_else(HttpError::internal)?
                    .transfer_action(id, before, after, *pause, restore.then_some(*enabled))
                    .await
                    .map_err(|_| conflict("ownership_changed"))?;
            }
            Self::Memory { id, before, after } => {
                let (id, before, after) = (id.clone(), before.clone(), after.clone());
                s.memory
                    .write(move |store| {
                        store.transfer_owner(&id, before.as_deref(), after.as_deref())
                    })
                    .await
                    .map_err(|_| conflict("ownership_changed"))?;
            }
            Self::Archive { agent, archived } => {
                let agent = s.agents.get(agent)?.ok_or_else(missing)?;
                if s.db.state(&agent.home_session_id).await?.archived != *archived {
                    s.emit(
                        &agent.home_session_id,
                        None,
                        EventKind::SessionUpdated {
                            mode: None,
                            title: None,
                            pinned: None,
                            archived: Some(*archived),
                        },
                    )
                    .await?;
                }
            }
            Self::Event {
                agent,
                after_seq,
                event,
            } => {
                let agent = s.agents.get(agent)?.ok_or_else(missing)?;
                if !s
                    .db
                    .events(&agent.home_session_id, *after_seq)
                    .await?
                    .iter()
                    .any(|stored| equivalent(&stored.kind, event))
                {
                    s.emit(&agent.home_session_id, None, event.clone()).await?;
                }
            }
            Self::Queue {
                agent,
                sender,
                id,
                text,
            } => {
                let agent = s.agents.get(agent)?.ok_or_else(missing)?;
                s.emit(
                    &agent.home_session_id,
                    None,
                    EventKind::MessageQueued {
                        message_id: MessageId::new(id),
                        text: text.clone(),
                        mode: openside_core::QueueMode::Queue,
                        attachments: Vec::new(),
                        author_kind: MessageAuthorKind::Internal,
                        from_agent: Some(sender.clone()),
                    },
                )
                .await?;
            }
        }
        Ok(())
    }
}

pub(super) async fn run(s: &Shared, key: &str, journal: Journal) -> Result<Agent> {
    let (mut step, journal) = match s.agents.get_transfer(key)? {
        Some((step, value)) => (
            step as usize,
            serde_json::from_value::<Journal>(value).map_err(|_| HttpError::internal())?,
        ),
        None => {
            s.agents.put_transfer(
                key,
                0,
                &serde_json::to_value(&journal).map_err(|_| HttpError::internal())?,
            )?;
            (0, journal)
        }
    };
    let value = serde_json::to_value(&journal).map_err(|_| HttpError::internal())?;
    while let Some(action) = journal.steps.get(step) {
        action.apply(s).await?;
        #[cfg(test)]
        if s.agents
            .get_transfer(&format!("test-crash:{key}"))?
            .is_some_and(|(boundary, _)| boundary == step as i64)
        {
            return Err(HttpError::internal());
        }
        step += 1;
        s.agents.put_transfer(key, step as i64, &value)?;
    }
    for step in &journal.steps {
        if let Step::Queue { agent, .. } = step {
            if let Some(agent) = s.agents.get(agent)? {
                super::wake(s, agent.home_session_id);
            }
        }
    }
    s.agents.get(&journal.result)?.ok_or_else(missing)
}

async fn resume_source(s: &Shared, source: &str) -> Result<()> {
    for (key, step, value) in s.agents.list_transfers()? {
        let Ok(journal) = serde_json::from_value::<Journal>(value) else {
            continue;
        };
        if references(&journal, source) && (step as usize) < journal.steps.len() {
            run(s, &key, journal).await?;
        }
    }
    Ok(())
}

fn references(journal: &Journal, id: &str) -> bool {
    journal.source == id
        || journal.result == id
        || journal.steps.iter().any(|step| match step {
            Step::Agent { before, after } => before.id == id || after.id == id,
            _ => false,
        })
}

pub(super) fn busy(s: &Shared, id: &str) -> Result<bool> {
    for (_, step, value) in s.agents.list_transfers()? {
        if let Ok(journal) = serde_json::from_value::<Journal>(value) {
            if (step as usize) < journal.steps.len() && references(&journal, id) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

async fn event(s: &Shared, agent: &str, value: EventKind) -> Result<Step> {
    let after_seq = match s.agents.get(agent)? {
        Some(agent) => s.db.state(&agent.home_session_id).await?.last_seq,
        None => 0,
    };
    Ok(Step::Event {
        agent: agent.into(),
        after_seq,
        event: value,
    })
}

async fn moves(
    s: &Shared,
    from: Option<&str>,
    to: &str,
    routine_ids: &[String],
    memory_ids: &[String],
    pause: bool,
) -> Result<Vec<Step>> {
    let mut steps = Vec::new();
    if !routine_ids.is_empty() {
        let scheduler = s.routines.get().ok_or_else(HttpError::internal)?;
        for id in routine_ids {
            let routine = scheduler
                .get(id)
                .map_err(|_| HttpError(StatusCode::NOT_FOUND, "routine_not_found"))?;
            if routine.action.owner_agent_id() != from
                && !(from.is_none() && routine.action.owner_agent_id() == Some("chief"))
            {
                return Err(conflict("already_assigned"));
            }
            let prompt = match &routine.action {
                RoutineAction::NewSession { prompt, .. }
                | RoutineAction::Heartbeat { prompt, .. }
                | RoutineAction::Agent { prompt, .. } => prompt.clone(),
            };
            steps.push(Step::Routine {
                id: id.clone(),
                before: routine.action,
                after: RoutineAction::Agent {
                    agent_id: to.into(),
                    prompt,
                },
                enabled: routine.enabled,
                pause,
                restore: false,
            });
        }
    }
    for id in memory_ids {
        let id = id.clone();
        let memory = s
            .memory
            .run(move |store| store.get(&id))
            .await
            .map_err(|_| HttpError(StatusCode::NOT_FOUND, "memory_not_found"))?;
        if memory.owner.as_deref() != from
            && !(from.is_none() && memory.owner.as_deref() == Some("chief"))
        {
            return Err(conflict("already_assigned"));
        }
        steps.push(Step::Memory {
            id: memory.id,
            before: memory.owner,
            after: Some(to.into()),
        });
    }
    Ok(steps)
}

pub(crate) async fn hire(s: &Shared, draft: &HireDraft, key: &str) -> Result<Agent> {
    let _settings = crate::settings_api::WRITE.lock().await;
    let _gate = s.agents.gate.lock().await;
    let agent = hire_locked(s, draft, key, None).await?;
    drop(_gate);
    notify(s, &agent).await?;
    Ok(agent)
}

pub(crate) async fn hire_tainted(s: &Shared, session: &SessionId, message: &str) -> Result<bool> {
    if active(s, "chief")?.home_session_id != *session {
        return Err(HttpError(StatusCode::FORBIDDEN, "not_requested"));
    }
    let events = s.db.events(session, 0).await?;
    let latest = events.iter().rev().find(|event| {
        matches!(
            &event.kind,
            EventKind::MessageQueued {
                author_kind: MessageAuthorKind::Person,
                from_agent: None,
                ..
            }
        )
    });
    let Some(latest) = latest.filter(|event| {
        matches!(&event.kind,
        EventKind::MessageQueued { message_id, .. } if message_id.as_str() == message)
    }) else {
        return Err(HttpError(StatusCode::FORBIDDEN, "not_requested"));
    };
    // Replaying completed outputs makes turn taint durable across a daemon restart.
    let tainted = events
        .iter()
        .filter(|event| event.seq > latest.seq)
        .any(|event| {
            matches!(
                &event.kind,
                EventKind::ToolEnded { .. }
                    | EventKind::ToolUpdated { .. }
                    | EventKind::MessageQueued { .. }
            )
        });
    Ok(tainted)
}

pub(super) async fn hire_explicit(
    s: &Shared,
    draft: &HireDraft,
    session: &SessionId,
    message: &str,
) -> Result<Agent> {
    let _settings = crate::settings_api::WRITE.lock().await;
    let _gate = s.agents.gate.lock().await;
    if hire_tainted(s, session, message).await? {
        return Err(HttpError(StatusCode::CONFLICT, "hire_requires_approval"));
    }
    let mut draft = draft.clone();
    draft.mode = None;
    let agent = hire_locked(
        s,
        &draft,
        &format!("person:{session}:{message}"),
        Some(MessageId::new(message)),
    )
    .await?;
    drop(_gate);
    notify(s, &agent).await?;
    Ok(agent)
}

async fn hire_locked(
    s: &Shared,
    draft: &HireDraft,
    key: &str,
    request_message_id: Option<MessageId>,
) -> Result<Agent> {
    let ticket_id = request_message_id.is_none().then(|| key.to_owned());
    let key = format!("hire:{key}");
    if let Some((_, value)) = s.agents.get_transfer(&key)? {
        return run(
            s,
            &key,
            serde_json::from_value(value).map_err(|_| HttpError::internal())?,
        )
        .await;
    }
    let id = crate::persistence::random_id();
    agents::prepare(s, draft, &id)?;
    let mut steps = vec![Step::Create {
        draft: draft.clone(),
        id: id.clone(),
    }];
    steps.extend(moves(s, None, &id, &draft.routine_ids, &draft.memory_ids, false).await?);
    steps.push(
        event(
            s,
            "chief",
            EventKind::TeamHired {
                agent_id: id.clone(),
                name: draft.name.clone(),
                ticket_id,
                request_message_id,
                card: None,
            },
        )
        .await?,
    );
    if !draft.history_summary.trim().is_empty() {
        steps.push(Step::Queue {
            agent: id.clone(),
            sender: "chief".into(),
            id: format!("team-notice-{key}"),
            text: format!("[team notice]\n{}", draft.history_summary),
        });
    }
    run(
        s,
        &key,
        Journal {
            source: "chief".into(),
            result: id,
            operation: "hire".into(),
            steps,
        },
    )
    .await
}

pub(crate) async fn transfer(
    s: &Shared,
    id: &str,
    request: &TransferRequest,
    key: &str,
) -> Result<Agent> {
    let _settings = crate::settings_api::WRITE.lock().await;
    let _gate = s.agents.gate.lock().await;
    resume_source(s, id).await?;
    let key = format!("transfer:{key}");
    if let Some((_, value)) = s.agents.get_transfer(&key)? {
        return run(
            s,
            &key,
            serde_json::from_value(value).map_err(|_| HttpError::internal())?,
        )
        .await;
    }
    let from = active(s, id)?;
    let to = active(s, &request.to)?;
    if from.id == to.id {
        return Err(conflict("already_assigned"));
    }
    let mut next_from = from.clone();
    let mut next_to = to.clone();
    for line in &request.responsibilities {
        if !next_from.persona.responsibilities.contains(line) {
            return Err(conflict("already_assigned"));
        }
        next_from.persona.responsibilities.retain(|old| old != line);
        if !next_to.persona.responsibilities.contains(line) {
            next_to.persona.responsibilities.push(line.clone());
        }
    }
    if let Some(skills) = next_to.skills.as_mut() {
        for skill in &request.skills {
            if !skills.contains(skill) {
                skills.push(skill.clone());
            }
        }
    }
    next_from.persona_version += 1;
    next_to.persona_version += 1;
    agents::validate(&next_from).map_err(|code| HttpError(StatusCode::BAD_REQUEST, code))?;
    agents::validate(&next_to).map_err(|code| HttpError(StatusCode::BAD_REQUEST, code))?;
    let mut steps = moves(
        s,
        Some(id),
        &to.id,
        &request.routine_ids,
        &request.memory_ids,
        false,
    )
    .await?;
    steps.push(Step::Agent {
        before: Box::new(from),
        after: Box::new(next_from),
    });
    steps.push(Step::Agent {
        before: Box::new(to),
        after: Box::new(next_to),
    });
    for owner in [id, request.to.as_str()] {
        steps.push(
            event(
                s,
                owner,
                EventKind::TeamTransferred {
                    from_agent: id.into(),
                    to_agent: request.to.clone(),
                    routine_ids: request.routine_ids.clone(),
                    responsibilities: request.responsibilities.clone(),
                    card: None,
                },
            )
            .await?,
        );
    }
    run(
        s,
        &key,
        Journal {
            source: id.into(),
            result: id.into(),
            operation: "transfer".into(),
            steps,
        },
    )
    .await
}

pub(crate) async fn retire(
    s: &Shared,
    id: &str,
    request: &RetireAgentRequest,
    key: &str,
) -> Result<Agent> {
    let _settings = crate::settings_api::WRITE.lock().await;
    let _gate = s.agents.gate.lock().await;
    resume_source(s, id).await?;
    let agent = s.agents.get(id)?.ok_or_else(missing)?;
    if agent.kind == AgentKind::Chief {
        return Err(conflict("chief_immutable"));
    }
    if agent.status == AgentStatus::Retired {
        drop(_gate);
        notify(s, &agent).await?;
        return Ok(agent);
    }
    let key = format!("retire:{key}:{}", agent.persona_version);
    let successor = request.successor.as_deref().unwrap_or("chief");
    let to = active(s, successor)?;
    if to.id == id {
        return Err(conflict("already_assigned"));
    }
    let mut retired = agent.clone();
    retired.status = AgentStatus::Retired;
    retired.retired_at = Some(Utc::now());
    retired.persona_version += 1;
    let mut steps = vec![Step::Agent {
        before: Box::new(agent.clone()),
        after: Box::new(retired),
    }];
    if request.revert_hire {
        let mut original = None;
        for (_, _, value) in s.agents.list_transfers()? {
            if let Ok(journal) = serde_json::from_value::<Journal>(value) {
                if journal.operation == "hire" && journal.result == id {
                    original = Some(journal);
                    break;
                }
            }
        }
        let original = original.ok_or_else(|| conflict("hire_not_found"))?;
        for step in original.steps {
            match step {
                Step::Routine {
                    id,
                    before,
                    after,
                    enabled,
                    ..
                } => steps.push(Step::Routine {
                    id,
                    before: after,
                    after: before,
                    enabled,
                    pause: false,
                    restore: true,
                }),
                Step::Memory { id, before, after } => steps.push(Step::Memory {
                    id,
                    before: after,
                    after: before,
                }),
                _ => {}
            }
        }
    }
    let scheduler = s.routines.get().ok_or_else(HttpError::internal)?;
    let already: Vec<String> = steps
        .iter()
        .filter_map(|step| match step {
            Step::Routine { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();
    let routines: Vec<String> = scheduler
        .list()
        .map_err(|_| HttpError::internal())?
        .into_iter()
        .filter(|routine| {
            routine.action.owner_agent_id() == Some(id) && !already.contains(&routine.id)
        })
        .map(|routine| routine.id)
        .collect();
    let owner = id.to_owned();
    let memories = s
        .memory
        .run(move |store| store.owned_ids(&owner))
        .await
        .map_err(|_| HttpError::internal())?;
    let already_memory: Vec<String> = steps
        .iter()
        .filter_map(|step| match step {
            Step::Memory { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();
    let memory_ids: Vec<String> = memories
        .into_iter()
        .filter(|id| !already_memory.contains(id))
        .collect();
    steps.extend(
        moves(
            s,
            Some(id),
            successor,
            &routines,
            &memory_ids,
            request.pause_routines,
        )
        .await?,
    );
    let mut successor_agent = to.clone();
    for line in &agent.persona.responsibilities {
        if !successor_agent.persona.responsibilities.contains(line) {
            successor_agent.persona.responsibilities.push(line.clone());
        }
    }
    if let Some(grants) = &mut successor_agent.skills {
        let source = match &agent.skills {
            Some(source) => source.clone(),
            None => crate::skills::enabled_names(s).await?,
        };
        for skill in source {
            if !grants.contains(&skill) {
                grants.push(skill);
            }
        }
    }
    successor_agent.persona_version += 1;
    agents::validate(&successor_agent).map_err(|code| HttpError(StatusCode::BAD_REQUEST, code))?;
    steps.push(Step::Agent {
        before: Box::new(to),
        after: Box::new(successor_agent),
    });
    steps.push(Step::Archive {
        agent: id.into(),
        archived: true,
    });
    for owner in ["chief", id] {
        steps.push(
            event(
                s,
                owner,
                EventKind::TeamRetired {
                    agent_id: id.into(),
                    successor: Some(successor.into()),
                    card: None,
                },
            )
            .await?,
        );
    }
    let agent = run(
        s,
        &key,
        Journal {
            source: id.into(),
            result: id.into(),
            operation: "retire".into(),
            steps,
        },
    )
    .await?;
    drop(_gate);
    notify(s, &agent).await?;
    Ok(agent)
}

pub(crate) async fn restore(s: &Shared, id: &str, key: &str) -> Result<Agent> {
    let _settings = crate::settings_api::WRITE.lock().await;
    let _gate = s.agents.gate.lock().await;
    resume_source(s, id).await?;
    let before = s.agents.get(id)?.ok_or_else(missing)?;
    if before.status == AgentStatus::Active {
        drop(_gate);
        notify(s, &before).await?;
        return Ok(before);
    }
    if s.agents
        .list()?
        .iter()
        .filter(|agent| agent.kind == AgentKind::Teammate && agent.status == AgentStatus::Active)
        .count()
        >= 8
    {
        return Err(conflict("roster_full"));
    }
    let key = format!("restore:{key}:{}", before.persona_version);
    let mut after = before.clone();
    after.status = AgentStatus::Active;
    after.retired_at = None;
    after.persona_version += 1;
    let mut steps = vec![
        Step::Agent {
            before: Box::new(before),
            after: Box::new(after),
        },
        Step::Archive {
            agent: id.into(),
            archived: false,
        },
    ];
    for owner in ["chief", id] {
        steps.push(
            event(
                s,
                owner,
                EventKind::TeamRestored {
                    agent_id: id.into(),
                    card: None,
                },
            )
            .await?,
        );
    }
    let agent = run(
        s,
        &key,
        Journal {
            source: id.into(),
            result: id.into(),
            operation: "restore".into(),
            steps,
        },
    )
    .await?;
    drop(_gate);
    notify(s, &agent).await?;
    Ok(agent)
}

async fn notify(s: &Shared, agent: &Agent) -> Result<()> {
    s.models.invalidate(s).await;
    if let Some(channel) = &agent.channel {
        if let Err(error) =
            crate::channels_api::binding_changed(s, &channel.kind, &channel.account_id).await
        {
            if agent.status != AgentStatus::Retired || error.1 != "agent_retired" {
                return Err(error);
            }
        }
    }
    Ok(())
}

pub(super) async fn recover(s: &Shared) -> Result<()> {
    let _settings = crate::settings_api::WRITE.lock().await;
    let _gate = s.agents.gate.lock().await;
    for (key, step, value) in s.agents.list_transfers()? {
        let Ok(journal) = serde_json::from_value::<Journal>(value) else {
            continue;
        };
        if (step as usize) < journal.steps.len() {
            run(s, &key, journal).await?;
        }
    }
    drop(_gate);
    for agent in s.agents.list()? {
        notify(s, &agent).await?;
    }
    Ok(())
}
