//! Subagents started by the `delegate` tool (ADR-020).
//!
//! The sidecar runs each child; the daemon admits it, guards its tool calls, and
//! projects its state from the parent session's event log. Child tickets belong to
//! the parent session and carry `origin.subagent_id`.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use futures_util::future::BoxFuture;
use openside_core::{ApprovalTier, SubagentControl, TicketId, ToolSpec};
use openside_policy::Action as PolicyAction;
use openside_proto::{
    Event, EventKind, SessionId, SteerSubagentRequest, Subagent, SubagentList, SubagentStatus,
    TicketStatus,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    api::HttpError,
    engine::{Action, Shared},
    persistence::{random_id, Result},
    tools::{self, DaemonTool, ToolEnv, ToolError, ToolExec},
};

pub(crate) const DELEGATE: &str = "delegate";
const TASK_SUMMARY_CHARS: usize = 500;

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Context {
    /// Start with only the task.
    Fresh,
    /// Also copy this conversation up to the delegating call.
    Fork,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DelegateArgs {
    /// Self-contained task for the subagent; its final answer is returned to you.
    task: String,
    /// Model for the subagent: this session's model (default) or a configured fallback.
    model: Option<String>,
    /// Tool names the subagent may use; defaults to all of your tools except `delegate`.
    tools: Option<Vec<String>>,
    /// `fresh` (default) or `fork` to share this conversation's history.
    context: Option<Context>,
}

pub(crate) struct Delegate;

impl Delegate {
    fn admit(&self, args: DelegateArgs, ctx: &ToolEnv, session_model: String) -> ToolExec {
        if args.task.trim().is_empty() {
            return ToolExec::Denied {
                reason: "Task is empty".into(),
                hint: Some("Describe the subtask the subagent should complete".into()),
            };
        }
        let models = &ctx.shared.config.loaded.config.models;
        let model = args.model.unwrap_or_else(|| session_model.clone());
        if model != session_model
            && models.default.as_deref() != Some(model.as_str())
            && !models.fallback.contains(&model)
        {
            return ToolExec::Denied {
                reason: "model_not_allowed".into(),
                hint: Some(
                    "Omit model, or use this session's model or a configured fallback model".into(),
                ),
            };
        }
        let available: Vec<String> = ctx
            .shared
            .registry
            .specs(ctx)
            .into_iter()
            .map(|spec| spec.name)
            .filter(|name| name != DELEGATE)
            .collect();
        let tools = match args.tools {
            None => available,
            Some(requested) => {
                if let Some(unknown) = requested.iter().find(|name| !available.contains(name)) {
                    return ToolExec::Denied {
                        reason: format!("Tool is not available to subagents: {unknown}"),
                        hint: Some(format!("Choose from: {}", available.join(", "))),
                    };
                }
                let mut tools = requested;
                tools.dedup();
                tools
            }
        };
        let context = match args.context.unwrap_or(Context::Fresh) {
            Context::Fresh => "fresh",
            Context::Fork => "fork",
        };
        ToolExec::Done {
            for_model: json!({
                "subagent_id": random_id(),
                "model": model,
                "tools": tools,
                "context": context,
            }),
            for_app: None,
        }
    }
}

impl DaemonTool for Delegate {
    fn name(&self) -> &'static str {
        DELEGATE
    }
    fn spec(&self) -> ToolSpec {
        tools::spec::<DelegateArgs>(
            self.name(),
            "Run a subtask in a subagent and wait for its final answer. The subagent's approvals and questions appear in this session's ticket list. Use it for focused work that does not need this conversation, or set context=fork to share it.",
            ApprovalTier::Read,
            true,
        )
    }
    fn visible(&self, _: &ToolEnv) -> bool {
        true
    }
    fn actions<'a>(
        &'a self,
        args: &'a Value,
        _: &'a ToolEnv,
    ) -> BoxFuture<'a, std::result::Result<Vec<PolicyAction>, ToolError>> {
        let parsed = tools::parse::<DelegateArgs>(args).map(|_| Vec::new());
        Box::pin(async move { parsed })
    }
    fn redact_args(&self, args: &Value) -> Value {
        args.clone()
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            let args: DelegateArgs = match tools::parse(&args) {
                Ok(args) => args,
                Err(error) => return ToolExec::Failed(error.0),
            };
            let session_model = match ctx.shared.db.state(&ctx.session).await {
                Ok(state) => state.model,
                Err(_) => return ToolExec::Failed("Session state is unavailable".into()),
            };
            let task = args.task.clone();
            let admitted = self.admit(args, ctx, session_model);
            let ToolExec::Done { for_model, .. } = &admitted else {
                return admitted;
            };
            let field = |name: &str| for_model[name].clone();
            let started = EventKind::SubagentStarted {
                subagent_id: field("subagent_id").as_str().unwrap_or_default().into(),
                parent_tool_call_id: ctx.tool_call_id.clone(),
                task: summary(&task),
                model: field("model").as_str().unwrap_or_default().into(),
                tools: serde_json::from_value(field("tools")).unwrap_or_default(),
            };
            if ctx.shared.emit(&ctx.session, None, started).await.is_err() {
                return ToolExec::Failed("Subagent could not be recorded".into());
            }
            admitted
        })
    }
}

