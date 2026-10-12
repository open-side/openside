//! Delegating to the openside browser agent: `run`, `steer`, `cancel` and `tasks`.
//!
//! `run` sends the goal as a message to a new or existing session, exactly as the web composer
//! does; the daemon's agent (with `[agent] orchestration = "front"`, its front agent and the
//! background Tasks it starts) does the work. Detached by default: it prints the session and
//! message ids. `--follow` streams the run like `openside "task"`; `--wait` waits for the run's
//! end, through any tickets a person answers meanwhile, and prints the agent's replies.

use std::{io::Write, time::Duration};

use openside_proto::{
    Event, EventKind, Message, MessageMode, Session, SteerSubagentRequest, Subagent, SubagentList,
    SubmitMessageRequest,
};
use serde_json::{json, Value};

use crate::{
    client::{self, Client},
    emit, output_error,
    wait::{self, Replay, Until},
    Summary,
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
        EventKind::TicketExpired { ticket_id } => format!("[ticket {ticket_id}] expired"),
        _ => return None,
    })
}

/// Page and model text printed to a terminal: control characters could carry escape sequences.
fn plain(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_control())
        .collect()
}

pub(crate) struct RunOptions<'a> {
    pub(crate) goal: &'a str,
    pub(crate) session: Option<&'a str>,
    pub(crate) create: openside_proto::CreateSessionRequest,
    pub(crate) wait: bool,
    pub(crate) timeout: Option<Duration>,
    pub(crate) json: bool,
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

/// `openside run` without `--follow`.
pub(crate) async fn run<W: Write, E: Write>(
    options: RunOptions<'_>,
    client: &Client,
    stdout: &mut W,
    stderr: &mut E,
    summary: &mut Summary,
) -> Result<(), String> {
    let Started { session, message } =
        start(client, options.session, &options.create, options.goal).await?;
    let id = session.id.to_string();
    summary.session_id = Some(id.clone());
    summary.last_seq = Some(session.last_seq);
    if !options.wait {
        if options.json {
            emit(
                stdout,
                &json!({"type":"run","session_id":id,"message_id":message.id,"after_seq":session.last_seq}),
            )?;
        } else {
            writeln!(stdout, "{id}\t{}", message.id).map_err(output_error)?;
        }
        return Ok(());
    }
    let json_output = options.json;
    let mut write_error = None;
    let result = wait::settle(
        client,
        &id,
        Replay {
            last_seen: session.last_seq,
            terminal_after: session.last_seq,
            message_id: Some(message.id.to_string()),
        },
        Until {
            timeout: options.timeout,
            through_human: true,
        },
        |event| {
            if write_error.is_some() {
                return;
            }
            let written = if json_output {
                emit(&mut *stdout, event)
            } else if let Some(line) = progress_line(event) {
                writeln!(stderr, "{line}")
                    .and_then(|()| stderr.flush())
                    .map_err(output_error)
            } else {
                Ok(())
            };
            write_error = written.err();
        },
    )
    .await?;
    if let Some(error) = write_error {
        return Err(error);
    }
    finish(&result, json_output, stdout, summary)
}

fn finish<W: Write>(
    result: &Value,
    json_output: bool,
    stdout: &mut W,
    summary: &mut Summary,
) -> Result<(), String> {
    summary.last_seq = result["last_seq"].as_u64();
    summary.status = match result["status"].as_str() {
        Some("settled") => "settled",
        Some("waiting_human") => "waiting_human",
        Some("stopped") => "stopped",
        Some("timeout") => "timeout",
        _ => "error",
    };
    if json_output {
        let mut object = result.clone();
        object["type"] = json!("result");
        emit(stdout, &object)?;
    } else {
        for reply in result["replies"].as_array().into_iter().flatten() {
            writeln!(stdout, "{}", reply.as_str().unwrap_or_default()).map_err(output_error)?;
        }
    }
    if let Some(error) = result["error"].as_str() {
        return Err(format!("runtime error {error}"));
    }
    if summary.status == "timeout" {
        summary.failed = true;
    }
    Ok(())
}

/// `openside steer`: a Task when `--task` names one, otherwise the session's agent.
pub(crate) async fn steer<W: Write>(
    client: &Client,
    session: &str,
    task: Option<&str>,
    text: &str,
    json_output: bool,
    stdout: &mut W,
    summary: &mut Summary,
) -> Result<(), String> {
    summary.session_id = Some(session.to_owned());
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
            print_task(json_output, stdout, &task)
        }
        None => {
            let message: Message = client
                .json(
                    "POST",
                    &client::session_path(session, "messages"),
                    Some(&SubmitMessageRequest {
                        message_id: None,
                        text: text.into(),
                        mode: MessageMode::Steer,
                        attachments: Vec::new(),
                    }),
                )
                .await?;
            if json_output {
                emit(stdout, &message)
            } else {
                writeln!(stdout, "{}", message.id).map_err(output_error)
            }
        }
    }
}

/// `openside cancel`: one Task when `--task` names it, otherwise the whole run (session stop).
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

/// `openside tasks`: the session's Tasks and delegated subagents.
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
