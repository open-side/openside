//! Direct tool calls for API clients: `POST /v1/sessions/{id}/tools/{name}` and `/repl`, with
//! the ticket handling both need. A call deferred on an approval is rerun under the approved
//! ticket; a call deferred on a person's answer (login, code, choice, human step) ends with that
//! answer as its result. Tickets show up in the web and phone UI like any agent ticket.

use std::{
    future::Future,
    time::{Duration, Instant},
};

use openside_proto::{Ticket, TicketStatus};
use serde_json::{json, Value};

use crate::client::{self, Client};

const TICKET_POLL: Duration = Duration::from_millis(500);

/// Polls a ticket until it leaves `pending`; `None` when `timeout` passes first.
pub(crate) async fn wait_ticket(
    client: &Client,
    ticket: &str,
    timeout: Option<Duration>,
) -> Result<Option<Ticket>, String> {
    let deadline = timeout.map(|timeout| Instant::now() + timeout);
    loop {
        let current: Ticket = client.get(&client::ticket_path(ticket, "")).await?;
        if current.status != TicketStatus::Pending {
            return Ok(Some(current));
        }
        if deadline.is_some_and(|deadline| Instant::now() + TICKET_POLL > deadline) {
            return Ok(None);
        }
        tokio::time::sleep(TICKET_POLL).await;
    }
}

pub(crate) fn approved(ticket: &Ticket) -> bool {
    ticket.status == TicketStatus::Resolved
        && ticket
            .resolution
            .as_ref()
            .and_then(|value| value.get("approved"))
            == Some(&Value::Bool(true))
}

fn status_name(status: TicketStatus) -> &'static str {
    match status {
        TicketStatus::Pending => "pending",
        TicketStatus::Resolved => "resolved",
        TicketStatus::Rejected => "rejected",
        TicketStatus::Expired => "expired",
        TicketStatus::NeedsDesktop => "needs_desktop",
    }
}

/// How a direct call ended.
pub(crate) struct Outcome {
    /// `{status, ...}`: `done` with `result`, `deferred` with `ticket_id`/`deferral`/`next`,
    /// `resolved`/`rejected`/`expired` with the ticket's `result` for a human step,
    /// `not_approved`, or the daemon's `denied`/`failed` response.
    pub(crate) value: Value,
    /// Whether the tool produced a result (including a person's answer).
    pub(crate) ok: bool,
}

/// Sends a call through `send` (given the approval ticket to run under) and handles its
/// tickets. `wait` is `None` to return a deferral at once, `Some(timeout)` to wait for the
/// person (`Some(None)`: no deadline). `on_ticket` is told about each ticket being waited on.
pub(crate) async fn drive<S, F, T>(
    client: &Client,
    mut approval_ticket: Option<String>,
    wait: Option<Option<Duration>>,
    mut send: S,
    mut on_ticket: T,
) -> Result<Outcome, String>
where
    S: FnMut(Option<String>) -> F,
    F: Future<Output = Result<Value, String>>,
    T: FnMut(&str, bool),
{
    loop {
        let response = send(approval_ticket.clone()).await?;
        match response["status"].as_str() {
            Some("done") => {
                let mut value = json!({
                    "status": "done",
                    "tool_call_id": response["tool_call_id"],
                    "result": response["result"],
                });
                if let Some(artifacts) = response.get("artifacts") {
                    value["artifacts"] = artifacts.clone();
                }
                // A REPL snippet that threw ran, but did not succeed.
                let ok = response["result"]["error"].as_str().is_none();
                return Ok(Outcome { value, ok });
            }
            Some("deferred") => {
                let ticket = response["ticket_id"]
                    .as_str()
                    .ok_or("deferred response without a ticket_id")?
                    .to_owned();
                // `/repl` only defers on approvals and carries no deferral field.
                let approval = response["deferral"].as_str().unwrap_or("approval") == "approval";
                let deferred = |next: &str| Outcome {
                    value: json!({
                        "status": "deferred",
                        "tool_call_id": response["tool_call_id"],
                        "ticket_id": ticket,
                        "deferral": if approval { "approval" } else { "external" },
                        "next": next,
                    }),
                    ok: true,
                };
                let next = if approval {
                    "A person must approve this on another paired device in the openside web or phone UI. This agent client cannot approve its own work. Use ticket_wait; once approved, call again with the same arguments plus approval_ticket set to ticket_id."
                } else {
                    "A person must answer this ticket in the openside web or phone UI (or with `openside ticket`). Its resolution is the tool's result; wait on the ticket to read it."
                };
                let Some(timeout) = wait else {
                    return Ok(deferred(next));
                };
                on_ticket(&ticket, approval);
                let Some(resolved) = wait_ticket(client, &ticket, timeout).await? else {
                    return Ok(deferred(next));
                };
                if !approval {
                    let status = status_name(resolved.status);
                    return Ok(Outcome {
                        ok: resolved.status == TicketStatus::Resolved,
                        value: json!({
                            "status": status,
                            "tool_call_id": response["tool_call_id"],
                            "ticket_id": ticket,
                            "result": resolved.resolution,
                        }),
                    });
                }
                if !approved(&resolved) {
                    return Ok(Outcome {
                        ok: false,
                        value: json!({
                            "status": "not_approved",
                            "ticket_id": ticket,
                            "resolution": resolved.resolution,
                        }),
                    });
                }
                approval_ticket = Some(ticket);
            }
            _ => {
                return Ok(Outcome {
                    ok: false,
                    value: json!({
                        "status": response["status"],
                        "reason": response["reason"],
                        "hint": response["hint"],
                    }),
                })
            }
        }
    }
}

/// One `POST /v1/sessions/{id}/tools/{name}` request.
pub(crate) async fn tool(
    client: &Client,
    session: &str,
    name: &str,
    args: &Value,
    approval_ticket: Option<String>,
) -> Result<Value, String> {
    let mut body = json!({ "args": args });
    if let Some(ticket) = approval_ticket {
        body["approval_ticket"] = json!(ticket);
    }
    client
        .json(
            "POST",
            &client::session_path(session, &format!("tools/{}", client::encode_segment(name))),
            Some(&body),
        )
        .await
}

/// One `POST /v1/sessions/{id}/repl` request.
pub(crate) async fn repl(
    client: &Client,
    session: &str,
    code: &str,
    tab_id: Option<&str>,
    approval_ticket: Option<String>,
) -> Result<Value, String> {
    let mut body = json!({ "code": code });
    if let Some(tab) = tab_id {
        body["tab_id"] = json!(tab);
    }
    if let Some(ticket) = approval_ticket {
        body["approval_ticket"] = json!(ticket);
    }
    client
        .json("POST", &client::session_path(session, "repl"), Some(&body))
        .await
}
