//! Responsiveness measurements (ignored; run on demand): what a person waits for while a long
//! task runs. Five experiments on the scripted model and real Chromium, each printing one
//! `RESPONSIVE {json}` line with client-observed milliseconds.
//!
//!   flock --close /tmp/openside-heavy.lock nice -n 10 cargo test -j 4 -p openside-e2e --test responsive \
//!     -- --ignored --nocapture --test-threads=2
//!
//! `OPENSIDE_RESPONSIVE_MODE=single` scripts the single agent (long tools run in the
//! conversation itself); the default scripts the front agent with background tasks.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{extract::RawQuery, response::Html, routing::get, Router};
use openside_e2e::Harness;
use openside_proto::{Event, EventKind};
use parking_lot::Mutex;
use serde_json::{json, Value};

const DAEMON: &str = env!("CARGO_BIN_EXE_e2e-opensided");
const LONG_MS: u64 = 15_000;

fn single() -> bool {
    std::env::var("OPENSIDE_RESPONSIVE_MODE").as_deref() == Ok("single")
}

fn text(text: &str) -> Value {
    json!({"parts":[{"type":"text","text":text}]})
}
fn tool(name: &str, args: Value, call: &str) -> Value {
    json!({"parts":[{"type":"tool_call","name":name,"args":args,"id":call}]})
}
fn sleep_call(call: &str) -> Value {
    tool(
        "bash",
        json!({"command": format!("sleep {}", LONG_MS / 1000)}),
        call,
    )
}

