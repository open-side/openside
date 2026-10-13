//! `openside browser`: the session browser and human-step tools the in-app agent has, called
//! directly through `POST /v1/sessions/{id}/tools/{name}` (policy, tickets and events as for an
//! agent call). Arguments are `key=value` pairs (values parsed as JSON when valid, otherwise
//! taken as strings) or one JSON object; `openside browser tools` prints every tool's schema.
//! A call that needs a person waits for the answer given in the web or phone UI (or with
//! `openside ticket`): an approval reruns the call, an answer is printed as its result.

use std::{io::Write, time::Duration};

use clap::Subcommand;
use openside_proto::ToolCatalog;
use serde_json::{Map, Value};

use crate::{
    client::{self, Client},
    emit, output_error,
    tool_call::{self, Outcome},
    Summary,
};

#[derive(Subcommand)]
pub(crate) enum BrowserCommand {
    /// List the directly callable tools with their argument schemas (GET /v1/tools).
    Tools,
    /// browser_tabs: list the session's tabs.
    Tabs(CallArgs),
    /// browser_open_tab: open a tab (url=...).
    Open(CallArgs),
    /// browser_navigate: go to a URL in a tab (tab_id=... url=...).
    Navigate(CallArgs),
    /// browser_snapshot: accessibility snapshot with eN refs (tab_id=... [scope=viewport]).
    Snapshot(CallArgs),
    /// browser_act: act on a snapshot ref (tab_id=... target=eN action=click; see `tools`).
    Act(CallArgs),
    /// browser_pointer: click, type or scroll at viewport coordinates.
    Pointer(CallArgs),
    /// browser_screenshot: observe a tab; output=true delivers it (`--out FILE` saves and delivers it).
    Screenshot(CallArgs),
    /// browser_close_tab: close a tab (tab_id=...).
    Close(CallArgs),
    /// request_login: sign in on the tab's login page; a person answers on their device.
    Login(CallArgs),
    /// request_choice: let a person pick among the page's options.
    Choice(CallArgs),
    /// request_form: let a person complete a page form.
    Form(CallArgs),
    /// present: deliver a structured result card to the person.
    Present(CallArgs),
    /// wait_for_approval: wait while a person approves the sign-in on their phone.
    WaitApproval(CallArgs),
    /// request_otp: fill a one-time code from the vault or from a person.
    Otp(CallArgs),
    /// register_authenticator: keep a site's 2FA setup key in the vault, with approval.
    RegisterAuthenticator(CallArgs),
    /// request_human_step: hand a step to a person (takeover, device approval, secret input).
    HumanStep(CallArgs),
    /// ask_user: ask a person a non-secret question.
    Ask(CallArgs),
    /// vault_login: fill a saved login (needs the vault_use scope over TCP).
    VaultLogin(CallArgs),
    /// vault_otp: fill a saved TOTP code (needs the vault_use scope over TCP).
    VaultOtp(CallArgs),
    /// vault_passkey: register or use a vault passkey (needs the vault_use scope over TCP).
    VaultPasskey(CallArgs),
}

#[derive(clap::Args)]
pub(crate) struct CallArgs {
    /// Session whose tabs the tool works in.
    session: String,
    /// Arguments as key=value pairs or one JSON object, e.g. `url=https://example.org` or
    /// `tab_id=T1 scope=viewport`.
    #[arg(value_name = "ARG")]
    args: Vec<String>,
    /// Retry under this approved ticket (normally done automatically while waiting).
    #[arg(long, value_name = "TICKET_ID")]
    approval_ticket: Option<String>,
    /// Return a pending ticket at once instead of waiting for the person's answer.
    #[arg(long)]
    no_wait: bool,
    /// Stop waiting for a person after this many seconds.
    #[arg(long, value_name = "SECS")]
    timeout: Option<u64>,
    /// Write the PNG the call saved (a screenshot) to this file.
    #[arg(long, value_name = "FILE")]
    out: Option<std::path::PathBuf>,
}

impl BrowserCommand {
    fn call(&self) -> Option<(&'static str, &CallArgs)> {
        Some(match self {
            Self::Tools => return None,
            Self::Tabs(args) => ("browser_tabs", args),
            Self::Open(args) => ("browser_open_tab", args),
            Self::Navigate(args) => ("browser_navigate", args),
            Self::Snapshot(args) => ("browser_snapshot", args),
            Self::Act(args) => ("browser_act", args),
            Self::Pointer(args) => ("browser_pointer", args),
            Self::Screenshot(args) => ("browser_screenshot", args),
            Self::Close(args) => ("browser_close_tab", args),
            Self::Login(args) => ("request_login", args),
            Self::Choice(args) => ("request_choice", args),
            Self::Form(args) => ("request_form", args),
            Self::Present(args) => ("present", args),
            Self::WaitApproval(args) => ("wait_for_approval", args),
            Self::Otp(args) => ("request_otp", args),
            Self::RegisterAuthenticator(args) => ("register_authenticator", args),
            Self::HumanStep(args) => ("request_human_step", args),
            Self::Ask(args) => ("ask_user", args),
            Self::VaultLogin(args) => ("vault_login", args),
            Self::VaultOtp(args) => ("vault_otp", args),
            Self::VaultPasskey(args) => ("vault_passkey", args),
        })
    }
}

