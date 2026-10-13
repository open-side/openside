use futures_util::{stream, StreamExt};
use rmcp::{
    model::{CallToolRequestParams, CallToolResult},
    service::{Peer, RunningService},
    RoleClient, ServiceExt,
};

use super::*;
use crate::{client::Client, mcp::Mcp};

#[derive(Default)]
struct Mock {
    requests: Vec<(String, String, Value)>,
    /// Events served by the SSE route, in order.
    events: Vec<Value>,
    /// Keep the SSE stream open after the last event.
    hang: bool,
    /// Answer every non-GET request with 403, like a read-only token.
    read_only: bool,
    /// What `GET /v1/sessions/s-1` reports; defaults to an idle session at sequence 7.
    status: Option<&'static str>,
    last_seq: Option<u64>,
    /// Pending tickets served by `GET /v1/tickets`.
    tickets: Vec<Value>,
}

type Shared = Arc<Mutex<Mock>>;

fn message_events(extra: Vec<Value>) -> Vec<Value> {
    let mut events = vec![
        event(
            8,
            "message.queued",
            json!({"message_id":"m-1","text":"hello","mode":"steer"}),
        ),
        event(9, "message.injected", json!({"message_id":"m-1"})),
        event(10, "turn.started", json!({})),
        event(
            11,
            "assistant.final",
            json!({"text":"hello from the daemon"}),
        ),
        event(
            12,
            "message.completed",
            json!({"message_id":"m-1","outcome":"completed"}),
        ),
        event(13, "session.settled", json!({})),
    ];
    events.extend(extra);
    events
}

fn session(status: &str) -> Value {
    json!({"id":"s-1","title":"t","model":"m","mode":"guard","workspace":null,
        "status":status,"last_seq":7,"queue":[],"pinned":false,"archived":false,"incognito":false})
}

