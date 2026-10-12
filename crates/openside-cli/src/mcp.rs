//! `openside mcp`: a stdio MCP server that is a thin client of the daemon's public API.
//!
//! Every tool call becomes one or more `/v1/*` requests through the same [`Client`] the rest of
//! the CLI uses, so requests outside `openside_proto::ROUTES` are refused before they leave the
//! process and the daemon enforces the capability scopes of whichever token (or the local
//! socket) the server was started with. The server holds no state of its own.
//!
//! stdout carries the MCP protocol only; diagnostics go to stderr.

use std::{io::Write, path::Path, time::Duration};

use http_body_util::BodyExt;
use openside_proto::{Event, EventKind, MessageMode, PolicyMode};
use rmcp::{
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        JsonObject, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
        Tool, ToolAnnotations,
    },
    service::RequestContext,
    ErrorData, RoleServer, ServerHandler, ServiceExt,
};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Map, Value};

use crate::{
    client::{self, Client},
    sse::Decoder,
};

const DEFAULT_WAIT_SECS: u64 = 120;
const MAX_WAIT_SECS: u64 = 3600;

/// Runs the server on stdin/stdout until the client disconnects. Returns the process exit code.
pub(crate) async fn run<E: Write>(
    socket: Option<&Path>,
    url: Option<&str>,
    token: Option<String>,
    stderr: &mut E,
) -> u8 {
    let client = match Client::new(socket, url, token.as_deref()) {
        Ok(client) => client,
        Err(error) => {
            let _ = writeln!(
                stderr,
                "openside: {}",
                redact(&error.to_string(), token.as_deref())
            );
            return 2;
        }
    };
    let served = match Mcp::new(client, token.clone())
        .serve(rmcp::transport::stdio())
        .await
    {
        Ok(running) => running
            .waiting()
            .await
            .map(|_| ())
            .map_err(|e| e.to_string()),
        Err(error) => Err(error.to_string()),
    };
    match served {
        Ok(()) => 0,
        Err(error) => {
            let _ = writeln!(
                stderr,
                "openside: MCP server stopped: {}",
                redact(&error, token.as_deref())
            );
            1
        }
    }
}

/// Messages echo caller input (argument errors) or daemon text, so the token is scrubbed from
/// every string that leaves the process, protocol errors and stderr included.
pub(crate) fn redact(message: &str, token: Option<&str>) -> String {
    match token.filter(|token| !token.is_empty()) {
        Some(token) => message.replace(token, "[REDACTED]"),
        None => message.to_owned(),
    }
}

pub(crate) struct Mcp {
    client: Client,
    token: Option<String>,
}