/// The daemon tool each `browser` verb calls, derived from the same clap definitions the CLI
/// parses with, so a verb added without its mapping cannot go unnoticed by the catalog parity
/// test in openside-e2e.
pub(crate) fn verb_tools() -> Vec<&'static str> {
    #[derive(clap::Parser)]
    struct Probe {
        #[command(subcommand)]
        command: BrowserCommand,
    }
    let verbs: Vec<String> = <Probe as clap::CommandFactory>::command()
        .get_subcommands()
        .map(|verb| verb.get_name().to_owned())
        .collect();
    let mut tools: Vec<&'static str> = verbs
        .iter()
        .filter_map(|verb| {
            let probe = <Probe as clap::Parser>::try_parse_from(["probe", verb, "session"]).ok()?;
            probe.command.call().map(|(tool, _)| tool)
        })
        .collect();
    tools.sort_unstable();
    tools
}

fn parse_args(args: &[String]) -> Result<Value, String> {
    if let [only] = args {
        if only.trim_start().starts_with('{') {
            return serde_json::from_str::<Value>(only)
                .ok()
                .filter(Value::is_object)
                .ok_or_else(|| "the JSON argument must be an object".to_owned());
        }
    }
    let mut object = Map::new();
    for pair in args {
        let (key, value) = pair
            .split_once('=')
            .filter(|(key, _)| !key.is_empty())
            .ok_or_else(|| format!("argument `{pair}` is not key=value"))?;
        let value = serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.into()));
        object.insert(key.to_owned(), value);
    }
    Ok(Value::Object(object))
}

pub(crate) async fn execute<W: Write, E: Write>(
    command: &BrowserCommand,
    json_output: bool,
    client: &Client,
    stdout: &mut W,
    stderr: &mut E,
    summary: &mut Summary,
) -> Result<(), String> {
    let Some((tool, call)) = command.call() else {
        let catalog: ToolCatalog = client.get("/v1/tools").await?;
        if json_output {
            return emit(stdout, &catalog);
        }
        for tool in &catalog.tools {
            writeln!(stdout, "{}\t{}", tool.name, tool.input_schema).map_err(output_error)?;
        }
        return Ok(());
    };
    summary.session_id = Some(call.session.clone());
    let mut args = parse_args(&call.args)?;
    if tool == "browser_screenshot" && call.out.is_some() {
        args["output"] = Value::Bool(true);
    }
    let wait = (!call.no_wait).then(|| call.timeout.map(Duration::from_secs));
    let Outcome { value, ok } = tool_call::drive(
        client,
        call.approval_ticket.clone(),
        wait,
        |approval| tool_call::tool(client, &call.session, tool, &args, approval),
        |ticket, approval| {
            // Best effort: a closed stderr must not abort a call a person is about to answer.
            let _ = if approval {
                writeln!(
                    stderr,
                    "Approval needed (ticket {ticket}). Ask a person to approve on another paired web or phone device; this client cannot approve its own work.\nWaiting; the call reruns once approved."
                )
            } else {
                writeln!(
                    stderr,
                    "Waiting for a person to answer ticket {ticket} in the openside app (or: {}).",
                    client.ticket_command(ticket, "resolve", None)
                )
            };
            let _ = stderr.flush();
        },
    )
    .await?;
    if !ok {
        summary.failed = true;
        summary.status = "error";
    }
    if let (true, Some(out)) = (ok, &call.out) {
        let artifact = value["artifacts"]
            .as_array()
            .and_then(|artifacts| artifacts.first())
            .and_then(Value::as_str)
            .ok_or("the call saved no file to write to --out")?;
        let path = client::encode_segment(artifact);
        let (_, bytes) = client
            .bytes(&client::session_path(
                &call.session,
                &format!("artifacts/{path}"),
            ))
            .await?;
        std::fs::write(out, bytes)
            .map_err(|error| format!("could not write {}: {error}", out.display()))?;
    }
    if json_output {
        return emit(stdout, &value);
    }
    if !ok {
        return writeln!(stderr, "openside: {tool}: {value}").map_err(output_error);
    }
    match &value["result"] {
        Value::String(text) => writeln!(stdout, "{text}"),
        Value::Null => Ok(()),
        other => writeln!(stdout, "{other}"),
    }
    .map_err(output_error)?;
    for artifact in value["artifacts"].as_array().into_iter().flatten() {
        writeln!(
            stdout,
            "artifact: {}",
            artifact.as_str().unwrap_or_default()
        )
        .map_err(output_error)?;
    }
    if value["status"] == "deferred" {
        writeln!(
            stderr,
            "Ticket {} is pending: {}",
            value["ticket_id"].as_str().unwrap_or_default(),
            value["next"].as_str().unwrap_or_default()
        )
        .map_err(output_error)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_args;
    use serde_json::json;

    #[test]
    fn arguments_are_pairs_or_one_object() {
        assert_eq!(
            parse_args(&["url=https://a.test/?x=1".into(), "n=3".into()]).unwrap(),
            json!({"url":"https://a.test/?x=1","n":3})
        );
        assert_eq!(
            parse_args(&[r#"{"tab_id":"t","scope":"viewport"}"#.into()]).unwrap(),
            json!({"tab_id":"t","scope":"viewport"})
        );
        assert!(parse_args(&["novalue".into()]).is_err());
        assert!(parse_args(&["[1]".into()]).is_err());
    }
}