fn summary(task: &str) -> String {
    task.chars().take(TASK_SUMMARY_CHARS).collect()
}

fn apply(subagents: &mut Vec<Subagent>, event: Event) {
    fn find<'a>(subagents: &'a mut [Subagent], id: &str) -> Option<&'a mut Subagent> {
        subagents.iter_mut().find(|subagent| subagent.id == id)
    }
    match event.kind {
        EventKind::SubagentStarted {
            subagent_id,
            parent_tool_call_id,
            task,
            model,
            tools,
        } => subagents.push(Subagent {
            id: subagent_id,
            parent_tool_call_id,
            task,
            model,
            tools,
            status: SubagentStatus::Running,
            started_at: event.ts,
            ended_at: None,
            reason: None,
            last_progress: None,
        }),
        EventKind::SubagentLifecycle {
            subagent_id,
            status,
        } => {
            if let (Some(subagent), Some(status)) =
                (find(subagents, &subagent_id), parse_status(&status))
            {
                if !subagent.status.is_terminal() && !status.is_terminal() {
                    subagent.status = status;
                }
            }
        }
        EventKind::SubagentProgress {
            subagent_id,
            progress,
        } => {
            if let Some(subagent) = find(subagents, &subagent_id) {
                subagent.last_progress = Some(progress);
            }
        }
        EventKind::SubagentEnded {
            subagent_id,
            status,
            reason,
        } => {
            if let Some(subagent) = find(subagents, &subagent_id) {
                if !subagent.status.is_terminal() {
                    subagent.status = status;
                    subagent.ended_at = Some(event.ts);
                    subagent.reason = reason;
                }
            }
        }
        _ => {}
    }
}

fn parse_status(status: &str) -> Option<SubagentStatus> {
    serde_json::from_value(Value::String(status.into())).ok()
}

/// Every subagent of a session, folded from its event log.
pub(crate) async fn list(shared: &Shared, session: &SessionId) -> Result<Vec<Subagent>> {
    let mut subagents = Vec::new();
    let mut after = 0;
    loop {
        let page = shared.db.events(session, after).await?;
        if page.is_empty() {
            return Ok(subagents);
        }
        for event in page {
            after = event.seq;
            apply(&mut subagents, event);
        }
    }
}

pub(crate) async fn get(
    shared: &Shared,
    session: &SessionId,
    id: &str,
) -> Result<Option<Subagent>> {
    Ok(list(shared, session)
        .await?
        .into_iter()
        .find(|subagent| subagent.id == id))
}

/// Deny a child's call unless the child is live and was granted the tool.
pub(crate) async fn guard(
    shared: &Shared,
    session: &SessionId,
    subagent: &str,
    tool: &str,
) -> Result<Option<&'static str>> {
    if tool == DELEGATE {
        return Ok(Some("Subagents cannot delegate"));
    }
    Ok(match get(shared, session, subagent).await? {
        None => Some("Unknown subagent"),
        Some(child) if child.status.is_terminal() => Some("Subagent is no longer running"),
        Some(child) if !child.tools.iter().any(|name| name == tool) => {
            Some("Tool is not available to this subagent")
        }
        Some(_) => None,
    })
}

