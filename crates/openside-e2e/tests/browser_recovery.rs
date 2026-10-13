//! PP-23 ②: Chromium is force-killed in the middle of a session; the real daemon relaunches
//! it on its own, reconciles the tab ledger (`tab.closed` for the tabs that died), the agent's
//! call naming a dead tab fails with `browser_restarted`, and its next browser call works.
//! Real daemon, real Python sidecar with the scripted model, real Chromium, loopback pages.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use axum::{response::Html, routing::get, Router};
use openside_e2e::{Harness, SseReader};
use openside_proto::{Event, EventKind};
use serde_json::{json, Value};

fn tool(name: &str, args: Value, id: &str) -> Value {
    json!({"parts":[{"type":"tool_call","name":name,"args":args,"id":id}]})
}

fn text(text: &str) -> Value {
    json!({"parts":[{"type":"text","text":text}]})
}

fn browser_pid(harness: &Harness) -> i32 {
    let state: Value = serde_json::from_slice(
        &std::fs::read(harness.data_dir.join("browser-state.json")).unwrap(),
    )
    .unwrap();
    i32::try_from(state["endpoint"]["pid"].as_u64().unwrap()).unwrap()
}

fn results(events: &[Event]) -> Vec<(String, Value, bool)> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::ToolEnded {
                tool_call_id,
                result,
                is_error,
                ..
            } => Some((tool_call_id.clone(), result.clone(), *is_error)),
            _ => None,
        })
        .collect()
}

async fn until_event(stream: &mut SseReader, predicate: impl Fn(&EventKind) -> bool) {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if predicate(&stream.next().await.kind) {
                break;
            }
        }
    })
    .await
    .expect("expected event did not arrive");
}

#[tokio::test]
async fn force_killed_browser_recovers_and_the_session_keeps_browsing() {
    let executable = match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(path) => path,
        Err(reason) => {
            eprintln!("SKIP: browser recovery acceptance requires Chromium: {reason}");
            return;
        }
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let app = Router::new()
        .route(
            "/first",
            get(|| async { Html("<title>First</title><h1>First page</h1>") }),
        )
        .route(
            "/second",
            get(|| async { Html("<title>Second</title><h1>Second page</h1>") }),
        );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let first_turn = vec![
        tool(
            "browser_open_tab",
            json!({"url":format!("{origin}/first")}),
            "open-first",
        ),
        text("opened"),
    ];
    let Some(mut harness) = Harness::prepare(
        env!("CARGO_BIN_EXE_e2e-opensided"),
        &json!({"responses": first_turn}),
    ) else {
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
    let messages = format!("/v1/sessions/{session}/messages");
    let mut stream = harness.events(&session, 0).await;
    let sent = harness
        .request(
            "POST",
            &messages,
            Some(json!({"message_id":"first","text":"open the first page","mode":"now"})),
        )
        .await;
    assert_eq!(sent.status, 202, "{}", sent.body);
    let opened = results(&stream.settled().await);
    let (_, result, failed) = opened
        .iter()
        .find(|(id, ..)| id == "open-first")
        .expect("browser_open_tab ran");
    assert!(!failed, "{result}");
    let dead_tab = result["tab_id"].as_str().unwrap().to_owned();

    // Force-kill the whole browser process group mid-session.
    let killed = browser_pid(&harness);
    nix::sys::signal::killpg(
        nix::unistd::Pid::from_raw(killed),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    // The daemon notices on its own (no agent action) and reconciles the ledger.
    until_event(
        &mut stream,
        |kind| matches!(kind, EventKind::TabClosed { tab_id } if tab_id == &dead_tab),
    )
    .await;
    // The ledger is reconciled before the relaunch persists its endpoint.
    let relaunched = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let pid = browser_pid(&harness);
            if pid != killed {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("a new browser replaced the killed one");
    assert!(std::path::Path::new(&format!("/proc/{relaunched}")).exists());

    // The next turn needs the dead tab id, so the script is rewritten and the sidecar is
    // restarted to load it; its cursor continues after the first turn.
    let mut script = json!({"responses": first_turn});
    let responses = script["responses"].as_array_mut().unwrap();
    responses.push(tool(
        "browser_snapshot",
        json!({"tab_id":dead_tab}),
        "stale",
    ));
    responses.push(tool(
        "browser_open_tab",
        json!({"url":format!("{origin}/second")}),
        "reopen",
    ));
    responses.push(tool(
        "browser_repl",
        json!({"code":"await page.snapshot()"}),
        "read",
    ));
    responses.push(text("recovered"));
    harness.replace_script(&script);
    harness.kill_sidecar();
    let boundary = stream.settled().await;
    assert!(boundary.iter().any(
        |event| matches!(&event.kind, EventKind::RuntimeError { code, .. } if code == "interrupted")
    ));
    let sent = harness
        .request(
            "POST",
            &messages,
            Some(json!({"message_id":"second","text":"keep going","mode":"now"})),
        )
        .await;
    assert_eq!(sent.status, 202, "{}", sent.body);
    let events = stream.settled().await;
    let second = results(&events);
    let find = |id: &str| {
        second
            .iter()
            .find(|(call, ..)| call == id)
            .unwrap_or_else(|| panic!("{id} did not run: {events:?}"))
            .clone()
    };
    let (_, stale, stale_failed) = find("stale");
    assert!(stale_failed, "a dead tab must not be usable: {stale}");
    assert!(
        stale.to_string().contains("browser_restarted"),
        "the agent is told the browser restarted: {stale}"
    );
    let (_, reopened, reopen_failed) = find("reopen");
    assert!(!reopen_failed, "{reopened}");
    let (_, read, read_failed) = find("read");
    assert!(!read_failed, "{read}");
    assert!(read.to_string().contains("Second page"), "{read}");
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        EventKind::AssistantFinal { text, .. } if text == "recovered"
    )));
    assert_eq!(
        browser_pid(&harness),
        relaunched,
        "no further restart was needed"
    );

    harness.kill().await;
    server.abort();
}
