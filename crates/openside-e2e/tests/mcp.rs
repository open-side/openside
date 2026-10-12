//! `openside mcp` driven over real stdio by an rmcp client against the real daemon and the
//! scripted Python sidecar.
//!
//! The MCP server reaches the daemon through a recording proxy, so every scenario also proves
//! (G-5) that it uses nothing but documented public `/v1/*` routes.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../openside-channels/tests/support/mod.rs"]
mod support;

use std::{net::SocketAddr, process::Stdio, time::Duration};

use axum::{response::Html, routing::get, Router};
use futures_util::{FutureExt, SinkExt, StreamExt};
use openside_browser::{Browser, BrowserConfig};
use openside_e2e::Harness;
use rmcp::{
    model::{CallToolRequestParams, CallToolResult},
    service::RunningService,
    transport::TokioChildProcess,
    RoleClient, ServiceExt,
};
use serde_json::{json, Value};
use support::{is_public_route, RecordingProxy};

const DAEMON: &str = env!("CARGO_BIN_EXE_e2e-opensided");
const CLI: &str = env!("CARGO_BIN_EXE_e2e-openside");

fn text(text: &str) -> Value {
    json!({"parts":[{"type":"text","text":text}]})
}

fn tool(name: &str, args: Value, call: &str) -> Value {
    json!({"parts":[{"type":"tool_call","name":name,"args":args,"id":call}]})
}

struct Rig {
    harness: Harness,
    proxy: RecordingProxy,
    mcp: RunningService<RoleClient, ()>,
    token: String,
}

impl Rig {
    /// `scopes` are the capabilities of the token `openside mcp` runs with.
    async fn start(script: Value, guard: bool, config: &str, scopes: &[&str]) -> Option<Self> {
        Self::start_with(script, guard, config, scopes, |_| None).await
    }

    async fn start_with(
        script: Value,
        guard: bool,
        config: &str,
        scopes: &[&str],
        rewrite: impl FnOnce(&std::path::Path) -> Option<Value>,
    ) -> Option<Self> {
        let mut harness = Harness::prepare(DAEMON, &script)?;
        if guard {
            // The harness defaults to `full`; use the product default so writes outside the
            // workspace open approval tickets.
            let path = harness.root().join("config/openside/config.toml");
            let text = std::fs::read_to_string(&path).unwrap();
            std::fs::write(&path, text.replace("mode = \"full\"", "mode = \"guard\"")).unwrap();
        }
        harness.append_config(config);
        if let Some(script) = rewrite(harness.root()) {
            harness.replace_script(&script);
        }
        harness.launch().await;
        let port = std::fs::read_to_string(harness.data_dir.join("opensided.port")).unwrap();
        let upstream: SocketAddr = format!("127.0.0.1:{}", port.trim()).parse().unwrap();
        let proxy = RecordingProxy::start(upstream).await;
        let token = mint_token(&harness, scopes).await;
        let mcp = spawn_mcp(&harness, &proxy.url, &token).await;
        Some(Self {
            harness,
            proxy,
            mcp,
            token,
        })
    }

