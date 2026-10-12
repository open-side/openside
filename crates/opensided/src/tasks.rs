//! Front agent and background Tasks (`docs/specs/sidecar-protocol.md` §11).
//!
//! With `[agent] orchestration = "front"` the session's parent agent is a fast front agent
//! that only talks, reads and manages Tasks. A Task is a background subagent admitted like
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

pub(crate) const TASK_START: &str = "task_start";
pub(crate) const TASK_STEER: &str = "task_steer";
pub(crate) const TASK_CANCEL: &str = "task_cancel";
pub(crate) const TASK_STATUS: &str = "task_status";
const TASK_TOOLS: [&str; 4] = [TASK_START, TASK_STEER, TASK_CANCEL, TASK_STATUS];

/// What the front agent may call besides the task tools: reading the session's tabs and
/// memory. Everything that acts (browser actions, shell, files, human steps) runs in Tasks.
const FRONT_READ_TOOLS: [&str; 5] = [
    "browser_tabs",
    "browser_snapshot",
    "browser_screenshot",
    "memory_search",
    "skill",
];

/// `message_id` prefix of the report a finished Task leaves in its session's queue.
pub(crate) const REPORT_PREFIX: &str = "task-report-";
/// Text prefix the front agent's instructions recognize as a Task report.
pub(crate) const REPORT_MARKER: &str = "[task report]";
/// The front agent cancelled the Task itself; it needs no report.
const CANCELLED_BY_AGENT: &str = "cancelled_by_agent";

pub(crate) fn front(shared: &Shared) -> bool {
    shared.config.loaded.config.agent.orchestration == openside_config::Orchestration::Front
}

pub(crate) fn is_task_tool(name: &str) -> bool {
    TASK_TOOLS.contains(&name)
}

pub(crate) fn is_front_tool(name: &str) -> bool {
    is_task_tool(name) || FRONT_READ_TOOLS.contains(&name)
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
) -> Option<(&'static str, &'static str)> {
    // API callers (`/repl`, `/tools/{name}`) drive the browser themselves; they are not the
    // front agent.
    if !front(shared)
        || call.subagent.is_some()
        || is_front_tool(&call.name)
        || call.tool_call_id.starts_with(crate::tools::API_CALL_PREFIX)
    {
        return None;
    }
    Some((
        "front_agent_cannot_act",
        "Start a Task with task_start for work that uses this tool",
    ))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct StartArgs {
    /// Self-contained goal for the Task, with everything it needs from this conversation.
    goal: String,
    /// Tool names the Task may use; defaults to every acting tool.
    tools: Option<Vec<String>>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SteerArgs {
    /// Task to steer, from `task_start` or `task_status`.
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

#[derive(Clone, Copy)]
pub(crate) enum TaskTool {
    Start,
    Steer,
    Cancel,
    Status,
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
            hint: Some("Use a task_id from task_start or task_status".into()),
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
        }
    }
    fn spec(&self) -> ToolSpec {
        match self {
            Self::Start => tools::spec::<StartArgs>(
                self.name(),
                "Start a background Task for work that takes more than a moment: anything in the browser, files or shell. Returns at once with its task_id; keep talking to the person. The Task's result arrives later as a [task report] message.",
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
        front(&ctx.shared)
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
                Self::Start => {
                    let args: StartArgs = match tools::parse(&args) {
                        Ok(args) => args,
                        Err(error) => return ToolExec::Failed(error.0),
                    };
                    let Some(model) = subagents::session_model(ctx).await else {
                        return ToolExec::Failed("Session state is unavailable".into());
                    };
                    // Tasks run on the session's (strong) model; the front agent's model is
                    // only for talking.
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
                    Ok(list) => ToolExec::Done {
                        for_model: json!({
                            "tasks": list.iter().filter(|task| task.background).map(status_view).collect::<Vec<_>>()
                        }),
                        for_app: None,
                    },
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
        text.push_str(&format!("\nReason: {reason}"));
    }
    if let Some(output) = &task.output {
        text.push_str(&format!("\nResult:\n{output}"));
    }
    Some(text)
}

/// Queue the report of every ended Task that has none yet. The store dedupes `message_id`,
/// so calling this again (after a crash between the end and the report) never adds a
/// second report. Returns whether anything may have been queued.
pub(crate) async fn report_ended(shared: &Shared, session: &SessionId) -> Result<bool> {
    if !front(shared) {
        return Ok(false);
    }
    let state = shared.db.state(session).await?;
    if state.deleted {
        return Ok(false);
    }
    let tasks = subagents::list(shared, session).await?;
    crate::browser_state::release_ended(shared, session, &tasks).await;
    let mut queued = false;
    for task in tasks {
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
                    mode: QueueMode::Steer,
                    attachments: Vec::new(),
                },
            )
            .await?;
        queued = true;
    }
    Ok(queued)
}
