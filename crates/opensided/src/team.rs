//! Team orchestration is daemon-authorized; delivery identities survive runtime restarts.
mod lifecycle;
pub(crate) use lifecycle::{hire, restore, retire, transfer};

use crate::{
    api::HttpError,
    engine::{Action, Shared},
    tools::{self, DaemonTool, ToolEnv, ToolError, ToolExec},
};
use axum::http::StatusCode;
use chrono::Utc;
use futures_util::future::BoxFuture;
use openside_core::{ApprovalTier, MessageStatus, QueueMode, ToolSpec};
use openside_proto::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

type Result<T, E = HttpError> = std::result::Result<T, E>;
pub(crate) fn lifecycle_busy(s: &Shared, id: &str) -> Result<bool> {
    lifecycle::busy(s, id)
}
pub(crate) async fn recover_ownership(s: &Shared) -> Result<()> {
    lifecycle::recover(s).await
}

fn denied(code: &'static str) -> HttpError {
    HttpError(StatusCode::FORBIDDEN, code)
}
fn invalid() -> HttpError {
    HttpError(StatusCode::BAD_REQUEST, "invalid_packet")
}
fn role(ctx: &ToolEnv, chief: bool) -> bool {
    ctx.subagent.is_none()
        && ctx
            .agent_id
            .as_deref()
            .and_then(|id| ctx.shared.agents.get(id).ok().flatten())
            .is_some_and(|agent| {
                agent.status == AgentStatus::Active && (agent.kind == AgentKind::Chief) == chief
            })
}

pub(crate) fn wake(s: &Shared, session: SessionId) {
    if s.models.default_model().is_none() {
        return;
    }
    let shared = s.clone();
    tokio::spawn(async move {
        let _ = shared.command(&session, Action::Deliver).await;
    });
}

async fn queue(
    s: &Shared,
    to: &str,
    from: &str,
    id: String,
    text: String,
    mode: QueueMode,
) -> Result<()> {
    let agent = lifecycle::active(s, to)?;
    s.emit(
        &agent.home_session_id,
        None,
        EventKind::MessageQueued {
            message_id: MessageId::new(id),
            text,
            mode,
            attachments: Vec::new(),
            author_kind: MessageAuthorKind::Internal,
            from_agent: Some(from.into()),
        },
    )
    .await?;
    wake(s, agent.home_session_id);
    Ok(())
}

fn event_identity(event: &EventKind) -> Result<Value> {
    let mut value = serde_json::to_value(event).map_err(|_| HttpError::internal())?;
    if let Some(object) = value.as_object_mut() {
        object.retain(|key, _| {
            matches!(
                key.as_str(),
                "kind" | "agent_id" | "handoff_id" | "question_id" | "from_agent" | "to_agent"
            )
        });
    }
    Ok(value)
}

async fn event_once(s: &Shared, agent: &str, event: EventKind) -> Result<()> {
    let agent = s.agents.get(agent)?.ok_or_else(lifecycle::missing)?;
    let wanted = event_identity(&event)?;
    for existing in s.db.events(&agent.home_session_id, 0).await? {
        if event_identity(&existing.kind)? == wanted {
            return Ok(());
        }
    }
    s.emit(&agent.home_session_id, None, event).await?;
    Ok(())
}

pub(crate) async fn delegate(
    s: &Shared,
    from: &str,
    to: &str,
    request: &CreateHandoffRequest,
) -> Result<Handoff> {
    let _gate = s.agents.gate.lock().await;
    if lifecycle::active(s, from)?.kind != AgentKind::Chief {
        return Err(denied("forbidden"));
    }
    lifecycle::active(s, to)?;
    if from == to
        || request.packet.goal.trim().is_empty()
        || request.packet.acceptance.trim().is_empty()
        || request.dedupe_key.trim().is_empty()
    {
        return Err(invalid());
    }
    if request.kind == HandoffKind::Review {
        let subject = request.subject_handoff_id.as_deref().ok_or_else(invalid)?;
        let subject = s.agents.get_handoff(subject)?.ok_or_else(invalid)?;
        if subject.to_agent == to {
            return Err(denied("reviewer_is_author"));
        }
    } else if request.subject_handoff_id.is_some() {
        return Err(invalid());
    }
    if let Some(existing) = s.agents.list_handoffs()?.into_iter().find(|handoff| {
        handoff.status == HandoffStatus::Open && handoff.dedupe_key == request.dedupe_key
    }) {
        if existing.to_agent != to
            || existing.from_agent != from
            || existing.packet != request.packet
            || existing.kind != request.kind
            || existing.subject_handoff_id != request.subject_handoff_id
        {
            return Err(lifecycle::conflict("already_assigned"));
        }
        dispatch_pending(s).await?;
        return Ok(existing);
    }
    let id = crate::persistence::random_id();
    let handoff = Handoff {
        id: id.clone(),
        from_agent: from.into(),
        to_agent: to.into(),
        kind: request.kind,
        packet: request.packet.clone(),
        dedupe_key: request.dedupe_key.clone(),
        subject_handoff_id: request.subject_handoff_id.clone(),
        status: HandoffStatus::Open,
        message_id: MessageId::new(format!("handoff-{id}")),
        created_at: Utc::now(),
        ended_at: None,
    };
    s.agents.put_handoff(&handoff)?;
    dispatch_pending(s).await?;
    Ok(handoff)
}