/// Map a sidecar lifecycle report onto the projection; terminal states are final.
pub(crate) async fn lifecycle(
    shared: &Shared,
    session: &SessionId,
    subagent_id: String,
    status: String,
) -> Result<Option<EventKind>> {
    let Some(current) = get(shared, session, &subagent_id).await? else {
        return Ok(None);
    };
    let Some(next) = parse_status(&status) else {
        return Ok(None);
    };
    if current.status.is_terminal() {
        return Ok(None);
    }
    Ok(Some(if next.is_terminal() {
        EventKind::SubagentEnded {
            subagent_id,
            status: next,
            reason: (next == SubagentStatus::Failed).then(|| "failed".into()),
        }
    } else {
        EventKind::SubagentLifecycle {
            subagent_id,
            status,
        }
    }))
}

/// End a child and reject its pending tickets. Holding the ticket gate across the
/// liveness check, the terminal event and the rejections makes termination atomic with
/// respect to `tickets::open`, which rechecks liveness under the same gate. A crash
/// between the terminal event and the rejections is completed by the next call (or by
/// `repair` at startup): an already-ended child still gets its leftover tickets closed.
async fn end(
    shared: &Shared,
    session: &SessionId,
    id: &str,
    status: SubagentStatus,
    reason: &str,
) -> Result<bool> {
    let _gate = shared.ticket_gate.lock().await;
    let ended = match get(shared, session, id).await? {
        Some(child) if !child.status.is_terminal() => {
            shared
                .emit(
                    session,
                    None,
                    EventKind::SubagentEnded {
                        subagent_id: id.into(),
                        status,
                        reason: Some(reason.into()),
                    },
                )
                .await?;
            true
        }
        Some(_) => false,
        None => return Ok(false),
    };
    let state = shared.db.state(session).await?;
    for ticket in state.tickets.values().filter(|ticket| {
        ticket.status == TicketStatus::Pending && ticket.origin.subagent_id.as_deref() == Some(id)
    }) {
        crate::tickets::close_unanswered(shared, ticket, Some("subagent_cancelled")).await?;
    }
    Ok(ended)
}

/// Startup repair: close tickets left pending by a child whose end was only half recorded.
pub(crate) async fn repair(shared: &Shared, session: &SessionId) -> Result<()> {
    for child in list(shared, session).await? {
        if child.status.is_terminal() {
            end(shared, session, &child.id, child.status, "repaired").await?;
        }
    }
    Ok(())
}

/// Whether any child has not finished; such a session needs its sidecar reopened at
/// startup so `reconcile` can compare the children with the restored snapshot.
pub(crate) async fn any_open(shared: &Shared, session: &SessionId) -> Result<bool> {
    Ok(list(shared, session)
        .await?
        .iter()
        .any(|child| !child.status.is_terminal()))
}

/// The restored snapshot still holds deferred calls, but every one of their tickets was
/// closed and an `assistant.final` was recorded after the last closure: the resumed run
/// already finished before the crash and only its snapshot was lost.
pub(crate) async fn continuation_finished(
    shared: &Shared,
    session: &SessionId,
    tickets: &std::collections::HashSet<TicketId>,
) -> Result<bool> {
    if tickets.is_empty() {
        return Ok(false);
    }
    let mut closed = std::collections::HashMap::new();
    let mut last_final = 0;
    let mut after = 0;
    loop {
        let page = shared.db.events(session, after).await?;
        if page.is_empty() {
            break;
        }
        for event in page {
            after = event.seq;
            match &event.kind {
                EventKind::TicketResolved { ticket_id, .. }
                | EventKind::TicketRejected { ticket_id, .. }
                | EventKind::TicketExpired { ticket_id } => {
                    closed.insert(ticket_id.clone(), event.seq);
                }
                EventKind::AssistantFinal { .. } => last_final = event.seq,
                _ => {}
            }
        }
    }
    let mut latest = 0;
    for ticket in tickets {
        match closed.get(ticket) {
            Some(seq) => latest = latest.max(*seq),
            None => return Ok(false),
        }
    }
    Ok(last_final > latest)
}

/// End every open child of a discarded continuation.
pub(crate) async fn discard_all(shared: &Shared, session: &SessionId) -> Result<()> {
    for child in list(shared, session).await? {
        if !child.status.is_terminal() {
            end(
                shared,
                session,
                &child.id,
                SubagentStatus::Cancelled,
                "discarded",
            )
            .await?;
        }
    }
    Ok(())
}

