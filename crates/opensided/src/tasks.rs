//! Front agent and background Tasks (`docs/specs/sidecar-protocol.md` §11).
//!
//! With `[agent] orchestration = "front"` the session's parent agent is a fast front agent
//! that only talks and manages Tasks. A Task is a background subagent admitted like
//! `delegate` (same model and tool checks, same `subagent.*` events and parent-inbox
//! tickets), but the sidecar does not wait for it: its result comes back as a
//! `task-report-<id>` message that the front agent summarizes.

use futures_util::future::BoxFuture;
use openside_core::{ApprovalTier, MessageId, QueueMode, ToolCall, ToolSpec};
use openside_policy::Action as PolicyAction;
use openside_proto::{EventKind, SessionId, Subagent, SubagentStatus};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    engine::Shared,
    persistence::Result,
    subagents,
    tools::{self, DaemonTool, ToolEnv, ToolError, ToolExec},
};

pub(crate) mod document;
pub(crate) mod model_routing;

pub(crate) const TASK_START: &str = "create_task";
pub(crate) const TASK_STEER: &str = "steer_task";
pub(crate) const TASK_CANCEL: &str = "cancel_task";
pub(crate) const TASK_STATUS: &str = "get_tasks";
pub(crate) const SEND_MESSAGE: &str = "send_message";
const TASK_TOOLS: [&str; 5] = [
    TASK_START,
    TASK_STEER,
    TASK_CANCEL,
    TASK_STATUS,
    SEND_MESSAGE,
];

/// `message_id` prefix of the report a finished Task leaves in its session's queue.
pub(crate) const REPORT_PREFIX: &str = "task-report-";
/// Text prefix the front agent's instructions recognize as a Task report.
pub(crate) const REPORT_MARKER: &str = "[task report]";
/// The front agent cancelled the Task itself; it needs no report.
const CANCELLED_BY_AGENT: &str = "cancelled_by_agent";

pub(crate) fn front(shared: &Shared, agent_id: Option<&str>) -> bool {
    match agent_id {
        Some(id) => shared.agents.get(id).ok().flatten().is_some_and(|agent| {
            agent.kind == openside_proto::AgentKind::Chief
                && agent.status == openside_proto::AgentStatus::Active
        }),
        None => {
            shared.config.loaded.config.agent.orchestration == openside_config::Orchestration::Front
        }
    }
}

pub(crate) fn is_task_tool(name: &str) -> bool {
    TASK_TOOLS.contains(&name) || name == "get_models"
}

pub(crate) fn is_front_tool(name: &str) -> bool {
    is_task_tool(name)
        || name.starts_with("team_")
        || name == crate::routines::proposals::PROPOSE_ROUTINE
}

/// The front agent's share of the session tools, sent as `open.config.agent.front_tools`.
pub(crate) fn front_tools(specs: &[ToolSpec]) -> Vec<String> {
    specs
        .iter()
        .map(|spec| spec.name.clone())
        .filter(|name| is_front_tool(name))
        .collect()
}

