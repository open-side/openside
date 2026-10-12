//! External-agent CLI and MCP capabilities through paired relay and direct remote profiles.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    os::unix::fs::PermissionsExt,
    process::{Output, Stdio},
    time::Duration,
};

use axum::{response::Html, routing::get, Router};
use openside_e2e::Harness;
use openside_relay::{
    client::Tunnel,
    server::{self, ServerConfig},
};
use rmcp::{
    model::{CallToolRequestParams, CallToolResult},
    service::RunningService,
    transport::TokioChildProcess,
    RoleClient, ServiceExt,
};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const DAEMON: &str = env!("CARGO_BIN_EXE_e2e-opensided");
const CLI: &str = env!("CARGO_BIN_EXE_e2e-openside");
const RELAY_TOKEN: &str = "remote-acceptance-private-registration-token";

struct RemoteComputer {
    root: tempfile::TempDir,
}

impl RemoteComputer {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        for dir in ["home", "config", "data", "state", "cache", "run"] {
            std::fs::create_dir_all(root.path().join(dir)).unwrap();
        }
        Self { root }
    }

    fn command(&self) -> tokio::process::Command {
        let root = self.root.path();
        let mut command = tokio::process::Command::new(CLI);
        command
            .env_clear()
            .current_dir(root)
            .env("HOME", root.join("home"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("XDG_RUNTIME_DIR", root.join("run"))
            .env("OPENSIDE_REMOTE_TOKEN_STORE", "file")
            .kill_on_drop(true);
        command
    }

    async fn cli(&self, args: &[&str], remote_env: Option<&str>) -> Output {
        let mut command = self.command();
        command.args(args);
        if let Some(name) = remote_env {
            command.env("OPENSIDE_REMOTE", name);
        }
        tokio::time::timeout(Duration::from_secs(60), command.output())
            .await
            .expect("remote CLI deadline")
            .unwrap()
    }

    async fn mcp(&self, name: &str) -> RunningService<RoleClient, ()> {
        let mut command = self.command();
        command.args(["mcp", "--remote", name]);
        let (transport, _) = TokioChildProcess::builder(command)
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        ().serve(transport).await.expect("remote MCP handshake")
    }
}

fn json_output(output: Output) -> Value {
    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(output.stdout.split(|byte| *byte == b'\n').next().unwrap())
        .unwrap_or_else(|e| panic!("CLI JSON: {e}: {}", String::from_utf8_lossy(&output.stdout)))
}

fn text(result: &CallToolResult) -> String {
    serde_json::to_value(&result.content[0]).unwrap()["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn call(mcp: &RunningService<RoleClient, ()>, name: &str, args: Value) -> CallToolResult {
    mcp.call_tool(
        CallToolRequestParams::new(name.to_owned())
            .with_arguments(args.as_object().unwrap().clone()),
    )
    .await
    .unwrap()
}

async fn ok(mcp: &RunningService<RoleClient, ()>, name: &str, args: Value) -> Value {
    let result = call(mcp, name, args).await;
    assert_ne!(result.is_error, Some(true), "{name}: {}", text(&result));
    serde_json::from_str(&text(&result)).unwrap()
}

async fn pairing_link(harness: &Harness, relay: bool, scopes: &[&str]) -> String {
    let mut args = vec!["--json", "pair", "--name", "remote acceptance"];
    if relay {
        args.extend(["--via", "relay"]);
    }
    for scope in scopes {
        args.extend(["--scope", scope]);
    }
    let offer = json_output(harness.cli(CLI, &args, None).await);
    if relay {
        assert_eq!(offer["transport"], "relay");
        offer["url"].as_str().unwrap().to_owned()
    } else {
        // Tailscale Serve and Cloudflare Tunnel terminate into this same bearer-token TCP API.
        let port = std::fs::read_to_string(harness.data_dir.join("opensided.port")).unwrap();
        format!(
            "http://127.0.0.1:{}/pair#{}",
            port.trim(),
            offer["code"].as_str().unwrap()
        )
    }
}

async fn assert_remote_response(
    harness: &Harness,
    link: &str,
    relay: bool,
    method: &str,
    path: &str,
    token: &str,
    body: Value,
) -> (u16, Value) {
    if !relay {
        let response = harness
            .tcp_request(method, path, Some(token), Some(body))
            .await;
        return (response.status, response.body);
    }
    let url = url::Url::parse(link).unwrap();
    let fields: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(url.fragment().unwrap().as_bytes())
            .into_owned()
            .collect();
    let key = openside_relay::noise::decode_key(&fields["k"]).unwrap();
    let tunnel = Tunnel::connect(
        url.origin().ascii_serialization().as_str(),
        &fields["d"],
        &key,
    )
    .await
    .unwrap();
    let mut stream = tunnel.open().await.unwrap();
    let body = body.to_string();
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.write_all(body.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), stream.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    let response = String::from_utf8(response).unwrap();
    let status = response.split(' ').nth(1).unwrap().parse().unwrap();
    let body = response.split("\r\n\r\n").nth(1).unwrap();
    (status, serde_json::from_str(body).unwrap())
}

async fn remote_agent_and_browser(relay: bool) {
    let executable = openside_config::discover_browser(&openside_config::Config::default())
        .expect("Chromium is required for remote browser acceptance");
    let mut harness = Harness::prepare(
        DAEMON,
        &json!({"responses":[
            {"parts":[{"type":"text","text":"remote scripted answer"}]}
        ]}),
    )
    .expect("scripted Python runtime is required");
    harness.append_config(&format!("[browser]\nexecutable = {}\n", json!(executable)));
    let relay_server = if relay {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let relay_url = format!("http://{}", listener.local_addr().unwrap());
        let token_file = harness.root().join("relay-token");
        std::fs::write(&token_file, RELAY_TOKEN).unwrap();
        std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o600)).unwrap();
        harness.append_config(&format!(
            "[remote]\nmode = [\"relay\"]\n[remote.relay]\nurl = {}\ntoken_file = {}\n",
            json!(relay_url),
            json!(token_file)
        ));
        Some(tokio::spawn(async move {
            axum::serve(
                listener,
                server::router(ServerConfig {
                    daemon_tokens: vec![RELAY_TOKEN.into()],
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        }))
    } else {
        None
    };
    harness.launch().await;
    if relay {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let status = harness.request("GET", "/v1/remote/status", None).await;
                if status.body["relay"]["state"] == "connected" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("daemon relay registration");
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let page_url = format!(
        "http://localhost:{}/",
        listener.local_addr().unwrap().port()
    );
    let site = tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/", get(|| async {
            Html("<!doctype html><title>Remote fixture</title><h1>Remote browser works</h1><button>Continue</button>")
        }))).await.unwrap();
    });

    let computer = RemoteComputer::new();
    let link = pairing_link(&harness, relay, &["read", "steer", "browse", "approve"]).await;
    let connected = json_output(
        computer
            .cli(&["--json", "connect", &link, "--name", "desk"], None)
            .await,
    );
    assert_eq!(connected["name"], "desk");
    assert_eq!(
        connected["transport"],
        if relay { "relay" } else { "direct" }
    );
    assert_eq!(connected["token_store"], "file");
    let profiles_path = computer.root.path().join("config/openside/remotes.toml");
    assert_eq!(
        std::fs::metadata(&profiles_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(
        !computer.root.path().join("run/opensided.sock").exists(),
        "remote machine has no local daemon"
    );

    let output = computer
        .cli(
            &[
                "--remote",
                "desk",
                "--json",
                "run",
                "--wait",
                "--timeout",
                "30",
                "answer remotely",
            ],
            None,
        )
        .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|value| value["type"] == "result")
        .expect("CLI waited for the delegated answer");
    assert_eq!(result["status"], "settled");
    assert_eq!(result["replies"], json!(["remote scripted answer"]));
    let summary: Value = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|value| value["type"] == "summary")
        .expect("CLI summary identifies the delegated session");
    assert_eq!(summary["status"], "settled");
    let session = summary["session_id"].as_str().unwrap();
    let listed = json_output(
        computer
            .cli(&["--json", "session", "list"], Some("desk"))
            .await,
    );
    assert!(listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["id"] == session));
    let opened = json_output(
        computer
            .cli(
                &[
                    "--remote",
                    "desk",
                    "--json",
                    "browser",
                    "open",
                    session,
                    &format!("url={page_url}"),
                ],
                None,
            )
            .await,
    );
    assert_eq!(opened["status"], "done", "{opened}");
    let tab = opened["result"]["tab_id"].as_str().unwrap();
    let snapshot = json_output(
        computer
            .cli(
                &[
                    "--remote",
                    "desk",
                    "--json",
                    "browser",
                    "snapshot",
                    session,
                    &format!("tab_id={tab}"),
                ],
                None,
            )
            .await,
    );
    assert_eq!(snapshot["status"], "done", "{snapshot}");
    assert!(
        snapshot["result"]
            .to_string()
            .contains("Remote browser works"),
        "{snapshot}"
    );

    let mut mcp = computer.mcp("desk").await;
    let run = ok(
        &mcp,
        "agent_run",
        json!({"goal":"answer from remote MCP","wait":true,"timeout_secs":30}),
    )
    .await;
    assert_eq!(run["status"], "settled", "{run}");
    assert_eq!(run["replies"], json!(["remote scripted answer"]));
    let snapshot = ok(
        &mcp,
        "browser_snapshot",
        json!({"session_id":session,"tab_id":tab}),
    )
    .await;
    assert_eq!(snapshot["status"], "done", "{snapshot}");
    assert!(snapshot["result"]
        .to_string()
        .contains("Remote browser works"));
    mcp.close().await.unwrap();

    let read_link = pairing_link(&harness, relay, &["read"]).await;
    let read_profile = json_output(
        computer
            .cli(&["--json", "connect", &read_link, "--name", "reader"], None)
            .await,
    );
    assert_eq!(read_profile["scopes"], json!(["read"]));
    let before = harness.request("GET", "/v1/sessions", None).await.body;
    let refused = computer
        .cli(
            &["--remote", "reader", "run", "--wait", "must not execute"],
            None,
        )
        .await;
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("403"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    let mut reader = computer.mcp("reader").await;
    ok(&reader, "session_list", json!({})).await;
    for (tool, args) in [
        (
            "agent_run",
            json!({"session_id":session,"goal":"must not execute"}),
        ),
        (
            "browser_snapshot",
            json!({"session_id":session,"tab_id":tab}),
        ),
    ] {
        let refused = call(&reader, tool, args).await;
        assert_eq!(refused.is_error, Some(true), "{tool}");
        assert!(text(&refused).contains("403"), "{}", text(&refused));
    }
    reader.close().await.unwrap();
    let after = harness.request("GET", "/v1/sessions", None).await.body;
    let previous_sessions = before["sessions"].as_array().unwrap();
    let current_sessions = after["sessions"].as_array().unwrap();
    assert_eq!(
        previous_sessions.len(),
        current_sessions.len(),
        "refusals created a session"
    );
    for previous in previous_sessions {
        let current = current_sessions
            .iter()
            .find(|entry| entry["id"] == previous["id"])
            .expect("refusals must preserve every existing session");
        let mut cursor = previous["last_seq"].as_u64().unwrap();
        let last = current["last_seq"].as_u64().unwrap();
        assert!(last >= cursor);
        if last > cursor {
            let mut events = harness
                .events(previous["id"].as_str().unwrap(), cursor)
                .await;
            while cursor < last {
                let event = events.next().await;
                // Idle preview updates advance the cursor without accepting a message or tool.
                assert!(
                    matches!(event.kind, openside_proto::EventKind::BrowserState(_)),
                    "read-only commands caused non-preview work: {event:?}"
                );
                cursor = event.seq;
            }
        }
        let mut previous = previous.as_object().unwrap().clone();
        let mut current = current.as_object().unwrap().clone();
        previous.remove("last_seq");
        current.remove("last_seq");
        assert_eq!(previous, current, "refusals changed session state");
    }

    let minted = harness
        .request(
            "POST",
            "/v1/devices/pair",
            Some(json!({"name":"remote admin","scopes":["admin"]})),
        )
        .await;
    assert_eq!(minted.status, 201);
    let admin = minted.body["token"].as_str().unwrap();
    let policy = harness.request("GET", "/v1/policy", None).await.body;
    let config = harness
        .request("GET", "/v1/settings/config", None)
        .await
        .body;
    for (path, body) in [
        ("/v1/policy", json!({"default_mode":"read-only","rules":[]})),
        ("/v1/settings/config", json!({"config":{}})),
    ] {
        let (status, denied) =
            assert_remote_response(&harness, &link, relay, "PUT", path, admin, body).await;
        assert_eq!(
            (status, denied["code"].as_str()),
            (403, Some("local_socket_required")),
            "{path}: {denied}"
        );
    }
    assert_eq!(
        harness.request("GET", "/v1/policy", None).await.body,
        policy
    );
    assert_eq!(
        harness
            .request("GET", "/v1/settings/config", None)
            .await
            .body,
        config
    );
    let mut behavior = config["config"].clone();
    let previous_ttl = behavior["tickets"]["confirm_ttl_secs"].as_u64().unwrap();
    behavior["tickets"]["confirm_ttl_secs"] = json!(previous_ttl + 1);
    let (status, updated) = assert_remote_response(
        &harness,
        &link,
        relay,
        "PUT",
        "/v1/settings/config",
        admin,
        json!({"config":behavior}),
    )
    .await;
    assert_eq!(status, 200, "{updated}");
    assert_eq!(
        updated["config"]["tickets"]["confirm_ttl_secs"],
        previous_ttl + 1
    );
    assert_eq!(
        harness
            .request("GET", "/v1/settings/config", None)
            .await
            .body["config"],
        behavior
    );
    // A local policy change is permitted, but a remote admin cannot widen the new default mode.
    assert_eq!(
        harness
            .request(
                "PUT",
                "/v1/policy",
                Some(json!({"default_mode":"guard","rules":[]}))
            )
            .await
            .status,
        200
    );
    for body in [
        json!({"mode":"full"}),
        json!({"mode":"guard","workspace":harness.root()}),
    ] {
        let (status, denied) = assert_remote_response(
            &harness,
            &link,
            relay,
            "POST",
            "/v1/sessions",
            admin,
            body.clone(),
        )
        .await;
        assert_eq!(
            (status, denied["code"].as_str()),
            (403, Some("local_socket_required")),
            "{denied}"
        );
        assert_eq!(
            harness
                .request("POST", "/v1/sessions", Some(body))
                .await
                .status,
            201
        );
    }

    harness.kill().await;
    site.abort();
    if let Some(server) = relay_server {
        server.abort();
    }
}

#[tokio::test]
async fn paired_relay_cli_and_mcp_run_browse_and_enforce_token_scopes() {
    remote_agent_and_browser(true).await;
}

#[tokio::test]
async fn paired_tcp_cli_and_mcp_run_browse_and_enforce_token_scopes() {
    remote_agent_and_browser(false).await;
}
