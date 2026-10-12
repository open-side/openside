//! `openside repl <session-id>`: run Playwright-syntax JavaScript in one of the session's
//! agent tabs through `POST /v1/sessions/{id}/repl` (`browser_repl`, same policy, tickets,
//! and audit as an agent call). `--eval CODE` runs one snippet; otherwise stdin is one snippet,
//! or with `--interactive` every stdin line is its own snippet. Each snippet gets a fresh
//! JavaScript context. When a step needs approval the command prints the ticket, waits until
//! it is resolved in another process, and reruns the snippet under that approval.

use std::{
    io::{BufRead, BufReader, Read, Write},
    time::Duration,
};

use openside_proto::{ReplRequest, ReplResponse, ReplStatus, Ticket, TicketStatus};
use serde_json::Value;

use crate::{client::Client, emit, output_error, Summary};

const TICKET_POLL: Duration = Duration::from_millis(500);

pub(crate) struct ReplOptions<'a> {
    pub(crate) session: &'a str,
    pub(crate) tab: Option<&'a str>,
    pub(crate) eval: Option<&'a str>,
    pub(crate) interactive: bool,
    pub(crate) json: bool,
}

pub(crate) async fn execute<R: Read, W: Write, E: Write>(
    options: ReplOptions<'_>,
    client: &Client,
    stdin: &mut R,
    stdout: &mut W,
    stderr: &mut E,
    summary: &mut Summary,
) -> Result<(), String> {
    summary.session_id = Some(options.session.to_owned());
    let mut tab = options.tab.map(str::to_owned);
    if let Some(code) = options.eval {
        return snippet(&options, client, code, &mut tab, stdout, stderr, summary).await;
    }
    if !options.interactive {
        let mut code = String::new();
        stdin
            .read_to_string(&mut code)
            .map_err(|_| "could not read the snippet from stdin".to_owned())?;
        if code.trim().is_empty() {
            return Err(
                "no code: pass --eval CODE, pipe a snippet to stdin, or use --interactive".into(),
            );
        }
        return snippet(&options, client, &code, &mut tab, stdout, stderr, summary).await;
    }
    let mut lines = BufReader::new(stdin);
    loop {
        if !options.json {
            write!(stderr, "> ").map_err(output_error)?;
            stderr.flush().map_err(output_error)?;
        }
        let mut line = String::new();
        if lines
            .read_line(&mut line)
            .map_err(|_| "could not read stdin".to_owned())?
            == 0
        {
            return Ok(());
        }
        if line.trim().is_empty() {
            continue;
        }
        snippet(&options, client, &line, &mut tab, stdout, stderr, summary).await?;
    }
}

async fn snippet<W: Write, E: Write>(
    options: &ReplOptions<'_>,
    client: &Client,
    code: &str,
    tab: &mut Option<String>,
    stdout: &mut W,
    stderr: &mut E,
    summary: &mut Summary,
) -> Result<(), String> {
    let path = crate::client::session_path(options.session, "repl");
    let mut request = ReplRequest {
        code: code.to_owned(),
        tab_id: tab.clone(),
        approval_ticket: None,
    };
    let response = loop {
        let response: ReplResponse = client.json("POST", &path, Some(&request)).await?;
        let (ReplStatus::Deferred, Some(ticket)) = (response.status, &response.ticket_id) else {
            break response;
        };
        if options.json {
            emit(stdout, &response)?;
        }
        writeln!(
            stderr,
            "Approval needed (ticket {ticket}). Approve in another process: {}\nWaiting; the snippet reruns from the start once approved.",
            client.ticket_command(ticket.as_str(), "resolve", Some("true"))
        )
        .map_err(output_error)?;
        stderr.flush().map_err(output_error)?;
        let resolved = wait(client, ticket.as_str()).await?;
        let approved = resolved.status == TicketStatus::Resolved
            && resolved
                .resolution
                .as_ref()
                .and_then(|value| value.get("approved"))
                == Some(&Value::Bool(true));
        if !approved {
            summary.failed = true;
            summary.status = "error";
            return writeln!(
                stderr,
                "openside: approval for ticket {ticket} was not granted"
            )
            .map_err(output_error);
        }
        request.approval_ticket = Some(ticket.clone());
    };
    if options.json {
        emit(stdout, &response)?;
    }
    match response.status {
        ReplStatus::Done => {
            let result = response.result.unwrap_or(Value::Null);
            if let Some(id) = result["tab_id"].as_str() {
                *tab = Some(id.to_owned());
            }
            if !options.json {
                for line in result["console"].as_array().into_iter().flatten() {
                    writeln!(stdout, "{}", line.as_str().unwrap_or_default())
                        .map_err(output_error)?;
                }
                if !result["return_value"].is_null() {
                    writeln!(stdout, "{}", result["return_value"]).map_err(output_error)?;
                }
            }
            if let Some(error) = result["error"].as_str() {
                summary.failed = true;
                if !options.json {
                    writeln!(stderr, "{error}").map_err(output_error)?;
                }
            }
        }
        ReplStatus::Denied | ReplStatus::Failed | ReplStatus::Deferred => {
            summary.failed = true;
            if !options.json {
                writeln!(
                    stderr,
                    "openside: {}{}",
                    response.reason.as_deref().unwrap_or("REPL call failed"),
                    response
                        .hint
                        .as_deref()
                        .map(|hint| format!(" ({hint})"))
                        .unwrap_or_default()
                )
                .map_err(output_error)?;
            }
        }
    }
    stdout.flush().map_err(output_error)
}

async fn wait(client: &Client, ticket: &str) -> Result<Ticket, String> {
    loop {
        let current: Ticket = client.get(&crate::client::ticket_path(ticket, "")).await?;
        if current.status != TicketStatus::Pending {
            return Ok(current);
        }
        tokio::time::sleep(TICKET_POLL).await;
    }
}
