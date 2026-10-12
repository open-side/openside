//! `openside mcp`: a stdio MCP server that is a thin client of the daemon's public API.
//!
//! Every tool call becomes one or more `/v1/*` requests through the same [`Client`] the rest of
//! the CLI uses, so requests outside `openside_proto::ROUTES` are refused before they leave the
//! process and the daemon enforces the capability scopes of whichever token (or the local
//! socket) the server was started with. The server holds no state of its own.
//!
//! Two ways to use the browser: delegate a goal to the openside agent (`agent_*`), or drive the
//! session browser directly with the same tools the agent has. Those direct tools are listed
//! from the daemon's catalog (`GET /v1/tools`) on every `tools/list`, so they always match the
//! daemon's own names and schemas; `browser_repl` runs Playwright-syntax scripts.
//!
//! stdout carries the MCP protocol only; diagnostics go to stderr.

use std::{io::Write, path::Path, time::Duration};

use base64::Engine;
use openside_proto::{BrowserSpace, MessageMode, PolicyMode, ToolCatalog};
use rmcp::{
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        JsonObject, ListToolsResult, PaginatedRequestParams, ProgressNotificationParam,
        ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
    },
    service::RequestContext,
    ErrorData, Peer, RoleServer, ServerHandler, ServiceExt,
};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Map, Value};

use crate::{
    agent,
    client::{self, Client},
    tool_call,
    wait::{self, Replay, Until},
};

const DEFAULT_WAIT_SECS: u64 = 120;
const MAX_WAIT_SECS: u64 = 3600;

