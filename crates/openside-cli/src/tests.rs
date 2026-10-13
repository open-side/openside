use std::{ffi::OsString, os::unix::fs::PermissionsExt, path::PathBuf, sync::Arc};

use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{Request, StatusCode},
    response::Response,
    Router,
};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::{
    net::{TcpListener, UnixListener},
    sync::{Mutex as AsyncMutex, MutexGuard},
    task::JoinHandle,
};

mod agent;
mod cards;
mod channels;
mod connections;
mod import;
mod mcp;
mod memory;
mod models;
mod policy;
mod reference;
mod repl;
mod routines;
mod skills;
mod sync;
mod vault;
use super::{cursor::Cursor, run, run_with_input, sse::Decoder};

static ENV_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());

struct Environment {
    _guard: MutexGuard<'static, ()>,
    directory: TempDir,
    saved: Vec<(OsString, Option<OsString>)>,
}

impl Environment {
    async fn new() -> Self {
        let guard = ENV_LOCK.lock().await;
        let directory = tempfile::tempdir().unwrap();
        let mut keys: Vec<OsString> = [
            "HOME",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "XDG_RUNTIME_DIR",
            "CODEX_HOME",
            "CLAUDE_CONFIG_DIR",
            "OMP_CODING_AGENT_DIR",
            "PI_CODING_AGENT_DIR",
            "PATH",
            "OPENSIDE_TOKEN",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        keys.extend(
            std::env::vars_os()
                .map(|(key, _)| key)
                .filter(|key| key.to_string_lossy().starts_with("OPENSIDE_")),
        );
        keys.sort();
        keys.dedup();
        let saved = keys
            .iter()
            .map(|key| (key.clone(), std::env::var_os(key)))
            .collect();
        for key in keys {
            if key.to_string_lossy().starts_with("OPENSIDE_") {
                std::env::remove_var(key);
            } else if key != "PATH" {
                std::env::set_var(key, directory.path());
            }
        }
        Self {
            _guard: guard,
            directory,
            saved,
        }
    }
}

impl Drop for Environment {
    fn drop(&mut self) {
        for (key, value) in &self.saved {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

struct RecordedRequest {
    method: String,
    path: String,
    body: Value,
    query: Option<String>,
    last_id: Option<String>,
    authorization: Option<String>,
}

#[derive(Default)]
struct MockState {
    session: Option<Value>,
    events: Vec<Value>,
    requests: Vec<RecordedRequest>,
    error: Option<u16>,
    tickets: bool,
    ticket: Option<Value>,
    ticket_terminal: Option<&'static str>,
    secret_error: bool,
    self_approval: bool,
    person_approval: bool,
    eof: bool,
    malformed: bool,
    duplicate: bool,
    runtime_error: bool,
    runtime_warning: bool,
    settle_on_events: bool,
}

struct Fixture {
    environment: Environment,
    socket: PathBuf,
    state: Arc<Mutex<MockState>>,
    server: JoinHandle<()>,
}

impl Fixture {
    async fn new() -> Self {
        let environment = Environment::new().await;
        std::fs::create_dir(environment.directory.path().join("openside")).unwrap();
        let socket = environment.directory.path().join("openside/opensided.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let state = Arc::new(Mutex::new(MockState::default()));
        let router = Router::new().fallback(mock).with_state(state.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            environment,
            socket,
            state,
            server,
        }
    }

    async fn run(&self, args: &[&str]) -> (u8, String, String) {
        self.run_with_input(args, &mut std::io::empty()).await
    }

    async fn run_with_input(
        &self,
        args: &[&str],
        stdin: &mut impl std::io::Read,
    ) -> (u8, String, String) {
        let mut argv = vec![
            OsString::from("openside"),
            OsString::from("--socket"),
            self.socket.clone().into_os_string(),
        ];
        argv.extend(args.iter().map(OsString::from));
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let code = run_with_input(argv, stdin, &mut stdout, &mut stderr).await;
        (
            code,
            String::from_utf8(stdout).unwrap(),
            String::from_utf8(stderr).unwrap(),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn event(seq: u64, kind: &str, payload: Value) -> Value {
    let mut event =
        json!({"session_id":"s-1","seq":seq,"ts":"2026-01-01T00:00:00Z","run_id":null,"kind":kind});
    event
        .as_object_mut()
        .unwrap()
        .extend(payload.as_object().unwrap().clone());
    event
}

fn ticket() -> Value {
    json!({"id":"t-1","session_id":"s-1","tool_call_id":"call-1",
        "deferral":"approval","kind":"confirm","summary":"Run tool?","instructions":null,
        "options":[{"value":"approve","label":"Approve"},{"value":"reject","label":"Reject"}],
        "origin":{"subagent_id":null},"tool":{"name":"test","args":{}},
        "target":null,"resume_when":null,"created_at":"2026-01-01T00:00:00Z",
        "expires_at":"2099-01-01T00:00:00Z","status":"pending","resolution":null})
}

fn respond(status: u16, body: Value) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

async fn mock(State(state): State<Arc<Mutex<MockState>>>, request: Request<Body>) -> Response {
    let method = request.method().to_string();
    let path = request.uri().path().to_owned();
    let query = request.uri().query().map(str::to_owned);
    let last_id = request
        .headers()
        .get("last-event-id")
        .map(|value| value.to_str().unwrap().to_owned());
    let authorization = request
        .headers()
        .get("authorization")
        .map(|value| value.to_str().unwrap().to_owned());
    let bytes = to_bytes(request.into_body(), 1024 * 1024).await.unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    let mut state = state.lock();
    state.requests.push(RecordedRequest {
        method: method.clone(),
        path: path.clone(),
        query: query.clone(),
        body: body.clone(),
        last_id: last_id.clone(),
        authorization,
    });
    if let Some(status) = state.error {
        return respond(
            status,
            json!({"code":"mock_error","message":"mock failure"}),
        );
    }
    match (method.as_str(), path.as_str()) {
        ("POST", "/v1/pairing") => respond(
            201,
            json!({
                "code":"pairing-code","path":"/pair#pairing-code","url":null,
                "scopes":body["scopes"],"kind":body["kind"],"transport":null,
                "expires_at":"2026-10-13T12:00:00Z"
            }),
        ),
        ("POST", "/v1/sessions") => {
            state.events = vec![event(
                1,
                "session.created",
                json!({"title":"test","model":body["model"].as_str().unwrap_or("fake"),
                    "mode":body["mode"].as_str().unwrap_or("guard"),"workspace":body["workspace"]}),
            )];
            let session = json!({"id":"s-1","title":"test",
                "model":body["model"].as_str().unwrap_or("fake"),
                "mode":body["mode"].as_str().unwrap_or("guard"),"workspace":body["workspace"],
                "status":"idle","last_seq":1,"queue":[]});
            state.session = Some(session.clone());
            respond(201, session)
        }
        ("GET", "/v1/sessions") => respond(
            200,
            json!({"sessions":state.session.iter().collect::<Vec<_>>()}),
        ),
        ("GET", "/v1/sessions/s-1") => match &state.session {
            Some(session) => respond(200, session.clone()),
            None => respond(404, json!({"code":"not_found","message":"unknown session"})),
        },
        ("POST", "/v1/sessions/s-1/messages") => {
            assert_eq!(body["mode"], "now");
            let seq = state.events.len() as u64 + 1;
            if state.tickets {
                let ticket = state.ticket.clone().unwrap_or_else(ticket);
                state
                    .events
                    .push(event(seq, "ticket.opened", json!({"ticket":ticket})));
                let seq = state.events.len() as u64 + 1;
                state.events.push(event(
                    seq,
                    "message.completed",
                    json!({"message_id":"m-1","outcome":"deferred"}),
                ));
                if let Some(kind) = state.ticket_terminal {
                    let seq = state.events.len() as u64 + 1;
                    let payload = match kind {
                        "ticket.resolved" => {
                            json!({"ticket_id":"t-1","resolution":{"approved":true}})
                        }
                        "ticket.rejected" => json!({"ticket_id":"t-1","reason":"no"}),
                        _ => json!({"ticket_id":"t-1"}),
                    };
                    state.events.push(event(seq, kind, payload));
                }
            }
            if state.runtime_warning {
                let seq = state.events.len() as u64 + 1;
                state.events.push(event(
                    seq,
                    "runtime.error",
                    json!({"code":"sandbox_unavailable","message":"policy-only isolation"}),
                ));
            }
            let seq = state.events.len() as u64 + 1;
            state
                .events
                .push(event(seq, "assistant.delta", json!({"text":"hello"})));
            state
                .events
                .push(event(seq + 1, "assistant.final", json!({"text":"hello"})));
            if state.runtime_error {
                state.events.push(event(
                    seq + 2,
                    "runtime.error",
                    json!({"code":"failed","message":"fake failed"}),
                ));
            } else if !state.eof {
                state
                    .events
                    .push(event(seq + 2, "session.settled", json!({})));
            }
            let last_seq = state.events.len() as u64;
            let eof = state.eof;
            let waiting = state.tickets && state.ticket_terminal.is_none();
            if let Some(session) = &mut state.session {
                session["last_seq"] = json!(last_seq);
                session["status"] = json!(if eof {
                    "running"
                } else if waiting {
                    "waiting_human"
                } else {
                    "idle"
                });
            }
            respond(
                202,
                json!({"id":"m-1","text":body["text"],"status":"delivered"}),
            )
        }
        ("POST", "/v1/sessions/s-1/stop") => {
            let seq = state.events.len() as u64 + 1;
            state.events.push(event(seq, "session.stopped", json!({})));
            let session = state.session.as_mut().unwrap();
            session["status"] = json!("stopped");
            session["last_seq"] = json!(seq);
            respond(200, session.clone())
        }
        ("GET", "/v1/sessions/s-1/events") => {
            if state.malformed {
                return Response::builder()
                    .header("content-type", "text/event-stream")
                    .body(Body::from("id: 1\ndata: not json\n\n"))
                    .unwrap();
            }
            if state.settle_on_events {
                let seq = state.events.len() as u64 + 1;
                state.events.push(event(seq, "session.settled", json!({})));
                let session = state.session.as_mut().unwrap();
                session["status"] = json!("idle");
                session["last_seq"] = json!(seq);
                state.settle_on_events = false;
            }
            let last_seq: u64 = last_id.unwrap().parse().unwrap();
            let mut sse = String::from(": keepalive\r\n\r\n");
            for event in &state.events {
                let seq = event["seq"].as_u64().unwrap();
                if seq <= last_seq && !(state.duplicate && seq == last_seq) {
                    continue;
                }
                let data = serde_json::to_string(event).unwrap();
                let split = data.find(',').unwrap() + 1;
                sse.push_str(&format!(
                    "id: {seq}\r\ndata: {}\r\ndata: {}\r\n\r\n",
                    &data[..split],
                    &data[split..]
                ));
            }
            Response::builder()
                .header("content-type", "text/event-stream; charset=utf-8")
                .body(Body::from(sse))
                .unwrap()
        }
        ("GET", "/v1/tickets") => {
            let tickets = state
                .ticket
                .clone()
                .into_iter()
                .filter(|ticket| {
                    query.as_deref() != Some("status=pending") || ticket["status"] == "pending"
                })
                .collect::<Vec<_>>();
            respond(200, json!({"tickets":tickets}))
        }
        ("GET", "/v1/tickets/t-1") => respond(200, state.ticket.clone().unwrap_or_else(ticket)),
        ("POST", "/v1/tickets/t-1/resolve") => {
            if state.self_approval {
                return respond(
                    403,
                    json!({"code":"self_approval_forbidden","message":"self approval forbidden"}),
                );
            }
            if state.person_approval {
                return respond(
                    403,
                    json!({"code":"person_approval_required","message":"person approval required"}),
                );
            }
            let mut ticket = state.ticket.clone().unwrap_or_else(ticket);
            if state.secret_error {
                return respond(400, json!({"code":"bad_secret","message":body["value"]}));
            }
            ticket["status"] = json!("resolved");
            ticket["resolution"] = match ticket["kind"].as_str() {
                _ if ticket["form"].is_object() => json!({"status":"filled","next":"review"}),
                Some("secret_input") => json!({"status":"filled"}),
                // Like the daemon: a login or choice resolution never carries the answer.
                Some("login") => json!({"status":"submitted","next":"done"}),
                Some("choice") => {
                    json!({"status":"chosen","option":body["value"]["option"],"next":"otp"})
                }
                _ => json!({"answer":body["value"]}),
            };
            if let Some(card) = ticket["card"].as_object_mut() {
                card.insert("status".into(), json!("done"));
                card.insert("actions".into(), json!([]));
                card.insert(
                    "blocks".into(),
                    json!([{
                        "id":"receipt", "type":"notice", "tone":"neutral", "placement":"inline",
                        "text":"Fields filled; review the page before submitting."
                    }]),
                );
            }
            respond(200, ticket)
        }
        ("POST", "/v1/tickets/t-1/done") => {
            let mut ticket = state.ticket.clone().unwrap_or_else(ticket);
            ticket["status"] = json!("resolved");
            ticket["resolution"] = json!({"status":"done"});
            respond(200, ticket)
        }
        ("POST", "/v1/tickets/t-1/reject") => {
            let mut ticket = state.ticket.clone().unwrap_or_else(ticket);
            ticket["status"] = json!("rejected");
            ticket["resolution"] = json!({"status":"rejected","reason":body["reason"]});
            respond(200, ticket)
        }
        _ => respond(404, json!({"code":"not_found","message":"unknown route"})),
    }
}

fn json_lines(output: &str) -> Vec<Value> {
    output
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[tokio::test]
async fn task_creates_session_streams_text_and_does_not_repeat_final() {
    let fixture = Fixture::new().await;
    let (code, output, error) = fixture.run(&["hello task"]).await;
    assert_eq!((code, output.as_str(), error.as_str()), (0, "hello\n", ""));
    let state = fixture.state.lock();
    assert_eq!(state.requests[0].method, "POST");
    assert_eq!(state.requests[0].path, "/v1/sessions");
    assert_eq!(state.requests[1].body["text"], "hello task");
    assert_eq!(state.requests[2].last_id.as_deref(), Some("0"));
}

#[tokio::test]
async fn json_mode_emits_raw_events_and_final_summary() {
    let fixture = Fixture::new().await;
    let (code, output, error) = fixture.run(&["--json", "hello"]).await;
    assert_eq!(code, 0, "{error}");
    let lines = json_lines(&output);
    assert_eq!(lines.len(), 5);
    for (index, line) in lines[..4].iter().enumerate() {
        let event: openside_proto::Event = serde_json::from_value(line.clone()).unwrap();
        assert_eq!(event.seq, index as u64 + 1);
        assert!(line.get("type").is_none());
    }
    assert_eq!(
        lines[4],
        json!({"type":"summary","status":"settled","exit_code":0,"session_id":"s-1","last_seq":4})
    );
}

#[tokio::test]
async fn session_list_and_stop_use_public_api() {
    let fixture = Fixture::new().await;
    assert_eq!(fixture.run(&["task"]).await.0, 0);
    let (code, output, error) = fixture.run(&["session", "list"]).await;
    assert_eq!(code, 0, "{error}");
    assert_eq!(output, "s-1\tidle\ttest\n");
    let (code, output, error) = fixture.run(&["--json", "session", "stop", "s-1"]).await;
    assert_eq!(code, 0, "{error}");
    assert_eq!(json_lines(&output)[0]["status"], "stopped");
    assert_eq!(
        fixture.state.lock().session.as_ref().unwrap()["status"],
        "stopped"
    );
}

#[tokio::test]
async fn resume_uses_persisted_cursor_and_can_submit_a_new_message() {
    let fixture = Fixture::new().await;
    assert_eq!(fixture.run(&["hello"]).await.0, 0);
    let (code, output, error) = fixture
        .run(&["--json", "session", "resume", "s-1", "again"])
        .await;
    assert_eq!(code, 0, "{error}");
    let lines = json_lines(&output);
    assert_eq!(lines[0]["seq"], 5);
    assert_eq!(lines.last().unwrap()["last_seq"], 7);
    let state = fixture.state.lock();
    assert_eq!(state.requests.last().unwrap().last_id.as_deref(), Some("4"));
    assert_eq!(
        state
            .requests
            .iter()
            .filter(|request| request.path.ends_with("messages"))
            .count(),
        2
    );
}

#[tokio::test]
async fn resume_override_replays_and_ignores_duplicate_sequences() {
    let fixture = Fixture::new().await;
    assert_eq!(fixture.run(&["hello"]).await.0, 0);
    fixture.state.lock().duplicate = true;
    let (code, output, error) = fixture
        .run(&["--json", "session", "resume", "s-1", "--last-event-id", "1"])
        .await;
    assert_eq!(code, 0, "{error}");
    let lines = json_lines(&output);
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0]["seq"], 2);
    assert_eq!(
        fixture
            .state
            .lock()
            .requests
            .last()
            .unwrap()
            .last_id
            .as_deref(),
        Some("1")
    );
}

#[tokio::test]
async fn resume_settled_session_with_current_cursor_returns_without_waiting() {
    let fixture = Fixture::new().await;
    assert_eq!(fixture.run(&["hello"]).await.0, 0);
    let (code, output, error) = fixture.run(&["--json", "session", "resume", "s-1"]).await;
    assert_eq!(code, 0, "{error}");
    assert_eq!(json_lines(&output).len(), 1);
    assert_eq!(json_lines(&output)[0]["last_seq"], 4);
}

#[tokio::test]
async fn resume_replays_post_terminal_metadata_without_waiting_for_another_settlement() {
    let fixture = Fixture::new().await;
    assert_eq!(fixture.run(&["hello"]).await.0, 0);
    {
        let mut state = fixture.state.lock();
        state.events.push(event(
            5,
            "usage",
            json!({"model":"script","input_tokens":3,"output_tokens":3}),
        ));
        state.events.push(event(
            6,
            "session.title_proposed",
            json!({"title":"Plan the work"}),
        ));
        let session = state.session.as_mut().unwrap();
        session["last_seq"] = json!(6);
        assert_eq!(session["status"], "idle");
    }
    let (code, output, error) = fixture.run(&["--json", "session", "resume", "s-1"]).await;
    assert_eq!(code, 0, "{error}");
    let records = json_lines(&output);
    assert_eq!(records.len(), 3);
    assert_eq!(records[0]["seq"], 5);
    assert_eq!(records[1]["seq"], 6);
    assert_eq!(records.last().unwrap()["status"], "settled");
    assert_eq!(records.last().unwrap()["last_seq"], 6);
    let (code, output, error) = fixture.run(&["--json", "session", "resume", "s-1"]).await;
    assert_eq!(code, 0, "{error}");
    assert_eq!(
        json_lines(&output).len(),
        1,
        "the emitted metadata advances the durable cursor"
    );
}

#[tokio::test]
async fn ticket_prints_instructions_and_settlement_waits_for_human() {
    let fixture = Fixture::new().await;
    fixture.state.lock().tickets = true;
    let (code, output, error) = fixture.run(&["--json", "hello"]).await;
    assert_eq!(code, 0, "{error}");
    assert!(error.contains("Ticket t-1 (confirm): Run tool?"));
    assert!(error.contains("another paired web or phone device"));
    assert!(!error.contains("ticket resolve t-1 true"));
    assert_eq!(
        json_lines(&output).last().unwrap()["status"],
        "waiting_human"
    );
    assert_eq!(json_lines(&output).last().unwrap()["exit_code"], 0);
    let (code, output, error) = fixture.run(&["session", "resume", "s-1"]).await;
    assert_eq!(code, 0, "{error}");
    assert_eq!(output, "Session s-1 is waiting on tickets.\n");
}

#[tokio::test]
async fn ticket_list_defaults_to_pending_and_all_omits_the_filter() {
    let fixture = Fixture::new().await;
    fixture.state.lock().ticket = Some(ticket());
    let (code, output, error) = fixture.run(&["--json", "ticket", "list"]).await;
    assert_eq!(code, 0, "{error}");
    assert_eq!(json_lines(&output)[0]["tickets"][0]["id"], "t-1");
    assert_eq!(
        fixture
            .state
            .lock()
            .requests
            .last()
            .unwrap()
            .query
            .as_deref(),
        Some("status=pending")
    );
    fixture.state.lock().ticket.as_mut().unwrap()["status"] = json!("resolved");
    let (_, output, _) = fixture.run(&["--json", "ticket", "list"]).await;
    assert!(json_lines(&output)[0]["tickets"]
        .as_array()
        .unwrap()
        .is_empty());
    let (code, output, error) = fixture.run(&["--json", "ticket", "list", "--all"]).await;
    assert_eq!(code, 0, "{error}");
    assert_eq!(json_lines(&output)[0]["tickets"][0]["status"], "resolved");
    assert!(fixture
        .state
        .lock()
        .requests
        .last()
        .unwrap()
        .query
        .is_none());
}

#[tokio::test]
async fn ticket_resolve_parses_json_or_preserves_text() {
    let fixture = Fixture::new().await;
    for (value, expected) in [
        ("true", json!(true)),
        ("42", json!(42)),
        ("null", Value::Null),
        (r#"{"answer":["one",2]}"#, json!({"answer":["one",2]})),
        (r#""true""#, json!("true")),
        ("some answer", json!("some answer")),
    ] {
        let (code, output, error) = fixture
            .run(&["--json", "ticket", "resolve", "t-1", value])
            .await;
        assert_eq!(code, 0, "{error}");
        assert_eq!(json_lines(&output)[0]["resolution"]["answer"], expected);
        assert_eq!(
            fixture.state.lock().requests.last().unwrap().body,
            json!({"value":expected})
        );
    }
}

#[tokio::test]
async fn self_approval_refusal_tells_cli_to_wait_for_another_device() {
    let fixture = Fixture::new().await;
    fixture.state.lock().self_approval = true;
    let (code, output, error) = fixture.run(&["ticket", "resolve", "t-1", "approve"]).await;
    assert_ne!(code, 0);
    assert!(output.is_empty());
    assert!(
        error.contains("403") && error.contains("self_approval_forbidden"),
        "{error}"
    );
    assert!(
        error.contains("another paired device") && error.contains("ticket_wait"),
        "{error}"
    );
}

#[tokio::test]
async fn person_approval_refusal_tells_cli_how_to_use_a_person_device() {
    let fixture = Fixture::new().await;
    fixture.state.lock().person_approval = true;
    let (code, _, error) = fixture.run(&["ticket", "resolve", "t-1", "true"]).await;
    assert_ne!(code, 0);
    assert!(
        error.contains("403") && error.contains("person_approval_required"),
        "{error}"
    );
    assert!(
        error.contains("token create --person") && error.contains("ticket_wait"),
        "{error}"
    );
}

#[tokio::test]
async fn secret_input_from_stdin_is_a_string_and_is_not_echoed() {
    let fixture = Fixture::new().await;
    let mut secret_ticket = ticket();
    secret_ticket["kind"] = json!("secret_input");
    secret_ticket["deferral"] = json!("external");
    fixture.state.lock().ticket = Some(secret_ticket);
    let secret = "123456";
    let mut stdin = std::io::Cursor::new(format!("{secret}\r\n"));
    let (code, output, error) = fixture
        .run_with_input(&["--json", "ticket", "resolve", "t-1", "-"], &mut stdin)
        .await;
    assert_eq!(code, 0, "{error}");
    assert_eq!(
        fixture.state.lock().requests.last().unwrap().body,
        json!({"value":secret})
    );
    assert_eq!(
        json_lines(&output)[0]["resolution"],
        json!({"status":"filled"})
    );
    assert!(!output.contains(secret));
    assert!(!error.contains(secret));
    fixture.state.lock().secret_error = true;
    let mut stdin = std::io::Cursor::new(secret);
    let (code, output, error) = fixture
        .run_with_input(&["--json", "ticket", "resolve", "t-1", "-"], &mut stdin)
        .await;
    assert_eq!(code, 1);
    assert!(error.contains("details withheld"));
    assert!(!output.contains(secret));
    assert!(!error.contains(secret));
}

#[tokio::test]
async fn totp_flag_sends_the_authenticator_from_stdin_and_never_from_argv() {
    let fixture = Fixture::new().await;
    let mut secret_ticket = ticket();
    secret_ticket["kind"] = json!("secret_input");
    secret_ticket["deferral"] = json!("external");
    fixture.state.lock().ticket = Some(secret_ticket);
    let uri = "otpauth://totp/Site:me?secret=JBSWY3DPEHPK3PXP&digits=6";
    let mut stdin = std::io::Cursor::new(format!("{uri}\n"));
    let (code, output, error) = fixture
        .run_with_input(
            &["--json", "ticket", "resolve", "t-1", "-", "--totp"],
            &mut stdin,
        )
        .await;
    assert_eq!(code, 0, "{error}");
    assert_eq!(
        fixture.state.lock().requests.last().unwrap().body,
        json!({"value":{"totp":uri}})
    );
    assert!(!output.contains("JBSWY3DPEHPK3PXP"));
    assert!(!error.contains("JBSWY3DPEHPK3PXP"));
    let requests = fixture.state.lock().requests.len();
    let (code, _, error) = fixture
        .run(&["ticket", "resolve", "t-1", "JBSWY3DPEHPK3PXP", "--totp"])
        .await;
    assert_eq!(code, 1);
    assert!(error.contains("stdin"), "{error}");
    assert_eq!(fixture.state.lock().requests.len(), requests);
}

#[tokio::test]
async fn omitted_value_asks_at_a_hidden_prompt_for_secret_input_only() {
    let fixture = Fixture::new().await;
    let mut secret_ticket = ticket();
    secret_ticket["kind"] = json!("secret_input");
    secret_ticket["deferral"] = json!("external");
    secret_ticket["target"] = json!({"origin":"https://bank.test","tab_id":"t","field_ref":"e1"});
    secret_ticket["otp"] = json!({"channel":"sms","can_register":false});
    fixture.state.lock().ticket = Some(secret_ticket);
    let code_text = "482913";
    // Only the first line is the answer; nothing after it is consumed.
    let mut stdin = std::io::Cursor::new(format!("{code_text}\r\nleft over"));
    let (code, output, error) = fixture
        .run_with_input(&["--json", "ticket", "resolve", "t-1"], &mut stdin)
        .await;
    assert_eq!(code, 0, "{error}");
    assert_eq!(
        fixture.state.lock().requests.last().unwrap().body,
        json!({"value":code_text})
    );
    assert_eq!(stdin.position(), code_text.len() as u64 + 2);
    assert!(
        error.starts_with("Verification code for https://bank.test: \n"),
        "{error}"
    );
    assert!(!output.contains(code_text) && !error.contains(code_text));

    let uri = "otpauth://totp/Bank:me?secret=JBSWY3DPEHPK3PXP";
    let mut stdin = std::io::Cursor::new(format!("{uri}\n"));
    let (code, _, error) = fixture
        .run_with_input(&["ticket", "resolve", "t-1", "--totp"], &mut stdin)
        .await;
    assert_eq!(code, 0, "{error}");
    assert_eq!(
        fixture.state.lock().requests.last().unwrap().body,
        json!({"value":{"totp":uri}})
    );
    assert!(error.starts_with("Authenticator setup key or otpauth:// link for https://bank.test: "));
    assert!(!error.contains("JBSWY3DPEHPK3PXP"));

    // A confirm ticket has no prompt: the value stays required and stdin is untouched.
    fixture.state.lock().ticket = Some(ticket());
    let requests = fixture.state.lock().requests.len();
    let mut stdin = std::io::Cursor::new("must not read");
    let (code, _, error) = fixture
        .run_with_input(&["ticket", "resolve", "t-1"], &mut stdin)
        .await;
    assert_eq!(code, 1);
    assert!(error.contains("a value is required"), "{error}");
    assert_eq!(stdin.position(), 0);
    assert_eq!(
        fixture.state.lock().requests.last().unwrap().method,
        "GET",
        "only the ticket was read"
    );
    assert_eq!(fixture.state.lock().requests.len(), requests + 1);
}

#[tokio::test]
async fn non_secret_dash_is_literal_and_never_reads_stdin() {
    let fixture = Fixture::new().await;
    let mut stdin = std::io::Cursor::new("must not read");
    let (code, _, error) = fixture
        .run_with_input(&["ticket", "resolve", "t-1", "-"], &mut stdin)
        .await;
    assert_eq!(code, 0, "{error}");
    assert_eq!(stdin.position(), 0);
    assert_eq!(
        fixture.state.lock().requests.last().unwrap().body,
        json!({"value":"-"})
    );
}

fn login_ticket(can_save: bool) -> Value {
    let mut ticket = ticket();
    ticket["kind"] = json!("login");
    ticket["deferral"] = json!("external");
    ticket["target"] = json!({"origin":"https://nid.naver.com","tab_id":"t","field_ref":null});
    ticket["login"] = json!({
        "origin":"https://nid.naver.com","site_name":"Naver\u{1b}[2J",
        "fields":[
            {"id":"id","kind":"identifier","label":"ID or phone number","masked":false,"required":true},
            {"id":"pw","kind":"password","label":"Password","masked":true,"required":true},
            {"id":"keep","kind":"checkbox","label":"Stay signed in","masked":false,"required":false}
        ],
        "alternatives":[{"id":"a1","label":"Sign in with Google","kind":"provider","provider":"google"}],
        "accounts":[{"item_id":"v1","username_hint":"j***@naver.com"}],
        "submit_label":"Log in","can_save":can_save
    });
    ticket
}

#[tokio::test]
async fn login_asks_each_field_with_the_password_hidden_and_sends_values_once() {
    let fixture = Fixture::new().await;
    fixture.state.lock().ticket = Some(login_ticket(true));
    let password = "pw-correct-horse";
    let mut stdin = std::io::Cursor::new(format!("jane\n{password}\ny\n"));
    let (code, output, error) = fixture
        .run_with_input(&["--json", "ticket", "resolve", "t-1"], &mut stdin)
        .await;
    assert_eq!(code, 0, "{error}");
    assert_eq!(
        fixture.state.lock().requests.last().unwrap().body,
        json!({"value":{"values":{"id":"jane","pw":password,"keep":true},"save":true}})
    );
    // Page text is printed without control characters.
    assert!(
        error.starts_with(
            "Log in to Naver[2J (https://nid.naver.com)\nID or phone number: \nPassword: "
        ),
        "{error}"
    );
    assert!(error.contains("Stay signed in [y/N]: "));
    assert!(!output.contains(password) && !error.contains(password));
    assert!(!output.contains("jane") && !error.contains("jane"));

    let mut stdin = std::io::Cursor::new(format!("jane\n{password}\n\n"));
    let (code, _, error) = fixture
        .run_with_input(&["ticket", "resolve", "t-1", "-", "--no-save"], &mut stdin)
        .await;
    assert_eq!(code, 0, "{error}");
    assert_eq!(
        fixture.state.lock().requests.last().unwrap().body,
        json!({"value":{"values":{"id":"jane","pw":password,"keep":false},"save":false}})
    );

    // A required field left empty sends nothing.
    let requests = fixture.state.lock().requests.len();
    let mut stdin = std::io::Cursor::new("jane\n\n");
    let (code, _, error) = fixture
        .run_with_input(&["ticket", "resolve", "t-1"], &mut stdin)
        .await;
    assert_eq!(code, 1);
    assert!(error.contains("Password is required"), "{error}");
    assert_eq!(
        fixture.state.lock().requests.len(),
        requests + 1,
        "only the ticket was read"
    );

    // A failed resolve keeps the daemon's message, which could echo a value, out of the error.
    fixture.state.lock().secret_error = true;
    let mut stdin = std::io::Cursor::new(format!("jane\n{password}\nn\n"));
    let (code, _, error) = fixture
        .run_with_input(&["ticket", "resolve", "t-1"], &mut stdin)
        .await;
    assert_eq!(code, 1);
    assert!(
        error.contains("could not resolve login ticket: API error (HTTP 400) (details withheld)"),
        "{error}"
    );
    assert!(!error.contains(password));
}

#[tokio::test]
async fn person_authority_errors_keep_login_and_enrollment_secrets_out_of_output() {
    let fixture = Fixture::new().await;
    fixture.state.lock().person_approval = true;
    fixture.state.lock().ticket = Some(login_ticket(true));
    let secret = "gate-canary-password";
    let mut input = std::io::Cursor::new(format!("gate-user\n{secret}\n\n"));
    let (code, output, error) = fixture
        .run_with_input(&["ticket", "resolve", "t-1", "--save"], &mut input)
        .await;
    assert_ne!(code, 0);
    assert!(
        error.contains("person_approval_required")
            && error.contains("--person")
            && error.contains("--no-save"),
        "{error}"
    );
    assert!(!error.contains(secret) && !output.contains(secret));

    let mut enrollment = ticket();
    enrollment["kind"] = json!("secret_input");
    fixture.state.lock().ticket = Some(enrollment);
    let seed = "JBSWY3DPEHPK3PXP";
    let mut input = std::io::Cursor::new(format!("{seed}\n"));
    let (code, output, error) = fixture
        .run_with_input(&["ticket", "resolve", "t-1", "-", "--totp"], &mut input)
        .await;
    assert_ne!(code, 0);
    assert!(
        error.contains("403")
            && error.contains("person_approval_required")
            && error.contains("--person")
            && error.contains("ticket_wait"),
        "{error}"
    );
    assert!(!error.contains(seed) && !output.contains(seed));
}

#[tokio::test]
async fn login_flags_pick_an_alternative_or_account_and_are_checked() {
    let fixture = Fixture::new().await;
    fixture.state.lock().ticket = Some(login_ticket(false));
    for (flag, id, body) in [
        ("--alternative", "a1", json!({"alternative":"a1"})),
        ("--account", "v1", json!({"account":"v1"})),
    ] {
        let mut stdin = std::io::Cursor::new("must not read");
        let (code, _, error) = fixture
            .run_with_input(&["ticket", "resolve", "t-1", flag, id], &mut stdin)
            .await;
        assert_eq!(code, 0, "{error}");
        assert_eq!(stdin.position(), 0);
        assert_eq!(
            fixture.state.lock().requests.last().unwrap().body,
            json!({"value":body})
        );
    }
    for (args, message) in [
        (vec!["--alternative", "zz"], "no other sign-in zz"),
        (vec!["--account", "zz"], "no saved account zz"),
        (vec!["--save"], "cannot be saved"),
    ] {
        let mut all = vec!["ticket", "resolve", "t-1"];
        all.extend(args);
        let (code, _, error) = fixture.run(&all).await;
        assert_eq!(code, 1);
        assert!(error.contains(message), "{error}");
    }
    let (code, _, error) = fixture
        .run(&["ticket", "resolve", "t-1", "x", "--no-save"])
        .await;
    assert_eq!(code, 1);
    assert!(error.contains("omit the value"), "{error}");
    fixture.state.lock().ticket = Some(ticket());
    let (code, _, error) = fixture
        .run(&["ticket", "resolve", "t-1", "--no-save"])
        .await;
    assert_eq!(code, 1);
    assert!(error.contains("only answer a login ticket"), "{error}");
    assert_eq!(
        fixture
            .run(&["ticket", "resolve", "t-1", "--save", "--no-save"])
            .await
            .0,
        2
    );
}

#[tokio::test]
async fn choice_lists_the_options_and_sends_the_picked_id() {
    let fixture = Fixture::new().await;
    let mut choice = ticket();
    choice["kind"] = json!("choice");
    choice["deferral"] = json!("external");
    choice["choice"] = json!({
        "origin":"https://nid.naver.com","site_name":"Naver","question":"How would you like to verify?",
        "options":[{"id":"sms","label":"Text me a code"},{"id":"email","label":"Email me a code","description":"j***@naver.com"}],
        "multiple":false
    });
    fixture.state.lock().ticket = Some(choice);
    let mut stdin = std::io::Cursor::new("2\n");
    let (code, output, error) = fixture
        .run_with_input(&["ticket", "resolve", "t-1"], &mut stdin)
        .await;
    assert_eq!(code, 0, "{error}");
    assert_eq!(
        error,
        "How would you like to verify?\n  1. Text me a code\n  2. Email me a code (j***@naver.com)\nOption number: "
    );
    assert_eq!(
        fixture.state.lock().requests.last().unwrap().body,
        json!({"value":{"option":"email"}})
    );
    assert!(output.contains("t-1\tchoice\t"), "{output}");
    let mut stdin = std::io::Cursor::new("7\n");
    let (code, _, error) = fixture
        .run_with_input(&["ticket", "resolve", "t-1"], &mut stdin)
        .await;
    assert_eq!(code, 1);
    assert!(error.contains("pick a number from 1 to 2"), "{error}");
}

#[tokio::test]
async fn ticket_show_done_and_reject_use_public_api() {
    let fixture = Fixture::new().await;
    let (code, output, error) = fixture.run(&["--json", "ticket", "show", "t-1"]).await;
    assert_eq!(code, 0, "{error}");
    assert_eq!(json_lines(&output)[0]["kind"], "confirm");
    assert_eq!(fixture.state.lock().requests.last().unwrap().method, "GET");
    // The approver sees what is being approved, not only the summary.
    let (code, output, error) = fixture.run(&["ticket", "show", "t-1"]).await;
    assert_eq!(code, 0, "{error}");
    assert_eq!(
        output,
        "t-1\tconfirm\tRun tool?\ntool: test {}\noptions: Approve, Reject\nstatus: pending, expires 2099-01-01T00:00:00+00:00\n"
    );
    let (code, output, error) = fixture.run(&["--json", "ticket", "done", "t-1"]).await;
    assert_eq!(code, 0, "{error}");
    assert_eq!(
        json_lines(&output)[0]["resolution"],
        json!({"status":"done"})
    );
    assert_eq!(
        fixture.state.lock().requests.last().unwrap().path,
        "/v1/tickets/t-1/done"
    );
    for reason in [None, Some("not now")] {
        let mut args = vec!["--json", "ticket", "reject", "t-1"];
        if let Some(reason) = reason {
            args.extend(["--reason", reason]);
        }
        let (code, output, error) = fixture.run(&args).await;
        assert_eq!(code, 0, "{error}");
        assert_eq!(json_lines(&output)[0]["status"], "rejected");
        assert_eq!(
            fixture.state.lock().requests.last().unwrap().body,
            json!({"reason":reason})
        );
    }
}

#[tokio::test]
async fn settled_tickets_are_removed_from_waiting_state() {
    let fixture = Fixture::new().await;
    fixture.state.lock().tickets = true;
    for kind in ["ticket.resolved", "ticket.rejected", "ticket.expired"] {
        fixture.state.lock().ticket_terminal = Some(kind);
        let (code, output, error) = fixture.run(&["--json", "hello"]).await;
        assert_eq!(code, 0, "{error}");
        assert_eq!(json_lines(&output).last().unwrap()["status"], "settled");
    }
}

#[tokio::test]
async fn api_errors_return_one_and_usage_errors_return_two() {
    let fixture = Fixture::new().await;
    fixture.state.lock().error = Some(403);
    let (code, output, error) = fixture.run(&["--json", "session", "list"]).await;
    assert_eq!(code, 1);
    assert!(error.contains("mock_error (HTTP 403)"));
    assert_eq!(json_lines(&output)[0]["exit_code"], 1);
    assert_eq!(fixture.run(&["session", "stop"]).await.0, 2);
    assert_eq!(fixture.run(&["--json"]).await.0, 2);
    assert_eq!(fixture.run(&["task", "session", "list"]).await.0, 2);
    assert_eq!(
        fixture
            .run(&[
                "session",
                "resume",
                "s-1",
                "--last-event-id",
                "not-a-number"
            ])
            .await
            .0,
        2
    );
}

#[tokio::test]
async fn disconnected_stream_preserves_cursor_and_returns_error_summary() {
    let fixture = Fixture::new().await;
    fixture.state.lock().eof = true;
    let (code, output, error) = fixture.run(&["--json", "hello"]).await;
    assert_eq!(code, 1);
    assert!(error.contains("ended before"));
    assert_eq!(json_lines(&output).last().unwrap()["last_seq"], 3);
    let key = format!("unix:{}", fixture.socket.display());
    assert_eq!(Cursor::new(&key, "s-1").unwrap().load().unwrap(), 3);
}

#[tokio::test]
async fn malformed_events_and_runtime_failures_do_not_claim_success() {
    let fixture = Fixture::new().await;
    fixture.state.lock().malformed = true;
    let (code, _, error) = fixture.run(&["hello"]).await;
    assert_eq!(code, 1);
    assert!(error.contains("invalid event JSON"));
    fixture.state.lock().malformed = false;
    fixture.state.lock().runtime_error = true;
    let (code, _, error) = fixture.run(&["hello"]).await;
    assert_eq!(code, 1);
    assert!(error.contains("runtime error failed"));
}

#[tokio::test]
async fn sandbox_warning_is_visible_but_task_streaming_still_settles() {
    let fixture = Fixture::new().await;
    fixture.state.lock().runtime_warning = true;
    let (code, output, error) = fixture.run(&["--json", "hello"]).await;
    assert_eq!(code, 0, "{error}");
    assert!(error.contains("warning: policy-only isolation"));
    let events = json_lines(&output);
    assert!(events.iter().any(|event| event["kind"] == "runtime.error"));
    assert!(events
        .iter()
        .any(|event| event["kind"] == "assistant.final"));
    assert_eq!(events.last().unwrap()["status"], "settled");
}

#[tokio::test]
async fn loopback_url_requires_token_and_sends_bearer_without_using_proxy() {
    let environment = Environment::new().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let state = Arc::new(Mutex::new(MockState::default()));
    let router = Router::new().fallback(mock).with_state(state.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let (mut output, mut error) = (Vec::new(), Vec::new());
    assert_eq!(
        run(
            ["openside", "--url", &url, "session", "list"],
            &mut output,
            &mut error
        )
        .await,
        2
    );
    std::env::set_var("OPENSIDE_TOKEN", "test-fixture-token");
    assert_eq!(
        run(
            ["openside", "--url", &url, "session", "list"],
            &mut output,
            &mut error
        )
        .await,
        0
    );
    assert_eq!(
        state.lock().requests[0].authorization.as_deref(),
        Some("Bearer test-fixture-token")
    );
    assert!(!String::from_utf8(error)
        .unwrap()
        .contains("test-fixture-token"));
    server.abort();
    drop(environment);
}

#[tokio::test]
async fn non_loopback_and_unsafe_urls_are_refused_before_connecting() {
    let _environment = Environment::new().await;
    for url in [
        "http://192.0.2.1:1234",
        "http://example.invalid:1234",
        "http://0.0.0.0:1234",
        "http://[::]:1234",
        "https://127.0.0.1:1234",
        "http://user:password@127.0.0.1:1234",
        "http://127.0.0.1:1234/v1",
    ] {
        let (mut output, mut error) = (Vec::new(), Vec::new());
        assert_eq!(
            run(
                [
                    "openside",
                    "--url",
                    url,
                    "--token",
                    "fixture-token",
                    "session",
                    "list"
                ],
                &mut output,
                &mut error
            )
            .await,
            2
        );
        assert!(!String::from_utf8(error).unwrap().contains("fixture-token"));
    }
}

#[tokio::test]
async fn default_xdg_socket_and_missing_daemon_work_without_tty() {
    let fixture = Fixture::new().await;
    let (mut output, mut error) = (Vec::new(), Vec::new());
    assert_eq!(
        run(["openside", "session", "list"], &mut output, &mut error).await,
        0
    );
    let path = fixture.environment.directory.path().join("missing.sock");
    let args = vec![
        OsString::from("openside"),
        OsString::from("--socket"),
        path.into_os_string(),
        OsString::from("session"),
        OsString::from("list"),
    ];
    assert_eq!(run(args, &mut output, &mut error).await, 1);
    assert!(!output.contains(&0x1b));
}

#[tokio::test]
async fn cursor_contains_only_session_sequence_and_is_private_endpoint_scoped() {
    let environment = Environment::new().await;
    let cursor = Cursor::new("unix:/tmp/endpoint-one", "s-1").unwrap();
    cursor.save(10).unwrap();
    cursor.save(2).unwrap();
    assert_eq!(cursor.load().unwrap(), 10);
    assert_eq!(
        Cursor::new("unix:/tmp/endpoint-two", "s-1")
            .unwrap()
            .load()
            .unwrap(),
        0
    );
    let files: Vec<_> = std::fs::read_dir(environment.directory.path().join("openside/cli"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1);
    assert_eq!(
        std::fs::metadata(&files[0]).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let contents: Value = serde_json::from_slice(&std::fs::read(&files[0]).unwrap()).unwrap();
    assert_eq!(contents, json!({"s-1":10}));
}

#[tokio::test]
async fn sse_parser_handles_every_fragment_boundary_multiline_and_id_tracking() {
    let _environment = Environment::new().await;
    let data = b"\xef\xbb\xbf: heartbeat\r\nid: 7\r\ndata: {\r\ndata: \"a\":1}\r\n\r\n: keepalive\n\ndata: second\n\nid:\ndata: third\n\nid: ignored\0value\ndata: fourth\n\n";
    for split in 0..=data.len() {
        let mut decoder = Decoder::default();
        let mut records = decoder.push(&data[..split]).unwrap();
        records.extend(decoder.push(&data[split..]).unwrap());
        assert_eq!(records.len(), 4);
        assert_eq!(records[0].data, b"{\n\"a\":1}");
        assert_eq!(records[0].id, Some(7));
        assert_eq!(records[1].id, Some(7));
        assert_eq!(records[2].id, None);
        assert_eq!(records[3].id, None);
    }
    let mut decoder = Decoder::default();
    assert!(decoder.push(b"id: xyz\n").is_err());
    assert!(Decoder::default()
        .push(b"id: 1\ndata: incomplete")
        .unwrap()
        .is_empty());
    assert_eq!(
        Decoder::default()
            .push(b"id: 1\rdata: value\r\r")
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn api_tokens_are_redacted_even_if_an_error_body_echoes_them() {
    let _environment = Environment::new().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let router = Router::new().fallback(|| async {
        respond(
            StatusCode::UNAUTHORIZED.as_u16(),
            json!({"code":"auth","message":"fixture-secret-token"}),
        )
    });
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let (mut output, mut error) = (Vec::new(), Vec::new());
    assert_eq!(
        run(
            [
                "openside",
                "--url",
                &url,
                "--token",
                "fixture-secret-token",
                "session",
                "list"
            ],
            &mut output,
            &mut error
        )
        .await,
        1
    );
    let error = String::from_utf8(error).unwrap();
    assert!(error.contains("[REDACTED]"));
    assert!(!error.contains("fixture-secret-token"));
    server.abort();
}

#[tokio::test]
async fn token_create_over_tcp_explains_that_minting_needs_the_local_socket() {
    let _environment = Environment::new().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let router = Router::new().fallback(|| async {
        respond(
            StatusCode::FORBIDDEN.as_u16(),
            json!({"code":"local_socket_required","message":"local_socket_required"}),
        )
    });
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let (mut output, mut error) = (Vec::new(), Vec::new());
    assert_eq!(
        run(
            [
                "openside",
                "--url",
                &url,
                "--token",
                "admin-fixture-token",
                "token",
                "create",
                "--scope",
                "browse"
            ],
            &mut output,
            &mut error
        )
        .await,
        1
    );
    assert!(output.is_empty());
    let error = String::from_utf8(error).unwrap();
    assert!(error.contains("local Unix socket"), "{error}");
    assert!(!error.contains("admin-fixture-token"));
    server.abort();
}

#[tokio::test]
async fn resume_replays_all_unseen_turns_before_the_latest_settlement() {
    let fixture = Fixture::new().await;
    assert_eq!(fixture.run(&["hello"]).await.0, 0);
    assert_eq!(
        fixture.run(&["session", "resume", "s-1", "again"]).await.0,
        0
    );
    let (code, output, error) = fixture
        .run(&["--json", "session", "resume", "s-1", "--last-event-id", "0"])
        .await;
    assert_eq!(code, 0, "{error}");
    let records = json_lines(&output);
    assert_eq!(records.last().unwrap()["last_seq"], 7);
    assert_eq!(records.len(), 8);
}

#[tokio::test]
async fn resume_running_session_ignores_an_older_settled_event() {
    let fixture = Fixture::new().await;
    assert_eq!(fixture.run(&["hello"]).await.0, 0);
    {
        let mut state = fixture.state.lock();
        state.events.push(event(5, "turn.started", json!({})));
        state
            .events
            .push(event(6, "assistant.delta", json!({"text":"second"})));
        let session = state.session.as_mut().unwrap();
        session["status"] = json!("running");
        session["last_seq"] = json!(6);
        state.settle_on_events = true;
    }
    let (code, output, error) = fixture
        .run(&["--json", "session", "resume", "s-1", "--last-event-id", "0"])
        .await;
    assert_eq!(code, 0, "{error}");
    let records = json_lines(&output);
    assert_eq!(records.last().unwrap()["last_seq"], 7);
    assert_eq!(records.len(), 8);
}

#[tokio::test]
async fn resume_text_of_consecutive_turns_is_not_run_together() {
    let fixture = Fixture::new().await;
    assert_eq!(fixture.run(&["hello"]).await.0, 0);
    {
        let mut state = fixture.state.lock();
        state.events.push(event(5, "turn.started", json!({})));
        state
            .events
            .push(event(6, "assistant.delta", json!({"text":"second"})));
        let session = state.session.as_mut().unwrap();
        session["status"] = json!("running");
        session["last_seq"] = json!(6);
        state.settle_on_events = true;
    }
    let (code, output, error) = fixture
        .run(&["session", "resume", "s-1", "--last-event-id", "0"])
        .await;
    assert_eq!(code, 0, "{error}");
    assert_eq!(output, "hello\nsecond\n");
}

#[tokio::test]
async fn an_unreachable_daemon_names_the_socket_and_how_to_start_it() {
    let fixture = Fixture::new().await;
    fixture.server.abort();
    // opensided removes its socket on exit; on Darwin a client can still complete a connect
    // against the stale file and only fail mid-request, which is not the unreachable-daemon
    // path under test.
    std::fs::remove_file(&fixture.socket).unwrap();
    let (code, _, error) = fixture.run(&["session", "list"]).await;
    assert_eq!(code, 1);
    assert!(
        error.contains("could not connect to daemon socket"),
        "{error}"
    );
    assert!(error.contains("opensided"), "{error}");
}

#[tokio::test]
async fn task_creation_forwards_model_modes_and_canonical_workspace() {
    let fixture = Fixture::new().await;
    let directory = fixture.environment.directory.path();
    std::fs::create_dir(directory.join("workspace")).unwrap();
    let workspace = directory
        .join("workspace/..")
        .to_string_lossy()
        .into_owned();
    let model = format!("{}:selected", module_path!());
    for mode in ["read-only", "guard", "full"] {
        let (code, _, error) = fixture
            .run(&[
                "--model",
                &model,
                "--mode",
                mode,
                "--workspace",
                &workspace,
                "task",
            ])
            .await;
        assert_eq!(code, 0, "{error}");
        let state = fixture.state.lock();
        let request = state
            .requests
            .iter()
            .rev()
            .find(|request| request.path == "/v1/sessions")
            .unwrap();
        assert_eq!(request.body["model"], model);
        assert_eq!(request.body["mode"], mode);
        assert_eq!(
            request.body["workspace"],
            directory.canonicalize().unwrap().to_str().unwrap()
        );
    }
    assert_eq!(fixture.run(&["--mode", "invalid", "task"]).await.0, 2);
    assert_eq!(
        fixture.run(&["--model", &model, "session", "list"]).await.0,
        2
    );
    let missing = directory.join("missing").to_string_lossy().into_owned();
    let (code, _, error) = fixture.run(&["--workspace", &missing, "task"]).await;
    assert_eq!(code, 1);
    assert!(error.contains("could not canonicalize --workspace"));
}

#[tokio::test]
async fn ticket_hints_select_the_action_for_each_kind() {
    let fixture = Fixture::new().await;
    fixture.state.lock().tickets = true;
    for (kind, command) in [
        ("secret_input", "ticket resolve t-1 -"),
        ("device_approval", "ticket done t-1"),
        ("takeover", "ticket done t-1"),
        ("info", "ticket resolve t-1 '\"true\"'"),
        ("login", "ticket resolve t-1\n"),
        ("choice", "ticket resolve t-1\n"),
    ] {
        let mut ticket = ticket();
        ticket["kind"] = json!(kind);
        ticket["options"] = json!([{"value":"true","label":"Yes"}]);
        fixture.state.lock().ticket = Some(ticket);
        let (code, output, error) = fixture.run(&["--json", "task"]).await;
        assert_eq!(code, 0, "{error}");
        assert!(error.contains(command), "{error}");
        assert!(!output.contains("Resolve in another process"));
        assert_eq!(
            json_lines(&output).last().unwrap()["status"],
            "waiting_human"
        );
    }
    let (code, output, error) = fixture.run(&["task"]).await;
    assert_eq!(code, 0, "{error}");
    assert_eq!(output, "hello\nSession s-1 is waiting on tickets.\n");
}

#[tokio::test]
async fn resume_seeds_pending_tickets_when_the_open_event_precedes_the_cursor() {
    let fixture = Fixture::new().await;
    {
        let mut state = fixture.state.lock();
        state.tickets = true;
        state.ticket = Some(ticket());
    }
    assert_eq!(fixture.run(&["task"]).await.0, 0);
    {
        let mut state = fixture.state.lock();
        state.events.push(event(7, "turn.started", json!({})));
        let session = state.session.as_mut().unwrap();
        session["status"] = json!("running");
        session["last_seq"] = json!(7);
        state.settle_on_events = true;
    }
    let key = format!("unix:{}", fixture.socket.display());
    Cursor::new(&key, "s-1").unwrap().save(7).unwrap();
    let (code, output, error) = fixture.run(&["--json", "session", "resume", "s-1"]).await;
    assert_eq!(code, 0, "{error}");
    let records = json_lines(&output);
    assert_eq!(records[0]["kind"], "session.settled");
    assert_eq!(records.last().unwrap()["status"], "waiting_human");
    assert_eq!(records.last().unwrap()["last_seq"], 8);
}

#[tokio::test]
async fn doctor_runs_a_fake_agent_without_a_daemon_and_reports_fail_checks() {
    let environment = Environment::new().await;
    std::env::set_var("PATH", environment.directory.path());
    let script = environment.directory.path().join("fake-agent");
    std::fs::write(&script, r#"#!/bin/sh
[ "$1" = doctor ] && [ "$2" = --json ] || exit 2
case "$OPENSIDE_DOCTOR_CONFIG" in *'"credentials":{}'*'"models":'* ) ;; *) exit 3 ;; esac
printf '%s\n' '{"python":"test-python","packages":{"pydantic-ai":"test-version"},"credentials":[],"models":{"default":null,"fallback":[],"providers_ok":false}}'
"#).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = environment.directory.path().join("config.toml");
    let config_text = format!(
        "[agent]\ncommand = [{}]\n[sandbox]\nbwrap = {}\n",
        serde_json::to_string(script.to_str().unwrap()).unwrap(),
        serde_json::to_string(
            environment
                .directory
                .path()
                .join("missing-bwrap")
                .to_str()
                .unwrap()
        )
        .unwrap(),
    );
    std::fs::write(&config, &config_text).unwrap();
    let path = config.to_str().unwrap();
    let (mut output, mut error) = (Vec::new(), Vec::new());
    let code = run(
        ["openside", "doctor", "--json", "--config", path],
        &mut output,
        &mut error,
    )
    .await;
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&error));
    assert!(error.is_empty());
    let records = json_lines(&String::from_utf8(output).unwrap());
    assert_eq!(records.len(), 2);
    let report: openside_config::doctor::DoctorReport =
        serde_json::from_value(records[0].clone()).unwrap();
    assert!(report
        .checks
        .iter()
        .any(|check| check.id == "agent.sidecar"
            && check.status == openside_config::doctor::Status::Ok));
    assert!(report.checks.iter().any(|check| check.id == "sandbox.bwrap"
        && check.status == openside_config::doctor::Status::Warn));
    assert_eq!(records[1]["exit_code"], 0);
    let (mut output, mut error) = (Vec::new(), Vec::new());
    let code = run(
        ["openside", "doctor", "--config", path],
        &mut output,
        &mut error,
    )
    .await;
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&error));
    assert_eq!(String::from_utf8(output).unwrap(), report.render_text());
    std::fs::write(&config, format!("{config_text}required = true\n")).unwrap();
    let (mut output, mut error) = (Vec::new(), Vec::new());
    let code = run(
        ["openside", "doctor", "--json", "--config", path],
        &mut output,
        &mut error,
    )
    .await;
    assert_eq!(code, 1, "{}", String::from_utf8_lossy(&error));
    let records = json_lines(&String::from_utf8(output).unwrap());
    assert!(records[0]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check["id"] == "sandbox.bwrap" && check["status"] == "fail"));
    assert_eq!(records[1]["exit_code"], 1);
    assert_eq!(records[1]["status"], "error");
    let missing = environment.directory.path().join("missing.toml");
    let (mut output, mut error) = (Vec::new(), Vec::new());
    let code = run(
        [
            "openside",
            "doctor",
            "--json",
            "--config",
            missing.to_str().unwrap(),
        ],
        &mut output,
        &mut error,
    )
    .await;
    assert_eq!(code, 1);
    assert!(error.is_empty());
    let records = json_lines(&String::from_utf8(output).unwrap());
    assert_eq!(records[0]["checks"][0]["id"], "config");
    assert_eq!(records[0]["checks"][0]["status"], "fail");
    assert_eq!(records[1]["exit_code"], 1);
}

#[tokio::test]
async fn pairing_purpose_sets_the_role_on_the_operator_offer() {
    let fixture = Fixture::new().await;
    for (arguments, kind, scopes) in [
        (
            vec!["--json", "pair"],
            "person_device",
            json!(["read", "approve", "human_step"]),
        ),
        (
            vec!["--json", "pair", "--agent"],
            "agent_client",
            json!(["read", "steer", "browse", "approve"]),
        ),
    ] {
        let (code, output, error) = fixture.run(&arguments).await;
        assert_eq!(code, 0, "{error}");
        assert_eq!(json_lines(&output)[0]["kind"], kind);
        let state = fixture.state.lock();
        let sent = state
            .requests
            .iter()
            .rev()
            .find(|request| request.path == "/v1/pairing")
            .unwrap();
        assert_eq!(sent.body["kind"], kind);
        assert_eq!(sent.body["scopes"], scopes);
    }
}