// Called under agents.gate. Durable queue presence, not actor handles, is the admission ledger.
async fn dispatch_pending(s: &Shared) -> Result<()> {
    let handoffs = s.agents.list_handoffs()?;
    let mut admitted = 0;
    for handoff in handoffs
        .iter()
        .filter(|handoff| handoff.status == HandoffStatus::Open)
    {
        let Some(agent) = s.agents.get(&handoff.to_agent)? else {
            continue;
        };
        let state = s.db.state(&agent.home_session_id).await?;
        if state.queue.status(&handoff.message_id).is_some() {
            admitted += 1;
        }
    }
    for handoff in handoffs
        .iter()
        .filter(|handoff| handoff.status == HandoffStatus::Open)
    {
        let agent = lifecycle::active(s, &handoff.to_agent)?;
        for owner in [&handoff.from_agent, &handoff.to_agent] {
            event_once(
                s,
                owner,
                EventKind::TeamHandoff {
                    handoff_id: handoff.id.clone(),
                    from_agent: handoff.from_agent.clone(),
                    to_agent: handoff.to_agent.clone(),
                    handoff_kind: handoff.kind,
                    goal: handoff.packet.goal.clone(),
                    dedupe_key: handoff.dedupe_key.clone(),
                    card: None,
                },
            )
            .await?;
        }
        let state = s.db.state(&agent.home_session_id).await?;
        if state.queue.status(&handoff.message_id).is_some() {
            continue;
        }
        if admitted >= s.config.loaded.config.team.max_running {
            continue;
        }
        let text = format!(
            "[handoff]\nGoal: {}\nContext: {}\nAcceptance: {}\nConstraints: {}\nHandoff: {}",
            handoff.packet.goal,
            handoff.packet.context,
            handoff.packet.acceptance,
            handoff.packet.constraints,
            handoff.id
        );
        queue(
            s,
            &handoff.to_agent,
            &handoff.from_agent,
            handoff.message_id.to_string(),
            text,
            QueueMode::Queue,
        )
        .await?;
        admitted += 1;
    }
    Ok(())
}

async fn deliver_report(s: &Shared, handoff: &Handoff, summary: &str) -> Result<()> {
    for owner in [&handoff.from_agent, &handoff.to_agent] {
        event_once(
            s,
            owner,
            EventKind::TeamHandoffEnded {
                handoff_id: handoff.id.clone(),
                status: handoff.status,
                summary: Some(summary.into()),
                card: None,
            },
        )
        .await?;
    }
    queue(
        s,
        &handoff.from_agent,
        &handoff.to_agent,
        format!("team-report-{}", handoff.id),
        format!(
            "[teammate report]\nHandoff: {}\nStatus: {:?}\n{}",
            handoff.id, handoff.status, summary
        ),
        QueueMode::Queue,
    )
    .await
}