    async fn call(&self, name: &str, arguments: Value) -> CallToolResult {
        let params = CallToolRequestParams::new(name.to_owned())
            .with_arguments(arguments.as_object().cloned().unwrap_or_default());
        self.mcp
            .call_tool(params)
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    async fn ok(&self, name: &str, arguments: Value) -> Value {
        let result = self.call(name, arguments).await;
        assert_ne!(result.is_error, Some(true), "{name}: {}", body(&result));
        serde_json::from_str(&body(&result)).unwrap_or_else(|_| Value::String(body(&result)))
    }

    /// G-5: every request the MCP server made is a documented public route, and the listed
    /// routes were actually exercised.
    fn assert_public_api_only(&self, expected: &[(&str, &str)]) {
        let requests = self.proxy.requests();
        assert!(
            !requests.is_empty(),
            "the MCP server made no daemon request"
        );
        for (method, path) in &requests {
            assert!(
                is_public_route(method, path),
                "non-public daemon route used: {method} {path}"
            );
        }
        for (method, template) in expected {
            let hit = requests.iter().any(|(m, path)| {
                m == method
                    && openside_proto::ROUTES
                        .iter()
                        .filter(|route| route.path == *template)
                        .any(|route| {
                            let (t, a): (Vec<_>, Vec<_>) =
                                (route.path.split('/').collect(), path.split('/').collect());
                            t.len() == a.len()
                                && t.iter().zip(&a).all(|(t, a)| t.starts_with('{') || t == a)
                        })
            });
            assert!(hit, "expected {method} {template}; saw {requests:?}");
        }
    }

    async fn finish(mut self) {
        let _ = self.mcp.close().await;
        self.harness.kill().await;
    }
}

fn body(result: &CallToolResult) -> String {
    serde_json::to_value(&result.content[0]).unwrap()["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn mint_token(harness: &Harness, scopes: &[&str]) -> String {
    let mut args = vec!["token", "create"];
    for scope in scopes {
        args.extend(["--scope", scope]);
    }
    let output = harness.cli(CLI, &args, None).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let line = String::from_utf8(output.stdout).unwrap();
    line.trim_end().split('\t').nth(1).unwrap().to_owned()
}

async fn spawn_mcp(harness: &Harness, url: &str, token: &str) -> RunningService<RoleClient, ()> {
    let root = harness.root();
    let mut command = tokio::process::Command::new(CLI);
    command
        .env_clear()
        .current_dir(harness.workspace())
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", &harness.data_dir)
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_RUNTIME_DIR", root.join("run"))
        .env("OPENSIDE_TOKEN", token)
        .args(["--url", url, "mcp"])
        .kill_on_drop(true);
    let (transport, _) = TokioChildProcess::builder(command)
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    ().serve(transport)
        .await
        .unwrap_or_else(|e| panic!("MCP handshake: {e}"))
}

fn names(tools: &[rmcp::model::Tool]) -> Vec<String> {
    let mut names: Vec<_> = tools.iter().map(|tool| tool.name.to_string()).collect();
    names.sort();
    names
}

#[tokio::test]
async fn mcp_client_lists_tools_sends_a_message_and_reads_the_reply() {
    let Some(rig) = Rig::start(
        json!({"responses":[text("hello from the real sidecar")]}),
        false,
        "",
        &["read", "steer", "approve"],
    )
    .await
    else {
        return;
    };

    let info = rig.mcp.peer_info().expect("initialize result");
    assert_eq!(
        info.server_info.as_ref().expect("server info").name,
        "openside"
    );
    let tools = rig.mcp.list_all_tools().await.unwrap();
    let names = names(&tools);
    for expected in [
        "session_create",
        "session_list",
        "session_send",
        "session_status",
        "session_wait",
        "ticket_list",
        "ticket_resolve",
        "memory_search",
        "agent_run",
        "agent_result",
        "agent_status",
        "agent_steer",
        "agent_cancel",
        "ticket_wait",
        "browser_open_tab",
        "browser_tabs",
        "browser_navigate",
        "browser_snapshot",
        "browser_act",
        "browser_pointer",
        "browser_screenshot",
        "browser_close_tab",
        "request_login",
        "request_choice",
        "wait_for_approval",
        "request_otp",
        "request_human_step",
        "browser_repl",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "missing {expected}: {names:?}"
        );
    }

    let session = rig
        .ok("session_create", json!({"title":"via mcp","mode":"full"}))
        .await;
    let id = session["id"].as_str().unwrap().to_owned();
    let sent = rig
        .ok(
            "session_send",
            json!({"session_id":id,"text":"say hello","wait":true,"timeout_secs":60}),
        )
        .await;
    assert_eq!(sent["status"], "settled", "{sent}");
    assert_eq!(sent["replies"], json!(["hello from the real sidecar"]));
    assert!(sent["message"]["status"].as_str().is_some());

    // The reply is also what the daemon recorded, not something the MCP server made up.
    let status = rig.ok("session_status", json!({"session_id":id})).await;
    assert_eq!(status["status"], "idle");
    assert_eq!(status["last_seq"], sent["last_seq"]);
    let listed = rig.ok("session_list", json!({})).await;
    assert!(listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["id"] == id.as_str()));
    let waited = rig.ok("session_wait", json!({"session_id":id})).await;
    assert_eq!(waited["status"], "settled");
    rig.ok("ticket_list", json!({})).await;
    rig.ok("memory_search", json!({"query":"anything"})).await;