/// Runs the server on stdin/stdout until the client disconnects. Returns the process exit code.
pub(crate) async fn run<E: Write>(
    socket: Option<&Path>,
    url: Option<&str>,
    token: Option<String>,
    stderr: &mut E,
    remote: Option<String>,
) -> u8 {
    let client = match Client::from_flags(socket, url, token.as_deref(), remote.as_deref()).await {
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

/// Where `notifications/progress` go for the current call, when the client asked for them.
struct Progress {
    peer: Peer<RoleServer>,
    token: rmcp::model::ProgressToken,
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
    #[serde(default)]
    browser_space: Option<BrowserSpace>,
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
struct TicketWait {
    ticket_id: String,
    timeout_secs: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MemorySearch {
    query: Option<String>,
    kind: Option<String>,
    limit: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserRepl {
    session_id: String,
    code: String,
    tab_id: Option<String>,
    approval_ticket: Option<String>,
    wait: Option<bool>,
    timeout_secs: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentRun {
    goal: String,
    session_id: Option<String>,
    #[serde(default)]
    wait: bool,
    timeout_secs: Option<u64>,
    model: Option<String>,
    mode: Option<PolicyMode>,
    browser_space: Option<BrowserSpace>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentResult {
    session_id: String,
    after_seq: u64,
    message_id: Option<String>,
    timeout_secs: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentSteer {
    session_id: String,
    text: String,
    task_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentCancel {
    session_id: String,
    task_id: Option<String>,
}

impl Mcp {
    pub(crate) fn new(client: Client, token: Option<String>) -> Self {
        Self { client, token }
    }

    fn redact(&self, message: String) -> String {
        redact(&message, self.token.as_deref())
    }

    async fn catalog(&self) -> Result<ToolCatalog, String> {
        self.client.get("/v1/tools").await
    }

    async fn dispatch(
        &self,
        name: &str,
        arguments: Value,
        progress: Option<Progress>,
    ) -> Result<Value, Failure> {
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
                if let Some(space) = args.browser_space {
                    body.insert("browser_space".into(), json!(space));
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
            "ticket_wait" => {
                let args: TicketWait = parse(arguments)?;
                let timeout = wait_timeout(args.timeout_secs)?;
                Ok(
                    match tool_call::wait_ticket(&self.client, &args.ticket_id, Some(timeout))
                        .await?
                    {
                        Some(ticket) => json!({
                            "status": ticket.status,
                            "kind": ticket.kind,
                            "summary": ticket.summary,
                            "resolution": ticket.resolution,
                        }),
                        None => json!({"status": "pending", "ticket_id": args.ticket_id}),
                    },
                )
            }
            "memory_search" => {
                let path = memory_path(&parse(arguments)?);
                Ok(self.client.get(&path).await?)
            }
            "browser_repl" => {
                let args: BrowserRepl = parse(arguments)?;
                let wait = direct_wait(args.wait, args.timeout_secs)?;
                let outcome = tool_call::drive(
                    &self.client,
                    args.approval_ticket,
                    wait,
                    |approval| {
                        tool_call::repl(
                            &self.client,
                            &args.session_id,
                            &args.code,
                            args.tab_id.as_deref(),
                            approval,
                        )
                    },
                    |_, _| {},
                )
                .await?;
                outcome_result(outcome)
            }
            "agent_run" => self.agent_run(parse(arguments)?, progress).await,
            "agent_status" => {
                let args: SessionRef = parse(arguments)?;
                self.agent_status(&args.session_id).await
            }
            "agent_result" => {
                let args: AgentResult = parse(arguments)?;
                let timeout = wait_timeout(args.timeout_secs)?;
                let replay = Replay {
                    last_seen: args.after_seq,
                    terminal_after: args.after_seq,
                    message_id: args.message_id,
                };
                let mut result = self
                    .settle_with_progress(&args.session_id, replay, timeout, progress)
                    .await?;
                result["tasks"] = self.tasks(&args.session_id).await?;
                Ok(result)
            }
            "agent_steer" => {
                let args: AgentSteer = parse(arguments)?;
                Ok(match &args.task_id {
                    Some(task) => {
                        self.client
                            .json(
                                "POST",
                                &client::session_path(
                                    &args.session_id,
                                    &format!("subagents/{}/steer", client::encode_segment(task)),
                                ),
                                Some(&json!({"text": args.text})),
                            )
                            .await?
                    }
                    None => {
                        self.client
                            .json(
                                "POST",
                                &client::session_path(&args.session_id, "messages"),
                                Some(&json!({"text": args.text, "mode": MessageMode::Steer})),
                            )
                            .await?
                    }
                })
            }
            "agent_cancel" => {
                let args: AgentCancel = parse(arguments)?;
                let path = match &args.task_id {
                    Some(task) => client::session_path(
                        &args.session_id,
                        &format!("subagents/{}/cancel", client::encode_segment(task)),
                    ),
                    None => client::session_path(&args.session_id, "stop"),
                };
                Ok(self
                    .client
                    .json::<Value, Value>("POST", &path, None)
                    .await?)
            }
            other => Err(Failure::Params(format!("unknown tool `{other}`"))),
        }
    }

    /// A daemon tool from the catalog, called through `POST /v1/sessions/{id}/tools/{name}`.
    /// Saved PNG artifacts (screenshots) come back as image content too, so the client's model
    /// sees the page the way the in-app agent does.
    async fn direct(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<(Value, Vec<ContentBlock>), Failure> {
        let catalog = self.catalog().await?;
        if !catalog.tools.iter().any(|tool| tool.name == name) {
            return Err(Failure::Params(format!("unknown tool `{name}`")));
        }
        let Value::Object(mut args) = (match arguments {
            Value::Null => json!({}),
            other => other,
        }) else {
            return Err(Failure::Params("arguments must be an object".into()));
        };
        let session = match args.remove("session_id") {
            Some(Value::String(session)) => session,
            _ => return Err(Failure::Params("session_id is required".into())),
        };
        let approval_ticket = match args.remove("approval_ticket") {
            None | Some(Value::Null) => None,
            Some(Value::String(ticket)) => Some(ticket),
            Some(_) => return Err(Failure::Params("approval_ticket must be a string".into())),
        };
        let wait = match args.remove("wait") {
            None | Some(Value::Null) => None,
            Some(Value::Bool(wait)) => Some(wait),
            Some(_) => return Err(Failure::Params("wait must be a boolean".into())),
        };
        let timeout = match args.remove("timeout_secs") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_u64()
                    .ok_or_else(|| Failure::Params("timeout_secs must be an integer".into()))?,
            ),
        };
        let wait = direct_wait(wait, timeout)?;
        let args = Value::Object(args);
        let outcome = tool_call::drive(
            &self.client,
            approval_ticket,
            wait,
            |approval| tool_call::tool(&self.client, &session, name, &args, approval),
            |_, _| {},
        )
        .await?;
        let value = outcome_result(outcome)?;
        let mut images = Vec::new();
        for artifact in value["artifacts"].as_array().into_iter().flatten() {
            let Some(artifact) = artifact.as_str().filter(|path| path.ends_with(".png")) else {
                continue;
            };
            let path = artifact
                .split('/')
                .map(client::encode_segment)
                .collect::<Vec<_>>()
                .join("/");
            let (content_type, bytes) = self
                .client
                .bytes(&client::session_path(
                    &session,
                    &format!("artifacts/{path}"),
                ))
                .await?;
            images.push(ContentBlock::image(
                base64::engine::general_purpose::STANDARD.encode(bytes),
                content_type,
            ));
        }
        Ok((value, images))
    }

    async fn agent_run(
        &self,
        args: AgentRun,
        progress: Option<Progress>,
    ) -> Result<Value, Failure> {
        let timeout = wait_timeout(args.timeout_secs)?;
        if args.goal.trim().is_empty() {
            return Err(Failure::Params("goal must not be empty".into()));
        }
        let create = openside_proto::CreateSessionRequest {
            title: None,
            model: args.model,
            mode: args.mode,
            workspace: None,
            incognito: false,
            browser_space: args.browser_space.unwrap_or_default(),
        };
        let started = agent::start(
            &self.client,
            args.session_id.as_deref(),
            &create,
            &args.goal,
        )
        .await?;
        let session = started.session.id.to_string();
        let mut result = json!({
            "session_id": session,
            "message_id": started.message.id,
            "after_seq": started.session.last_seq,
        });
        if !args.wait {
            result["next"] = json!("Poll agent_status, or call agent_result with session_id, message_id and after_seq to wait for the replies.");
            return Ok(result);
        }
        let replay = Replay {
            last_seen: started.session.last_seq,
            terminal_after: started.session.last_seq,
            message_id: Some(started.message.id.to_string()),
        };
        let waited = self
            .settle_with_progress(&session, replay, timeout, progress)
            .await?;
        if let (Some(result), Value::Object(waited)) = (result.as_object_mut(), waited) {
            result.extend(waited);
        }
        result["tasks"] = self.tasks(&session).await?;
        Ok(result)
    }

    async fn agent_status(&self, session: &str) -> Result<Value, Failure> {
        let current: Value = self.client.get(&client::session_path(session, "")).await?;
        Ok(json!({
            "session_id": session,
            "status": current["status"],
            "last_seq": current["last_seq"],
            "queue": current["queue"],
            "tasks": self.tasks(session).await?,
            "pending_tickets": wait::pending_tickets(&self.client, session).await?,
        }))
    }

    async fn tasks(&self, session: &str) -> Result<Value, String> {
        let list: Value = self
            .client
            .get(&client::session_path(session, "subagents"))
            .await?;
        Ok(Value::Array(
            list["subagents"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|task| {
                    json!({
                        "task_id": task["id"],
                        "task": task["task"],
                        "status": task["status"],
                        "background": task["background"],
                        "output": task["output"],
                        "reason": task["reason"],
                        "last_progress": task["last_progress"],
                    })
                })
                .collect(),
        ))
    }

    /// [`wait::settle`] that reports tasks, browser activity and tickets as
    /// `notifications/progress` when the client sent a progress token.
    async fn settle_with_progress(
        &self,
        session: &str,
        replay: Replay,
        timeout: Duration,
        progress: Option<Progress>,
    ) -> Result<Value, String> {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<String>();
        let forward = progress.map(|progress| {
            tokio::spawn(async move {
                let mut count = 0.0;
                while let Some(message) = receiver.recv().await {
                    count += 1.0;
                    let mut param = ProgressNotificationParam::new(progress.token.clone(), count);
                    param.message = Some(message);
                    if progress.peer.notify_progress(param).await.is_err() {
                        break;
                    }
                }
            })
        });
        let result = wait::settle(
            &self.client,
            session,
            replay,
            Until {
                timeout: Some(timeout),
                through_human: false,
            },
            |event| {
                if let Some(line) = agent::progress_line(event) {
                    let _ = sender.send(line);
                }
            },
        )
        .await;
        drop(sender);
        if let Some(forward) = forward {
            let _ = forward.await;
        }
        result
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
            let pending = wait::pending_tickets(&self.client, &args.session_id).await?;
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

    async fn wait_settled(
        &self,
        session: &str,
        replay: Replay,
        timeout: Duration,
    ) -> Result<Value, String> {
        wait::settle(
            &self.client,
            session,
            replay,
            Until {
                timeout: Some(timeout),
                through_human: false,
            },
            |_| {},
        )
        .await
    }

    async fn ticket_resolve(&self, args: TicketResolve) -> Result<Value, Failure> {
        let path = client::ticket_path(&args.ticket_id, "");
        let ticket: Value = self.client.get(&path).await?;
        if ticket["kind"] == "choice"
            || (ticket["kind"] == "confirm" && (args.value == true || args.value == "approve"))
        {
            return Err(Failure::Tool(
                "person_approval_required: MCP cannot approve confirmations or answer choice tickets. Ask the user to respond on a paired person device, then use ticket_wait. Denying a confirmation with false or \"reject\" is allowed.".into(),
            ));
        }
        if matches!(ticket["kind"].as_str(), Some("secret_input" | "login")) {
            // Secrets must reach the daemon without passing through a model-driven client.
            return Err(Failure::Tool(
                "secret_input and login tickets are never resolved through MCP; ask the user \
                 to answer them in the openside web UI or with `openside ticket resolve <id>`, then use ticket_wait"
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

/// Direct tools wait for a person by default: the call completes once the approval or answer
/// is given in the web or phone UI, and returns the pending ticket if `timeout_secs` passes.
fn direct_wait(
    wait: Option<bool>,
    timeout: Option<u64>,
) -> Result<Option<Option<Duration>>, Failure> {
    let timeout = wait_timeout(timeout)?;
    Ok(wait.unwrap_or(true).then_some(Some(timeout)))
}

fn outcome_result(outcome: tool_call::Outcome) -> Result<Value, Failure> {
    if outcome.ok {
        Ok(outcome.value)
    } else {
        Err(Failure::Tool(outcome.value.to_string()))
    }
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

fn tool(name: &'static str, description: &'static str, read_only: bool, input: JsonObject) -> Tool {
    Tool::new(name, description, input)
        .with_annotations(ToolAnnotations::new().read_only(read_only))
}

fn direct_properties() -> Value {
    json!({
        "session_id": {"type": "string", "description": "Session whose browser is driven (session_create or agent_run gives one)."},
        "approval_ticket": {"type": "string",
            "description": "Approved ticket id from an earlier `deferred` result; normally the call waits and reruns by itself."},
        "wait": {"type": "boolean",
            "description": "Wait (default) until a person answers a ticket this call opens in the openside web or phone UI; false returns the pending ticket at once."},
        "timeout_secs": {"type": "integer", "minimum": 1, "maximum": MAX_WAIT_SECS,
            "description": "Give up waiting for a person after this long; default 120."},
    })
}

/// The MCP form of a daemon catalog tool: its own schema plus [`direct_properties`].
fn direct_tool(descriptor: &openside_proto::ToolDescriptor) -> Tool {
    let mut input = object(descriptor.input_schema.clone());
    input.insert("type".into(), json!("object"));
    let properties = input.entry("properties").or_insert_with(|| json!({}));
    if let Value::Object(properties) = properties {
        properties.extend(object(direct_properties()));
    }
    let required = input.entry("required").or_insert_with(|| json!([]));
    if let Value::Array(required) = required {
        required.push(json!("session_id"));
    }
    let read_only = matches!(
        descriptor.name.as_str(),
        "browser_tabs" | "browser_snapshot" | "browser_screenshot"
    );
    let description = format!(
        "{} (Direct daemon tool, same as the openside agent's; needs the {} scope over TCP.)",
        descriptor.description,
        serde_json::to_value(descriptor.scope)
            .ok()
            .and_then(|scope| scope.as_str().map(str::to_owned))
            .unwrap_or_default()
    );
    Tool::new(descriptor.name.clone(), description, input)
        .with_annotations(ToolAnnotations::new().read_only(read_only))
}

/// The tools that do not come from the daemon's catalog.
pub(crate) fn tools() -> Vec<Tool> {
    let session_id = json!({"type": "string", "description": "Session id."});
    let timeout = json!({"type": "integer", "minimum": 1, "maximum": MAX_WAIT_SECS,
        "description": "Give up waiting after this long; default 120."});
    let wait_args = json!({ "timeout_secs": timeout });
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
    let mut repl_props = object(json!({
        "code": {"type": "string", "description": "Playwright-syntax script; may run many actions in one call. The last expression is returned."},
        "tab_id": {"type": "string", "description": "Agent tab; optional when the session owns at most one tab."},
    }));
    repl_props.extend(object(direct_properties()));
    vec![
        tool(
            "agent_run",
            "Delegate a goal to the openside browser agent (it plans, browses, signs in through the user's vault, and asks the user on their phone when it needs a person). Starts a new session unless session_id is given. Returns session_id, message_id and after_seq at once; with wait=true, waits for the run (progress arrives as notifications/progress when the client sends a progress token) and returns the replies. Needs the steer scope, plus read to wait.",
            false,
            schema(
                json!({
                    "goal": {"type": "string", "description": "What the agent should do, self-contained."},
                    "session_id": {"type": "string", "description": "Continue in this session instead of a new one."},
                    "wait": {"type": "boolean", "description": "Wait for the run's end and return the replies."},
                    "timeout_secs": timeout,
                    "model": {"type": "string", "description": "Provider-qualified model for a new session."},
                    "mode": {"type": "string", "enum": ["read-only", "guard", "full"]},
                    "browser_space": {"type": "string", "enum": ["shared", "isolated"], "description": "shared (default): the user's managed browser profile with its logins. isolated: own cookies and storage."},
                }),
                &["goal"],
            ),
        ),
        tool(
            "agent_status",
            "A run's state without waiting: session status, Tasks (with their latest progress and results) and pending tickets.",
            true,
            schema(json!({"session_id": session_id}), &["session_id"]),
        ),
        tool(
            "agent_result",
            "Wait for a run started by agent_run and return the agent's replies, its Tasks and pending tickets. Status `waiting_human` means a person must answer a ticket in the openside app; `timeout` means call again.",
            true,
            schema(
                json!({
                    "session_id": session_id,
                    "after_seq": {"type": "integer", "minimum": 0, "description": "after_seq from agent_run."},
                    "message_id": {"type": "string", "description": "message_id from agent_run."},
                    "timeout_secs": timeout,
                }),
                &["session_id", "after_seq"],
            ),
        ),
        tool(
            "agent_steer",
            "Steer a running agent with new instructions, or one Task with task_id. Needs the steer scope.",
            false,
            schema(
                json!({
                    "session_id": session_id,
                    "text": {"type": "string"},
                    "task_id": {"type": "string", "description": "Task id from agent_status."},
                }),
                &["session_id", "text"],
            ),
        ),
        tool(
            "agent_cancel",
            "Cancel one Task with task_id, otherwise stop the session's whole run. Needs the steer scope.",
            false,
            schema(
                json!({"session_id": session_id, "task_id": {"type": "string"}}),
                &["session_id"],
            ),
        ),
        tool(
            "session_create",
            "Create a session, e.g. to drive its browser directly. Needs the steer scope.",
            false,
            schema(
                json!({
                    "title": {"type": "string"},
                    "model": {"type": "string", "description": "Provider-qualified model; default from settings."},
                    "mode": {"type": "string", "enum": ["read-only", "guard", "full"]},
                    "workspace": {"type": "string", "description": "Absolute workspace path."},
                    "incognito": {"type": "boolean", "description": "Private session whose browsing is never remembered."},
                    "browser_space": {"type": "string", "enum": ["shared", "isolated"], "description": "shared (default): the managed browser profile with its logins. isolated: own cookies and storage, wiped when the session is archived or deleted."},
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
            "Answer non-secret information tickets or deny confirmations (false or reject). Refuses confirmation approvals and choice answers: those require a person on a paired person device; use ticket_wait. Also refuses secret_input and login tickets. Needs the approve and read scopes.",
            false,
            schema(
                json!({
                    "ticket_id": {"type": "string"},
                    "value": {"description": "A non-secret info answer, or false/reject to deny a confirmation. Positive approvals and choice answers require a person; use ticket_wait."},
                }),
                &["ticket_id", "value"],
            ),
        ),
        tool(
            "ticket_wait",
            "Wait until a person answers a ticket in the openside web or phone UI; returns its status and resolution (status `pending` after the timeout).",
            true,
            schema(
                json!({"ticket_id": {"type": "string"}, "timeout_secs": timeout}),
                &["ticket_id"],
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
            "browser_repl",
            "Run a Playwright-syntax script (page, expect, console; page.evaluate where the daemon allows it) in the session's tab: batch many actions in one call. See docs/specs/browser-repl.md. Sensitive steps wait for the user's approval.",
            false,
            schema(Value::Object(repl_props), &["session_id", "code"]),
        ),
    ]
}

impl ServerHandler for Mcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("openside", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "The user's openside browser agent and browser. For a multi-step web task, \
                 delegate with agent_run (it uses the user's logins and asks the user on their \
                 phone when a person is needed); to drive the browser yourself, create a session \
                 and use the browser_* tools or browser_repl for a batch of steps. Each call is \
                 limited by the token's scopes.",
            )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let catalog = self.catalog().await.map_err(|error| {
            ErrorData::internal_error(
                self.redact(format!("could not read the daemon's tool catalog: {error}")),
                None,
            )
        })?;
        let mut tools = tools();
        tools.extend(
            catalog
                .tools
                .iter()
                .filter(|descriptor| !tools.iter().any(|tool| tool.name == descriptor.name))
                .map(direct_tool)
                .collect::<Vec<_>>(),
        );
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let progress = context.meta.get_progress_token().map(|token| Progress {
            peer: context.peer.clone(),
            token,
        });
        let arguments = request.arguments.map_or(Value::Null, Value::Object);
        let builtin = tools().iter().any(|tool| tool.name == request.name);
        let result = if builtin {
            self.dispatch(&request.name, arguments, progress)
                .await
                .map(|value| (value, Vec::new()))
        } else {
            self.direct(&request.name, arguments).await
        };
        match result {
            Ok((value, images)) => {
                let text = match value {
                    Value::String(text) => text,
                    other => other.to_string(),
                };
                let mut content = vec![ContentBlock::text(self.redact(text))];
                content.extend(images);
                Ok(CallToolResult::success(content).into())
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