/// Deny a front-agent call that would act: the front agent answers and delegates, Tasks act.
pub(crate) fn front_denial(
    shared: &Shared,
    call: &ToolCall,
    agent_id: Option<&str>,
) -> Option<(&'static str, &'static str)> {
    // API callers (`/repl`, `/tools/{name}`) drive the browser themselves; they are not the
    // front agent.
    if !front(shared, agent_id)
        || call.subagent.is_some()
        || is_front_tool(&call.name)
        || call.tool_call_id.starts_with(crate::tools::API_CALL_PREFIX)
    {
        return None;
    }
    Some((
        "front_agent_cannot_act",
        "Start a Task with create_task for work that uses this tool",
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct StartArgs {
    /// Self-contained goal for the Task, with everything it needs from this conversation.
    goal: String,
    /// Tool names the Task may use; defaults to every acting tool.
    tools: Option<Vec<String>>,
    /// Available connected model from get_models; omitted uses the reasoning model.
    model: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SteerArgs {
    /// Task to steer, from `create_task` or `get_tasks`.
    task_id: String,
    /// Instruction the Task receives before its next step.
    message: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CancelArgs {
    /// Task to cancel.
    task_id: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct StatusArgs {}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MessageArgs {
    text: String,
    #[serde(default)]
    attachments: Vec<DocumentArgs>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DocumentArgs {
    artifact: String,
    display: openside_proto::DocumentDisplay,
    title: Option<String>,
}

#[derive(Clone, Copy)]
pub(crate) enum TaskTool {
    Start,
    Steer,
    Cancel,
    Status,
    Message,
}

/// A live background Task of this session, or the denial the model should see.
async fn live_task(ctx: &ToolEnv, task_id: &str) -> std::result::Result<Subagent, ToolExec> {
    let task = subagents::get(&ctx.shared, &ctx.session, task_id)
        .await
        .map_err(|_| ToolExec::Failed("Task state is unavailable".into()))?
        .filter(|task| task.background);
    match task {
        None => Err(ToolExec::Denied {
            reason: "unknown_task".into(),
            hint: Some("Use a task_id from create_task or get_tasks".into()),
        }),
        Some(task) if task.status.is_terminal() => Err(ToolExec::Denied {
            reason: "task_finished".into(),
            hint: Some("The Task already ended; start a new one if more work is needed".into()),
        }),
        Some(task) => Ok(task),
    }
}

fn status_view(task: &Subagent) -> Value {
    json!({
        "task_id": task.id,
        "goal": task.task,
        "status": task.status,
        "started_at": task.started_at,
        "ended_at": task.ended_at,
        "reason": task.reason,
        "output": task.output,
        "last_progress": task.last_progress,
    })
}

impl DaemonTool for TaskTool {
    fn name(&self) -> &'static str {
        match self {
            Self::Start => TASK_START,
            Self::Steer => TASK_STEER,
            Self::Cancel => TASK_CANCEL,
            Self::Status => TASK_STATUS,
            Self::Message => SEND_MESSAGE,
        }
    }
    fn spec(&self) -> ToolSpec {
        match self {
            Self::Message => tools::spec::<MessageArgs>(
                self.name(),
                "Send a concise answer or necessary question. Attach up to two Markdown Output artifacts for longer content: inline for brief tables/summaries, panel for reports. Never narrate tools, task bookkeeping, model choices, or routing.",
                ApprovalTier::Read,
                false,
            ),
            Self::Start => tools::spec::<StartArgs>(
                self.name(),
                "Start background work on an available connected model, or omit model to use the reasoning model. Returns immediately; wait silently for the result unless the person needs an answer or question.",
                ApprovalTier::Read,
                false,
            ),
            Self::Steer => tools::spec::<SteerArgs>(
                self.name(),
                "Give a running Task a new instruction (for example a changed requirement). It reaches the Task before its next step.",
                ApprovalTier::Read,
                false,
            ),
            Self::Cancel => tools::spec::<CancelArgs>(
                self.name(),
                "Cancel one running Task. Other Tasks keep running.",
                ApprovalTier::Read,
                false,
            ),
            Self::Status => tools::spec::<StatusArgs>(
                self.name(),
                "List this session's Tasks with their status, latest progress and results.",
                ApprovalTier::Read,
                false,
            ),
        }
    }
    fn visible(&self, ctx: &ToolEnv) -> bool {
        front(&ctx.shared, ctx.agent_id.as_deref())
    }
    fn actions<'a>(
        &'a self,
        args: &'a Value,
        _: &'a ToolEnv,
    ) -> BoxFuture<'a, std::result::Result<Vec<PolicyAction>, ToolError>> {
        let parsed = match self {
            Self::Start => tools::parse::<StartArgs>(args).map(|_| ()),
            Self::Steer => tools::parse::<SteerArgs>(args).map(|_| ()),
            Self::Cancel => tools::parse::<CancelArgs>(args).map(|_| ()),
            Self::Status => tools::parse::<StatusArgs>(args).map(|_| ()),
            Self::Message => tools::parse::<MessageArgs>(args).map(|_| ()),
        }
        .map(|()| Vec::new());
        Box::pin(async move { parsed })
    }
    fn redact_args(&self, args: &Value) -> Value {
        args.clone()
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            match self {
                Self::Message => {
                    let args: MessageArgs = match tools::parse(&args) {
                        Ok(args) => args,
                        Err(error) => return ToolExec::Failed(error.0),
                    };
                    if args.text.trim().is_empty() {
                        return ToolExec::Failed("Message is empty".into());
                    }
                    let attachments = match crate::artifacts::admit_message_documents(
                        ctx,
                        args.attachments
                            .into_iter()
                            .map(|attachment| {
                                (attachment.artifact, attachment.title, attachment.display)
                            })
                            .collect(),
                    )
                    .await
                    {
                        Ok(attachments) => attachments,
                        Err(denied) => return denied,
                    };
                    match ctx
                        .shared
                        .emit(
                            &ctx.session,
                            None,
                            EventKind::AssistantFinal {
                                text: args.text,
                                attachments,
                            },
                        )
                        .await
                    {
                        Ok(_) => ToolExec::Done {
                            for_model: json!({"status": "sent"}),
                            for_app: None,
                        },
                        Err(_) => ToolExec::Failed("Message could not be recorded".into()),
                    }
                }
                Self::Start => {
                    let args: StartArgs = match tools::parse(&args) {
                        Ok(args) => args,
                        Err(error) => return ToolExec::Failed(error.0),
                    };
                    let Some(model) = subagents::session_model(ctx).await else {
                        return ToolExec::Failed("Session state is unavailable".into());
                    };
                    let model = if let Some(selected) = args.model {
                        if let Err(error) = model_routing::validate(ctx, &selected).await {
                            return error;
                        }
                        selected
                    } else {
                        model
                    };
                    let admitted =
                        subagents::admit(&args.goal, None, args.tools, "fresh", ctx, model);
                    subagents::record_start(ctx, &args.goal, admitted, true).await
                }
                Self::Steer => {
                    let args: SteerArgs = match tools::parse(&args) {
                        Ok(args) => args,
                        Err(error) => return ToolExec::Failed(error.0),
                    };
                    if args.message.trim().is_empty() {
                        return ToolExec::Denied {
                            reason: "Message is empty".into(),
                            hint: Some("Say what the Task should change".into()),
                        };
                    }
                    if args.message.len() > openside_agent_sidecar::MAX_USER_TEXT_BYTES {
                        return ToolExec::Denied {
                            reason: "message_too_large".into(),
                            hint: Some("Keep steering under 1 MiB".into()),
                        };
                    }
                    match live_task(ctx, &args.task_id).await {
                        Ok(task) => ToolExec::Done {
                            for_model: json!({"task_id": task.id, "status": "pending"}),
                            for_app: None,
                        },
                        Err(denied) => denied,
                    }
                }
                Self::Cancel => {
                    let args: CancelArgs = match tools::parse(&args) {
                        Ok(args) => args,
                        Err(error) => return ToolExec::Failed(error.0),
                    };
                    let task = match live_task(ctx, &args.task_id).await {
                        Ok(task) => task,
                        Err(denied) => return denied,
                    };
                    // Recorded (and its tickets rejected) before the sidecar stops the child,
                    // exactly like the cancel route.
                    match subagents::end(
                        &ctx.shared,
                        &ctx.session,
                        &task.id,
                        SubagentStatus::Cancelled,
                        CANCELLED_BY_AGENT,
                    )
                    .await
                    {
                        Ok(_) => ToolExec::Done {
                            for_model: json!({"task_id": task.id, "status": "cancelled"}),
                            for_app: None,
                        },
                        Err(_) => {
                            ToolExec::Failed("Task cancellation could not be recorded".into())
                        }
                    }
                }
                Self::Status => match subagents::list(&ctx.shared, &ctx.session).await {
                    Ok(list) => {
                        let mut tasks = Vec::new();
                        for task in list.iter().filter(|task| task.background) {
                            let mut view = status_view(task);
                            match crate::artifacts::task_documents(
                                &ctx.shared,
                                &ctx.session,
                                &task.id,
                            )
                            .await
                            {
                                Ok(paths) => view["artifacts"] = json!(paths),
                                Err(_) => {
                                    return ToolExec::Failed(
                                        "Task artifacts are unavailable".into(),
                                    )
                                }
                            }
                            tasks.push(view);
                        }
                        ToolExec::Done {
                            for_model: json!({"tasks": tasks}),
                            for_app: None,
                        }
                    }
                    Err(_) => ToolExec::Failed("Task state is unavailable".into()),
                },
            }
        })
    }
}

/// Whether a background Task is still running; such a session is not settled.
pub(crate) async fn any_running(shared: &Shared, session: &SessionId) -> Result<bool> {
    Ok(subagents::list(shared, session)
        .await?
        .iter()
        .any(|task| task.background && !task.status.is_terminal()))
}

/// Emit `session.settled` for an idle session whose last Task just ended without a report
/// (cancelled by the front agent or with the session); nothing else would settle it.
pub(crate) async fn settle_unreported(
    shared: &Shared,
    session: &SessionId,
    task: &str,
) -> Result<()> {
    let ended = subagents::get(shared, session, task)
        .await?
        .is_some_and(|task| task.background && task.status.is_terminal());
    if ended
        && !any_running(shared, session).await?
        && shared
            .db
            .state(session)
            .await?
            .queue
            .pending_ids()
            .next()
            .is_none()
    {
        shared
            .emit(session, None, EventKind::SessionSettled {})
            .await?;
    }
    Ok(())
}

fn report_excerpt(text: &str) -> &str {
    let end = text
        .char_indices()
        .nth(1024)
        .map_or(text.len(), |(index, _)| index);
    &text[..end]
}

fn report_text(task: &Subagent) -> Option<String> {
    let reason = task.reason.as_deref();
    let line = match task.status {
        SubagentStatus::Completed => "completed",
        SubagentStatus::Failed => "failed",
        // Stopping or deleting the session ends every Task on purpose; the front agent
        // cancelling one already knows.
        SubagentStatus::Cancelled
            if matches!(
                reason,
                Some("stopped" | "deleted" | "discarded" | CANCELLED_BY_AGENT)
            ) =>
        {
            return None
        }
        SubagentStatus::Cancelled => "was cancelled by the person",
        _ => return None,
    };
    let mut text = format!(
        "{REPORT_MARKER} Task {} {line}.\nGoal: {}",
        task.id, task.task
    );
    if let Some(reason) = reason.filter(|_| task.status == SubagentStatus::Failed) {
        text.push_str(&format!("\nReason: {}", report_excerpt(reason)));
    }
    if let Some(output) = &task.output {
        text.push_str(&format!("\nResult:\n{}", report_excerpt(output)));
    }
    Some(text)
}

/// Stable report IDs repair missed lifecycle notifications after a crash without
/// replaying an already delivered outcome or waiting-for-person request.
pub(crate) async fn report_updates(shared: &Shared, session: &SessionId) -> Result<bool> {
    let state = shared.db.state(session).await?;
    if !front(shared, state.agent_id.as_deref()) {
        return Ok(false);
    }
    if state.deleted {
        return Ok(false);
    }
    let tasks = subagents::list(shared, session).await?;
    crate::browser_state::release_ended(shared, session, &tasks).await;
    let mut queued = false;
    for task in tasks {
        if task.background && task.status == SubagentStatus::WaitingHuman {
            for ticket in state.tickets.values().filter(|ticket| {
                ticket.status == openside_proto::TicketStatus::Pending
                    && ticket.origin.subagent_id.as_deref() == Some(task.id.as_str())
            }) {
                let message_id =
                    MessageId::new(format!("{REPORT_PREFIX}{}-waiting-{}", task.id, ticket.id));
                if state.queue.status(&message_id).is_some() {
                    continue;
                }
                shared.emit(session, None, EventKind::MessageQueued {
                    message_id,
                    text: format!("{REPORT_MARKER} Task {} waiting for the person.\nGoal: {}\nNeeds you: {}",
                        task.id, task.task, report_excerpt(&ticket.summary)),
                    mode: QueueMode::Queue,
                    attachments: Vec::new(),
                    author_kind: openside_proto::MessageAuthorKind::Internal,
                    from_agent: None,
                }).await?;
                queued = true;
            }
        }
        if !task.background || !task.status.is_terminal() {
            continue;
        }
        let Some(text) = report_text(&task) else {
            continue;
        };
        let message_id = MessageId::new(format!("{REPORT_PREFIX}{}", task.id));
        if state.queue.status(&message_id).is_some() {
            continue;
        }
        shared
            .emit(
                session,
                None,
                EventKind::MessageQueued {
                    message_id,
                    text,
                    mode: QueueMode::Queue,
                    attachments: Vec::new(),
                    author_kind: openside_proto::MessageAuthorKind::Internal,
                    from_agent: None,
                },
            )
            .await?;
        queued = true;
    }
    Ok(queued)
}
