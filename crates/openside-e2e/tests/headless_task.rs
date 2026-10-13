//! PP-3: on a server without a display, a task that uses the browser runs to completion. The
//! real daemon starts with a cleared environment (no `DISPLAY`/`WAYLAND_DISPLAY`), the real
//! Python sidecar runs the scripted model, and the model drives real headless Chromium on a
//! loopback page through `browser_repl` before answering.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use axum::{response::Html, routing::get, Router};
use openside_e2e::Harness;
use openside_proto::EventKind;
use serde_json::{json, Value};

const PAGE: &str = "<!doctype html><title>Headless</title><h1>Rendered without a display</h1>";

#[tokio::test]
async fn displayless_task_drives_headless_chromium_to_a_settled_answer() {
    let executable = match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(path) => path,
        Err(reason) => {
            eprintln!("SKIP: headless task acceptance requires Chromium: {reason}");
            return;
        }
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://127.0.0.1:{}/",
        listener.local_addr().unwrap().port()
    );
    let app = Router::new().route("/", get(|| async { Html(PAGE) }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let code = format!("await page.goto({}); await page.snapshot()", json!(url));
    let script = json!({"responses":[
        {"parts":[{"type":"tool_call","name":"browser_repl","args":{"code":code},"id":"look"}]},
        {"parts":[{"type":"text","text":"The page says: Rendered without a display"}]},
    ]});
    let Some(mut harness) = Harness::prepare(env!("CARGO_BIN_EXE_e2e-opensided"), &script) else {
        server.abort();
        return;
    };
    harness.append_config(&format!("[browser]\nexecutable = {}\n", json!(executable)));
    harness.launch().await;

    let created = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"mode":"full","workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(created.status, 201, "{}", created.body);
    let session = created.body["id"].as_str().unwrap().to_owned();
    let mut stream = harness.events(&session, 0).await;
    let submitted = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            Some(json!({"message_id":"headless","text":"What does the page say?","mode":"now"})),
        )
        .await;
    assert_eq!(submitted.status, 202, "{}", submitted.body);
    let events = stream.settled().await;

    let result = events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::ToolEnded {
                result, is_error, ..
            } => Some((result.clone(), *is_error)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no browser tool result: {events:?}"));
    assert!(!result.1, "browser_repl failed: {}", result.0);
    assert!(
        result.0.to_string().contains("Rendered without a display"),
        "the snapshot came from the loopback page: {}",
        result.0
    );
    let answer = events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::AssistantFinal { text, .. } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no final answer: {events:?}"));
    assert_eq!(answer, "The page says: Rendered without a display");

    // The browser that served the task was headless and had no display in its environment.
    let state: Value = serde_json::from_slice(
        &std::fs::read(harness.data_dir.join("browser-state.json")).unwrap(),
    )
    .unwrap();
    let pid = state["endpoint"]["pid"].as_u64().unwrap();
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap();
    let args: Vec<&[u8]> = cmdline.split(|byte| *byte == 0).collect();
    assert!(args.contains(&b"--headless=new".as_slice()));
    let environ = std::fs::read(format!("/proc/{pid}/environ")).unwrap();
    for variable in environ.split(|byte| *byte == 0) {
        assert!(
            !variable.starts_with(b"DISPLAY=") && !variable.starts_with(b"WAYLAND_DISPLAY="),
            "the browser must not see a display"
        );
    }

    harness.kill().await;
    server.abort();
}