struct Site {
    origin: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Site {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn site() -> Site {
    let app = Router::new().route(
        "/page",
        get(|RawQuery(query): RawQuery| async move {
            let delay = query
                .as_deref()
                .and_then(|q| q.strip_prefix("ms="))
                .and_then(|ms| ms.parse().ok());
            if let Some(ms) = delay {
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
            Html("<!doctype html><title>Fixture</title><h1>Fixture</h1><p>Ready</p>")
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Site { origin, task }
}

#[derive(Default)]
struct Log(
    Arc<Mutex<Vec<(Instant, Event)>>>,
    Option<tokio::task::AbortHandle>,
);
impl Log {
    async fn record(harness: &Harness, session: &str, after: u64) -> Self {
        let mut log = Self::default();
        let mut stream = harness.events(session, after).await;
        let sink = Arc::clone(&log.0);
        let task = tokio::spawn(async move {
            loop {
                let event = stream.next().await;
                sink.lock().push((Instant::now(), event));
            }
        });
        log.1 = Some(task.abort_handle());
        log
    }
    /// First event at or after `from` matching `want`, as milliseconds since `from`.
    fn first(&self, from: Instant, want: impl Fn(&EventKind) -> bool) -> Option<u128> {
        self.0
            .lock()
            .iter()
            .find(|(at, event)| *at >= from && want(&event.kind))
            .map(|(at, _)| at.duration_since(from).as_millis())
    }
    async fn wait(
        &self,
        from: Instant,
        limit: Duration,
        want: impl Fn(&EventKind) -> bool,
    ) -> Option<u128> {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(ms) = self.first(from, &want) {
                return Some(ms);
            }
            if Instant::now() > deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    fn kinds(&self) -> Vec<String> {
        self.0
            .lock()
            .iter()
            .map(|(_, event)| {
                serde_json::to_value(&event.kind).unwrap()["kind"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }
    fn last_seq(&self) -> u64 {
        self.0.lock().last().map_or(0, |(_, e)| e.seq)
    }
    fn stop(&self) {
        if let Some(task) = &self.1 {
            task.abort();
        }
    }
}
impl Drop for Log {
    fn drop(&mut self) {
        self.stop();
    }
}

fn delta(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::AssistantDelta { .. } | EventKind::AssistantFinal { .. }
    )
}
fn final_with(expected: &'static str) -> impl Fn(&EventKind) -> bool {
    move |kind| matches!(kind, EventKind::AssistantFinal { text } if text.contains(expected))
}
fn tool_started(name: &'static str) -> impl Fn(&EventKind) -> bool {
    move |kind| matches!(kind, EventKind::ToolStarted { name: n, .. } if n == name)
}
fn settled(kind: &EventKind) -> bool {
    matches!(kind, EventKind::SessionSettled {})
}

async fn start(script: &Value) -> Option<(Harness, Site)> {
    let browser = match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(path) => path,
        Err(reason) => {
            eprintln!("SKIP: responsiveness needs Chromium: {reason}");
            return None;
        }
    };
    let site = site().await;
    let mut harness = Harness::prepare(DAEMON, script)?;
    harness.append_config(&format!("[browser]\nexecutable = {}\n", json!(browser)));
    if !single() {
        let config = harness.root().join("config/openside/config.toml");
        let text = std::fs::read_to_string(&config).unwrap();
        std::fs::write(
            &config,
            text.replacen("orchestration = \"single\"", "orchestration = \"front\"", 1),
        )
        .unwrap();
    }
    harness.launch().await;
    Some((harness, site))
}

async fn session(harness: &Harness) -> String {
    let created = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"mode":"full","workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(created.status, 201, "{}", created.body);
    let id = created.body["id"].as_str().unwrap().to_owned();
    if prewarm() {
        let response = harness
            .request("POST", &format!("/v1/sessions/{id}/prewarm"), None)
            .await;
        assert_eq!(response.status, 202, "{}", response.body);
        let mut stream = harness.events(&id, 0).await;
        loop {
            if matches!(stream.next().await.kind, EventKind::RuntimeStarted { .. }) {
                break;
            }
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let tabs = harness.request("GET", "/v1/browser/tabs", None).await;
            if tabs.body["tabs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tab| tab["owner_session_id"] == id && tab["url"] == "about:blank")
            {
                break;
            }
            assert!(Instant::now() < deadline, "prewarm tab deadline");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    id
}

/// Submit as the web composer does (`queue`); returns the client-side send instant.
async fn say(harness: &Harness, session: &str, id: &str, words: &str, mode: &str) -> Instant {
    let at = Instant::now();
    let response = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            Some(json!({"message_id":id,"text":words,"mode":mode})),
        )
        .await;
    assert_eq!(response.status, 202, "{}", response.body);
    at
}

fn report(name: &str, value: Value) {
    eprintln!(
        "RESPONSIVE {}",
        json!({"experiment": name, "mode": if single() {"single"} else {"front"}, "data": value})
    );
}

/// Two rounds of "open the page". Front mode: the session script is the front agent's
/// turns; each task runs the `subagents[<task_start call>]` turns.
fn script_cold(url: &str) -> Value {
    let open = |call: &str| tool("browser_open_tab", json!({"url":url}), call);
    if single() {
        json!({"responses":[open("open-1"), text("opened"), open("open-2"), text("opened")]})
    } else {
        json!({"responses":[
            tool("task_start", json!({"goal":"open the page"}), "task-1"),
            text("Started."),
            text("Opened the page."),
            tool("task_start", json!({"goal":"open the page"}), "task-2"),
            text("Started."),
            text("Opened the page."),
        ], "subagents": {
            "task-1": [open("open-1"), text("opened")],
            "task-2": [open("open-2"), text("opened")],
        }})
    }
}

#[tokio::test]
#[ignore = "measurement: run on demand"]
async fn cold_and_warm_first_browser_action() {
    let Some((mut harness, site)) = start(&json!({"responses":[]})).await else {
        return;
    };
    let url = format!("{}/page", site.origin);
    harness.replace_script(&script_cold(&url));
    let mut rows = Vec::new();
    let mut current = String::new();
    let mut log = Log::default();
    let mut created_at = Instant::now();
    for (label, round) in [
        ("cold", 1),
        ("warm_browser_new_session", 1),
        ("warm_sidecar_same_session", 2),
    ] {
        let mut create_ms = None;
        if round == 1 {
            created_at = Instant::now();
            current = session(&harness).await;
            create_ms = Some(created_at.elapsed().as_millis());
            log = Log::record(&harness, &current, 0).await;
        }
        let sent = say(
            &harness,
            &current,
            &format!("m-{label}"),
            "open the page",
            "queue",
        )
        .await;
        let call = format!("open-{round}");
        let open_ended = {
            let call = call.clone();
            log.wait(sent, Duration::from_secs(60), move |k| matches!(k, EventKind::ToolEnded { tool_call_id, .. } if *tool_call_id == call)).await
        };
        log.wait(sent, Duration::from_secs(60), settled).await;
        rows.push(json!({
            "label": label,
            "session_create_ms": create_ms,
            "first_sse_event": log.first(sent, |_| true),
            "runtime_started": log.first(sent, |k| matches!(k, EventKind::RuntimeStarted { .. })),
            "startup_ms": log.0.lock().iter().find_map(|(at, e)| match &e.kind { EventKind::RuntimeStarted { startup_ms, .. } if *at >= sent => Some(*startup_ms), _ => None }),
            "first_turn_started": log.first(sent, |k| matches!(k, EventKind::TurnStarted {})),
            "first_assistant_delta": log.first(sent, delta),
            "browser_open_started": log.first(sent, tool_started("browser_open_tab")),
            "browser_open_ended": open_ended,
            "settled": log.first(sent, settled),
            "first_browser_state": log.first(sent, |k| matches!(k, EventKind::BrowserState(_))),
            "prewarmed": prewarm(),
            "send_after_session_create_ms": sent.duration_since(created_at).as_millis(),
            "session_create_to_tab_ready_ms": open_ended.map(|ms| sent.duration_since(created_at).as_millis() + ms),
            "session_create_to_first_token_ms": log.first(sent, delta).map(|ms| sent.duration_since(created_at).as_millis() + ms),
        }));
    }
    report("cold_warm", json!(rows));
    log.stop();
    harness.kill().await;
}

fn prewarm() -> bool {
    std::env::var("OPENSIDE_RESPONSIVE_PREWARM").is_ok_and(|value| value == "1")
}

/// Front mode: the front agent starts one Task; `steer` routes the person's correction to it.
/// Both agents `echo`, so the final text proves which instruction reached the Task.
fn script_long(final_text: &str, steer: bool) -> Value {
    if single() {
        json!({"responses":[sleep_call("long-1"), text(final_text), text("quick answer: 4"), text("[unused]")]})
    } else if steer {
        json!({"responses":[
            tool("task_start", json!({"goal":"wait then answer"}), "task-1"),
            text("Started the long task."),
            tool("task_steer", json!({"task_id":"{{result:task-1.task_id}}","message":"change the final answer to BETA"}), "steer-1"),
            text("Steered the task."),
            {"parts":[{"type":"echo"}]},
        ], "subagents": {"task-1": [sleep_call("long-1"), {"parts":[{"type":"echo"}]}]}})
    } else {
        json!({"responses":[
            tool("task_start", json!({"goal":"wait then answer"}), "task-1"),
            text("Started the long task."),
            text("quick answer: 4"),
            {"parts":[{"type":"echo"}]},
        ], "subagents": {"task-1": [sleep_call("long-1"), text(final_text)]}})
    }
}

#[tokio::test]
#[ignore = "measurement: run on demand"]
async fn e1_steering_reaches_the_running_task() {
    let Some((mut harness, _site)) = start(&script_long("ALPHA", true)).await else {
        return;
    };
    let id = session(&harness).await;
    let log = Log::record(&harness, &id, 0).await;
    let sent = say(
        &harness,
        &id,
        "m1",
        "wait then answer exactly ALPHA",
        "queue",
    )
    .await;
    log.wait(sent, Duration::from_secs(30), tool_started("bash"))
        .await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let steer = say(
        &harness,
        &id,
        "m2",
        "change the final answer to BETA",
        "queue",
    )
    .await;
    log.wait(sent, Duration::from_secs(90), settled).await;
    report(
        "e1_steering",
        json!({
            "first_sse_event": log.first(sent, |_| true),
            "first_assistant_delta": log.first(sent, delta),
            "first_tool_started": log.first(sent, |k| matches!(k, EventKind::ToolStarted { .. })),
            "steer_injected": log.first(steer, |k| matches!(k, EventKind::MessageInjected { message_id } if message_id.as_str() == "m2")),
            "steer_first_reply": log.first(steer, delta),
            "steer_accepted_by_task": log.first(steer, |k| matches!(k, EventKind::ToolEnded { tool_call_id, .. } if tool_call_id == "steer-1")),
            "steer_reached_task": log.first(steer, |k| if single() { matches!(k, EventKind::TurnStarted {}) } else { matches!(k, EventKind::SubagentProgress { progress, .. } if progress["kind"] == "steer_injected") }),
            "completion": log.first(sent, final_with("ALPHA")).or(log.first(sent, final_with("BETA"))),
            "settled": log.first(sent, settled),
            "kinds": log.kinds(),
        }),
    );
    log.stop();
    harness.kill().await;
}

#[tokio::test]
#[ignore = "measurement: run on demand"]
async fn e2_quick_question_while_a_long_task_runs() {
    let Some((mut harness, _site)) = start(&script_long("task done", false)).await else {
        return;
    };
    let id = session(&harness).await;
    let log = Log::record(&harness, &id, 0).await;
    let sent = say(&harness, &id, "m1", "long task", "queue").await;
    log.wait(sent, Duration::from_secs(30), tool_started("bash"))
        .await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let asked = say(&harness, &id, "q1", "what is 2+2?", "queue").await;
    let reply = log.wait(asked, Duration::from_secs(60), |k| matches!(k, EventKind::AssistantDelta { text } if text.contains("quick") || text.contains("task")) ).await;
    log.wait(sent, Duration::from_secs(90), settled).await;
    report(
        "e2_quick_question",
        json!({
            "question_queued": log.first(asked, |k| matches!(k, EventKind::MessageQueued { message_id, .. } if message_id.as_str() == "q1")),
            "question_injected": log.first(asked, |k| matches!(k, EventKind::MessageInjected { message_id } if message_id.as_str() == "q1")),
            "question_first_reply_delta": reply,
            "task_completion": log.first(sent, |k| matches!(k, EventKind::ToolEnded { tool_call_id, .. } if tool_call_id == "long-1")),
            "settled": log.first(sent, settled),
        }),
    );
    log.stop();
    harness.kill().await;
}

#[tokio::test]
#[ignore = "measurement: run on demand"]
async fn e3_cancel_only_the_task() {
    let Some((mut harness, _site)) = start(&script_long("task done", false)).await else {
        return;
    };
    let id = session(&harness).await;
    let log = Log::record(&harness, &id, 0).await;
    let sent = say(&harness, &id, "m1", "long task", "queue").await;
    log.wait(sent, Duration::from_secs(30), tool_started("bash"))
        .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let cancelled = Instant::now();
    let (route, status) = if single() {
        let r = harness
            .request("POST", &format!("/v1/sessions/{id}/stop"), None)
            .await;
        ("stop".to_owned(), r.status)
    } else {
        let list = harness
            .request("GET", &format!("/v1/sessions/{id}/subagents"), None)
            .await;
        let sid = list.body["subagents"][0]["id"].as_str().unwrap().to_owned();
        let r = harness
            .request(
                "POST",
                &format!("/v1/sessions/{id}/subagents/{sid}/cancel"),
                None,
            )
            .await;
        ("subagents/{sid}/cancel".to_owned(), r.status)
    };
    let ended = log
        .wait(cancelled, Duration::from_secs(30), |k| {
            matches!(
                k,
                EventKind::SessionStopped {} | EventKind::SubagentEnded { .. }
            ) || matches!(k, EventKind::ToolEnded { tool_call_id, .. } if tool_call_id == "long-1")
        })
        .await;
    // Can the conversation continue right away?
    let asked = say(&harness, &id, "q1", "still there?", "queue").await;
    let reply = log.wait(asked, Duration::from_secs(30), delta).await;
    report(
        "e3_cancel_task",
        json!({
            "route": route, "status": status,
            "cancel_observed": ended,
            "session_stopped": log.first(cancelled, |k| matches!(k, EventKind::SessionStopped {})),
            "next_message_reply": reply,
            "kinds_after_cancel": log.kinds(),
        }),
    );
    log.stop();
    harness.kill().await;
}

#[tokio::test]
#[ignore = "measurement: run on demand"]
async fn e4_reconnect_mid_task_keeps_state() {
    let Some((mut harness, _site)) = start(&script_long("task done", false)).await else {
        return;
    };
    let id = session(&harness).await;
    let log = Log::record(&harness, &id, 0).await;
    let sent = say(&harness, &id, "m1", "long task", "queue").await;
    log.wait(sent, Duration::from_secs(30), tool_started("bash"))
        .await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    // A reloaded page replays from zero; a reconnecting one resumes from its last id.
    let reloaded_at = Instant::now();
    let reload = Log::record(&harness, &id, 0).await;
    let replayed = reload
        .wait(reloaded_at, Duration::from_secs(5), tool_started("bash"))
        .await;
    let resume_seq = log.last_seq();
    let resumed_at = Instant::now();
    let resumed = Log::record(&harness, &id, resume_seq).await;
    log.wait(sent, Duration::from_secs(90), settled).await;
    resumed
        .wait(resumed_at, Duration::from_secs(5), settled)
        .await;
    report(
        "e4_reconnect",
        json!({
            "reload_first_event": reload.first(reloaded_at, |_| true),
            "reload_sees_running_tool": replayed,
            "reload_events_replayed": reload.kinds().len(),
            "resume_after_seq": resume_seq,
            "resume_first_event": resumed.first(resumed_at, |_| true),
            "resume_sees_completion": resumed.first(resumed_at, settled),
            "no_duplicates": resumed.0.lock().iter().all(|(_, e)| e.seq > resume_seq),
        }),
    );
    log.stop();
    reload.stop();
    resumed.stop();
    harness.kill().await;
}

fn script_two() -> Value {
    if single() {
        json!({"responses":[sleep_call("long-1"), sleep_call("long-2"), text("both done"), text("[unused]")]})
    } else {
        json!({"responses":[
            tool("task_start", json!({"goal":"first"}), "task-1"),
            text("Started the first task."),
            tool("task_start", json!({"goal":"second"}), "task-2"),
            text("Started the second task."),
            text("First finished."),
            text("Second finished."),
        ], "subagents": {
            "task-1": [sleep_call("long-1"), text("first done")],
            "task-2": [sleep_call("long-2"), text("second done")],
        }})
    }
}

#[tokio::test]
#[ignore = "measurement: run on demand"]
async fn e5_two_parallel_tasks() {
    let Some((mut harness, _site)) = start(&script_two()).await else {
        return;
    };
    let id = session(&harness).await;
    let log = Log::record(&harness, &id, 0).await;
    let sent = say(&harness, &id, "m1", "first long task", "queue").await;
    log.wait(sent, Duration::from_secs(30), tool_started("bash"))
        .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let second = say(&harness, &id, "m2", "second long task", "queue").await;
    log.wait(sent, Duration::from_secs(120), settled).await;
    let ended = |call: &'static str| {
        log.first(
            sent,
            move |k| matches!(k, EventKind::ToolEnded { tool_call_id, .. } if tool_call_id == call),
        )
    };
    report(
        "e5_parallel",
        json!({
            "second_sent_at": second.duration_since(sent).as_millis(),
            "second_first_reply": log.first(second, delta),
            "first_task_tool_ended": ended("long-1"),
            "second_task_tool_started": log.first(second, |k| matches!(k, EventKind::ToolStarted { tool_call_id, .. } if tool_call_id == "long-2")),
            "second_task_tool_ended": ended("long-2"),
            "settled": log.first(sent, settled),
        }),
    );
    log.stop();
    harness.kill().await;
}