    rig.assert_public_api_only(&[
        ("POST", "/v1/sessions"),
        ("GET", "/v1/sessions"),
        ("GET", "/v1/sessions/{id}"),
        ("POST", "/v1/sessions/{id}/messages"),
        ("GET", "/v1/sessions/{id}/events"),
        ("GET", "/v1/tickets"),
        ("GET", "/v1/memory"),
    ]);
    rig.finish().await;
}

#[tokio::test]
async fn read_only_token_reads_but_gets_clean_errors_on_every_write_tool() {
    let Some(rig) = Rig::start(
        json!({"responses":[text("never delivered")]}),
        false,
        "",
        &["read"],
    )
    .await
    else {
        return;
    };
    // A session made by the local user, which the read-only MCP client may look at but not drive.
    let created = rig
        .harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"mode":"full","workspace":rig.harness.workspace()})),
        )
        .await;
    assert_eq!(created.status, 201, "{}", created.body);
    let id = created.body["id"].as_str().unwrap().to_owned();

    let listed = rig.ok("session_list", json!({})).await;
    assert_eq!(listed["sessions"].as_array().unwrap().len(), 1);
    rig.ok("session_status", json!({"session_id":id})).await;
    rig.ok("ticket_list", json!({})).await;
    rig.ok("memory_search", json!({})).await;

    for (tool, arguments) in [
        ("session_create", json!({"title":"nope"})),
        ("session_send", json!({"session_id":id,"text":"hi"})),
        (
            "browser_open_tab",
            json!({"session_id":id,"url":"http://localhost/"}),
        ),
        ("browser_snapshot", json!({"session_id":id,"tab_id":"t"})),
        (
            "browser_act",
            json!({"session_id":id,"tab_id":"t","action":"click","target":"e1"}),
        ),
        ("agent_run", json!({"session_id":id,"goal":"hi"})),
        ("browser_repl", json!({"session_id":id,"code":"1+1"})),
    ] {
        let result = rig.call(tool, arguments).await;
        assert_eq!(
            result.is_error,
            Some(true),
            "{tool} must fail with a read-only token"
        );
        let message = body(&result);
        assert!(
            message.contains("forbidden") && message.contains("403"),
            "{tool}: {message}"
        );
    }

    // Nothing was created or delivered behind the refusals.
    let after = rig.harness.request("GET", "/v1/sessions", None).await;
    assert_eq!(after.body["sessions"].as_array().unwrap().len(), 1);
    let session = rig
        .harness
        .request("GET", &format!("/v1/sessions/{id}"), None)
        .await;
    assert_eq!(session.body["last_seq"], created.body["last_seq"]);
    rig.assert_public_api_only(&[("GET", "/v1/sessions")]);
    rig.finish().await;
}

