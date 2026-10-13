//! Front/Task acceptance against the real daemon, Python sidecar and headless Chromium.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::{Duration, Instant};

use axum::{
    http::{header, HeaderMap},
    response::Html,
    routing::get,
    Router,
};
use openside_e2e::{Harness, SseReader};
use openside_proto::{Event, EventKind, SubagentStatus};
use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_e2e-opensided");

fn text(text: &str) -> Value {
    json!({"parts":[{"type":"text","text":text}]})
}
fn echo() -> Value {
    json!({"parts":[{"type":"echo"}]})
}
fn tool(name: &str, args: Value, call: &str) -> Value {
    json!({"parts":[{"type":"tool_call","name":name,"args":args,"id":call}]})
}
fn start(call: &str, goal: &str) -> Value {
    tool("task_start", json!({"goal":goal}), call)
}
fn blocked(call: &str, marker: &str) -> Value {
    tool(
        "bash",
        json!({"command":format!("timeout 25 sh -c 'while [ ! -f {marker} ]; do sleep 0.05; done'")}),
        call,
    )
}
async fn harness(script: &Value, browser: bool) -> Harness {
    let harness = Harness::prepare(BIN, script).expect("uv sync --project agent is required");
    let config = harness.root().join("config/openside/config.toml");
    let original = std::fs::read_to_string(&config).unwrap();
    // Omitting orchestration exercises the production default, including the session-model fallback.
    std::fs::write(config, original.replace("orchestration = \"single\"\n", "")).unwrap();
    if browser {
        let executable = openside_config::discover_browser(&openside_config::Config::default())
            .expect("Chromium is required for Task browser acceptance");
        harness.append_config(&format!("[browser]\nexecutable = {}\n", json!(executable)));
    }
    let mut harness = harness;
    harness.launch().await;
    harness
}
async fn create(harness: &Harness) -> String {
    let response = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"mode":"full","workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(response.status, 201, "{}", response.body);
    response.body["id"].as_str().unwrap().to_owned()
}
async fn say(harness: &Harness, id: &str, message: &str) {
    let response = harness
        .request(
            "POST",
            &format!("/v1/sessions/{id}/messages"),
            Some(json!({"message_id":message,"text":message,"mode":"queue"})),
        )
        .await;
    assert_eq!(response.status, 202, "{}", response.body);
}
async fn until(stream: &mut SseReader, want: impl Fn(&EventKind) -> bool) -> Vec<Event> {
    let mut events = Vec::new();
    loop {
        let event = stream.next().await;
        let done = want(&event.kind);
        events.push(event);
        if done {
            return events;
        }
    }
}
fn final_is(kind: &EventKind, expected: &str) -> bool {
    matches!(kind, EventKind::AssistantFinal { text } if text == expected)
}
fn tool_started(kind: &EventKind, call: &str) -> bool {
    matches!(kind, EventKind::ToolStarted { tool_call_id, .. } if tool_call_id == call)
}
fn task_id(events: &[Event], call: &str) -> String {
    events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::SubagentStarted {
                subagent_id,
                parent_tool_call_id,
                background: true,
                model,
                ..
            } if parent_tool_call_id == call => {
                assert_eq!(model, "script", "Tasks use the session model");
                Some(subagent_id.clone())
            }
            _ => None,
        })
        .expect("background task started")
}
fn release(harness: &Harness, marker: &str) {
    std::fs::write(harness.workspace().join(marker), "ready").unwrap();
}

fn durable_events(harness: &Harness, session: &str) -> Vec<Event> {
    let store = openside_sessions::Store::open(harness.data_dir.join("events.db")).unwrap();
    store
        .events_after(&openside_proto::SessionId::new(session), 0, usize::MAX)
        .unwrap()
}

