//! Delegating to the openside browser agent: `exec` starts a run, and `session steer`,
//! `session queue`, `session stop` and `session tasks` manage it and its Tasks.
//!
//! A prompt is sent as a message to a session exactly as the web composer does; the daemon's
//! agent (with `[agent] orchestration = "front"`, its front agent and the background Tasks it
//! starts) does the work.

use std::io::Write;

use openside_proto::{
    Event, EventKind, Message, MessageMode, Session, SteerSubagentRequest, Subagent, SubagentList,
    SubmitMessageRequest,
};
use serde_json::Value;

use crate::{
    client::{self, Client},
    emit, output_error, Summary,
};

/// One stderr line of progress for a person watching a run: tasks, what the browser is doing,
/// tickets. Text replies are printed separately.
pub(crate) fn progress_line(event: &Event) -> Option<String> {
    Some(match &event.kind {
        EventKind::SubagentStarted {
            subagent_id, task, ..
        } => format!("[task {subagent_id}] started: {}", plain(task)),
        EventKind::SubagentEnded {
            subagent_id,
            status,
            reason,
            ..
        } => {
            let status = serde_json::to_value(status)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default();
            match reason {
                Some(reason) => format!("[task {subagent_id}] {status}: {}", plain(reason)),
                None => format!("[task {subagent_id}] {status}"),
            }
        }
        EventKind::BrowserState(state) => {
            let who = state
                .task_id
                .as_deref()
                .map(|task| format!("task {task}"))
                .unwrap_or_else(|| "agent".into());
            let doing = state
                .action
                .as_ref()
                .map(|action| match &action.target {
                    Some(target) => format!("{} {}", plain(&action.phrase), plain(target)),
                    None => plain(&action.phrase),
                })
                .unwrap_or_else(|| plain(&state.title));
            format!("[browser {who}] {doing} ({})", plain(&state.host))
        }
        EventKind::TicketOpened { ticket } => format!(
            "[ticket {}] waiting for a person: {}",
            ticket.id,
            plain(&ticket.summary)
        ),
        EventKind::TicketResolved { ticket_id, .. } => format!("[ticket {ticket_id}] answered"),
        EventKind::TicketRejected { ticket_id, .. } => format!("[ticket {ticket_id}] rejected"),
        EventKind::TicketExpired { ticket_id, .. } => format!("[ticket {ticket_id}] expired"),
        _ => return None,
    })
}

/// Page and model text printed to a terminal: control characters could carry escape sequences.
fn plain(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_control())
        .collect()
}

/// The message a run starts with and where its events begin.
pub(crate) struct Started {
    pub(crate) session: Session,
    pub(crate) message: Message,
}

pub(crate) async fn start(
    client: &Client,
    session: Option<&str>,
    create: &openside_proto::CreateSessionRequest,
    goal: &str,
) -> Result<Started, String> {
    let session: Session = match session {
        Some(id) => client.get(&client::session_path(id, "")).await?,
        None => client.json("POST", "/v1/sessions", Some(create)).await?,
    };
    let message: Message = client
        .json(
            "POST",
            &client::session_path(session.id.as_str(), "messages"),
            Some(&SubmitMessageRequest {
                message_id: None,
                text: goal.into(),
                mode: MessageMode::Now,
                attachments: Vec::new(),
            }),
        )
        .await?;
    Ok(Started { session, message })
}

/// `session steer` and `session queue`: a Task when `--task` names one (steer only), otherwise
/// a message to the session's agent delivered with `mode`. Prints `ok`.
pub(crate) async fn deliver<W: Write>(
    client: &Client,
    session: &str,
    task: Option<&str>,
    text: &str,
    mode: MessageMode,
    json_output: bool,
    stdout: &mut W,
) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("the prompt is empty".into());
    }
    match task {
        Some(task) => {
            let task: Subagent = client
                .json(
                    "POST",
                    &client::session_path(
                        session,
                        &format!("subagents/{}/steer", client::encode_segment(task)),
                    ),
                    Some(&SteerSubagentRequest { text: text.into() }),
                )
                .await?;
            if json_output {
                return emit(stdout, &task);
            }
        }
        None => {
            let message: Message = client
                .json(
                    "POST",
                    &client::session_path(session, "messages"),
                    Some(&SubmitMessageRequest {
                        message_id: None,
                        text: text.into(),
                        mode,
                        attachments: Vec::new(),
                    }),
                )
                .await?;
            if json_output {
                return emit(stdout, &message);
            }
        }
    }
    writeln!(stdout, "ok").map_err(output_error)
}

/// `session stop`: one Task when `--task` names it, otherwise the whole run.
pub(crate) async fn cancel<W: Write>(
    client: &Client,
    session: &str,
    task: Option<&str>,
    json_output: bool,
    stdout: &mut W,
    summary: &mut Summary,
) -> Result<(), String> {
    summary.session_id = Some(session.to_owned());
    match task {
        Some(task) => {
            let task: Subagent = client
                .json::<_, Value>(
                    "POST",
                    &client::session_path(
                        session,
                        &format!("subagents/{}/cancel", client::encode_segment(task)),
                    ),
                    None,
                )
                .await?;
            print_task(json_output, stdout, &task)
        }
        None => {
            let stopped: Session = client
                .json::<_, Value>("POST", &client::session_path(session, "stop"), None)
                .await?;
            summary.last_seq = Some(stopped.last_seq);
            if json_output {
                emit(stdout, &stopped)
            } else {
                writeln!(stdout, "Stopped session {}", stopped.id).map_err(output_error)
            }
        }
    }
}

/// `session tasks`: the session's Tasks and delegated subagents.
pub(crate) async fn tasks<W: Write>(
    client: &Client,
    session: &str,
    json_output: bool,
    stdout: &mut W,
    summary: &mut Summary,
) -> Result<(), String> {
    summary.session_id = Some(session.to_owned());
    let list: SubagentList = client
        .get(&client::session_path(session, "subagents"))
        .await?;
    if json_output {
        return emit(stdout, &list);
    }
    if let Some(ui) = crate::ui::pretty() {
        return write!(stdout, "{}", crate::ui::tasks(ui, &list)).map_err(output_error);
    }
    for task in &list.subagents {
        print_task(false, stdout, task)?;
    }
    Ok(())
}

fn print_task<W: Write>(json_output: bool, stdout: &mut W, task: &Subagent) -> Result<(), String> {
    if json_output {
        return emit(stdout, task);
    }
    let status = serde_json::to_value(task.status)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default();
    writeln!(stdout, "{}\t{status}\t{}", task.id, plain(&task.task)).map_err(output_error)?;
    if let Some(output) = &task.output {
        writeln!(stdout, "{}", plain(output)).map_err(output_error)?;
    }
    Ok(())
}