#[tokio::test]
async fn mcp_cannot_approve_its_own_session_but_a_person_device_resumes_it() {
    let Some(rig) = Rig::start_with(
        json!({"responses":[]}),
        true,
        "",
        &["read", "steer", "approve"],
        |root| {
            Some(json!({"responses":[
                tool("write", json!({"path": root.join("outside.txt"), "content": "approved over mcp"}), "w1"),
                text("write finished")
            ]}))
        },
    )
    .await
    else {
        return;
    };
    let session = rig.ok("session_create", json!({"mode":"guard"})).await;
    let id = session["id"].as_str().unwrap().to_owned();
    let sent = rig
        .ok(
            "session_send",
            json!({"session_id":id,"text":"write the file","wait":true,"timeout_secs":60}),
        )
        .await;
    assert_eq!(sent["status"], "waiting_human", "{sent}");
    let ticket_id = sent["pending_tickets"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(sent["pending_tickets"][0]["kind"], "confirm");
    let outside = rig.harness.root().join("outside.txt");
    assert!(!outside.exists(), "the write must wait for approval");

    let pending = rig.ok("ticket_list", json!({})).await;
    assert_eq!(pending["tickets"][0]["id"], ticket_id.as_str());
    let refused = rig
        .call(
            "ticket_resolve",
            json!({"ticket_id":ticket_id,"value":true}),
        )
        .await;
    assert_eq!(refused.is_error, Some(true));
    let message = body(&refused);
    assert!(message.contains("person_approval_required"), "{message}");
    assert!(message.contains("ticket_wait"), "{message}");
    let cli_refused = rig
        .harness
        .cli_remote(
            CLI,
            &[
                "--url",
                &rig.proxy.url,
                "ticket",
                "resolve",
                &ticket_id,
                "true",
            ],
            &[("OPENSIDE_TOKEN", rig.token.as_str())],
        )
        .await;
    assert!(!cli_refused.status.success());
    let cli_error = String::from_utf8_lossy(&cli_refused.stderr);
    assert!(
        cli_error.contains("403") && cli_error.contains("person_approval_required"),
        "{cli_error}"
    );
    assert!(cli_error.contains("ticket_wait"), "{cli_error}");
    assert!(!cli_error.contains(&rig.token));
    assert!(!outside.exists());
    let pending = rig
        .ok(
            "ticket_wait",
            json!({"ticket_id":ticket_id,"timeout_secs":1}),
        )
        .await;
    assert_eq!(pending["status"], "pending");

    let resolved = rig
        .harness
        .human_request(
            "POST",
            &format!("/v1/tickets/{ticket_id}/resolve"),
            Some(json!({"value":true})),
        )
        .await;
    assert_eq!(resolved.status, 200, "{}", resolved.body);
    let observed = rig
        .ok(
            "ticket_wait",
            json!({"ticket_id":ticket_id,"timeout_secs":1}),
        )
        .await;
    assert_eq!(observed["status"], "resolved");

    let resumed = rig
        .ok(
            "session_wait",
            json!({"session_id":id,"after_seq":sent["last_seq"],"timeout_secs":60}),
        )
        .await;
    assert_eq!(resumed["status"], "settled", "{resumed}");
    assert_eq!(resumed["replies"], json!(["write finished"]));
    assert_eq!(
        std::fs::read_to_string(&outside).unwrap(),
        "approved over mcp"
    );
    rig.assert_public_api_only(&[
        ("GET", "/v1/tickets/{id}"),
        ("POST", "/v1/tickets/{id}/resolve"),
    ]);
    rig.finish().await;
}

#[tokio::test]
async fn mcp_direct_confirmations_refuse_approval_but_accept_its_own_denial() {
    let Some(rig) = Rig::start(
        json!({"responses":[]}),
        true,
        "",
        &["read", "steer", "browse", "approve"],
    )
    .await
    else {
        return;
    };
    let session = rig.ok("session_create", json!({})).await;
    let id = session["id"].as_str().unwrap();
    let direct = rig.ok("request_human_step", json!({
        "session_id":id,"kind":"confirm","summary":"Approve a sensitive operation?","wait":false
    })).await;
    assert_eq!(direct["status"], "deferred", "{direct}");
    let ticket = direct["ticket_id"].as_str().unwrap();
    let refused = rig
        .call(
            "ticket_resolve",
            json!({"ticket_id":ticket,"value":"approve"}),
        )
        .await;
    assert_eq!(refused.is_error, Some(true));
    assert!(
        body(&refused).contains("person_approval_required"),
        "{}",
        body(&refused)
    );
    let denied = rig
        .ok("ticket_resolve", json!({"ticket_id":ticket,"value":false}))
        .await;
    assert_eq!(denied["resolution"]["approved"], false);
    let observed = rig
        .ok("ticket_wait", json!({"ticket_id":ticket,"timeout_secs":1}))
        .await;
    assert_eq!(observed["status"], "resolved");
    assert_eq!(observed["resolution"]["approved"], false);
    rig.assert_public_api_only(&[
        ("POST", "/v1/sessions/{id}/tools/{name}"),
        ("POST", "/v1/tickets/{id}/resolve"),
    ]);
    rig.finish().await;
}

#[tokio::test]
async fn secret_input_tickets_are_refused_by_mcp_and_stay_pending() {
    let Some(rig) = Rig::start(
        json!({"responses":[
            tool("request_human_step", json!({
                "kind": "secret_input", "summary": "Enter the verification code",
                "tab_id": "tab-1", "origin": "https://example.test", "field_ref": "e1"}), "secret1"),
            text("never reached")
        ]}),
        false,
        "",
        &["read", "steer", "approve", "human_step"],
    )
    .await
    else {
        return;
    };
    let session = rig.ok("session_create", json!({"mode":"full"})).await;
    let id = session["id"].as_str().unwrap().to_owned();
    let sent = rig
        .ok(
            "session_send",
            json!({"session_id":id,"text":"log me in","wait":true,"timeout_secs":60}),
        )
        .await;
    assert_eq!(sent["status"], "waiting_human", "{sent}");
    assert_eq!(sent["pending_tickets"][0]["kind"], "secret_input");
    let ticket_id = sent["pending_tickets"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let refused = rig
        .call(
            "ticket_resolve",
            json!({"ticket_id":ticket_id,"value":"canary-4242"}),
        )
        .await;
    assert_eq!(refused.is_error, Some(true));
    assert!(
        body(&refused).contains("secret_input"),
        "{}",
        body(&refused)
    );
    let ticket = rig
        .harness
        .request("GET", &format!("/v1/tickets/{ticket_id}"), None)
        .await;
    assert_eq!(ticket.body["status"], "pending");
    // The refusal happens before any resolve request, so the proxy never saw the value.
    assert!(rig
        .proxy
        .requests()
        .iter()
        .all(|(_, path)| !path.ends_with("/resolve")));
    rig.harness.assert_no_canaries(&["canary-4242"]);
    rig.finish().await;
}

const PAGE: &str = r#"<!doctype html><title>MCP fixture</title><h1>Fixture page</h1>
<button aria-label="Continue" onclick="location.assign('/next')">Continue</button>"#;
const NEXT: &str = r#"<!doctype html><title>Next</title><h1>Reached the next page</h1>"#;

#[tokio::test]
async fn browser_tools_and_a_repl_batch_drive_a_loopback_page() {
    let executable = match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(path) => path,
        Err(reason) => {
            eprintln!("SKIP: MCP browser tools need Chromium: {reason}");
            return;
        }
    };
    let app = Router::new()
        .route("/", get(|| async { Html(PAGE) }))
        .route("/next", get(|| async { Html(NEXT) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let site = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let Some(rig) = Rig::start(
        json!({"responses":[text("unused")]}),
        false,
        &format!("[browser]\nexecutable = {}\n", json!(executable)),
        &["read", "steer"],
    )
    .await
    else {
        site.abort();
        return;
    };
    let session = rig.ok("session_create", json!({"mode":"full"})).await;
    let id = session["id"].as_str().unwrap().to_owned();

    let opened = rig
        .ok(
            "browser_open_tab",
            json!({"session_id":id,"url":format!("{origin}/")}),
        )
        .await;
    assert_eq!(opened["status"], "done", "{opened}");
    let tab = opened["result"]["tab_id"].as_str().unwrap().to_owned();

    let snapshot = rig
        .ok("browser_snapshot", json!({"session_id":id,"tab_id":tab}))
        .await;
    let snapshot = snapshot["result"].to_string();
    assert!(snapshot.contains("Fixture page"), "{snapshot}");

    // One REPL call runs a batch of steps in the tab the catalog tools opened.
    let batch = rig
        .ok(
            "browser_repl",
            json!({"session_id":id,"tab_id":tab,"code":
                "await page.getByRole('button', { name: 'Continue' }).click();\n\
                 await page.waitForURL('**/next');\n\
                 await expect(page.getByRole('heading')).toHaveText('Reached the next page');\n\
                 page.url()"}),
        )
        .await;
    assert_eq!(batch["status"], "done", "{batch}");
    assert_eq!(batch["result"]["return_value"], format!("{origin}/next"));

    let tabs = rig.ok("browser_tabs", json!({"session_id":id})).await;
    assert!(tabs["result"].to_string().contains("/next"), "{tabs}");
    let shot = rig
        .ok("browser_screenshot", json!({"session_id":id,"tab_id":tab}))
        .await;
    assert!(
        shot["artifacts"][0].as_str().unwrap().ends_with(".png"),
        "{shot}"
    );
    rig.assert_public_api_only(&[
        ("GET", "/v1/tools"),
        ("POST", "/v1/sessions/{id}/tools/{name}"),
        ("POST", "/v1/sessions/{id}/repl"),
    ]);
    rig.finish().await;
    site.abort();
}

/// Event kinds of a session's first run, in order, with run-specific payloads dropped.
async fn run_kinds(harness: &Harness, session: &str) -> Vec<String> {
    harness
        .events(session, 0)
        .await
        .settled()
        .await
        .iter()
        .map(|event| {
            serde_json::to_value(event).unwrap()["kind"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

#[tokio::test]
async fn agent_run_records_the_same_events_as_an_in_app_run() {
    let Some(rig) = Rig::start(
        json!({"responses":[text("the cheapest is 42")]}),
        false,
        "",
        &["read", "steer"],
    )
    .await
    else {
        return;
    };
    // The web composer's path: create a session and post a message over the local socket.
    let in_app = rig
        .harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"mode":"full","workspace":rig.harness.workspace()})),
        )
        .await;
    let in_app = in_app.body["id"].as_str().unwrap().to_owned();
    let posted = rig
        .harness
        .request(
            "POST",
            &format!("/v1/sessions/{in_app}/messages"),
            Some(json!({"text":"find the cheapest","mode":"now"})),
        )
        .await;
    assert!(posted.status < 300, "{}", posted.body);
    let in_app_kinds = run_kinds(&rig.harness, &in_app).await;

    let run = rig
        .ok(
            "agent_run",
            json!({"goal":"find the cheapest","mode":"full","wait":true,"timeout_secs":60}),
        )
        .await;
    assert_eq!(run["status"], "settled", "{run}");
    assert_eq!(run["replies"], json!(["the cheapest is 42"]));
    let id = run["session_id"].as_str().unwrap().to_owned();
    assert_eq!(run_kinds(&rig.harness, &id).await, in_app_kinds);

    let status = rig.ok("agent_status", json!({"session_id":id})).await;
    assert_eq!(status["status"], "idle", "{status}");
    assert_eq!(status["pending_tickets"], json!([]));
    rig.assert_public_api_only(&[
        ("POST", "/v1/sessions"),
        ("POST", "/v1/sessions/{id}/messages"),
        ("GET", "/v1/sessions/{id}/events"),
        ("GET", "/v1/sessions/{id}/subagents"),
    ]);
    rig.finish().await;
}

async fn ui_wait(browser: &Browser, tab: &str, predicate: &str) {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if browser.evaluate(tab, predicate).await.unwrap() == true {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("UI predicate failed: {predicate}"));
}

async fn phone_viewport(
    browser: &Browser,
    tab: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    use tokio_tungstenite::tungstenite::Message;
    let (mut socket, _) = tokio_tungstenite::connect_async(&browser.endpoint().ws_url)
        .await
        .unwrap();
    async fn call(
        socket: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        request: Value,
    ) -> Value {
        socket
            .send(Message::Text(request.to_string().into()))
            .await
            .unwrap();
        loop {
            let message = tokio::time::timeout(Duration::from_secs(10), socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            if let Message::Text(text) = message {
                let reply: Value = serde_json::from_str(&text).unwrap();
                if reply["id"] == request["id"] {
                    assert!(reply["error"].is_null(), "{reply}");
                    return reply["result"].clone();
                }
            }
        }
    }
    let attached = call(
        &mut socket,
        json!({"id":1,"method":"Target.attachToTarget","params":{"targetId":tab,"flatten":true}}),
    )
    .await;
    call(
        &mut socket,
        json!({"id":2,"sessionId":attached["sessionId"],
        "method":"Emulation.setDeviceMetricsOverride",
        "params":{"width":390,"height":844,"deviceScaleFactor":3,"mobile":true}}),
    )
    .await;
    call(
        &mut socket,
        json!({"id":3,"sessionId":attached["sessionId"],
        "method":"Emulation.setTouchEmulationEnabled",
        "params":{"enabled":true,"maxTouchPoints":5}}),
    )
    .await;
    socket
}

#[tokio::test]
async fn delegated_cli_and_mcp_tickets_appear_live_on_desktop_and_phone_and_wait_for_approval() {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist");
    assert!(
        dist.join("index.html").is_file(),
        "build the web UI before this acceptance test"
    );
    let executable = openside_config::discover_browser(&openside_config::Config::default())
        .expect("Chromium is required for external-agent UI acceptance");
    let rig = Rig::start_with(
        json!({"responses":[]}), true,
        &format!("[ui]\ndir = {}\n", json!(dist.canonicalize().unwrap())),
        &["read", "steer", "approve"],
        |root| Some(json!({"responses":[
            tool("write", json!({"path":root.join("delegated.txt"),"content":"approved delegation"}), "external-write"),
            text("external delegation finished")
        ]})),
    ).await.expect("scripted Python runtime is required");
    let token = rig.harness.human_token().await;
    let port = std::fs::read_to_string(rig.harness.data_dir.join("opensided.port")).unwrap();
    let origin = format!("http://127.0.0.1:{}", port.trim());
    let (browser, _events) = Browser::launch(BrowserConfig::new(executable))
        .await
        .unwrap();
    let outcome = std::panic::AssertUnwindSafe(async {
    let desktop = browser.create_agent_tab("external-ui", &origin).await.unwrap().id;
    ui_wait(&browser, &desktop, "!!document.querySelector('.token-entry')").await;
    browser.evaluate(&desktop, &format!("localStorage.setItem('openside.token', {}); true", json!(token)))
        .await.unwrap();
    let phone = browser.create_agent_tab("external-ui", &origin).await.unwrap().id;
    let _phone_cdp = phone_viewport(&browser, &phone).await;
    assert_eq!(browser.evaluate(&phone, "innerWidth").await.unwrap(), 390);
    let pending = "!!document.querySelector('section.ticket-pending[aria-label=\"Approval needed\"]')";
    let approve = "Array.from(document.querySelector('section.ticket-pending').querySelectorAll('button')).find(b => b.textContent.trim() === 'Approve').click(); true";

    for caller in ["cli", "mcp"] {
        let created = rig.harness.request("POST", "/v1/sessions",
            Some(json!({"mode":"guard","workspace":rig.harness.workspace()}))).await;
        assert_eq!(created.status, 201, "{}", created.body);
        let id = created.body["id"].as_str().unwrap();
        for tab in [&desktop, &phone] {
            browser.navigate("external-ui", tab, &format!("{origin}/sessions/{id}")).await.unwrap();
            ui_wait(&browser, tab, "!!document.querySelector('main.session .composer')").await;
        }

        if caller == "cli" {
            let args = ["run", "--session", id, "--wait", "--timeout", "25", "write the file"];
            let waiting = rig.harness.cli(CLI, &args, None);
            tokio::pin!(waiting);
            tokio::select! {
                output = &mut waiting => panic!("CLI finished before approval: {output:?}"),
                () = async {
                    ui_wait(&browser, &desktop, pending).await;
                    ui_wait(&browser, &phone, pending).await;
                } => {}
            }
            assert!(!rig.harness.root().join("delegated.txt").exists());
            browser.evaluate(&phone, approve).await.unwrap();
            let output = waiting.await;
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            assert!(String::from_utf8_lossy(&output.stdout).contains("external delegation finished"));
        } else {
            let run = rig.ok("agent_run",
                json!({"session_id":id,"goal":"write the file","wait":true,"timeout_secs":25})).await;
            assert_eq!(run["status"], "waiting_human", "{run}");
            let ticket = run["pending_tickets"][0]["id"].as_str().unwrap();
            ui_wait(&browser, &desktop, pending).await;
            ui_wait(&browser, &phone, pending).await;
            let waiting = rig.ok("ticket_wait", json!({"ticket_id":ticket,"timeout_secs":25}));
            tokio::pin!(waiting);
            tokio::select! {
                result = &mut waiting => panic!("MCP finished before approval: {result}"),
                () = tokio::time::sleep(Duration::from_millis(150)) => {}
            }
            browser.evaluate(&desktop, approve).await.unwrap();
            assert_eq!(waiting.await["status"], "resolved");
            let result = rig.ok("agent_result",
                json!({"session_id":id,"after_seq":run["last_seq"],"timeout_secs":25})).await;
            assert_eq!(result["status"], "settled", "{result}");
            assert_eq!(result["replies"], json!(["external delegation finished"]));
        }
        for tab in [&desktop, &phone] {
            ui_wait(&browser, tab,
                "document.querySelector('main.session')?.dataset.status === 'idle' && document.body.innerText.includes('external delegation finished')").await;
        }
    }
    }).catch_unwind().await;
    browser.shutdown().await.unwrap();
    rig.finish().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}