/// Why a tool call did not produce a result.
enum Failure {
    /// The arguments do not fit the tool's schema: a protocol-level error.
    Params(String),
    /// The call was valid but the daemon (or a safety rule) refused or failed it: reported to
    /// the model as a tool result with `isError`, which MCP clients surface verbatim.
    Tool(String),
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self::Tool(message)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionCreate {
    title: Option<String>,
    model: Option<String>,
    mode: Option<PolicyMode>,
    workspace: Option<String>,
    #[serde(default)]
    incognito: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionRef {
    session_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionSend {
    session_id: String,
    text: String,
    mode: Option<MessageMode>,
    message_id: Option<String>,
    #[serde(default)]
    wait: bool,
    timeout_secs: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionWait {
    session_id: String,
    after_seq: Option<u64>,
    timeout_secs: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TicketList {
    #[serde(default)]
    all: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TicketResolve {
    ticket_id: String,
    value: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MemorySearch {
    query: Option<String>,
    kind: Option<String>,
    limit: Option<u32>,
}

/// Fields every browser tool shares: the agent tab and the approval to retry under.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserOpen {
    session_id: String,
    url: String,
    tab_id: Option<String>,
    approval_ticket: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserSnapshot {
    session_id: String,
    tab_id: Option<String>,
    approval_ticket: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserAct {
    session_id: String,
    action: String,
    #[serde(rename = "ref")]
    target: String,
    text: Option<String>,
    tab_id: Option<String>,
    approval_ticket: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserRepl {
    session_id: String,
    code: String,
    tab_id: Option<String>,
    approval_ticket: Option<String>,
}

/// Where a browser tool runs and which approval, if any, it retries under.
struct Target {
    session_id: String,
    tab_id: Option<String>,
    approval_ticket: Option<String>,
}

impl Mcp {
    pub(crate) fn new(client: Client, token: Option<String>) -> Self {
        Self { client, token }
    }

    fn redact(&self, message: String) -> String {
        redact(&message, self.token.as_deref())
    }

    async fn dispatch(&self, name: &str, arguments: Value) -> Result<Value, Failure> {
        match name {
            "session_create" => {
                let args: SessionCreate = parse(arguments)?;
                if let Some(workspace) = &args.workspace {
                    if !Path::new(workspace).is_absolute() {
                        return Err(Failure::Params("workspace must be an absolute path".into()));
                    }
                }
                let mut body = Map::new();
                body.insert("title".into(), json!(args.title));
                body.insert("model".into(), json!(args.model));
                body.insert("mode".into(), json!(args.mode));
                body.insert("workspace".into(), json!(args.workspace));
                if args.incognito {
                    body.insert("incognito".into(), json!(true));
                }
                Ok(self
                    .client
                    .json("POST", "/v1/sessions", Some(&Value::Object(body)))
                    .await?)
            }
            "session_list" => {
                let _: Empty = parse(arguments)?;
                Ok(self.client.get("/v1/sessions").await?)
            }
            "session_status" => {
                let args: SessionRef = parse(arguments)?;
                Ok(self
                    .client
                    .get(&client::session_path(&args.session_id, ""))
                    .await?)
            }
            "session_send" => self.session_send(parse(arguments)?).await,
            "session_wait" => self.session_wait(parse(arguments)?).await,
            "ticket_list" => {
                let args: TicketList = parse(arguments)?;
                let path = if args.all {
                    "/v1/tickets"
                } else {
                    "/v1/tickets?status=pending"
                };
                Ok(self.client.get(path).await?)
            }
            "ticket_resolve" => self.ticket_resolve(parse(arguments)?).await,
            "memory_search" => {
                let path = memory_path(&parse(arguments)?);
                Ok(self.client.get(&path).await?)
            }
            "browser_open" => {
                let args: BrowserOpen = parse(arguments)?;
                let code = format!(
                    "await page.goto({}); await page.url()",
                    js_string(&args.url)
                );
                let target = Target {
                    session_id: args.session_id,
                    tab_id: args.tab_id,
                    approval_ticket: args.approval_ticket,
                };
                self.browser(
                    target,
                    code,
                    |result| json!({"tab_id": result["tab_id"], "url": result["return_value"]}),
                )
                .await
            }
            "browser_snapshot" => {
                let args: BrowserSnapshot = parse(arguments)?;
                let target = Target {
                    session_id: args.session_id,
                    tab_id: args.tab_id,
                    approval_ticket: args.approval_ticket,
                };
                self.browser(target, "await page.snapshot()".into(), |result| {
                    let tab = result["tab_id"].as_str().unwrap_or_default();
                    let snapshot = match &result["return_value"] {
                        Value::String(text) => text.clone(),
                        other => other.to_string(),
                    };
                    Value::String(format!("tab_id: {tab}\n{snapshot}"))
                })
                .await
            }
            "browser_act" => {
                let args: BrowserAct = parse(arguments)?;
                let code = act_code(&args)?;
                let target = Target {
                    session_id: args.session_id,
                    tab_id: args.tab_id,
                    approval_ticket: args.approval_ticket,
                };
                self.browser(
                    target,
                    code,
                    |result| json!({"tab_id": result["tab_id"], "url": result["return_value"]}),
                )
                .await
            }
            "browser_repl" => {
                let args: BrowserRepl = parse(arguments)?;
                let target = Target {
                    session_id: args.session_id,
                    tab_id: args.tab_id,
                    approval_ticket: args.approval_ticket,
                };
                self.browser(target, args.code, Value::clone).await
            }
            other => Err(Failure::Params(format!("unknown tool `{other}`"))),
        }
    }

    async fn session_send(&self, args: SessionSend) -> Result<Value, Failure> {
        let timeout = wait_timeout(args.timeout_secs)?;
        let session: Value = self
            .client
            .get(&client::session_path(&args.session_id, ""))
            .await?;
        // Replies are read from the stream after this point, so a settle from an earlier
        // turn cannot be mistaken for the answer to this message.
        let after_seq = session["last_seq"].as_u64().unwrap_or(0);
        let mode = args.mode.unwrap_or(MessageMode::Now);
        let mut body = Map::new();
        body.insert("text".into(), json!(args.text));
        body.insert("mode".into(), json!(mode));
        body.insert("message_id".into(), json!(args.message_id));
        let message: Value = self
            .client
            .json(
                "POST",
                &client::session_path(&args.session_id, "messages"),
                Some(&Value::Object(body)),
            )
            .await?;
        let mut result = json!({"message": message, "after_seq": after_seq});
        if args.wait {
            let message_id = result["message"]["id"].as_str().map(str::to_owned);
            let replay = Replay {
                last_seen: after_seq,
                terminal_after: after_seq,
                message_id,
            };
            let waited = self.wait_settled(&args.session_id, replay, timeout).await?;
            if let (Some(result), Value::Object(waited)) = (result.as_object_mut(), waited) {
                result.extend(waited);
            }
        }
        Ok(result)
    }

    async fn session_wait(&self, args: SessionWait) -> Result<Value, Failure> {
        let timeout = wait_timeout(args.timeout_secs)?;
        // Always read the session: it tells a settled session (nothing to wait for) from a busy
        // one, and bounds which settlements in the replayed log are history.
        let session: Value = self
            .client
            .get(&client::session_path(&args.session_id, ""))
            .await?;
        let status = session["status"].as_str().unwrap_or_default();
        let last_seq = session["last_seq"].as_u64().unwrap_or(0);
        let active =
            status == "running" || session["queue"].as_array().is_some_and(|q| !q.is_empty());
        let after_seq = args.after_seq.unwrap_or(last_seq);
        if after_seq > last_seq {
            return Err(Failure::Tool(
                "after_seq is ahead of the session's event log".into(),
            ));
        }
        if after_seq == last_seq && !active {
            // Nothing in flight: report the current state instead of waiting for an event that
            // will not come.
            let pending = self.pending_tickets(&args.session_id).await?;
            let status = match status {
                "idle" if pending.is_empty() => "settled",
                "idle" => "waiting_human",
                other => other,
            };
            return Ok(json!({
                "status": status,
                "last_seq": last_seq,
                "replies": [],
                "pending_tickets": pending,
            }));
        }
        // While a run is active only newer settlements count. For an idle session the last
        // logged event is the one that ended the run, so it still counts.
        let terminal_after = if active {
            last_seq
        } else {
            last_seq.saturating_sub(1)
        };
        let replay = Replay {
            last_seen: after_seq,
            terminal_after,
            message_id: None,
        };
        Ok(self.wait_settled(&args.session_id, replay, timeout).await?)
    }

    async fn pending_tickets(&self, session: &str) -> Result<Vec<Value>, String> {
        let tickets: Value = self.client.get("/v1/tickets?status=pending").await?;
        Ok(tickets["tickets"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|ticket| ticket["session_id"].as_str() == Some(session))
            .map(summarize)
            .collect())
    }

    /// Streams the session's events after `replay.last_seen` until it settles or stops, or the
    /// timeout passes (reported as status `timeout`, not as an error).
    async fn wait_settled(
        &self,
        session: &str,
        replay: Replay,
        timeout: Duration,
    ) -> Result<Value, String> {
        let mut state = Wait::new(replay);
        let outcome = tokio::time::timeout(timeout, self.drive(session, &mut state)).await;
        match outcome {
            Ok(result) => result?,
            Err(_) => state.status = Some("timeout"),
        }
        // Read at the end, not before the replay: tickets opened and resolved while we were
        // catching up must not be reported, and one opened last must not be missed.
        let pending = self.pending_tickets(session).await?;
        if state.status == Some("settled") && !pending.is_empty() {
            state.status = Some("waiting_human");
        }
        Ok(json!({
            "status": state.status,
            "last_seq": state.last_seq,
            "replies": state.replies,
            "pending_tickets": pending,
            "error": state.error,
        }))
    }

    async fn drive(&self, session: &str, state: &mut Wait) -> Result<(), String> {
        let mut response = self.client.events(session, state.last_seq).await?;
        let mut decoder = Decoder::default();
        while let Some(frame) = response.body_mut().frame().await {
            let frame = frame.map_err(|_| "event stream connection failed".to_owned())?;
            let Ok(bytes) = frame.into_data() else {
                continue;
            };
            for record in decoder.push(&bytes)? {
                let event: Event = serde_json::from_slice(&record.data)
                    .map_err(|_| "invalid event JSON in SSE stream".to_owned())?;
                if event.session_id.as_str() != session
                    || event.seq == 0
                    || record.id != Some(event.seq)
                {
                    return Err(
                        "SSE event session/sequence does not match its envelope or id".into(),
                    );
                }
                if event.seq <= state.last_seq {
                    continue;
                }
                state.last_seq = event.seq;
                state.apply(event.seq, event.kind);
                if state.status.is_some() {
                    return Ok(());
                }
            }
        }
        Err("event stream ended before the session settled or stopped".into())
    }

    async fn ticket_resolve(&self, args: TicketResolve) -> Result<Value, Failure> {
        let path = client::ticket_path(&args.ticket_id, "");
        let ticket: Value = self.client.get(&path).await?;
        if ticket["kind"].as_str() == Some("secret_input") {
            // Secrets must reach the daemon without passing through a model-driven client.
            return Err(Failure::Tool(
                "secret_input tickets are never resolved through MCP; ask the user to run \
                 `openside ticket resolve <id> -` and pipe the secret on stdin"
                    .into(),
            ));
        }
        Ok(self
            .client
            .json(
                "POST",
                &client::ticket_path(&args.ticket_id, "resolve"),
                Some(&json!({"value": args.value})),
            )
            .await?)
    }

    /// Runs `code` in the session's agent tab through `POST /v1/sessions/{id}/repl`.
    async fn browser(
        &self,
        target: Target,
        code: String,
        render: impl FnOnce(&Value) -> Value,
    ) -> Result<Value, Failure> {
        let mut body = Map::new();
        body.insert("code".into(), json!(code));
        if let Some(tab) = target.tab_id {
            body.insert("tab_id".into(), json!(tab));
        }
        if let Some(ticket) = target.approval_ticket {
            body.insert("approval_ticket".into(), json!(ticket));
        }
        let reply: Value = self
            .client
            .json(
                "POST",
                &client::session_path(&target.session_id, "repl"),
                Some(&Value::Object(body)),
            )
            .await?;
        match reply["status"].as_str() {
            Some("done") => {
                let result = &reply["result"];
                if result["error"].as_str().is_some() {
                    return Err(Failure::Tool(result.to_string()));
                }
                Ok(render(result))
            }
            Some("deferred") => Ok(json!({
                "status": "deferred",
                "ticket_id": reply["ticket_id"],
                "next": "A human approval is pending. Once the ticket is resolved, call this tool \
                         again with the same arguments plus approval_ticket set to ticket_id.",
            })),
            _ => Err(Failure::Tool(
                json!({
                    "status": reply["status"],
                    "reason": reply["reason"],
                    "hint": reply["hint"],
                })
                .to_string(),
            )),
        }
    }
}

/// Where a wait starts and which part of the replayed log counts.
struct Replay {
    /// Events up to and including this sequence are not replayed.
    last_seen: u64,
    /// A settlement, stop or error at or before this sequence is history, not an outcome.
    terminal_after: u64,
    /// The message whose turn is awaited, when the wait follows a send.
    message_id: Option<String>,
}

struct Wait {
    last_seq: u64,
    terminal_after: u64,
    message_id: Option<String>,
    /// Replies are only ours once our message was dispatched; earlier queued turns may still
    /// finish first.
    started: bool,
    replies: Vec<String>,
    message_done: bool,
    status: Option<&'static str>,
    error: Option<String>,
}

impl Wait {
    fn new(replay: Replay) -> Self {
        Self {
            last_seq: replay.last_seen,
            terminal_after: replay.terminal_after,
            started: replay.message_id.is_none(),
            message_id: replay.message_id,
            replies: Vec::new(),
            message_done: false,
            status: None,
            error: None,
        }
    }

    fn is_ours(&self, id: &str) -> bool {
        self.message_id.as_deref() == Some(id)
    }

    fn apply(&mut self, seq: u64, kind: EventKind) {
        let current = seq > self.terminal_after;
        match kind {
            EventKind::AssistantFinal { text } if self.started => self.replies.push(text),
            EventKind::MessageInjected { message_id } if self.is_ours(message_id.as_str()) => {
                self.started = true;
            }
            EventKind::MessageCompleted { message_id: id, .. }
            | EventKind::MessageDropped { message_id: id, .. }
                if self.is_ours(id.as_str()) =>
            {
                self.message_done = true;
            }
            EventKind::RuntimeError { code, message } if current => {
                self.error = Some(format!("{code}: {message}"));
                self.status = Some("error");
            }
            EventKind::SessionSettled {}
                if current && (self.message_id.is_none() || self.message_done) =>
            {
                self.status = Some("settled");
            }
            EventKind::SessionStopped {} if current => self.status = Some("stopped"),
            _ => {}
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

fn parse<T: DeserializeOwned>(arguments: Value) -> Result<T, Failure> {
    let arguments = if arguments.is_null() {
        json!({})
    } else {
        arguments
    };
    serde_json::from_value(arguments).map_err(|error| Failure::Params(error.to_string()))
}

// Kept out of `dispatch` so the non-`Send` form serializer is never held across an await.
fn memory_path(args: &MemorySearch) -> String {
    let mut params = url::form_urlencoded::Serializer::new(String::new());
    if let Some(query) = &args.query {
        params.append_pair("q", query);
    }
    if let Some(kind) = &args.kind {
        params.append_pair("kind", kind);
    }
    if let Some(limit) = args.limit {
        params.append_pair("limit", &limit.to_string());
    }
    let params = params.finish();
    if params.is_empty() {
        "/v1/memory".to_owned()
    } else {
        format!("/v1/memory?{params}")
    }
}

fn wait_timeout(secs: Option<u64>) -> Result<Duration, Failure> {
    match secs.unwrap_or(DEFAULT_WAIT_SECS) {
        0 => Err(Failure::Params("timeout_secs must be at least 1".into())),
        secs => Ok(Duration::from_secs(secs.min(MAX_WAIT_SECS))),
    }
}

fn summarize(ticket: &Value) -> Value {
    json!({"id": ticket["id"], "kind": ticket["kind"], "summary": ticket["summary"]})
}

/// A JSON string literal, which is also a valid JavaScript string literal.
fn js_string(value: &str) -> String {
    Value::String(value.to_owned()).to_string()
}

/// REPL code for one `browser_act`; `ref` comes from a `browser_snapshot`.
fn act_code(args: &BrowserAct) -> Result<String, Failure> {
    let valid = !args.target.is_empty()
        && args
            .target
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    if !valid {
        return Err(Failure::Params(
            "ref must be an element ref from browser_snapshot, such as e12".into(),
        ));
    }
    let locator = format!(
        "page.locator({})",
        js_string(&format!("aria-ref={}", args.target))
    );
    let text = || {
        args.text
            .as_deref()
            .map(js_string)
            .ok_or_else(|| Failure::Params(format!("action `{}` needs `text`", args.action)))
    };
    let call = match args.action.as_str() {
        "click" => "click()".to_owned(),
        "hover" => "hover()".to_owned(),
        "check" => "check()".to_owned(),
        "uncheck" => "uncheck()".to_owned(),
        "fill" => format!("fill({})", text()?),
        "press" => format!("press({})", text()?),
        "select_option" => format!("selectOption({})", text()?),
        other => {
            return Err(Failure::Params(format!(
                "unknown action `{other}`; use click, hover, check, uncheck, fill, press or select_option"
            )))
        }
    };
    Ok(format!("await {locator}.{call}; await page.url()"))
}

fn object(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

fn schema(properties: Value, required: &[&str]) -> JsonObject {
    object(json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    }))
}

fn browser_schema(extra: Value, required: &[&str]) -> JsonObject {
    let mut properties = object(json!({
        "session_id": {"type": "string", "description": "Session whose agent tab is driven."},
        "tab_id": {"type": "string",
            "description": "Agent tab; optional when the session owns at most one tab."},
        "approval_ticket": {"type": "string",
            "description": "Approved ticket id from an earlier `deferred` result."},
    }));
    properties.extend(object(extra));
    let mut required = required.to_vec();
    required.push("session_id");
    schema(Value::Object(properties), &required)
}

fn tool(name: &'static str, description: &'static str, read_only: bool, input: JsonObject) -> Tool {
    Tool::new(name, description, input)
        .with_annotations(ToolAnnotations::new().read_only(read_only))
}

pub(crate) fn tools() -> Vec<Tool> {
    let session_id = json!({"type": "string", "description": "Session id."});
    let wait_args = json!({
        "timeout_secs": {"type": "integer", "minimum": 1, "maximum": MAX_WAIT_SECS,
            "description": "Give up waiting after this long; default 120."},
    });
    let mut send_props = object(json!({
        "session_id": session_id,
        "text": {"type": "string", "description": "Message for the agent."},
        "mode": {"type": "string", "enum": ["queue", "steer", "now"],
            "description": "queue: after the current turn; steer: at the next boundary; now (default): start or steer immediately."},
        "message_id": {"type": "string", "description": "Optional idempotency key."},
        "wait": {"type": "boolean",
            "description": "Wait until the session settles and return the agent's replies."},
    }));
    send_props.extend(object(wait_args.clone()));
    let mut wait_props = object(json!({
        "session_id": session_id,
        "after_seq": {"type": "integer", "minimum": 0,
            "description": "Only consider events after this sequence (`after_seq` from session_send, or `last_seq` from a timed-out wait)."},
    }));
    wait_props.extend(object(wait_args));
    vec![
        tool(
            "session_create",
            "Create an agent session. Needs the steer scope.",
            false,
            schema(
                json!({
                    "title": {"type": "string"},
                    "model": {"type": "string", "description": "Provider-qualified model; default from settings."},
                    "mode": {"type": "string", "enum": ["read-only", "guard", "full"]},
                    "workspace": {"type": "string", "description": "Absolute workspace path."},
                    "incognito": {"type": "boolean", "description": "Private session whose browsing is never remembered."},
                }),
                &[],
            ),
        ),
        tool("session_list", "List sessions.", true, schema(json!({}), &[])),
        tool(
            "session_status",
            "Show one session: status, last event sequence and the pending message queue.",
            true,
            schema(json!({"session_id": session_id}), &["session_id"]),
        ),
        tool(
            "session_send",
            "Send a message to a session. With wait=true, returns the agent's replies once the session settles, waits on a human ticket, or stops. Needs the steer scope, plus read when wait=true.",
            false,
            schema(Value::Object(send_props), &["session_id", "text"]),
        ),
        tool(
            "session_wait",
            "Wait until a session settles, waits on a human ticket, or stops, and return replies seen meanwhile. Status `timeout` means keep waiting from the returned last_seq. Needs the read scope.",
            true,
            schema(Value::Object(wait_props), &["session_id"]),
        ),
        tool(
            "ticket_list",
            "List human-intervention tickets (pending by default).",
            true,
            schema(
                json!({"all": {"type": "boolean", "description": "Include resolved, rejected and expired tickets."}}),
                &[],
            ),
        ),
        tool(
            "ticket_resolve",
            "Resolve a confirmation or information ticket. Refuses secret_input tickets; the user must supply those. Needs the approve and read scopes.",
            false,
            schema(
                json!({
                    "ticket_id": {"type": "string"},
                    "value": {"description": "true/false for confirmations, or the answer for information tickets."},
                }),
                &["ticket_id", "value"],
            ),
        ),
        tool(
            "memory_search",
            "Search the user's local memory notes and browsing history.",
            true,
            schema(
                json!({
                    "query": {"type": "string"},
                    "kind": {"type": "string", "enum": ["note", "history"]},
                    "limit": {"type": "integer", "minimum": 1},
                }),
                &[],
            ),
        ),
        tool(
            "browser_open",
            "Navigate the session's agent tab to a URL. Needs the browse scope; policy may require human approval.",
            false,
            browser_schema(
                json!({"url": {"type": "string", "description": "Address to open."}}),
                &["url"],
            ),
        ),
        tool(
            "browser_snapshot",
            "Accessibility snapshot of the agent tab; its eN refs feed browser_act. Large snapshots come back truncated.",
            true,
            browser_schema(json!({}), &[]),
        ),
        tool(
            "browser_act",
            "Act on an element by its snapshot ref: click, hover, check, uncheck, fill, press or select_option. Policy may require human approval.",
            false,
            browser_schema(
                json!({
                    "action": {"type": "string",
                        "enum": ["click", "hover", "check", "uncheck", "fill", "press", "select_option"]},
                    "ref": {"type": "string", "description": "Element ref from browser_snapshot, such as e12."},
                    "text": {"type": "string", "description": "Value for fill, key for press, option for select_option."},
                }),
                &["action", "ref"],
            ),
        ),
        tool(
            "browser_repl",
            "Run Playwright-syntax JavaScript (page, expect, console) in the agent tab; see docs/specs/browser-repl.md.",
            false,
            browser_schema(
                json!({"code": {"type": "string", "description": "Snippet; the last expression is returned."}}),
                &["code"],
            ),
        ),
    ]
}

impl ServerHandler for Mcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("openside", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Tools over the openside daemon's public API: sessions, human tickets, memory \
                 search and the session browser. Each call is limited by the token's scopes.",
            )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(tools()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let arguments = request.arguments.map_or(Value::Null, Value::Object);
        match self.dispatch(&request.name, arguments).await {
            Ok(value) => {
                let text = match value {
                    Value::String(text) => text,
                    other => other.to_string(),
                };
                Ok(CallToolResult::success(vec![ContentBlock::text(self.redact(text))]).into())
            }
            Err(Failure::Params(message)) => {
                Err(ErrorData::invalid_params(self.redact(message), None))
            }
            Err(Failure::Tool(message)) => {
                Ok(CallToolResult::error(vec![ContentBlock::text(self.redact(message))]).into())
            }
        }
    }
}