#[tokio::test]
async fn front_replies_before_task_ends_and_status_arrives_within_300ms() {
    let script = json!({"responses":[start("task-1", "long work"), text("Started"), text("4"), echo()],
        "subagents":{"task-1":[blocked("work-1", "release-1"), text("finished")]}});
    let mut harness = harness(&script, false).await;
    let id = create(&harness).await;
    let mut stream = harness.events(&id, 0).await;
    until(&mut stream, |kind| {
        matches!(kind, EventKind::SessionCreated { .. })
    })
    .await;
    let sent = Instant::now();
    let (_, status_ms) = tokio::join!(say(&harness, &id, "start"), async {
        until(&mut stream, |kind| matches!(kind, EventKind::MessageQueued { message_id, .. } if message_id.as_str() == "start")).await;
        sent.elapsed()
    });
    assert!(
        status_ms <= Duration::from_millis(300),
        "status took {status_ms:?}"
    );
    let mut seen = until(&mut stream, |kind| tool_started(kind, "work-1")).await;
    if !seen.iter().any(|e| final_is(&e.kind, "Started")) {
        seen.extend(until(&mut stream, |kind| final_is(kind, "Started")).await);
    }
    say(&harness, &id, "quick question").await;
    seen.extend(until(&mut stream, |kind| final_is(kind, "4")).await);
    assert!(
        !seen.iter().any(|e| matches!(
            e.kind,
            EventKind::SubagentEnded { .. } | EventKind::SessionSettled {}
        )),
        "front must reply while the Task remains running"
    );
    let task = task_id(&seen, "task-1");
    release(&harness, "release-1");
    let ended = stream.settled().await;
    assert!(ended.iter().any(|e| matches!(&e.kind, EventKind::SubagentEnded { subagent_id, status: SubagentStatus::Completed, output: Some(output), .. } if subagent_id == &task && output == "finished")));
    harness.kill().await;
}

#[tokio::test]
async fn steer_reaches_only_the_addressed_task_and_cancel_leaves_the_other_running() {
    let script = json!({"responses":[
        {"parts":[{"type":"tool_call","name":"task_start","args":{"goal":"first"},"id":"task-1"},{"type":"tool_call","name":"task_start","args":{"goal":"second"},"id":"task-2"}]},
        text("Started both"),
        tool("task_steer",json!({"task_id":"{{result:task-2.task_id}}","message":"answer BETA"}),"steer"), text("Steered"),
        tool("task_cancel",json!({"task_id":"{{result:task-1.task_id}}"}),"cancel"),
        tool("task_status",json!({}),"status"), text("Cancelled first"), echo()],
        "subagents":{"task-1":[blocked("work-1","release-1"), echo()],"task-2":[blocked("work-2","release-2"), echo()]}});
    let mut harness = harness(&script, false).await;
    let id = create(&harness).await;
    let mut stream = harness.events(&id, 0).await;
    say(&harness, &id, "start two").await;
    let mut seen = until(&mut stream, |k| final_is(k, "Started both")).await;
    while !["work-1", "work-2"]
        .iter()
        .all(|call| seen.iter().any(|e| tool_started(&e.kind, call)))
    {
        seen.push(stream.next().await);
    }
    let first = task_id(&seen, "task-1");
    let second = task_id(&seen, "task-2");
    assert_ne!(first, second);
    say(&harness, &id, "steer second").await;
    until(&mut stream, |k| final_is(k, "Steered")).await;
    say(&harness, &id, "cancel first").await;
    until(&mut stream, |k| final_is(k, "Cancelled first")).await;
    let tasks = harness
        .request("GET", &format!("/v1/sessions/{id}/subagents"), None)
        .await;
    let listed = tasks.body["subagents"].as_array().unwrap();
    assert_eq!(
        listed.iter().find(|t| t["id"] == first).unwrap()["status"],
        "cancelled"
    );
    assert_eq!(
        listed.iter().find(|t| t["id"] == second).unwrap()["status"],
        "running"
    );
    release(&harness, "release-2");
    let events = stream.settled().await;
    assert!(events.iter().any(|e| matches!(&e.kind, EventKind::SubagentProgress { subagent_id, progress } if subagent_id == &second && progress["kind"] == "steer_injected")));
    assert!(!events.iter().any(|e| matches!(&e.kind, EventKind::SubagentProgress { subagent_id, progress } if subagent_id == &first && progress["kind"] == "steer_injected")));
    assert!(events.iter().any(|e| matches!(&e.kind, EventKind::SubagentEnded { subagent_id, status: SubagentStatus::Completed, output: Some(output), .. } if subagent_id == &second && output == "answer BETA")));
    harness.kill().await;
}

