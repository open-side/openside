//! Real daemon + sidecar + loopback broker/provider: rotation and credential-output audit.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use axum::{
    extract::{Path as ApiPath, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use openside_e2e::Harness;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const BROKER_TOKEN: &str = "test-broker-token-canary-738af";
const ENV_KEY: &str = "test-env-key-canary-512ba";
const ACCESS: &str = "test-access-canary-893bf";
const EMAIL: &str = "private-account@example.test";

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
        * 1000.0
}
#[derive(Default)]
struct Fake {
    requests: Mutex<Vec<String>>,
    provider_accounts: Mutex<Vec<String>>,
    blocks: Mutex<Vec<(u32, Value)>>,
    used: Mutex<Vec<f64>>,
}
async fn snapshot(State(fake): State<Arc<Fake>>, headers: HeaderMap) -> Response {
    assert_eq!(headers["authorization"], format!("Bearer {BROKER_TOKEN}"));
    fake.requests.lock().push("GET /v1/snapshot".into());
    let blocks = fake.blocks.lock();
    Json(json!({"generation":1,"serverNowMs":now_ms(),"credentials":(1..=3).map(|id| json!({
        "id":id,"provider":"anthropic","credential":{"type":"oauth","access":format!("{ACCESS}-{id}"),
        "refresh":"sentinel","expires":now_ms()+3_600_000.0,"accountId":format!("upstream-{id}"),"email":format!("private-{id}-{EMAIL}")},
        "blocks":blocks.iter().filter(|(cid,_)| *cid==id).map(|(_,block)| block.clone()).collect::<Vec<_>>()
    })).collect::<Vec<_>>() })).into_response()
}
async fn usage(State(fake): State<Arc<Fake>>) -> Json<Value> {
    fake.requests.lock().push("GET /v1/usage".into());
    let used = fake.used.lock();
    Json(
        json!({"reports":(1..=3).map(|id| json!({"provider":"anthropic","metadata":{"credentialId":id,"email":format!("private-{id}-{EMAIL}")},
        "limits":[{"window":{"id":"5h","label":EMAIL,"resetsAt":now_ms()+100_000.0},"amount":{"usedFraction":used[id-1]},"scope":{}}]})).collect::<Vec<_>>() }),
    )
}
async fn block(
    State(fake): State<Arc<Fake>>,
    ApiPath(id): ApiPath<u32>,
    Json(body): Json<Value>,
) -> Json<Value> {
    fake.requests
        .lock()
        .push(format!("POST /v1/credential/{id}/block"));
    assert_eq!(body["providerKey"], "anthropic");
    assert_eq!(body["blockScope"], "");
    assert!(body["blockedUntilMs"].as_f64().unwrap() > now_ms());
    fake.blocks.lock().push((id, body));
    Json(json!({"ok":true}))
}
async fn messages(
    State(fake): State<Arc<Fake>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    assert_eq!(body["model"], "test/model-a");
    let account = if headers.get("x-api-key").is_some_and(|key| key == ENV_KEY) {
        "env".to_owned()
    } else {
        headers["authorization"]
            .to_str()
            .unwrap()
            .strip_prefix(&format!("Bearer {ACCESS}-"))
            .unwrap()
            .to_owned()
    };
    let mut seen = fake.provider_accounts.lock();
    seen.push(account.clone());
    if account == "1" && seen.len() >= 2 {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [("retry-after", "120")],
            Json(json!({"error":{"type":"rate_limit_error","message":ACCESS}})),
        )
            .into_response();
    }
    drop(seen);
    let events = [
        json!({"type":"message_start","message":{"id":"test-msg","type":"message","role":"assistant","model":"test/model-a","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":0}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"public answer"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":2}}),
        json!({"type":"message_stop"}),
    ];
    let stream = events
        .iter()
        .map(|event| {
            format!(
                "event: {}\ndata: {event}\n\n",
                event["type"].as_str().unwrap()
            )
        })
        .collect::<String>();
    ([("content-type", "text/event-stream")], stream).into_response()
}
struct Server {
    url: String,
    state: Arc<Fake>,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    async fn start() -> Self {
        let state = Arc::new(Fake {
            used: Mutex::new(vec![0.1, 0.3, 0.5]),
            ..Fake::default()
        });
        let router = Router::new()
            .route("/v1/snapshot", get(snapshot))
            .route("/v1/usage", get(usage))
            .route("/v1/credential/{id}/block", post(block))
            .route("/v1/messages", post(messages))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self { url, state, task }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn prepare(server: &Server, broker: bool) -> Option<Harness> {
    let h = Harness::prepare(
        env!("CARGO_BIN_EXE_e2e-opensided"),
        &json!({"responses":[]}),
    )?;
    let path = h.root().join("config/openside/config.toml");
    let config = std::fs::read_to_string(&path).unwrap().replace(
        "default = \"script\"",
        "default = \"anthropic:test/model-a\"",
    );
    std::fs::write(&path, config).unwrap();
    std::fs::write(h.root().join("source.token"), BROKER_TOKEN).unwrap();
    h.append_config(&format!("\n[credentials.order]\nanthropic = [{}]\n[credentials.env]\nanthropic = \"TEST_API_KEY\"\n", if broker { "\"omp-broker\",\"env\"" } else { "\"env\"" }));
    if broker {
        h.append_config(&format!("\n[credentials.omp_broker]\nurl = {:?}\ntoken_file = {:?}\nsnapshot_ttl_seconds = 0\ntimeout_seconds = 2\n", server.url, h.root().join("source.token").to_string_lossy()));
    }
    let hooks = h.root().join("python-hooks/sitecustomize.py");
    let existing = std::fs::read_to_string(&hooks).unwrap();
    // Only the disposable fixture redirects provider traffic; production code still
    // refuses credentials to any origin other than its configured provider client.
    let injection = format!(
        r#"
os.environ.pop('OPENSIDE_AGENT_SCRIPT', None)
os.environ['TEST_API_KEY'] = {ENV_KEY:?}
from openside_agent.providers import models, sources
from pydantic_ai.providers.anthropic import AnthropicProvider
endpoint = {endpoint:?}
models.PROVIDER_URLS['anthropic'] = endpoint
original = models.AsyncAnthropic
def fake_client(**kwargs):
    return original(base_url=endpoint, **kwargs)
models.AsyncAnthropic = fake_client
sources.api_key_provider = lambda provider, key: AnthropicProvider(api_key=key, base_url=endpoint)
"#,
        endpoint = server.url
    );
    std::fs::write(hooks, existing + &injection).unwrap();
    Some(h)
}
async fn turn(h: &Harness, id: &str, number: usize) -> Vec<Value> {
    let before = h
        .request("GET", &format!("/v1/sessions/{id}"), None)
        .await
        .body["last_seq"]
        .as_u64()
        .unwrap();
    let mut events = h.events(id, before).await;
    let response = h
        .request(
            "POST",
            &format!("/v1/sessions/{id}/messages"),
            Some(json!({"text":format!("turn {number}"),"mode":"queue"})),
        )
        .await;
    assert_eq!(response.status, 202);
    let mut observed = Vec::new();
    loop {
        let event = events.next().await;
        let value = serde_json::to_value(&event).unwrap();
        let done = value["kind"] == "session.settled";
        observed.push(value);
        if done {
            break;
        }
    }
    assert!(
        observed
            .iter()
            .any(|event| event["kind"] == "assistant.final" && event["text"] == "public answer"),
        "{observed:?}"
    );
    observed
}
fn audit_bytes(bytes: &[u8]) {
    for canary in [
        BROKER_TOKEN,
        ENV_KEY,
        ACCESS,
        EMAIL,
        "upstream-1",
        "upstream-2",
        "upstream-3",
    ] {
        assert!(
            !bytes
                .windows(canary.len())
                .any(|chunk| chunk == canary.as_bytes()),
            "credential leaked to audited output"
        );
    }
}
fn scan(path: &Path) {
    for entry in std::fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            scan(&entry.path());
        } else {
            audit_bytes(&std::fs::read(entry.path()).unwrap());
        }
    }
}
async fn audit(h: &mut Harness, id: &str, observed: &[Value]) {
    audit_bytes(&serde_json::to_vec(observed).unwrap());
    audit_bytes(&serde_json::to_vec(&h.snapshot(id)).unwrap());
    for path in [
        "/v1/accounts",
        "/v1/settings",
        "/v1/settings/config",
        "/v1/sessions",
        "/v1/tickets",
        "/v1/vault/audit",
    ] {
        let response = h.request("GET", path, None).await;
        assert_eq!(response.status, 200, "{path}");
        audit_bytes(&serde_json::to_vec(&response.body).unwrap());
    }
    for args in [
        vec!["accounts"],
        vec!["accounts", "--json"],
        vec!["doctor", "--json"],
    ] {
        let output = h.cli(env!("CARGO_BIN_EXE_e2e-openside"), &args, None).await;
        if args[0] == "accounts" {
            assert!(output.status.success());
        }
        audit_bytes(&output.stdout);
        audit_bytes(&output.stderr);
    }
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for path in ["docs/specs/openapi.json", "web/src/api/schema.ts"] {
        audit_bytes(&std::fs::read(repo.join(path)).unwrap());
    }
    h.kill().await;
    scan(&h.data_dir);
    scan(&h.root().join("config"));
    scan(&h.root().join("state"));
    scan(&h.root().join("stderr"));
    audit_bytes(&std::fs::read(h.root().join("daemon.stderr")).unwrap());
    assert_eq!(
        std::fs::read_to_string(h.root().join("source.token")).unwrap(),
        BROKER_TOKEN
    );
}

#[tokio::test]
async fn broker_rate_limit_rotates_same_session_and_accounts_api_is_secret_free() {
    let server = Server::start().await;
    let Some(mut h) = prepare(&server, true) else {
        return;
    };
    h.launch().await;
    let read = h
        .request(
            "POST",
            "/v1/devices/pair",
            Some(json!({"name":"read fixture","scopes":["read"]})),
        )
        .await
        .body["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let steer = h
        .request(
            "POST",
            "/v1/devices/pair",
            Some(json!({"name":"steer fixture","scopes":["steer"]})),
        )
        .await
        .body["token"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        h.tcp_request("GET", "/v1/accounts", Some(&steer), None)
            .await
            .status,
        403
    );
    let initial = h
        .tcp_request("GET", "/v1/accounts", Some(&read), None)
        .await;
    assert_eq!(initial.status, 200, "{}", initial.body);
    assert_eq!(
        initial.body["sources"][0]["accounts"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        initial.body["sources"][0]["accounts"][0]["models"],
        json!(["anthropic:test/model-a"])
    );
    assert_eq!(
        initial.body["sources"][0]["accounts"][0]["usage_windows"][0]["used_percent"],
        10.0
    );
    let fetches = server.state.requests.lock().len();
    let (one, two) = tokio::join!(
        h.request("GET", "/v1/accounts", None),
        h.request("GET", "/v1/accounts", None)
    );
    assert_eq!(one.body, initial.body);
    assert_eq!(two.body, initial.body);
    assert_eq!(server.state.requests.lock().len(), fetches);
    let id = h
        .request("POST", "/v1/sessions", Some(json!({"mode":"read-only"})))
        .await
        .body["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut events = turn(&h, &id, 1).await;
    // Better quota does not break a live session pin.
    *server.state.used.lock() = vec![0.9, 0.1, 0.5];
    events.extend(turn(&h, &id, 2).await);
    events.extend(turn(&h, &id, 3).await);
    assert_eq!(
        *server.state.provider_accounts.lock(),
        vec!["1", "1", "2", "2"]
    );
    assert_eq!(server.state.blocks.lock().len(), 1);
    assert_eq!(server.state.blocks.lock()[0].0, 1);
    assert!(server
        .state
        .requests
        .lock()
        .contains(&"POST /v1/credential/1/block".to_owned()));
    tokio::time::sleep(Duration::from_millis(5100)).await;
    let accounts = h.request("GET", "/v1/accounts", None).await;
    assert!(
        accounts.body["sources"][0]["accounts"][0]["blocked_until_ms"]
            .as_f64()
            .unwrap()
            > now_ms()
    );
    audit(&mut h, &id, &events).await;
}

#[tokio::test]
async fn env_source_runs_without_broker_and_never_persists_credentials() {
    let server = Server::start().await;
    let Some(mut h) = prepare(&server, false) else {
        return;
    };
    h.launch().await;
    let id = h
        .request("POST", "/v1/sessions", Some(json!({"mode":"read-only"})))
        .await
        .body["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let events = turn(&h, &id, 1).await;
    assert_eq!(*server.state.provider_accounts.lock(), vec!["env"]);
    assert!(server.state.requests.lock().is_empty());
    let accounts = h.request("GET", "/v1/accounts", None).await;
    assert_eq!(accounts.body["sources"][0]["name"], "env");
    assert_eq!(accounts.body["sources"][0]["available"], true);
    audit(&mut h, &id, &events).await;
}

#[tokio::test]
async fn no_credential_source_fails_closed_before_provider_requests() {
    let server = Server::start().await;
    let Some(mut h) = prepare(&server, false) else {
        return;
    };
    let hooks = h.root().join("python-hooks/sitecustomize.py");
    let text = std::fs::read_to_string(&hooks).unwrap();
    std::fs::write(
        hooks,
        text.replace(
            &format!("os.environ['TEST_API_KEY'] = {ENV_KEY:?}"),
            "os.environ.pop('TEST_API_KEY', None)",
        ),
    )
    .unwrap();
    h.launch().await;
    let accounts = h.request("GET", "/v1/accounts", None).await;
    assert_eq!(accounts.status, 200);
    assert_eq!(accounts.body["sources"][0]["available"], false);
    let id = h
        .request("POST", "/v1/sessions", Some(json!({"mode":"read-only"})))
        .await
        .body["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut events = h.events(&id, 0).await;
    assert_eq!(
        h.request(
            "POST",
            &format!("/v1/sessions/{id}/messages"),
            Some(json!({"text":"fail closed","mode":"queue"}))
        )
        .await
        .status,
        409
    );
    let mut auth_failure = false;
    loop {
        let value = serde_json::to_value(events.next().await).unwrap();
        auth_failure |= value["kind"] == "runtime.error" && value["code"] == "auth_unavailable";
        if value["kind"] == "session.stopped" {
            break;
        }
    }
    assert!(auth_failure);
    assert_eq!(
        h.request("GET", &format!("/v1/sessions/{id}"), None)
            .await
            .body["status"],
        "stopped"
    );
    assert!(server.state.provider_accounts.lock().is_empty());
    assert!(server.state.requests.lock().is_empty());
    h.kill().await;
    scan(&h.data_dir);
    scan(&h.root().join("config"));
    scan(&h.root().join("stderr"));
}

#[tokio::test]
async fn command_source_runs_without_broker_and_never_persists_credentials() {
    let server = Server::start().await;
    let Some(mut h) = prepare(&server, false) else {
        return;
    };
    let config = h.root().join("config/openside/config.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(
        &config,
        text.replace("anthropic = [\"env\"]", "anthropic = [\"command\"]"),
    )
    .unwrap();
    let python = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../agent/.venv/bin/python")
        .canonicalize()
        .unwrap();
    let argv = json!([python, "-c", "import os; print(os.environ['TEST_API_KEY'])"]);
    h.append_config(&format!("\n[credentials.command]\nanthropic = {argv}\n"));
    h.launch().await;
    let id = h
        .request("POST", "/v1/sessions", Some(json!({"mode":"read-only"})))
        .await
        .body["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let events = turn(&h, &id, 1).await;
    assert_eq!(*server.state.provider_accounts.lock(), vec!["env"]);
    assert!(server.state.requests.lock().is_empty());
    let accounts = h.request("GET", "/v1/accounts", None).await;
    assert_eq!(accounts.body["sources"][0]["name"], "command");
    assert_eq!(accounts.body["sources"][0]["available"], true);
    audit(&mut h, &id, &events).await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn adversarial_file_reads_cannot_expose_daemon_environment_or_credential_hardlinks() {
    use std::os::unix::fs::PermissionsExt;
    let wrapper_dir = tempfile::tempdir().unwrap();
    let wrapper = wrapper_dir.path().join("daemon-wrapper");
    std::fs::write(&wrapper, format!("#!/bin/sh\nexport TEST_API_KEY='{ENV_KEY}'\nprintf '%s' \"$$\" > \"$XDG_STATE_HOME/daemon.pid\"\nexec '{}' \"$@\"\n", env!("CARGO_BIN_EXE_e2e-opensided"))).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let Some(mut h) = Harness::prepare(wrapper.to_str().unwrap(), &json!({"responses":[]})) else {
        return;
    };
    let source = h.workspace().join("source.txt");
    std::fs::write(&source, BROKER_TOKEN).unwrap();
    std::fs::hard_link(&source, h.workspace().join("ordinary.txt")).unwrap();
    h.append_config(&format!(
        "\n[credentials.test_source]\ntoken_file = {:?}\n",
        source.to_string_lossy()
    ));
    h.launch().await;
    let pid = std::fs::read_to_string(h.root().join("state/daemon.pid")).unwrap();
    let proc_path = format!("/proc/{pid}/environ");
    std::os::unix::fs::symlink(&proc_path, h.workspace().join("environment.txt")).unwrap();
    h.replace_script(&json!({"responses":[
        {"parts":[
            {"type":"tool_call","name":"read","args":{"path":"/proc/self/environ"},"id":"proc-self"},
            {"type":"tool_call","name":"read","args":{"path":proc_path},"id":"proc-pid"},
            {"type":"tool_call","name":"read","args":{"path":"environment.txt"},"id":"proc-alias"},
            {"type":"tool_call","name":"read","args":{"path":"ordinary.txt"},"id":"hardlink"}
        ]},
        {"parts":[{"type":"text","text":"public answer"}]}
    ]}));
    let id = h
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"mode":"full","workspace":h.workspace()})),
        )
        .await
        .body["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let events = turn(&h, &id, 1).await;
    for name in ["proc-self", "proc-pid", "proc-alias", "hardlink"] {
        assert!(
            events.iter().any(|event| event["kind"] == "tool.ended"
                && event["tool_call_id"] == name
                && event["is_error"] == true),
            "adversarial file tool was not denied"
        );
    }
    audit_bytes(&serde_json::to_vec(&events).unwrap());
    audit_bytes(&serde_json::to_vec(&h.snapshot(&id)).unwrap());
    h.kill().await;
    scan(&h.data_dir);
    scan(&h.root().join("config"));
    scan(&h.root().join("state"));
    scan(&h.root().join("stderr"));
    audit_bytes(&std::fs::read(h.root().join("daemon.stderr")).unwrap());
    assert_eq!(std::fs::read_to_string(source).unwrap(), BROKER_TOKEN);
}