pub(crate) async fn settled(s: &Shared, session: &SessionId) -> Result<()> {
    let _gate = s.agents.gate.lock().await;
    let state = s.db.state(session).await?;
    let Some(owner) = state.agent_id.as_deref() else {
        return Ok(());
    };
    let events = s.db.events(session, 0).await?;
    for mut handoff in s
        .agents
        .list_handoffs()?
        .into_iter()
        .filter(|handoff| handoff.to_agent == owner)
    {
        let key = format!("team-report:{}", handoff.id);
        if let Some((2, _)) = s.agents.get_transfer(&key)? {
            continue;
        }
        let stored = s.agents.get_transfer(&key)?.map(|(_, value)| value);
        if handoff.status == HandoffStatus::Open {
            let outcome = state.completions.get(&handoff.message_id);
            let failed = match outcome {
                Some(MessageOutcome::Completed) => false,
                Some(MessageOutcome::Failed | MessageOutcome::Aborted) => true,
                _ if matches!(
                    state.queue.status(&handoff.message_id),
                    Some(MessageStatus::Dropped(_))
                ) =>
                {
                    true
                }
                _ => continue,
            };
            handoff.status = if failed
                || stored
                    .as_ref()
                    .and_then(|value| value.get("status"))
                    .and_then(Value::as_str)
                    == Some("failed")
            {
                HandoffStatus::Failed
            } else {
                HandoffStatus::Done
            };
            handoff.ended_at = Some(Utc::now());
        }
        let injected_at = events.iter().find_map(|event| match &event.kind {
            EventKind::MessageInjected { message_id } if message_id == &handoff.message_id => {
                Some(event.seq)
            }
            _ => None,
        });
        let summary = stored
            .as_ref()
            .and_then(|value| value.get("summary"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                injected_at.and_then(|after| {
                    events
                        .iter()
                        .rev()
                        .filter(|event| event.seq > after)
                        .find_map(|event| match &event.kind {
                            EventKind::AssistantFinal { text, .. } => Some(text.clone()),
                            _ => None,
                        })
                })
            })
            .unwrap_or_else(|| "The teammate's run ended without a final response.".into());
        let report = json!({"summary": summary, "status": handoff.status});
        s.agents.put_transfer(&key, 1, &report)?;
        s.agents.put_handoff(&handoff)?;
        deliver_report(s, &handoff, &summary).await?;
        s.agents.put_transfer(&key, 2, &report)?;
    }
    for (key, step, report) in s.agents.list_transfers()? {
        if !key.starts_with("standalone-report:")
            || step == 2
            || report["owner"].as_str() != Some(owner)
        {
            continue;
        }
        if report["report_to_chief"].as_bool().unwrap_or(false) {
            let summary = report["summary"].as_str().ok_or_else(invalid)?;
            queue(
                s,
                "chief",
                owner,
                format!("team-report-{key}"),
                format!("[teammate report]\n{summary}"),
                QueueMode::Queue,
            )
            .await?;
        }
        s.agents.put_transfer(&key, 2, &report)?;
    }
    dispatch_pending(s).await
}

pub(crate) async fn recover(s: &Shared) -> Result<()> {
    lifecycle::recover(s).await?;
    {
        let _gate = s.agents.gate.lock().await;
        for (key, step, question) in s.agents.list_transfers()? {
            if let Some(id) = key.strip_prefix("question:") {
                deliver_question(s, id, step, &question).await?;
            }
        }
    }
    for agent in s.agents.list()? {
        settled(s, &agent.home_session_id).await?;
    }
    Ok(())
}

async fn deliver_question(s: &Shared, id: &str, step: i64, question: &Value) -> Result<()> {
    if step == 2 {
        return Ok(());
    }
    let from = question["from"].as_str().ok_or_else(invalid)?;
    if step == 1 {
        let answer = question["answer"].as_str().ok_or_else(invalid)?;
        queue(
            s,
            from,
            "chief",
            format!("team-answer-{id}"),
            format!("[team answer]\n{answer}"),
            QueueMode::Steer,
        )
        .await?;
        event_once(
            s,
            "chief",
            EventKind::TeamAnswered {
                question_id: id.into(),
                card: None,
            },
        )
        .await?;
        s.agents
            .put_transfer(&format!("question:{id}"), 2, question)?;
    } else {
        let text = question["question"].as_str().ok_or_else(invalid)?;
        let options: Vec<String> =
            serde_json::from_value(question["options"].clone()).map_err(|_| invalid())?;
        event_once(
            s,
            "chief",
            EventKind::TeamQuestion {
                question_id: id.into(),
                from_agent: from.into(),
                text: text.into(),
                options: options.clone(),
                card: None,
            },
        )
        .await?;
        queue(s, "chief", from, format!("team-question-{id}"), format!("[teammate question]\nQuestion: {text}\nOptions: {}\nDefault: {}\nBlocks: {}\nQuestion ID: {id}", options.join("; "), question["default"].as_str().unwrap_or_default(), question["blocks"].as_str().unwrap_or_default()), QueueMode::Queue).await?;
    }
    Ok(())
}