#[tokio::test]
async fn task_report_is_delivered_exactly_once_across_daemon_crash_and_restart() {
    let script = json!({"responses":[start("task-1","durable work"),text("Started"),echo(),text("still here")],
        "subagents":{"task-1":[blocked("work","release"),text("durable result")]}});
    let mut harness = harness(&script, false).await;
    let id = create(&harness).await;
    let mut stream = harness.events(&id, 0).await;
    say(&harness, &id, "start").await;
    let mut seen = until(&mut stream, |k| tool_started(k, "work")).await;
    if !seen.iter().any(|e| final_is(&e.kind, "Started")) {
        seen.extend(until(&mut stream, |k| final_is(k, "Started")).await);
    }
    let task = task_id(&seen, "task-1");
    release(&harness, "release");
    seen.extend(stream.settled().await);
    harness.crash_daemon().await;
    harness.launch().await;
    say(&harness, &id, "after restart").await;
    let mut replay = harness.events(&id, 0).await;
    let all = replay.settled().await;
    let report = format!("task-report-{task}");
    assert_eq!(all.iter().filter(|e| matches!(&e.kind, EventKind::MessageQueued { message_id,.. } if message_id.as_str() == report)).count(),1);
    assert!(all
        .iter()
        .any(|event| matches!(&event.kind, EventKind::MessageQueued {
        message_id, author_kind: openside_proto::MessageAuthorKind::Internal, from_agent: None, ..
    } if message_id.as_str() == report)));
    assert_eq!(all.iter().filter(|e| matches!(&e.kind, EventKind::AssistantFinal { text } if text.contains("[task report]") && text.contains("durable result"))).count(),1);
    let next = until(&mut replay, |k| final_is(k, "still here")).await;
    assert!(!next.iter().any(|e| matches!(&e.kind,EventKind::MessageQueued { message_id,.. } if message_id.as_str() == report)));
    replay.settled().await;
    harness.crash_daemon().await;
    harness.launch().await;
    let events = durable_events(&harness, &id);
    assert_eq!(events.iter().filter(|e|matches!(&e.kind,EventKind::MessageQueued { message_id,.. } if message_id.as_str() == report)).count(),1);
    harness.kill().await;
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
    let app = Router::new()
        .route(
            "/login",
            get(|| async {
                (
                    [(
                        header::SET_COOKIE,
                        "task_login=shared; Path=/; SameSite=Lax",
                    )],
                    Html("<title>Signed in</title><h1>Signed in</h1><button>Continue</button>"),
                )
            }),
        )
        .route(
            "/check",
            get(|headers: HeaderMap| async move {
                let authenticated = headers
                    .get(header::COOKIE)
                    .is_some_and(|c| c.to_str().unwrap().contains("task_login=shared"));
                Html(format!(
                    "<title>Account</title><h1>{}</h1><button>Continue</button>",
                    if authenticated {
                        "Authenticated"
                    } else {
                        "Anonymous"
                    }
                ))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Site { origin, task }
}

#[tokio::test]
async fn tasks_share_cookies_not_tabs_and_front_reads_but_cannot_act() {
    let site = site().await;
    let script = json!({"responses":[start("task-1","sign in"),text("First started"),start("task-2","check login"),text("Second started"),
        tool("browser_tabs",json!({}),"front-tabs"),
        tool("browser_snapshot",json!({"tab_id":"{{result:front-tabs.0.tab_id}}"}),"front-read"),
        tool("browser_navigate",json!({"tab_id":"{{result:front-tabs.0.tab_id}}","url":format!("{}/check",site.origin)}),"front-act"),
        text("Read only"),echo(),echo()],
        "subagents":{"task-1":[tool("browser_open_tab",json!({"url":format!("{}/login",site.origin)}),"open-1"),blocked("work-1","release-1"),text("first done")],
        "task-2":[tool("browser_tabs",json!({}),"other-tabs"),
        tool("browser_navigate",json!({"tab_id":"{{result:other-tabs.0.tab_id}}","url":format!("{}/check",site.origin)}),"steal-tab"),
        tool("browser_repl",json!({"code":"await page.title()"}),"implicit-steal"),
        tool("browser_open_tab",json!({"url":format!("{}/check",site.origin)}),"open-2"),
        tool("browser_snapshot",json!({"tab_id":"{{result:open-2.tab_id}}"}),"snapshot-2"),blocked("work-2","release-2"),text("second done")]}});
    let mut harness = harness(&script, true).await;
    let id = create(&harness).await;
    let mut stream = harness.events(&id, 0).await;
    say(&harness, &id, "start first").await;
    let mut seen = until(&mut stream, |k| tool_started(k, "work-1")).await;
    if !seen.iter().any(|e| final_is(&e.kind, "First started")) {
        seen.extend(until(&mut stream, |k| final_is(k, "First started")).await);
    }
    let tabs = harness.request("GET", "/v1/browser/tabs", None).await;
    let first_tab = tabs.body["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["url"].as_str().is_some_and(|u| u.ends_with("/login")))
        .unwrap()["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    say(&harness, &id, "start second").await;
    seen.extend(until(&mut stream, |k| tool_started(k, "work-2")).await);
    let tasks = harness
        .request("GET", &format!("/v1/sessions/{id}/subagents"), None)
        .await;
    assert_eq!(
        tasks.body["subagents"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["status"] == "running")
            .count(),
        2
    );
    let snapshot = seen
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::ToolEnded {
                tool_call_id,
                result,
                ..
            } if tool_call_id == "snapshot-2" => Some(result),
            _ => None,
        })
        .unwrap();
    assert!(snapshot.to_string().contains("Authenticated"), "{snapshot}");
    let tabs = harness.request("GET", "/v1/browser/tabs", None).await;
    let second_tab = tabs.body["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["url"].as_str().is_some_and(|u| u.ends_with("/check")))
        .unwrap()["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(first_tab, second_tab);
    if !seen.iter().any(|e| final_is(&e.kind, "Second started")) {
        seen.extend(until(&mut stream, |k| final_is(k, "Second started")).await);
    }
    say(&harness, &id, "read first but do not act").await;
    seen.extend(until(&mut stream, |k| final_is(k, "Read only")).await);
    assert!(seen.iter().any(|e| matches!(&e.kind,EventKind::ToolEnded { tool_call_id, result, is_error:true,.. } if tool_call_id == "steal-tab" && result.to_string().contains("tab_owned_by_task"))));
    assert!(seen.iter().any(|e| matches!(&e.kind,EventKind::ToolEnded { tool_call_id, result, is_error:true,.. } if tool_call_id == "implicit-steal" && result.to_string().contains("tab_owned_by_task"))));
    assert!(seen.iter().any(|e| matches!(&e.kind,EventKind::ToolEnded { tool_call_id, is_error:false,.. } if tool_call_id == "front-read")));
    assert!(
        !seen.iter().any(|e| tool_started(&e.kind, "front-act")),
        "front must not expose acting tools"
    );
    let tabs = harness.request("GET", "/v1/browser/tabs", None).await;
    assert!(tabs.body["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["tab_id"] == first_tab
            && t["url"].as_str().is_some_and(|u| u.ends_with("/login"))));
    if !seen.iter().any(|e| matches!(&e.kind,EventKind::BrowserState(state) if state.tab_id == second_tab && state.thumbnail.is_some())) {
        seen.extend(until(&mut stream, |kind| matches!(kind,EventKind::BrowserState(state) if state.task_id.is_some() && state.tab_id == second_tab && state.thumbnail.is_some())).await);
    }
    let thumbnail = seen
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::BrowserState(state) if state.tab_id == second_tab => {
                state.thumbnail.as_ref()
            }
            _ => None,
        })
        .unwrap();
    let route = format!("/v1/sessions/{id}/browser/thumbnails/{thumbnail}");
    let (status, jpeg) = harness.get_bytes(&route).await;
    assert_eq!(status, 200);
    assert!(jpeg.starts_with(&[0xff, 0xd8]));
    let other = create(&harness).await;
    assert_eq!(
        harness
            .get_bytes(&format!(
                "/v1/sessions/{other}/browser/thumbnails/{thumbnail}"
            ))
            .await
            .0,
        404
    );
    release(&harness, "release-1");
    release(&harness, "release-2");
    stream.settled().await;
    harness.kill().await;
}

async fn parked_tab(harness: &Harness, session: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let tabs = harness.request("GET", "/v1/browser/tabs", None).await;
        if let Some(tab) = tabs.body["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tab| tab["owner_session_id"] == session && tab["url"] == "about:blank")
        {
            return tab["tab_id"].as_str().unwrap().to_owned();
        }
        assert!(
            Instant::now() < deadline,
            "prewarm did not create a parked tab: {}",
            tabs.body
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn compositor() -> Option<(Value, Value)> {
    let read = |argument: &str| {
        let output = std::process::Command::new("hyprctl")
            .args([argument, "-j"])
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| serde_json::from_slice::<Value>(&output.stdout).ok())
            .flatten()
    };
    Some((read("activewindow")?, read("clients")?))
}

#[tokio::test]
async fn prewarm_creates_no_window_or_focus_and_disposes_unused_tabs() {
    let mut harness = harness(&json!({"responses":[]}), true).await;
    let before = compositor();
    let id = create(&harness).await;
    parked_tab(&harness, &id).await;
    let mut events = harness.events(&id, 0).await;
    until(&mut events, |kind| {
        matches!(kind, EventKind::RuntimeStarted { .. })
    })
    .await;
    for _ in 0..3 {
        assert_eq!(
            harness
                .request("POST", &format!("/v1/sessions/{id}/prewarm"), None)
                .await
                .status,
            202
        );
    }
    tokio::time::sleep(Duration::from_millis(150)).await;
    let tabs = harness.request("GET", "/v1/browser/tabs", None).await;
    assert_eq!(
        tabs.body["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["owner_session_id"] == id)
            .count(),
        1,
        "prewarm must be idempotent"
    );
    if let Some((active, clients)) = before {
        let (after_active, after_clients) = compositor().expect("compositor IPC disappeared");
        assert_eq!(
            active["address"], after_active["address"],
            "prewarm changed the desktop's active window"
        );
        let addresses = |items: Value| {
            items
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["address"].as_str().unwrap().to_owned())
                .collect::<std::collections::BTreeSet<_>>()
        };
        assert_eq!(
            addresses(clients),
            addresses(after_clients),
            "prewarm opened a desktop window"
        );
    } else {
        eprintln!("SKIP compositor focus/window assertion: Hyprland IPC unavailable");
    }
    let restored = parked_tab(&harness, &id).await;
    harness.crash_daemon().await;
    let saved_path = openside_config::data_layout::DataPath::BrowserState.path(&harness.data_dir);
    let mut saved: Value =
        serde_json::from_str(&std::fs::read_to_string(&saved_path).unwrap()).unwrap();
    saved["warm_tabs"][&id]["expires_at"] = json!("2000-01-01T00:00:00Z");
    std::fs::write(saved_path, saved.to_string()).unwrap();
    harness.launch().await;
    let tabs = harness.request("GET", "/v1/browser/tabs", None).await;
    assert!(
        !tabs.body["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tab| tab["tab_id"] == restored),
        "restart must expire the same parked lease"
    );
    let ready = harness
        .request("POST", &format!("/v1/sessions/{id}/prewarm"), None)
        .await;
    assert_eq!(ready.status, 202);
    for action in ["stop", "archive", "delete"] {
        let session = if action == "stop" {
            id.clone()
        } else {
            create(&harness).await
        };
        let parked = parked_tab(&harness, &session).await;
        let response = match action {
            "stop" => {
                harness
                    .request("POST", &format!("/v1/sessions/{session}/stop"), None)
                    .await
            }
            "archive" => {
                harness
                    .request(
                        "PATCH",
                        &format!("/v1/sessions/{session}"),
                        Some(json!({"archived":true})),
                    )
                    .await
            }
            _ => {
                harness
                    .request("DELETE", &format!("/v1/sessions/{session}"), None)
                    .await
            }
        };
        assert!(
            response.status < 300,
            "{action}: {} {}",
            response.status,
            response.body
        );
        let tabs = harness.request("GET", "/v1/browser/tabs", None).await;
        assert!(
            !tabs.body["tabs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["tab_id"] == parked),
            "{action} left an unused prewarm tab"
        );
        let _ = harness
            .request("POST", &format!("/v1/sessions/{session}/prewarm"), None)
            .await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        let tabs = harness.request("GET", "/v1/browser/tabs", None).await;
        assert!(
            !tabs.body["tabs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["owner_session_id"] == session),
            "late prewarm recreated a tab after {action}"
        );
    }
    harness.kill().await;
}

#[tokio::test]
async fn interrupted_front_summary_preserves_one_task_finished_row_across_two_restarts() {
    let script = json!({"responses":[start("task-1","durable result"),text("Started"),
        {"parts":[{"type":"wait_steer","timeout_ms":30000},{"type":"echo"}]}],
        "subagents":{"task-1":[blocked("work","release"),text("stored Task output")]}});
    let mut harness = harness(&script, false).await;
    let id = create(&harness).await;
    let mut stream = harness.events(&id, 0).await;
    say(&harness, &id, "start").await;
    let mut seen = until(&mut stream, |kind| tool_started(kind, "work")).await;
    if !seen.iter().any(|e| final_is(&e.kind, "Started")) {
        seen.extend(until(&mut stream, |kind| final_is(kind, "Started")).await);
    }
    let task = task_id(&seen, "task-1");
    let report = format!("task-report-{task}");
    release(&harness, "release");
    seen.extend(until(&mut stream, |kind|matches!(kind,EventKind::MessageInjected { message_id } if message_id.as_str() == report)).await);
    assert!(!seen.iter().any(
        |e| matches!(&e.kind,EventKind::AssistantFinal { text } if text.contains("[task report]"))
    ));
    harness.kill_sidecar();
    seen.extend(until(&mut stream, |kind|matches!(kind,EventKind::MessageCompleted { message_id, outcome:openside_proto::MessageOutcome::Failed, reason:Some(reason) } if message_id.as_str() == report && reason == "interrupted")).await);
    let mut resumed_script = script.clone();
    resumed_script["responses"][2] = text("still here");
    harness.replace_script(&resumed_script);
    harness.crash_daemon().await;
    harness.launch().await;
    say(&harness, &id, "after crash").await;
    let mut replay = harness.events(&id, 0).await;
    let all = until(&mut replay, |kind| final_is(kind, "still here")).await;
    assert_eq!(all.iter().filter(|e|matches!(&e.kind,EventKind::MessageQueued { message_id,.. } if message_id.as_str() == report)).count(),1);
    assert_eq!(all.iter().filter(|e|matches!(&e.kind,EventKind::MessageInjected { message_id } if message_id.as_str() == report)).count(),1);
    assert_eq!(all.iter().filter(|e|matches!(&e.kind,EventKind::SubagentEnded { subagent_id, output:Some(output), .. } if subagent_id == &task && output == "stored Task output")).count(),1);
    assert!(!all.iter().any(
        |e| matches!(&e.kind,EventKind::AssistantFinal { text } if text.contains("[task report]"))
    ));
    replay.settled().await;
    harness.crash_daemon().await;
    harness.launch().await;
    let children = harness
        .request("GET", &format!("/v1/sessions/{id}/subagents"), None)
        .await;
    assert_eq!(
        children.body["subagents"][0]["output"],
        "stored Task output"
    );
    let events = durable_events(&harness, &id);
    assert_eq!(events.iter().filter(|e|matches!(&e.kind,EventKind::MessageQueued { message_id,.. } if message_id.as_str() == report)).count(),1);
    assert_eq!(events.iter().filter(|e|matches!(&e.kind,EventKind::MessageInjected { message_id } if message_id.as_str() == report)).count(),1);
    harness.kill().await;
}

#[tokio::test]
async fn a_failed_task_does_not_stop_a_parallel_task() {
    let invalid = tool("not_a_tool", json!({}), "invalid");
    let script = json!({"responses":[
        {"parts":[{"type":"tool_call","name":"task_start","args":{"goal":"first"},"id":"task-1"},{"type":"tool_call","name":"task_start","args":{"goal":"second"},"id":"task-2"}]},
        text("Started both"),echo(),echo()],
        "subagents":{"task-1":[blocked("work-1","release-1"),invalid.clone(),invalid.clone(),invalid.clone(),invalid],
            "task-2":[blocked("work-2","release-2"),text("second completed")]}});
    let mut harness = harness(&script, false).await;
    let id = create(&harness).await;
    let mut stream = harness.events(&id, 0).await;
    say(&harness, &id, "start").await;
    let mut seen = until(&mut stream, |kind| final_is(kind, "Started both")).await;
    while !["work-1", "work-2"]
        .iter()
        .all(|call| seen.iter().any(|e| tool_started(&e.kind, call)))
    {
        seen.push(stream.next().await);
    }
    let first = task_id(&seen, "task-1");
    let second = task_id(&seen, "task-2");
    release(&harness, "release-1");
    until(&mut stream,|kind|matches!(kind,EventKind::SubagentEnded { subagent_id,status:SubagentStatus::Failed,.. } if subagent_id == &first)).await;
    let children = harness
        .request("GET", &format!("/v1/sessions/{id}/subagents"), None)
        .await;
    assert_eq!(
        children.body["subagents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["id"] == second)
            .unwrap()["status"],
        "running"
    );
    release(&harness, "release-2");
    let done = stream.settled().await;
    assert!(done.iter().any(|e|matches!(&e.kind,EventKind::SubagentEnded { subagent_id,status:SubagentStatus::Completed,output:Some(output),.. } if subagent_id == &second && output == "second completed")));
    assert!(!done
        .iter()
        .any(|e| matches!(e.kind, EventKind::SessionStopped {})));
    harness.kill().await;
}

async fn wait_file(harness: &Harness, file: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !harness.workspace().join(file).exists() {
        assert!(Instant::now() < deadline, "tool never reached {file}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn cancelled_task_host_shell_cannot_perform_its_delayed_effect_while_sibling_completes() {
    let script = json!({"responses":[
    {"parts":[{"type":"tool_call","name":"task_start","args":{"goal":"first"},"id":"task-1"},{"type":"tool_call","name":"task_start","args":{"goal":"second"},"id":"task-2"}]},
    text("Started both"), echo()],
    "subagents":{
        "task-1":[tool("bash", json!({"command":"touch first-entered; while [ ! -f first-release ]; do sleep 0.05; done; touch forbidden-effect"}), "delayed-effect"), text("must not finish")],
        "task-2":[blocked("sibling-work", "sibling-release"), tool("bash", json!({"command":"touch sibling-effect"}), "sibling-effect"), text("sibling completed")]
    }});
    let mut harness = harness(&script, false).await;
    let id = create(&harness).await;
    let mut stream = harness.events(&id, 0).await;
    say(&harness, &id, "start").await;
    let mut seen = until(&mut stream, |kind| final_is(kind, "Started both")).await;
    while !["delayed-effect", "sibling-work"]
        .iter()
        .all(|call| seen.iter().any(|event| tool_started(&event.kind, call)))
    {
        seen.push(stream.next().await);
    }
    wait_file(&harness, "first-entered").await;
    let first = task_id(&seen, "task-1");
    let sibling = task_id(&seen, "task-2");
    let cancelled = harness
        .request(
            "POST",
            &format!("/v1/sessions/{id}/subagents/{first}/cancel"),
            None,
        )
        .await;
    assert_eq!(cancelled.status, 200, "{}", cancelled.body);
    release(&harness, "first-release");
    release(&harness, "sibling-release");
    seen.extend(stream.settled().await);
    assert!(harness.workspace().join("sibling-effect").exists());
    assert!(!harness.workspace().join("forbidden-effect").exists());
    assert!(seen
        .iter()
        .any(|event| matches!(&event.kind, EventKind::SubagentEnded {
        subagent_id, status: SubagentStatus::Completed, output: Some(output), ..
    } if subagent_id == &sibling && output == "sibling completed")));
    let steer = harness
        .request(
            "POST",
            &format!("/v1/sessions/{id}/subagents/{first}/steer"),
            Some(json!({"text":"too late"})),
        )
        .await;
    assert_eq!(steer.status, 409);
    assert_eq!(steer.body["code"], "subagent_finished");
    harness.kill().await;
}

#[tokio::test]
async fn waiting_task_keeps_its_repl_claimed_prewarm_tab_across_restart_and_stale_lease_expiry() {
    let site = site().await;
    let script = json!({"responses":[start("task-1", "wait in my tab"), text("First started"), start("task-2", "open my own tab"), text("Second started"), echo(), echo()],
    "subagents":{
        "task-1":[tool("browser_repl", json!({"code":"await page.title()"}), "claim-warm"),
            tool("ask_user", json!({"question":"Continue the waiting Task?"}), "wait"),
            tool("browser_repl", json!({"tab_id":"{{result:claim-warm.tab_id}}","code":"await page.title()"}), "resume-own"),
            text("first resumed")],
        "task-2":[tool("browser_tabs", json!({}), "other-tabs"),
            tool("browser_repl", json!({"tab_id":"{{result:other-tabs.0.tab_id}}","code":"await page.title()"}), "steal-restored"),
            tool("browser_open_tab", json!({"url":format!("{}/check",site.origin)}), "own-tab"),
            blocked("sibling-wait", "release-sibling"), text("second done")]
    }});
    let mut harness = harness(&script, true).await;
    let id = create(&harness).await;
    let parked = parked_tab(&harness, &id).await;
    let mut stream = harness.events(&id, 0).await;
    say(&harness, &id, "start first").await;
    let mut seen = until(&mut stream, |kind| matches!(kind, EventKind::SubagentLifecycle { status, .. } if status == "waiting_human")).await;
    if !seen
        .iter()
        .any(|event| final_is(&event.kind, "First started"))
    {
        seen.extend(until(&mut stream, |kind| final_is(kind, "First started")).await);
    }
    let first = task_id(&seen, "task-1");
    let ticket = seen
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::TicketOpened { ticket } if ticket.tool_call_id == "wait" => {
                Some(ticket.id.to_string())
            }
            _ => None,
        })
        .unwrap();
    let saved_path = openside_config::data_layout::DataPath::BrowserState.path(&harness.data_dir);
    let saved: Value = serde_json::from_slice(&std::fs::read(&saved_path).unwrap()).unwrap();
    assert!(
        saved["warm_tabs"].get(&id).is_none(),
        "REPL must retire the warm lease"
    );
    assert_eq!(
        saved["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tab| tab["id"] == parked)
            .unwrap()["task"],
        first
    );
    // The sidecar checkpoint must also preserve the deferred child before simulating a crash.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !harness.snapshot(&id)["subagents"]
        .as_array()
        .is_some_and(|children| {
            children
                .iter()
                .any(|task| task["id"] == first && task["status"] == "waiting_human")
        })
    {
        assert!(
            Instant::now() < deadline,
            "waiting Task was not checkpointed"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    harness.crash_daemon().await;
    let mut saved: Value = serde_json::from_slice(&std::fs::read(&saved_path).unwrap()).unwrap();
    // Also exercise recovery of an old/crash-interrupted lease alongside its durable owner.
    saved["warm_tabs"] = json!({id.clone(): {"tab_id":parked,"expires_at":"2000-01-01T00:00:00Z"}});
    std::fs::write(&saved_path, saved.to_string()).unwrap();
    harness.launch().await;
    let tasks = harness
        .request("GET", &format!("/v1/sessions/{id}/subagents"), None)
        .await;
    assert_eq!(
        tasks.body["subagents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["id"] == first)
            .unwrap()["status"],
        "waiting_human"
    );
    let tabs = harness.request("GET", "/v1/browser/tabs", None).await;
    assert!(
        tabs.body["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tab| tab["tab_id"] == parked),
        "expiry closed the claimed tab"
    );
    let boundary = durable_events(&harness, &id).last().unwrap().seq;
    let mut stream = harness.events(&id, boundary).await;
    say(&harness, &id, "start competitor").await;
    let mut seen = until(&mut stream, |kind| tool_started(kind, "sibling-wait")).await;
    assert!(seen.iter().any(
        |event| matches!(&event.kind, EventKind::ToolEnded { tool_call_id, is_error: true, result, .. }
        if tool_call_id == "steal-restored" && result.to_string().contains("tab_owned_by_task"))
    ));
    let opened = seen
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::ToolEnded {
                card: _,
                tool_call_id,
                result,
                is_error: false,
            } if tool_call_id == "own-tab" => Some(result),
            _ => None,
        })
        .unwrap();
    assert_ne!(opened["tab_id"], parked);
    let resumed = harness
        .request(
            "POST",
            &format!("/v1/tickets/{ticket}/resolve"),
            Some(json!({"value":"Continue"})),
        )
        .await;
    assert_eq!(resumed.status, 200, "{}", resumed.body);
    seen.extend(until(&mut stream, |kind| matches!(kind, EventKind::ToolEnded { tool_call_id, .. } if tool_call_id == "resume-own")).await);
    assert!(seen.iter().any(
        |event| matches!(&event.kind, EventKind::ToolEnded { tool_call_id, result, is_error: false, .. }
        if tool_call_id == "resume-own" && result["tab_id"] == parked)
    ));
    release(&harness, "release-sibling");
    stream.settled().await;
    harness.kill().await;
}