/// Whether a child may still act; callers needing atomicity hold `ticket_gate`.
pub(crate) async fn is_live(shared: &Shared, session: &SessionId, id: &str) -> Result<bool> {
    Ok(get(shared, session, id)
        .await?
        .is_some_and(|child| !child.status.is_terminal()))
}

/// Record a cancellation and reject the child's pending tickets; false if already over.
pub(crate) async fn cancel(shared: &Shared, session: &SessionId, id: &str) -> Result<bool> {
    end(shared, session, id, SubagentStatus::Cancelled, "cancelled").await
}

/// Stopping the session ends every child that has not finished.
pub(crate) async fn stop_all(shared: &Shared, session: &SessionId) -> Result<()> {
    for child in list(shared, session).await? {
        if !child.status.is_terminal() {
            end(
                shared,
                session,
                &child.id,
                SubagentStatus::Cancelled,
                "stopped",
            )
            .await?;
        }
    }
    Ok(())
}

/// A child that was running when its process died cannot resume.
pub(crate) async fn interrupt_running(shared: &Shared, session: &SessionId) -> Result<()> {
    for child in list(shared, session).await? {
        if child.status == SubagentStatus::Running {
            end(
                shared,
                session,
                &child.id,
                SubagentStatus::Failed,
                "interrupted",
            )
            .await?;
        }
    }
    Ok(())
}

/// After the sidecar restored its snapshot, any nonterminal child it does not know is
/// unwakeable: fail it so its tickets are rejected and the parent queue is not blocked.
pub(crate) async fn reconcile(
    shared: &Shared,
    session: &SessionId,
    known: &std::collections::HashSet<String>,
) -> Result<()> {
    for child in list(shared, session).await? {
        if !child.status.is_terminal() && !known.contains(&child.id) {
            end(
                shared,
                session,
                &child.id,
                SubagentStatus::Failed,
                "not_recoverable",
            )
            .await?;
        }
    }
    Ok(())
}

type ApiResult<T> = std::result::Result<T, HttpError>;

async fn existing(s: &Shared, session: &SessionId, sid: &str) -> ApiResult<Subagent> {
    s.db.state(session)
        .await
        .map_err(|_| HttpError::not_found())?;
    get(s, session, sid)
        .await
        .map_err(|_| HttpError::internal())?
        .ok_or_else(HttpError::not_found)
}

fn live(child: &Subagent) -> ApiResult<()> {
    if child.status.is_terminal() {
        Err(HttpError(StatusCode::CONFLICT, "subagent_finished"))
    } else {
        Ok(())
    }
}

pub(crate) async fn list_route(
    State(s): State<Shared>,
    Path(id): Path<String>,
) -> ApiResult<Json<SubagentList>> {
    let id = SessionId::new(id);
    s.db.state(&id).await.map_err(|_| HttpError::not_found())?;
    Ok(Json(SubagentList {
        subagents: list(&s, &id).await.map_err(|_| HttpError::internal())?,
    }))
}

pub(crate) async fn cancel_route(
    State(s): State<Shared>,
    Path((id, sid)): Path<(String, String)>,
) -> ApiResult<Json<Subagent>> {
    let id = SessionId::new(id);
    live(&existing(&s, &id, &sid).await?)?;
    s.command(&id, Action::Subagent(sid.clone(), SubagentControl::Cancel))
        .await?;
    Ok(Json(existing(&s, &id, &sid).await?))
}

pub(crate) async fn steer_route(
    State(s): State<Shared>,
    Path((id, sid)): Path<(String, String)>,
    Json(request): Json<SteerSubagentRequest>,
) -> ApiResult<(StatusCode, Json<Subagent>)> {
    if request.text.len() > openside_agent_sidecar::MAX_USER_TEXT_BYTES {
        return Err(HttpError(
            StatusCode::PAYLOAD_TOO_LARGE,
            "message_too_large",
        ));
    }
    let id = SessionId::new(id);
    if request.text.trim().is_empty() {
        return Err(HttpError::invalid_value());
    }
    live(&existing(&s, &id, &sid).await?)?;
    s.command(
        &id,
        Action::Subagent(sid.clone(), SubagentControl::Steer { text: request.text }),
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(existing(&s, &id, &sid).await?)))
}
