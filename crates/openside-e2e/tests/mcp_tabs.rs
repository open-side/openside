//! B-7 ④: a tab opened by an external agent through `openside mcp` (the same public
//! `POST /v1/sessions/{id}/repl` path the CLI uses) is an agent tab of that session. On the
//! patched browser (`OPENSIDE_PATCHED_BROWSER`) it sits in the browser's "Agent Tabs" tab
//! group, as reported by `browser_tabs`; stock Chromium has no tab groups over CDP, so there
//! `group` is null and the session-scoped ownership ledger is the grouping.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::Stdio;

use axum::{response::Html, routing::get, Router};
use openside_e2e::Harness;
use openside_proto::EventKind;
use rmcp::{
    model::{CallToolRequestParams, CallToolResult},
    service::RunningService,
    transport::TokioChildProcess,
    RoleClient, ServiceExt,
};
use serde_json::{json, Value};

const DAEMON: &str = env!("CARGO_BIN_EXE_e2e-opensided");
const CLI: &str = env!("CARGO_BIN_EXE_e2e-openside");

fn body(result: &CallToolResult) -> String {
    serde_json::to_value(&result.content[0]).unwrap()["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn call(
    mcp: &RunningService<RoleClient, ()>,
    name: &str,
    arguments: Value,
) -> CallToolResult {
    let params = CallToolRequestParams::new(name.to_owned())
        .with_arguments(arguments.as_object().cloned().unwrap_or_default());
    mcp.call_tool(params)
        .await
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}

async fn ok(mcp: &RunningService<RoleClient, ()>, name: &str, arguments: Value) -> Value {
    let result = call(mcp, name, arguments).await;
    assert_ne!(result.is_error, Some(true), "{name}: {}", body(&result));
    serde_json::from_str(&body(&result)).unwrap_or_else(|_| Value::String(body(&result)))
}

#[tokio::test]
async fn tabs_opened_by_an_external_mcp_agent_join_the_agent_tabs_group() {
    let (executable, patched) = match std::env::var_os("OPENSIDE_PATCHED_BROWSER") {
        Some(path) if std::path::Path::new(&path).is_file() => (path.into(), true),
        _ => match openside_config::discover_browser(&openside_config::Config::default()) {
            Ok(path) => {
                eprintln!(
                    "note: OPENSIDE_PATCHED_BROWSER unset; checking the stock-Chromium ledger fallback only"
                );
                (path, false)
            }
            Err(reason) => {
                eprintln!("SKIP: MCP Agent Tabs acceptance requires Chromium: {reason}");
                return;
            }
        },
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let app = Router::new().route(
        "/",
        get(|| async { Html("<title>External</title><h1>Opened by an external agent</h1>") }),
    );
    let site = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let script = json!({"responses":[
        {"parts":[{"type":"tool_call","name":"browser_tabs","args":{},"id":"list"}]},
        {"parts":[{"type":"text","text":"listed"}]},
    ]});
    let Some(mut harness) = Harness::prepare(DAEMON, &script) else {
        site.abort();
        return;
    };
    harness.append_config(&format!("[browser]\nexecutable = {}\n", json!(executable)));
    harness.launch().await;
    let port = std::fs::read_to_string(harness.data_dir.join("opensided.port")).unwrap();
    let output = harness
        .cli(
            CLI,
            &[
                "token", "create", "--scope", "read", "--scope", "steer", "--scope", "browse",
            ],
            None,
        )
        .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let token = String::from_utf8(output.stdout)
        .unwrap()
        .trim_end()
        .split('\t')
        .nth(1)
        .unwrap()
        .to_owned();
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
        .env("OPENSIDE_TOKEN", &token)
        .args(["--url", &format!("http://127.0.0.1:{}", port.trim()), "mcp"])
        .kill_on_drop(true);
    let (transport, _) = TokioChildProcess::builder(command)
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mcp = ().serve(transport).await.unwrap();

    let session = ok(&mcp, "session_create", json!({"mode":"full"})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let opened = ok(
        &mcp,
        "browser_open_tab",
        json!({"session_id":session,"url":format!("{origin}/")}),
    )
    .await;
    let tab = opened["result"]["tab_id"].as_str().unwrap().to_owned();

    // Another session of the same external agent cannot drive this session's tab.
    let other = ok(&mcp, "session_create", json!({"mode":"full"})).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let foreign = call(
        &mcp,
        "browser_snapshot",
        json!({"session_id":other,"tab_id":tab}),
    )
    .await;
    assert_eq!(foreign.is_error, Some(true), "{}", body(&foreign));

    // The session's own agent sees the MCP-opened tab as one of its tabs, in the group.
    let mut stream = harness.events(&session, 0).await;
    ok(
        &mcp,
        "session_send",
        json!({"session_id":session,"text":"list your tabs"}),
    )
    .await;
    let events = stream.settled().await;
    let listed = events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::ToolEnded {
                tool_call_id,
                result,
                is_error: false,
                ..
            } if tool_call_id == "list" => Some(result.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("browser_tabs did not complete: {events:?}"));
    let tabs: Value = match &listed {
        Value::String(text) => serde_json::from_str(text).unwrap(),
        value => value.clone(),
    };
    let entry = tabs
        .as_array()
        .unwrap_or_else(|| panic!("tab list: {tabs}"))
        .iter()
        .find(|entry| entry["tab_id"] == tab)
        .unwrap_or_else(|| panic!("the MCP-opened tab is the session's agent tab: {tabs}"));
    if patched {
        assert_eq!(entry["group"], "Agent Tabs", "{tabs}");
    } else {
        assert_eq!(entry["group"], Value::Null, "{tabs}");
    }

    let _ = mcp.cancel().await;
    harness.kill().await;
    site.abort();
}