async fn handle(State(shared): State<Shared>, request: Request<Body>) -> Response {
    assert_eq!(request.headers().get("x-openside-client").unwrap(), "mcp");
    let method = request.method().to_string();
    let target = request
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_default();
    let path = request.uri().path().to_owned();
    let bytes = to_bytes(request.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let mut mock = shared.lock();
    mock.requests.push((method.clone(), target, body.clone()));
    if mock.read_only && method != "GET" {
        return respond(
            403,
            json!({"code":"forbidden","message":"token lacks the required scope"}),
        );
    }
    match (method.as_str(), path.as_str()) {
        ("GET", "/v1/sessions") => respond(200, json!({"sessions":[session("idle")]})),
        ("POST", "/v1/sessions") => respond(201, session("idle")),
        ("GET", "/v1/sessions/s-1") => {
            let mut current = session(mock.status.unwrap_or("idle"));
            current["last_seq"] = json!(mock.last_seq.unwrap_or(7));
            respond(200, current)
        }
        ("GET", "/v1/sessions/running") => respond(200, session("running")),
        ("POST", "/v1/sessions/s-1/messages") => respond(
            202,
            json!({"id": body["message_id"].as_str().unwrap_or("m-1"),
                "text": body["text"], "status":"delivered"}),
        ),
        ("GET", "/v1/tickets") => respond(200, json!({"tickets": mock.tickets})),
        ("GET", "/v1/tickets/t-1") => respond(200, ticket()),
        ("GET", "/v1/tickets/t-login") => {
            let mut login = ticket();
            login["id"] = json!("t-login");
            login["deferral"] = json!("external");
            login["kind"] = json!("login");
            login["status"] = json!("resolved");
            login["resolution"] = json!({"signed_in": true});
            respond(200, login)
        }
        ("GET", "/v1/tickets/t-choice") => {
            let mut choice = ticket();
            choice["id"] = json!("t-choice");
            choice["kind"] = json!("choice");
            respond(200, choice)
        }
        ("GET", "/v1/tools") => respond(
            200,
            json!({"tools":[
                {"name":"browser_open_tab","description":"Open a tab.","scope":"browse","human":false,
                 "input_schema":{"type":"object","properties":{"url":{"type":"string"}},"required":["url"],"additionalProperties":false}},
                {"name":"browser_snapshot","description":"Snapshot.","scope":"browse","human":false,
                 "input_schema":{"type":"object","properties":{"tab_id":{"type":"string"},"scope":{"type":"string"}},"additionalProperties":false}},
                {"name":"request_login","description":"Sign in.","scope":"browse","human":true,
                 "input_schema":{"type":"object","properties":{"tab_id":{"type":"string"}},"additionalProperties":false}},
            ]}),
        ),
        ("POST", "/v1/sessions/s-1/tools/browser_open_tab") => respond(
            200,
            json!({"tool_call_id":"api-tool-1","status":"done","result":{"tab_id":"tab-1"}}),
        ),
        ("POST", "/v1/sessions/s-1/tools/browser_snapshot") => respond(
            200,
            json!({"tool_call_id":"api-tool-2","status":"done","result":"- heading \"Mock\" [e1]"}),
        ),
        ("POST", "/v1/sessions/s-1/tools/request_login") => respond(
            200,
            json!({"tool_call_id":"api-tool-3","status":"deferred","ticket_id":"t-login","deferral":"external"}),
        ),
        ("GET", "/v1/sessions/s-1/subagents") => respond(
            200,
            json!({"subagents":[{"id":"task-1","parent_tool_call_id":"c1","task":"compare prices",
                "model":"m","tools":[],"status":"running","started_at":"2026-01-01T00:00:00Z",
                "background":true}]}),
        ),
        ("POST", "/v1/sessions/s-1/subagents/task-1/steer") => respond(
            202,
            json!({"id":"task-1","parent_tool_call_id":"c1","task":"compare prices",
                "model":"m","tools":[],"status":"running","started_at":"2026-01-01T00:00:00Z",
                "background":true}),
        ),
        ("POST", "/v1/sessions/s-1/stop") => respond(200, session("idle")),
        ("GET", "/v1/tickets/t-secret") => {
            let mut secret = ticket();
            secret["id"] = json!("t-secret");
            secret["kind"] = json!("secret_input");
            respond(200, secret)
        }
        ("POST", "/v1/tickets/t-1/resolve") => {
            let mut resolved = ticket();
            resolved["status"] = json!("resolved");
            respond(200, resolved)
        }
        ("GET", "/v1/memory") => respond(200, json!({"mode":"lexical","hits":[]})),
        ("GET", "/v1/sessions/s-1/events") | ("GET", "/v1/sessions/running/events") => {
            let mut chunks: Vec<Result<Vec<u8>, std::io::Error>> = mock
                .events
                .iter()
                .filter(|event| event["session_id"] == "s-1")
                .map(|event| {
                    let seq = event["seq"].as_u64().unwrap();
                    Ok(format!("id: {seq}\ndata: {event}\n\n").into_bytes())
                })
                .collect();
            let hang = mock.hang;
            let stream = stream::iter(std::mem::take(&mut chunks));
            let body = if hang {
                Body::from_stream(stream.chain(stream::pending()))
            } else {
                Body::from_stream(stream)
            };
            Response::builder()
                .status(200)
                .header("content-type", "text/event-stream")
                .body(body)
                .unwrap()
        }
        ("POST", "/v1/sessions/s-1/repl") => {
            let code = body["code"].as_str().unwrap_or_default();
            if code.contains("approval-needed") && body["approval_ticket"].is_null() {
                return respond(
                    200,
                    json!({"tool_call_id":"c1","status":"deferred","ticket_id":"t-9"}),
                );
            }
            if code.contains("policy-denied") {
                return respond(
                    200,
                    json!({"tool_call_id":"c2","status":"denied",
                        "reason":"policy_denied: no","hint":"pick another site"}),
                );
            }
            if code.contains("throws") {
                return respond(
                    200,
                    json!({"tool_call_id":"c3","status":"done",
                        "result":{"return_value":null,"console":[],"error":"Error: boom","tab_id":"tab-1"}}),
                );
            }
            let value = if code.contains("page.snapshot") {
                json!("- heading \"Mock\" [e1]")
            } else {
                json!("http://127.0.0.1/mock")
            };
            respond(
                200,
                json!({"tool_call_id":"c4","status":"done",
                    "result":{"return_value":value,"console":[],"tab_id":"tab-1"}}),
            )
        }
        _ => respond(
            404,
            json!({"code":"not_found","message":format!("unknown route {path}")}),
        ),
    }
}

struct Rig {
    mock: Shared,
    peer: RunningService<RoleClient, ()>,
    _directory: TempDir,
    server: JoinHandle<()>,
}

impl Rig {
    async fn new(configure: impl FnOnce(&mut Mock)) -> Self {
        Self::with_token(None, configure).await
    }

    async fn with_token(token: Option<&str>, configure: impl FnOnce(&mut Mock)) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("mcp.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let mut mock = Mock::default();
        configure(&mut mock);
        let shared: Shared = Arc::new(Mutex::new(mock));
        let router = Router::new().fallback(handle).with_state(shared.clone());
        let daemon = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let client = Client::new(Some(&socket), None, None).unwrap();
        let (server_io, client_io) = tokio::io::duplex(1 << 20);
        let handler = Mcp::new(client, token.map(str::to_owned));
        let server = tokio::spawn(async move {
            let running = handler.serve(server_io).await.unwrap();
            let _ = running.waiting().await;
            daemon.abort();
        });
        let peer = ().serve(client_io).await.unwrap();
        Self {
            mock: shared,
            peer,
            _directory: directory,
            server,
        }
    }

    async fn call(&self, name: &str, arguments: Value) -> Result<CallToolResult, String> {
        let mut params = CallToolRequestParams::new(name.to_owned());
        params.arguments = arguments.as_object().cloned();
        Peer::call_tool(&self.peer, params)
            .await
            .map_err(|error| error.to_string())
    }

    fn requests(&self) -> Vec<(String, String, Value)> {
        self.mock.lock().requests.clone()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn text(result: &CallToolResult) -> String {
    serde_json::to_value(&result.content[0]).unwrap()["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn is_public_route(method: &str, target: &str) -> bool {
    let path = target.split('?').next().unwrap();
    openside_proto::ROUTES.iter().any(|route| {
        let template: Vec<_> = route.path.split('/').collect();
        let actual: Vec<_> = path.split('/').collect();
        route.method.as_str() == method
            && template.len() == actual.len()
            && template
                .iter()
                .zip(&actual)
                .all(|(t, a)| (t.starts_with('{') && !a.is_empty()) || t == a)
    })
}

#[tokio::test]
async fn tools_list_is_the_documented_set_plus_the_daemon_catalog() {
    let rig = Rig::new(|_| {}).await;
    let tools = rig.peer.list_all_tools().await.unwrap();
    let mut names: Vec<_> = tools.iter().map(|tool| tool.name.to_string()).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "agent_cancel",
            "agent_result",
            "agent_run",
            "agent_status",
            "agent_steer",
            "browser_open_tab",
            "browser_repl",
            "browser_snapshot",
            "memory_search",
            "request_login",
            "session_create",
            "session_list",
            "session_send",
            "session_status",
            "session_wait",
            "ticket_list",
            "ticket_resolve",
            "ticket_wait",
        ]
    );
    for tool in &tools {
        assert_eq!(tool.input_schema["type"], "object", "{}", tool.name);
        assert_eq!(
            tool.input_schema["additionalProperties"], false,
            "{}",
            tool.name
        );
        let read_only = tool.annotations.as_ref().and_then(|a| a.read_only_hint);
        let expected = matches!(
            tool.name.as_ref(),
            "session_list"
                | "session_status"
                | "session_wait"
                | "ticket_list"
                | "ticket_wait"
                | "memory_search"
                | "agent_status"
                | "agent_result"
                | "browser_snapshot"
        );
        assert_eq!(read_only, Some(expected), "{}", tool.name);
    }
    // Catalog tools keep the daemon's schema and gain the session and ticket arguments.
    let open = tools
        .iter()
        .find(|tool| tool.name == "browser_open_tab")
        .unwrap();
    assert_eq!(open.input_schema["required"], json!(["url", "session_id"]));
    assert!(open.input_schema["properties"]["approval_ticket"].is_object());
    assert!(
        !tools.iter().any(
            |tool| tool.input_schema["properties"]
                .as_object()
                .is_some_and(|properties| properties
                    .keys()
                    .any(|key| key.contains("password") || key.contains("secret")))
        ),
        "no tool may take a secret"
    );
}

#[tokio::test]
async fn session_send_with_wait_returns_the_reply_over_public_routes_only() {
    let rig = Rig::new(|mock| mock.events = message_events(vec![])).await;
    let created = rig
        .call("session_create", json!({"title":"t","mode":"guard"}))
        .await
        .unwrap();
    assert_ne!(created.is_error, Some(true));
    assert_eq!(
        serde_json::from_str::<Value>(&text(&created)).unwrap()["id"],
        "s-1"
    );
    let sent = rig
        .call(
            "session_send",
            json!({"session_id":"s-1","text":"hello","wait":true,"timeout_secs":20}),
        )
        .await
        .unwrap();
    assert_ne!(sent.is_error, Some(true), "{}", text(&sent));
    let sent: Value = serde_json::from_str(&text(&sent)).unwrap();
    assert_eq!(sent["status"], "settled");
    assert_eq!(sent["replies"], json!(["hello from the daemon"]));
    assert_eq!(sent["after_seq"], 7);
    assert_eq!(sent["last_seq"], 13);
    assert_eq!(sent["message"]["id"], "m-1");

    let requests = rig.requests();
    for (method, target, _) in &requests {
        assert!(is_public_route(method, target), "{method} {target}");
    }
    let message = requests
        .iter()
        .find(|(method, target, _)| method == "POST" && target == "/v1/sessions/s-1/messages")
        .unwrap();
    assert_eq!(message.2["text"], "hello");
    assert_eq!(message.2["mode"], "now");
    let events = requests
        .iter()
        .any(|(method, target, _)| method == "GET" && target == "/v1/sessions/s-1/events");
    assert!(events, "{requests:?}");
}

#[tokio::test]
async fn session_wait_reports_idle_immediately_and_resumes_after_a_timeout() {
    let rig = Rig::new(|mock| {
        mock.events = vec![event(8, "assistant.delta", json!({"text":"partial"}))];
        mock.hang = true;
    })
    .await;
    // An idle session with an empty queue never opens the event stream.
    let idle = rig
        .call("session_wait", json!({"session_id":"s-1"}))
        .await
        .unwrap();
    let idle: Value = serde_json::from_str(&text(&idle)).unwrap();
    assert_eq!(idle["status"], "settled");
    assert_eq!(idle["last_seq"], 7);
    assert!(!rig
        .requests()
        .iter()
        .any(|(_, target, _)| target.ends_with("/events")));

    // A running session whose stream never settles ends in `timeout`, with a cursor to continue
    // from.
    drop(rig);
    let rig = Rig::new(|mock| {
        mock.events = vec![event(8, "assistant.delta", json!({"text":"partial"}))];
        mock.hang = true;
        mock.status = Some("running");
    })
    .await;
    let timed_out = rig
        .call(
            "session_wait",
            json!({"session_id":"s-1","after_seq":7,"timeout_secs":1}),
        )
        .await
        .unwrap();
    assert_ne!(timed_out.is_error, Some(true));
    let timed_out: Value = serde_json::from_str(&text(&timed_out)).unwrap();
    assert_eq!(timed_out["status"], "timeout");
    assert_eq!(timed_out["last_seq"], 8);
}

#[tokio::test]
async fn waiting_on_a_human_ticket_is_reported_instead_of_hanging() {
    let mut ticket = ticket();
    ticket["id"] = json!("t-7");
    let rig = Rig::new(|mock| {
        mock.tickets = vec![ticket.clone()];
        mock.events = vec![
            event(8, "message.injected", json!({"message_id":"m-1"})),
            event(9, "ticket.opened", json!({"ticket": ticket})),
            event(
                10,
                "message.completed",
                json!({"message_id":"m-1","outcome":"deferred"}),
            ),
            event(11, "session.settled", json!({})),
        ];
    })
    .await;
    let sent = rig
        .call(
            "session_send",
            json!({"session_id":"s-1","text":"buy","message_id":"m-1","wait":true}),
        )
        .await
        .unwrap();
    let sent: Value = serde_json::from_str(&text(&sent)).unwrap();
    assert_eq!(sent["status"], "waiting_human");
    assert_eq!(sent["pending_tickets"][0]["id"], "t-7");
    assert_eq!(sent["pending_tickets"][0]["kind"], "confirm");
}

#[tokio::test]
async fn ticket_resolve_refuses_secret_input_and_never_calls_the_resolve_route() {
    let rig = Rig::new(|_| {}).await;
    let refused = rig
        .call(
            "ticket_resolve",
            json!({"ticket_id":"t-secret","value":"hunter2"}),
        )
        .await
        .unwrap();
    assert_eq!(refused.is_error, Some(true));
    let message = text(&refused);
    assert!(message.contains("secret_input"), "{message}");
    assert!(!message.contains("hunter2"));
    let requests = rig.requests();
    assert!(
        requests
            .iter()
            .all(|(_, target, _)| !target.ends_with("/resolve")),
        "{requests:?}"
    );
    assert!(
        requests
            .iter()
            .all(|(_, _, body)| !body.to_string().contains("hunter2")),
        "the secret must not even be forwarded"
    );

    let resolved = rig
        .call("ticket_resolve", json!({"ticket_id":"t-1","value":false}))
        .await
        .unwrap();
    assert_ne!(resolved.is_error, Some(true));
    let resolved: Value = serde_json::from_str(&text(&resolved)).unwrap();
    assert_eq!(resolved["status"], "resolved");
    let post = rig
        .requests()
        .into_iter()
        .find(|(method, target, _)| method == "POST" && target == "/v1/tickets/t-1/resolve")
        .unwrap();
    assert_eq!(post.2, json!({"value": false}));

    let listed = rig.call("ticket_list", json!({})).await.unwrap();
    assert_ne!(listed.is_error, Some(true));
    assert!(rig
        .requests()
        .iter()
        .any(|(_, target, _)| target == "/v1/tickets?status=pending"));
    rig.call("ticket_list", json!({"all":true})).await.unwrap();
    assert!(rig
        .requests()
        .iter()
        .any(|(_, target, _)| target == "/v1/tickets"));
}

#[tokio::test]
async fn person_decisions_are_refused_before_posting_and_tell_mcp_to_wait() {
    let rig = Rig::new(|_| {}).await;
    for (id, value) in [
        ("t-1", json!(true)),
        ("t-1", json!("approve")),
        ("t-choice", json!({"option":"o1"})),
    ] {
        let refused = rig
            .call("ticket_resolve", json!({"ticket_id":id,"value":value}))
            .await
            .unwrap();
        assert_eq!(refused.is_error, Some(true));
        let message = text(&refused);
        assert!(
            message.contains("person_approval_required") && message.contains("ticket_wait"),
            "{message}"
        );
    }
    assert!(rig
        .requests()
        .iter()
        .all(|(_, path, _)| !path.ends_with("/resolve")));
}

#[tokio::test]
async fn daemon_refusals_are_tool_errors_and_bad_arguments_are_protocol_errors() {
    let rig = Rig::new(|mock| mock.read_only = true).await;
    // Reads still work with a read-only token.
    let listed = rig.call("session_list", json!({})).await.unwrap();
    assert_ne!(listed.is_error, Some(true));
    // Writes surface the daemon's refusal as an `isError` result the model can read.
    let denied = rig
        .call("session_create", json!({"title":"x"}))
        .await
        .unwrap();
    assert_eq!(denied.is_error, Some(true));
    let message = text(&denied);
    assert!(
        message.contains("forbidden") && message.contains("403"),
        "{message}"
    );

    let missing = rig.call("session_send", json!({"session_id":"s-1"})).await;
    assert!(missing.unwrap_err().contains("text"));
    let unknown = rig.call("no_such_tool", json!({})).await;
    assert!(unknown.unwrap_err().contains("no_such_tool"));
    let extra = rig
        .call("session_list", json!({"surprise": 1}))
        .await
        .unwrap_err();
    assert!(extra.contains("surprise"), "{extra}");
    let relative = rig
        .call("session_create", json!({"workspace":"relative/dir"}))
        .await
        .unwrap_err();
    assert!(relative.contains("absolute"), "{relative}");
}

#[tokio::test]
async fn memory_search_encodes_its_query_and_tokens_are_redacted_from_errors() {
    let rig = Rig::with_token(Some("tok-secret-1"), |_| {}).await;
    let found = rig
        .call(
            "memory_search",
            json!({"query":"a b&c","kind":"note","limit":5}),
        )
        .await
        .unwrap();
    assert_ne!(found.is_error, Some(true));
    let target = rig
        .requests()
        .into_iter()
        .map(|(_, target, _)| target)
        .find(|target| target.starts_with("/v1/memory"))
        .unwrap();
    assert_eq!(target, "/v1/memory?q=a+b%26c&kind=note&limit=5");

    // The redaction applies to every message that leaves the server, including ones that
    // echo the token (here: through the unknown-ticket error path of the mock).
    let failed = rig
        .call("session_status", json!({"session_id":"tok-secret-1"}))
        .await
        .unwrap();
    assert_eq!(failed.is_error, Some(true));
    assert!(!text(&failed).contains("tok-secret-1"), "{}", text(&failed));
}

#[tokio::test]
async fn catalog_tools_call_the_tools_route_and_wait_for_the_person() {
    let rig = Rig::new(|_| {}).await;
    let open = rig
        .call(
            "browser_open_tab",
            json!({"session_id":"s-1","url":"http://127.0.0.1/"}),
        )
        .await
        .unwrap();
    assert_ne!(open.is_error, Some(true), "{}", text(&open));
    let open: Value = serde_json::from_str(&text(&open)).unwrap();
    assert_eq!(open["result"]["tab_id"], "tab-1");
    let post = rig
        .requests()
        .into_iter()
        .find(|(_, target, _)| target == "/v1/sessions/s-1/tools/browser_open_tab")
        .unwrap();
    // The MCP-only arguments never reach the daemon tool.
    assert_eq!(post.2, json!({"args":{"url":"http://127.0.0.1/"}}));

    // A login ticket's resolution, given in the web or phone UI, is the call's result.
    let login = rig
        .call(
            "request_login",
            json!({"session_id":"s-1","tab_id":"tab-1"}),
        )
        .await
        .unwrap();
    assert_ne!(login.is_error, Some(true), "{}", text(&login));
    let login: Value = serde_json::from_str(&text(&login)).unwrap();
    assert_eq!(login["status"], "resolved");
    assert_eq!(login["result"], json!({"signed_in": true}));

    // Without waiting, the pending ticket comes back at once.
    let pending = rig
        .call(
            "request_login",
            json!({"session_id":"s-1","tab_id":"tab-1","wait":false}),
        )
        .await
        .unwrap();
    let pending: Value = serde_json::from_str(&text(&pending)).unwrap();
    assert_eq!(pending["status"], "deferred");
    assert_eq!(pending["deferral"], "external");
    assert_eq!(pending["ticket_id"], "t-login");

    let missing = rig
        .call("browser_snapshot", json!({"tab_id":"tab-1"}))
        .await
        .unwrap_err();
    assert!(missing.contains("session_id"), "{missing}");

    let waited = rig
        .call("ticket_wait", json!({"ticket_id":"t-login"}))
        .await
        .unwrap();
    let waited: Value = serde_json::from_str(&text(&waited)).unwrap();
    assert_eq!(waited["status"], "resolved");
    assert_eq!(waited["resolution"], json!({"signed_in": true}));
    let still = rig
        .call("ticket_wait", json!({"ticket_id":"t-1","timeout_secs":1}))
        .await
        .unwrap();
    let still: Value = serde_json::from_str(&text(&still)).unwrap();
    assert_eq!(still["status"], "pending");
    for (method, target, _) in rig.requests() {
        assert!(is_public_route(&method, &target), "{method} {target}");
    }
}

#[tokio::test]
async fn browser_repl_maps_every_status() {
    let rig = Rig::new(|_| {}).await;
    let deferred = rig
        .call(
            "browser_repl",
            json!({"session_id":"s-1","code":"approval-needed","wait":false}),
        )
        .await
        .unwrap();
    assert_ne!(deferred.is_error, Some(true));
    let deferred: Value = serde_json::from_str(&text(&deferred)).unwrap();
    assert_eq!(deferred["status"], "deferred");
    assert_eq!(deferred["ticket_id"], "t-9");
    let approved = rig
        .call(
            "browser_repl",
            json!({"session_id":"s-1","code":"approval-needed","approval_ticket":"t-9"}),
        )
        .await
        .unwrap();
    assert_ne!(approved.is_error, Some(true));
    let retried = rig
        .requests()
        .into_iter()
        .rfind(|(_, target, _)| target == "/v1/sessions/s-1/repl")
        .unwrap();
    assert_eq!(retried.2["approval_ticket"], "t-9");

    let snapshot = rig
        .call(
            "browser_repl",
            json!({"session_id":"s-1","code":"await page.snapshot()","tab_id":"tab-1"}),
        )
        .await
        .unwrap();
    let snapshot: Value = serde_json::from_str(&text(&snapshot)).unwrap();
    assert_eq!(
        snapshot["result"]["return_value"],
        "- heading \"Mock\" [e1]"
    );

    let denied = rig
        .call(
            "browser_repl",
            json!({"session_id":"s-1","code":"policy-denied"}),
        )
        .await
        .unwrap();
    assert_eq!(denied.is_error, Some(true));
    assert!(text(&denied).contains("policy_denied"));
    let thrown = rig
        .call("browser_repl", json!({"session_id":"s-1","code":"throws"}))
        .await
        .unwrap();
    assert_eq!(thrown.is_error, Some(true));
    assert!(text(&thrown).contains("boom"));
    for (method, target, _) in rig.requests() {
        assert!(is_public_route(&method, &target), "{method} {target}");
    }
}

#[tokio::test]
async fn agent_run_delegates_the_goal_and_waits_for_the_replies() {
    let rig = Rig::new(|mock| mock.events = message_events(vec![])).await;
    let started = rig
        .call("agent_run", json!({"goal":"find the cheapest flight"}))
        .await
        .unwrap();
    assert_ne!(started.is_error, Some(true), "{}", text(&started));
    let started: Value = serde_json::from_str(&text(&started)).unwrap();
    assert_eq!(started["session_id"], "s-1");
    assert_eq!(started["message_id"], "m-1");
    assert_eq!(started["after_seq"], 7);
    let message = rig
        .requests()
        .into_iter()
        .find(|(method, target, _)| method == "POST" && target == "/v1/sessions/s-1/messages")
        .unwrap();
    assert_eq!(message.2["text"], "find the cheapest flight");
    assert_eq!(message.2["mode"], "now");

    let result = rig
        .call(
            "agent_result",
            json!({"session_id":"s-1","after_seq":7,"message_id":"m-1","timeout_secs":20}),
        )
        .await
        .unwrap();
    let result: Value = serde_json::from_str(&text(&result)).unwrap();
    assert_eq!(result["status"], "settled");
    assert_eq!(result["replies"], json!(["hello from the daemon"]));
    assert_eq!(result["tasks"][0]["task_id"], "task-1");

    let waited = rig
        .call(
            "agent_run",
            json!({"goal":"again","session_id":"s-1","wait":true,"timeout_secs":20}),
        )
        .await
        .unwrap();
    let waited: Value = serde_json::from_str(&text(&waited)).unwrap();
    assert_eq!(waited["status"], "settled");
    assert_eq!(waited["replies"], json!(["hello from the daemon"]));

    let status = rig
        .call("agent_status", json!({"session_id":"s-1"}))
        .await
        .unwrap();
    let status: Value = serde_json::from_str(&text(&status)).unwrap();
    assert_eq!(status["status"], "idle");
    assert_eq!(status["tasks"][0]["status"], "running");

    let steered = rig
        .call(
            "agent_steer",
            json!({"session_id":"s-1","text":"only nonstop","task_id":"task-1"}),
        )
        .await
        .unwrap();
    assert_ne!(steered.is_error, Some(true), "{}", text(&steered));
    rig.call("agent_steer", json!({"session_id":"s-1","text":"hurry"}))
        .await
        .unwrap();
    let cancelled = rig
        .call("agent_cancel", json!({"session_id":"s-1"}))
        .await
        .unwrap();
    assert_ne!(cancelled.is_error, Some(true), "{}", text(&cancelled));

    let requests = rig.requests();
    assert!(requests.iter().any(|(_, target, body)| target
        == "/v1/sessions/s-1/subagents/task-1/steer"
        && body["text"] == "only nonstop"));
    assert!(requests
        .iter()
        .any(|(_, target, body)| target == "/v1/sessions/s-1/messages" && body["mode"] == "steer"));
    for (method, target, _) in &requests {
        assert!(is_public_route(method, target), "{method} {target}");
    }
}

#[tokio::test]
async fn session_wait_with_after_seq_on_a_settled_session_returns_without_streaming() {
    let rig = Rig::new(|mock| mock.hang = true).await;
    let done = rig
        .call("session_wait", json!({"session_id":"s-1","after_seq":7}))
        .await
        .unwrap();
    let done: Value = serde_json::from_str(&text(&done)).unwrap();
    assert_eq!(done["status"], "settled");
    assert_eq!(done["last_seq"], 7);
    assert!(!rig
        .requests()
        .iter()
        .any(|(_, target, _)| target.ends_with("/events")));

    let ahead = rig
        .call("session_wait", json!({"session_id":"s-1","after_seq":99}))
        .await
        .unwrap();
    assert_eq!(ahead.is_error, Some(true));
    assert!(text(&ahead).contains("ahead"), "{}", text(&ahead));
}

#[tokio::test]
async fn session_wait_ignores_settlements_that_were_already_history() {
    // Running session at sequence 12: the settlement at 12 ended an earlier run.
    let rig = Rig::new(|mock| {
        mock.status = Some("running");
        mock.last_seq = Some(12);
        mock.events = vec![
            event(8, "assistant.final", json!({"text":"old"})),
            event(12, "session.settled", json!({})),
            event(13, "assistant.final", json!({"text":"new"})),
            event(14, "session.settled", json!({})),
        ];
    })
    .await;
    let waited = rig
        .call("session_wait", json!({"session_id":"s-1","after_seq":7}))
        .await
        .unwrap();
    let waited: Value = serde_json::from_str(&text(&waited)).unwrap();
    assert_eq!(waited["status"], "settled");
    assert_eq!(waited["last_seq"], 14);
    assert_eq!(waited["replies"], json!(["old", "new"]));
}

#[tokio::test]
async fn session_wait_on_an_idle_session_replays_up_to_its_last_settlement() {
    let rig = Rig::new(|mock| {
        mock.last_seq = Some(12);
        mock.events = vec![
            event(8, "assistant.final", json!({"text":"answer"})),
            event(12, "session.settled", json!({})),
        ];
        mock.hang = true;
    })
    .await;
    let waited = rig
        .call(
            "session_wait",
            json!({"session_id":"s-1","after_seq":7,"timeout_secs":20}),
        )
        .await
        .unwrap();
    let waited: Value = serde_json::from_str(&text(&waited)).unwrap();
    assert_eq!(waited["status"], "settled");
    assert_eq!(waited["last_seq"], 12);
    assert_eq!(waited["replies"], json!(["answer"]));
}

#[tokio::test]
async fn replies_and_settlements_of_earlier_messages_are_not_attributed_to_ours() {
    let rig = Rig::new(|mock| {
        mock.events = vec![
            event(8, "message.injected", json!({"message_id":"m-0"})),
            event(9, "assistant.final", json!({"text":"old reply"})),
            event(
                10,
                "message.completed",
                json!({"message_id":"m-0","outcome":"completed"}),
            ),
            event(11, "session.settled", json!({})),
            event(12, "message.injected", json!({"message_id":"m-2"})),
            event(13, "assistant.final", json!({"text":"new reply"})),
            event(
                14,
                "message.completed",
                json!({"message_id":"m-2","outcome":"completed"}),
            ),
            event(15, "session.settled", json!({})),
        ];
    })
    .await;
    let sent = rig
        .call(
            "session_send",
            json!({"session_id":"s-1","text":"hi","message_id":"m-2","wait":true}),
        )
        .await
        .unwrap();
    let sent: Value = serde_json::from_str(&text(&sent)).unwrap();
    assert_eq!(sent["status"], "settled");
    assert_eq!(sent["replies"], json!(["new reply"]));
    assert_eq!(sent["last_seq"], 15);
}

#[tokio::test]
async fn pending_tickets_are_read_when_the_session_settles_not_before_the_replay() {
    // The ticket was opened and resolved inside the replayed window: nothing is pending now.
    let mut old = ticket();
    old["id"] = json!("t-old");
    let rig = Rig::new(|mock| {
        mock.events = vec![
            event(8, "message.injected", json!({"message_id":"m-1"})),
            event(9, "ticket.opened", json!({"ticket": old})),
            event(
                10,
                "ticket.resolved",
                json!({"ticket_id":"t-old","resolution":{"value":true}}),
            ),
            event(
                11,
                "message.completed",
                json!({"message_id":"m-1","outcome":"completed"}),
            ),
            event(12, "session.settled", json!({})),
        ];
    })
    .await;
    let sent = rig
        .call(
            "session_send",
            json!({"session_id":"s-1","text":"hi","message_id":"m-1","wait":true}),
        )
        .await
        .unwrap();
    let sent: Value = serde_json::from_str(&text(&sent)).unwrap();
    assert_eq!(sent["status"], "settled");
    assert_eq!(sent["pending_tickets"], json!([]));
}

#[tokio::test]
async fn the_token_is_scrubbed_from_argument_errors_and_stderr_text() {
    let rig = Rig::with_token(Some("tok-secret-1"), |_| {}).await;
    let rejected = rig
        .call("session_list", json!({"tok-secret-1": 1}))
        .await
        .unwrap_err();
    assert!(!rejected.contains("tok-secret-1"), "{rejected}");
    assert!(rejected.contains("[REDACTED]"), "{rejected}");
    assert_eq!(
        crate::mcp::redact("bad tok-secret-1 here", Some("tok-secret-1")),
        "bad [REDACTED] here"
    );
    assert_eq!(crate::mcp::redact("same", None), "same");
}