pub(crate) async fn start(s: &Shared) -> Result<()> {
    let mut events = s.events.subscribe();
    let mut shutdown = s.shutdown.subscribe();
    recover(s).await?;
    let shared = s.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown.recv() => break,
                event = events.recv() => {
                    let result = match event {
                        Ok(event) if matches!(event.kind, EventKind::SessionSettled {} | EventKind::SessionStopped {} | EventKind::TeamRetired { .. }) => settled(&shared, &event.session_id).await,
                        Ok(_) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => recover(&shared).await,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    };
                    if result.is_err() { crate::diag!("opensided: team delivery remains pending for recovery"); }
                }
            }
        }
    });
    Ok(())
}

pub(crate) async fn assign_responsibility(
    s: &Shared,
    id: &str,
    line: &str,
    _key: &str,
) -> Result<Agent> {
    let _gate = s.agents.gate.lock().await;
    let mut agent = lifecycle::active(s, id)?;
    if !agent
        .persona
        .responsibilities
        .iter()
        .any(|existing| existing == line)
    {
        agent.persona.responsibilities.push(line.into());
        crate::agents::validate(&agent).map_err(|code| HttpError(StatusCode::BAD_REQUEST, code))?;
        agent.persona_version += 1;
        s.agents.put(&agent, "assigned_responsibility")?;
    }
    Ok(agent)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct HireArgs {
    draft: HireDraft,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ProposeArgs {
    draft: HireDraft,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DelegateArgs {
    agent_id: String,
    packet: HandoffPacket,
    kind: HandoffKind,
    dedupe_key: String,
    subject_handoff_id: Option<String>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SteerArgs {
    agent_id: String,
    text: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Empty {}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AnswerArgs {
    question_id: String,
    text: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ResponsibilityArgs {
    agent_id: String,
    line: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RetireArgs {
    agent_id: String,
    successor: Option<String>,
    pause_routines: Option<bool>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AskArgs {
    question: String,
    options: Vec<String>,
    default: Option<String>,
    blocks: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReportArgs {
    handoff_id: Option<String>,
    status: ReportStatus,
    summary: String,
    report_to_chief: Option<bool>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ReportStatus {
    Done,
    Failed,
}

#[derive(Clone, Copy)]
pub(crate) enum TeamTool {
    Hire,
    Propose,
    Delegate,
    Steer,
    Status,
    Answer,
    Responsibility,
    Retire,
    Ask,
    Report,
}
impl TeamTool {
    fn chief(self) -> bool {
        !matches!(self, Self::Ask | Self::Report)
    }
    fn parse(self, value: &Value) -> std::result::Result<(), ToolError> {
        match self {
            Self::Hire => tools::parse::<HireArgs>(value).map(|_| ()),
            Self::Propose => tools::parse::<ProposeArgs>(value).map(|_| ()),
            Self::Delegate => tools::parse::<DelegateArgs>(value).map(|_| ()),
            Self::Steer => tools::parse::<SteerArgs>(value).map(|_| ()),
            Self::Status => tools::parse::<Empty>(value).map(|_| ()),
            Self::Answer => tools::parse::<AnswerArgs>(value).map(|_| ()),
            Self::Responsibility => tools::parse::<ResponsibilityArgs>(value).map(|_| ()),
            Self::Retire => tools::parse::<RetireArgs>(value).map(|_| ()),
            Self::Ask => tools::parse::<AskArgs>(value).map(|_| ()),
            Self::Report => tools::parse::<ReportArgs>(value).map(|_| ()),
        }
    }
}
impl DaemonTool for TeamTool {
    fn name(&self) -> &'static str {
        match self {
            Self::Hire => "team_hire",
            Self::Propose => "team_propose_hire",
            Self::Delegate => "team_delegate",
            Self::Steer => "team_steer",
            Self::Status => "team_status",
            Self::Answer => "team_answer",
            Self::Responsibility => "team_assign_responsibility",
            Self::Retire => "team_retire",
            Self::Ask => "team_ask_chief",
            Self::Report => "team_report",
        }
    }
    fn spec(&self) -> ToolSpec {
        let name = self.name();
        match self {
            Self::Hire => tools::spec::<HireArgs>(name, "Hire a teammate for a recurring responsibility, long-running domain, or independent work stream. Reuse existing owners; use create_task for one-off work. After success send one short send_message notice naming the teammate and responsibility. Authority-expanding bindings require approval.", ApprovalTier::Read, false),
            Self::Propose => tools::spec::<ProposeArgs>(name, "Propose a focused teammate for the person's approval. One pending proposal and a 30-day topic cooldown apply.", ApprovalTier::Read, false),
            Self::Delegate => tools::spec::<DelegateArgs>(name, "Give a teammate a self-contained packet. Results arrive automatically; choose a different author for independent review.", ApprovalTier::Read, false),
            Self::Steer => tools::spec::<SteerArgs>(name, "Send a live instruction to a teammate.", ApprovalTier::Read, false),
            Self::Status => tools::spec::<Empty>(name, "Inspect the team when needed; completion reports arrive without polling.", ApprovalTier::Read, false),
            Self::Answer => tools::spec::<AnswerArgs>(name, "Answer a teammate's decision question.", ApprovalTier::Read, false),
            Self::Responsibility => tools::spec::<ResponsibilityArgs>(name, "Ask the person to approve a new teammate responsibility.", ApprovalTier::Sensitive, false),
            Self::Retire => tools::spec::<RetireArgs>(name, "Ask the person to approve retirement and ownership transfer.", ApprovalTier::Sensitive, false),
            Self::Ask => tools::spec::<AskArgs>(name, "Ask the chief for a decision, with options, a default and what is blocked. Never delegate directly.", ApprovalTier::Read, false),
            Self::Report => tools::spec::<ReportArgs>(name, "Record the outcome; the daemon delivers it to the chief exactly once when your turn settles.", ApprovalTier::Read, false),
        }
    }
    fn visible(&self, ctx: &ToolEnv) -> bool {
        role(ctx, self.chief())
    }
    fn actions<'a>(
        &'a self,
        args: &'a Value,
        _: &'a ToolEnv,
    ) -> BoxFuture<'a, std::result::Result<Vec<openside_policy::Action>, ToolError>> {
        Box::pin(async move {
            self.parse(args)?;
            Ok(if matches!(self, Self::Retire | Self::Responsibility) {
                vec![openside_policy::Action::new(
                    openside_policy::Capability::Sensitive,
                    self.name(),
                )]
            } else {
                Vec::new()
            })
        })
    }
    fn redact_args(&self, args: &Value) -> Value {
        args.clone()
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            if !role(ctx, self.chief()) {
                return ToolExec::Denied {
                    reason: "forbidden".into(),
                    hint: None,
                };
            }
            if let Err(error) = self.parse(&args) {
                return ToolExec::Failed(error.0);
            }
            match self {
                Self::Hire => {
                    let args = match tools::parse::<HireArgs>(&args) {
                        Ok(args) => args,
                        Err(error) => return ToolExec::Failed(error.0),
                    };
                    return match lifecycle::hire_autonomous(
                        &ctx.shared,
                        &args.draft,
                        &ctx.session,
                        &ctx.tool_call_id,
                    )
                    .await
                    {
                        Ok(agent) => ToolExec::Done {
                            for_model: json!(agent),
                            for_app: None,
                        },
                        Err(error) if error.1 == "hire_requires_approval" => {
                            crate::tickets::propose_hire(ctx, args.draft).await
                        }
                        Err(error) => ToolExec::Denied {
                            reason: error.1.into(),
                            hint: None,
                        },
                    };
                }
                Self::Propose => {
                    let args = match tools::parse::<ProposeArgs>(&args) {
                        Ok(args) => args,
                        Err(error) => return ToolExec::Failed(error.0),
                    };
                    return crate::tickets::propose_hire(ctx, args.draft).await;
                }
                Self::Retire | Self::Responsibility
                    if !ctx.approved_actions.iter().any(|action| {
                        action.capability == openside_policy::Capability::Sensitive
                    }) =>
                {
                    return ToolExec::Denied {
                        reason: "approval_required".into(),
                        hint: None,
                    };
                }
                _ => {}
            }
            match execute(*self, args, ctx).await {
                Ok(value) => ToolExec::Done {
                    for_model: value,
                    for_app: None,
                },
                Err(error) => ToolExec::Denied {
                    reason: error.1.into(),
                    hint: None,
                },
            }
        })
    }
}

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T> {
    serde_json::from_value(args).map_err(|_| invalid())
}
async fn execute(tool: TeamTool, args: Value, ctx: &ToolEnv) -> Result<Value> {
    let s = &ctx.shared;
    let owner = ctx.agent_id.as_deref().ok_or_else(|| denied("forbidden"))?;
    match tool {
        TeamTool::Hire => {
            let args: HireArgs = parse(args)?;
            Ok(json!(
                lifecycle::hire_autonomous(s, &args.draft, &ctx.session, &ctx.tool_call_id).await?
            ))
        }
        TeamTool::Delegate => {
            let args: DelegateArgs = parse(args)?;
            Ok(json!(
                delegate(
                    s,
                    owner,
                    &args.agent_id,
                    &CreateHandoffRequest {
                        packet: args.packet,
                        kind: args.kind,
                        dedupe_key: args.dedupe_key,
                        subject_handoff_id: args.subject_handoff_id
                    }
                )
                .await?
            ))
        }
        TeamTool::Steer => {
            let args: SteerArgs = parse(args)?;
            if args.text.trim().is_empty() {
                return Err(invalid());
            }
            let _gate = s.agents.gate.lock().await;
            queue(
                s,
                &args.agent_id,
                owner,
                format!("team-notice-{}-{}", ctx.session, ctx.tool_call_id),
                args.text,
                QueueMode::Steer,
            )
            .await?;
            Ok(json!({"delivered":true}))
        }
        TeamTool::Status => {
            Ok(json!({"agents":s.agents.list()?,"handoffs":s.agents.list_handoffs()?}))
        }
        TeamTool::Ask => {
            let args: AskArgs = parse(args)?;
            if args.question.trim().is_empty() || args.blocks.trim().is_empty() {
                return Err(invalid());
            }
            let _gate = s.agents.gate.lock().await;
            let id = format!("{}-{}", ctx.session, ctx.tool_call_id);
            let key = format!("question:{id}");
            if s.agents.get_transfer(&key)?.is_none() {
                s.agents.put_transfer(&key, 0, &json!({"from":owner,"question":args.question,"options":args.options,"default":args.default,"blocks":args.blocks}))?;
            }
            let (step, question) = s
                .agents
                .get_transfer(&key)?
                .ok_or_else(HttpError::internal)?;
            deliver_question(s, &id, step, &question).await?;
            Ok(json!({"question_id":id}))
        }
        TeamTool::Answer => {
            let args: AnswerArgs = parse(args)?;
            if args.text.trim().is_empty() {
                return Err(invalid());
            }
            let _gate = s.agents.gate.lock().await;
            let key = format!("question:{}", args.question_id);
            let (step, mut question) = s.agents.get_transfer(&key)?.ok_or_else(invalid)?;
            if step == 2 {
                return Ok(json!({"answered":true}));
            }
            if question.get("answer").is_none() {
                question["answer"] = json!(args.text);
                s.agents.put_transfer(&key, 1, &question)?;
            }
            deliver_question(s, &args.question_id, 1, &question).await?;
            Ok(json!({"answered":true}))
        }
        TeamTool::Report => {
            let args: ReportArgs = parse(args)?;
            if args.summary.trim().is_empty() {
                return Err(invalid());
            }
            let _gate = s.agents.gate.lock().await;
            let handoffs = s.agents.list_handoffs()?;
            let selected: Vec<_> = handoffs
                .into_iter()
                .filter(|handoff| {
                    handoff.to_agent == owner
                        && handoff.status == HandoffStatus::Open
                        && args
                            .handoff_id
                            .as_ref()
                            .map_or(true, |id| id == &handoff.id)
                })
                .collect();
            if selected.len() > 1 || args.handoff_id.is_some() && selected.is_empty() {
                return Err(invalid());
            }
            let key = selected
                .first()
                .map(|handoff| format!("team-report:{}", handoff.id))
                .unwrap_or_else(|| format!("standalone-report:{owner}:{}", ctx.tool_call_id));
            if s.agents.get_transfer(&key)?.is_none() {
                s.agents.put_transfer(&key, 0, &json!({"owner":owner,"summary":args.summary,"status":match args.status { ReportStatus::Done=>"done",ReportStatus::Failed=>"failed" },"report_to_chief":args.report_to_chief.unwrap_or(!selected.is_empty())}))?;
            }
            Ok(json!({"recorded":true,"delivery":"on_settle"}))
        }
        TeamTool::Retire => {
            let args: RetireArgs = parse(args)?;
            Ok(json!(
                retire(
                    s,
                    &args.agent_id,
                    &RetireAgentRequest {
                        successor: args.successor,
                        pause_routines: args.pause_routines.unwrap_or(false),
                        revert_hire: false
                    },
                    &ctx.tool_call_id
                )
                .await?
            ))
        }
        TeamTool::Responsibility => {
            let args: ResponsibilityArgs = parse(args)?;
            Ok(json!(
                assign_responsibility(s, &args.agent_id, &args.line, &ctx.tool_call_id).await?
            ))
        }
        TeamTool::Propose => Err(invalid()),
    }
}
