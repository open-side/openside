//! Front/Task acceptance against the real daemon, Python sidecar and headless Chromium.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod ui_support;

use ui_support::{send, silent};

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
    tool("create_task", json!({"goal":goal}), call)
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
    matches!(kind, EventKind::AssistantFinal { text, .. } if text == expected)
}
fn tool_started(kind: &EventKind, call: &str) -> bool {
    matches!(kind, EventKind::ToolStarted { tool_call_id, .. } if tool_call_id == call)
        || matches!(kind, EventKind::SubagentProgress { progress, .. }
            if progress["kind"] == "tool.started" && progress["tool_call_id"] == call)
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
async fn owner_priority_keeps_task_report_behind_owner_without_effect_replay() {
    let script = json!({
        "responses": [
            start("priority-task", "perform one effect"),
            {"parts":[{"type":"wait_steer","timeout_ms":10000},send("generation finished")["parts"][0]]},
            send("{{user_text}}"),
            send("{{user_text}}")
        ],
        "subagents": {
            "priority-task": [
                tool("bash", json!({"command":"printf x >> priority-effect"}), "one-effect"),
                text("effect complete")
            ]
        }
    });
    let mut harness = harness(&script, false).await;
    let session = create(&harness).await;
    let mut stream = harness.events(&session, 0).await;
    say(&harness, &session, "initial-owner").await;
    let mut seen = until(&mut stream, |kind| {
        matches!(
            kind,
            EventKind::MessageQueued {
                author_kind: openside_proto::MessageAuthorKind::Internal,
                ..
            }
        )
    })
    .await;
    let report = seen
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::MessageQueued {
                message_id,
                author_kind: openside_proto::MessageAuthorKind::Internal,
                ..
            } => Some(message_id.clone()),
            _ => None,
        })
        .unwrap();
    assert!(!seen.iter().any(|event| matches!(&event.kind,
        EventKind::MessageInjected { message_id } if message_id == &report)));
    say(&harness, &session, "updated-owner").await;
    seen.extend(
        until(&mut stream, |kind| {
            matches!(kind,
                EventKind::MessageCompleted { message_id, .. } if message_id == &report
            )
        })
        .await,
    );
    let owner = seen
        .iter()
        .position(|event| {
            matches!(&event.kind,
        EventKind::MessageInjected { message_id } if message_id.as_str() == "updated-owner")
        })
        .unwrap();
    let report_read = seen
        .iter()
        .position(|event| {
            matches!(&event.kind,
        EventKind::MessageInjected { message_id } if message_id == &report)
        })
        .unwrap();
    assert!(owner < report_read);
    assert_eq!(
        std::fs::read_to_string(harness.workspace().join("priority-effect")).unwrap(),
        "x"
    );
    assert_eq!(
        seen.iter()
            .filter(|event| tool_started(&event.kind, "one-effect"))
            .count(),
        1
    );
    harness.kill().await;
}

#[tokio::test]
async fn idle_task_report_burst_settles_each_message_in_one_batch() {
    let script = json!({
        "responses": [
            {"parts":[
                start("batch-a", "first")["parts"][0],
                start("batch-b", "second")["parts"][0],
                start("batch-c", "third")["parts"][0]
            ]},
            {"parts":[{"type":"wait_steer","timeout_ms":10000},silent()["parts"][0]]},
            send("owner update read"),
            send("all reports read")
        ],
        "subagents": {
            "batch-a":[text("first done")],
            "batch-b":[text("second done")],
            "batch-c":[text("third done")]
        }
    });
    let mut harness = harness(&script, false).await;
    let session = create(&harness).await;
    let mut stream = harness.events(&session, 0).await;
    say(&harness, &session, "batch-owner").await;
    let mut seen = Vec::new();
    while seen.iter().filter(|event: &&Event| matches!(&event.kind,
        EventKind::MessageQueued { message_id, .. } if message_id.as_str().starts_with("task-report-"))).count() < 3
    {
        seen.push(stream.next().await);
    }
    say(&harness, &session, "batch-owner-update").await;
    while seen.iter().filter(|event: &&Event| matches!(&event.kind,
        EventKind::MessageCompleted { message_id, .. } if message_id.as_str().starts_with("task-report-"))).count() < 3
    {
        seen.push(stream.next().await);
    }
    let reports: Vec<_> = seen
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::MessageCompleted {
                message_id,
                outcome,
                ..
            } if message_id.as_str().starts_with("task-report-") => {
                assert_eq!(*outcome, openside_proto::MessageOutcome::Completed);
                Some(message_id)
            }
            _ => None,
        })
        .collect();
    assert_eq!(reports.len(), 3);
    let first_report = seen.iter().position(|event| matches!(&event.kind,
        EventKind::MessageInjected { message_id } if message_id.as_str().starts_with("task-report-"))).unwrap();
    assert_eq!(
        seen[first_report..]
            .iter()
            .filter(|event| matches!(&event.kind, EventKind::TurnStarted { .. }))
            .count(),
        1
    );
    harness.kill().await;
}

#[tokio::test]
async fn configured_fast_front_starts_tasks_on_reasoning_model() {
    for fast in [Some("openai:gpt-6-luna"), None] {
        let script = json!({"responses":[start("role-task", "reason about this"), send("Started"), send("{{user_text}}")],
            "subagents":{"role-task":[text("Reasoned")]}});
        let mut harness = Harness::prepare(BIN, &script).expect("scripted Python environment");
        let config = harness.root().join("config/openside/config.toml");
        let original = std::fs::read_to_string(&config).unwrap();
        let mut configured = original.replace("orchestration = \"single\"\n", "");
        if let Some(fast) = fast {
            configured = configured.replace("[models]", &format!("[models]\nfront = {fast:?}"));
        }
        std::fs::write(config, configured).unwrap();
        harness.launch().await;
        let session = create(&harness).await;
        let mut stream = harness.events(&session, 0).await;
        say(&harness, &session, "start-role").await;
        let seen = until(&mut stream, |kind| {
            matches!(
                kind,
                EventKind::SubagentEnded {
                    status: SubagentStatus::Completed,
                    ..
                }
            )
        })
        .await;
        assert!(seen.iter().any(|event| matches!(&event.kind,
            EventKind::SubagentStarted { parent_tool_call_id, model, background: true, .. }
            if parent_tool_call_id == "role-task" && model == "script")));
        harness.kill().await;
    }
}

#[tokio::test]
async fn chief_selects_an_available_connected_task_model() {
    let selected = "anthropic:claude-haiku-5-5";
    let script = json!({"responses":[
        tool("get_models", json!({}), "connected-models"),
        tool("create_task", json!({"goal":"quick lookup","model":selected}), "selected-task"),
        send("Task admitted"), send("{{user_text}}")
    ], "subagents":{"selected-task":[text("Selected model completed the work")]}});
    let mut harness = Harness::prepare(BIN, &script).expect("scripted Python environment");
    let config = harness.root().join("config/openside/config.toml");
    let original = std::fs::read_to_string(&config).unwrap();
    std::fs::write(config, original.replace("orchestration = \"single\"\n", "")).unwrap();
    harness.append_config("[credentials.order]\nanthropic = [\"command\"]\n[credentials.command]\nanthropic = [\"printf\", \"offline-test-key\"]\n");
    harness.launch().await;
    let session = create(&harness).await;
    let mut stream = harness.events(&session, 0).await;
    say(&harness, &session, "select-model").await;
    until(&mut stream, |kind| {
        matches!(
            kind,
            EventKind::SubagentEnded {
                status: SubagentStatus::Completed,
                ..
            }
        )
    })
    .await;
    let events = durable_events(&harness, &session);
    assert!(events.iter().any(|event| matches!(&event.kind,
        EventKind::SubagentStarted { parent_tool_call_id, model, .. }
        if parent_tool_call_id == "selected-task" && model == selected)));
    assert!(events.iter().any(|event| matches!(&event.kind,
        EventKind::SubagentEnded { output: Some(output), status: SubagentStatus::Completed, .. }
        if output == "Selected model completed the work")));
    harness.kill().await;
}

#[tokio::test]
async fn front_sends_complete_result_and_task_steps_stay_in_task_history() {
    let site = site().await;
    let card = json!({"card":{"id":"","kind":"table","site":{"origin":site.origin,"name":"Local page","favicon_ref":null},"title":"Result","status":"done","blocks":[{"type":"table","id":"table","columns":[{"id":"name","label":"Name"}],"rows":[[{"type":"text","text":"Signed in"}]],"layout":"grid"}],"actions":[],"interaction":"none","collapsed_summary":"One result"},"images":[]});
    let script = json!({"responses":[
        start("browse-task", "Read the local page"),
        tool("stay_silent", json!({}), "wait-browse"),
        tool("send_message", json!({"text":"The local page says Signed in."}), "say-result")
    ], "subagents":{"browse-task":[
        tool("browser_open_tab", json!({"url":format!("{}/login", site.origin)}), "open-local"),
        tool("browser_snapshot", json!({"tab_id":"{{result:open-local.tab_id}}"}), "read-local"),
        tool("present", card, "show-result"),
        text("The local page says Signed in.")
    ]}});
    let mut harness = harness(&script, true).await;
    let id = create(&harness).await;
    let mut stream = harness.events(&id, 0).await;
    say(&harness, &id, "read-local-page").await;
    let seen = until(&mut stream, |kind| {
        matches!(kind, EventKind::SessionSettled {})
    })
    .await;
    assert_eq!(
        seen.iter()
            .filter(|event| final_is(&event.kind, "The local page says Signed in."))
            .count(),
        1
    );
    let completed = seen
        .iter()
        .find(|event| {
            matches!(
                &event.kind,
                EventKind::SubagentEnded {
                    status: SubagentStatus::Completed,
                    ..
                }
            )
        })
        .unwrap();
    let delivered = seen
        .iter()
        .find(|event| final_is(&event.kind, "The local page says Signed in."))
        .unwrap();
    assert!(
        completed.seq < delivered.seq,
        "the result message must follow the Task report"
    );
    assert!(!seen.iter().any(|event| matches!(
        &event.kind,
        EventKind::AssistantDelta { .. } | EventKind::ThinkingDelta { .. }
    )));
    for call in ["open-local", "read-local", "show-result"] {
        assert!(seen.iter().any(|event| matches!(&event.kind,
            EventKind::SubagentProgress { progress, .. }
            if progress["kind"] == "tool.ended" && progress["tool_call_id"] == call)));
        assert!(!seen.iter().any(|event| matches!(&event.kind,
            EventKind::ToolStarted { tool_call_id, .. } | EventKind::ToolEnded { tool_call_id, .. }
            if tool_call_id == call)));
    }
    assert!(seen.iter().any(|event| matches!(&event.kind,
        EventKind::SubagentProgress { progress, .. }
        if progress["kind"] == "tool.ended" && progress["tool_call_id"] == "open-local"
            && progress["status_text"] == "Opened 127.0.0.1")));
    assert!(seen.iter().any(|event| matches!(&event.kind,
        EventKind::CardPresented { card, .. } if card.title == "Result")));
    harness.kill().await;
}

#[tokio::test]
async fn front_replies_before_task_ends_and_status_arrives_within_300ms() {
    let script = json!({"responses":[start("task-1", "long work"), send("Started"), send("4"), send("{{user_text}}")],
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
        {"parts":[{"type":"tool_call","name":"create_task","args":{"goal":"first"},"id":"task-1"},{"type":"tool_call","name":"create_task","args":{"goal":"second"},"id":"task-2"}]},
        send("Started both"),
        tool("steer_task",json!({"task_id":"{{result:task-2.task_id}}","message":"answer BETA"}),"steer"), send("Steered"),
        tool("cancel_task",json!({"task_id":"{{result:task-1.task_id}}"}),"cancel"),
        tool("get_tasks",json!({}),"status"), send("Cancelled first"), send("{{user_text}}")],
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
async fn ordinary_owner_message_automatically_steers_running_task() {
    let script = json!({"responses":[
        start("weekly-task", "research a daily summary"),
        send("The current cadence is daily."),
        tool("steer_task", json!({"task_id":"{{result:weekly-task.task_id}}","message":"make it weekly instead"}), "weekly-steer"),
        send("The cadence is now weekly."),
        send("Weekly summary ready.")
    ], "subagents":{"weekly-task":[blocked("weekly-work", "weekly-release"), echo()]}});
    let mut harness = harness(&script, false).await;
    let session = create(&harness).await;
    let mut stream = harness.events(&session, 0).await;
    say(&harness, &session, "research daily").await;
    let mut seen = until(&mut stream, |kind| tool_started(kind, "weekly-work")).await;
    let task = task_id(&seen, "weekly-task");
    while !seen.iter().any(|event| {
        matches!(&event.kind,
        EventKind::MessageCompleted { message_id, .. } if message_id.as_str() == "research daily")
    }) {
        seen.push(stream.next().await);
    }
    say(&harness, &session, "make it weekly instead").await;
    seen.extend(
        until(&mut stream, |kind| {
            matches!(kind,
                EventKind::SubagentProgress { subagent_id, progress }
                if subagent_id == &task && progress["kind"] == "steer_queued"
            )
        })
        .await,
    );
    assert!(seen.iter().any(|event| matches!(&event.kind,
        EventKind::ToolStarted { name, args, .. }
        if name == "steer_task" && args["task_id"] == task && args["message"] == "make it weekly instead")));
    release(&harness, "weekly-release");
    seen.extend(
        until(&mut stream, |kind| {
            matches!(kind,
                EventKind::SubagentEnded { subagent_id, .. } if subagent_id == &task
            )
        })
        .await,
    );
    assert!(seen.iter().any(|event| matches!(&event.kind,
        EventKind::SubagentProgress { subagent_id, progress }
        if subagent_id == &task && progress["kind"] == "steer_injected")));
    assert!(seen.iter().any(|event| matches!(&event.kind,
        EventKind::SubagentEnded { subagent_id, output: Some(output), .. }
        if subagent_id == &task && output == "make it weekly instead")));
    harness.kill().await;
}

#[tokio::test]
async fn task_report_is_delivered_exactly_once_across_daemon_crash_and_restart() {
    let script = json!({"responses":[start("task-1","durable work"),send("Started"),send("{{user_text}}"),send("still here")],
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
    assert_eq!(all.iter().filter(|e| matches!(&e.kind, EventKind::AssistantFinal { text, .. } if text.contains("[task report]") && text.contains("durable result"))).count(),1);
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
async fn tasks_share_cookies_not_tabs_and_front_cannot_read_or_act() {
    let site = site().await;
    let script = json!({"responses":[start("task-1","sign in"),send("First started"),start("task-2","check login"),send("Second started"),
        tool("browser_tabs",json!({}),"front-tabs"),
        tool("browser_snapshot",json!({"tab_id":"task-tab"}),"front-read"),
        tool("browser_navigate",json!({"tab_id":"task-tab","url":format!("{}/check",site.origin)}),"front-act"),
        send("Delegation only"),send("{{user_text}}"),send("{{user_text}}")],
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
            EventKind::SubagentProgress { progress, .. }
                if progress["kind"] == "tool.ended" && progress["tool_call_id"] == "snapshot-2" =>
            {
                Some(&progress["result"])
            }
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
    say(&harness, &id, "try reading and acting").await;
    seen.extend(until(&mut stream, |k| final_is(k, "Delegation only")).await);
    for call in ["steal-tab", "implicit-steal"] {
        assert!(seen.iter().any(|e| matches!(&e.kind, EventKind::SubagentProgress { progress, .. }
            if progress["kind"] == "tool.ended" && progress["tool_call_id"] == call
                && progress["is_error"] == true && progress["result"].to_string().contains("tab_owned_by_task"))));
    }
    for call in ["front-tabs", "front-read", "front-act"] {
        assert!(
            !seen.iter().any(|e| tool_started(&e.kind, call)),
            "front must not expose browser tools"
        );
    }
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
        eprintln!("SKIP compositor focus/window assertion: compositor IPC unavailable");
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
    let script = json!({"responses":[start("task-1","durable result"),send("Started"),
        {"parts":[{"type":"wait_steer","timeout_ms":30000},send("{{user_text}}")["parts"][0]]}],
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
        |e| matches!(&e.kind,EventKind::AssistantFinal { text, .. } if text.contains("[task report]"))
    ));
    harness.kill_sidecar();
    seen.extend(until(&mut stream, |kind|matches!(kind,EventKind::MessageCompleted { message_id, outcome:openside_proto::MessageOutcome::Failed, reason:Some(reason) } if message_id.as_str() == report && reason == "interrupted")).await);
    let mut resumed_script = script.clone();
    resumed_script["responses"][2] = send("still here");
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
        |e| matches!(&e.kind,EventKind::AssistantFinal { text, .. } if text.contains("[task report]"))
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
        {"parts":[{"type":"tool_call","name":"create_task","args":{"goal":"first"},"id":"task-1"},{"type":"tool_call","name":"create_task","args":{"goal":"second"},"id":"task-2"}]},
        send("Started both"),send("{{user_text}}"),send("{{user_text}}")],
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
    {"parts":[{"type":"tool_call","name":"create_task","args":{"goal":"first"},"id":"task-1"},{"type":"tool_call","name":"create_task","args":{"goal":"second"},"id":"task-2"}]},
    send("Started both"), send("{{user_text}}")],
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
async fn waiting_task_wakes_front_once_without_another_person_message() {
    let script = json!({"responses":[
        start("needs-choice", "Choose a destination"),
        tool("stay_silent", json!({}), "wait-choice"),
        tool("send_message", json!({"text":"Which destination do you prefer?"}), "ask-person")
    ],"subagents":{"needs-choice":[
        blocked("wait-before-question", "allow-question"),
        tool("ask_user", json!({"question":"Choose a destination"}), "choice"),
        text("Chosen")
    ]}});
    let mut harness = harness(&script, false).await;
    let id = create(&harness).await;
    let mut stream = harness.events(&id, 0).await;
    say(&harness, &id, "choose-for-me").await;
    until(&mut stream, |kind| {
        matches!(kind, EventKind::MessageCompleted { message_id, .. }
        if message_id.as_str() == "choose-for-me")
    })
    .await;
    release(&harness, "allow-question");
    until(&mut stream, |kind| {
        final_is(kind, "Which destination do you prefer?")
    })
    .await;
    let events = durable_events(&harness, &id);
    assert_eq!(events.iter().filter(|event| matches!(&event.kind,
        EventKind::MessageQueued { message_id, text, author_kind: openside_proto::MessageAuthorKind::Internal, .. }
        if message_id.as_str().contains("-waiting-") && text.contains("Needs you: Choose a destination"))).count(), 1);
    assert_eq!(
        events
            .iter()
            .filter(|event| final_is(&event.kind, "Which destination do you prefer?"))
            .count(),
        1
    );
    assert!(!events
        .iter()
        .any(|event| matches!(&event.kind, EventKind::AssistantDelta { .. })));
    harness.kill().await;
}

#[tokio::test]
async fn waiting_task_keeps_its_repl_claimed_prewarm_tab_across_restart_and_stale_lease_expiry() {
    let site = site().await;
    let script = json!({"responses":[start("task-1", "wait in my tab"), send("First started"), send("Needs your answer"), start("task-2", "open my own tab"), send("Second started"), send("{{user_text}}"), send("{{user_text}}")],
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
    if !seen
        .iter()
        .any(|event| final_is(&event.kind, "Needs your answer"))
    {
        seen.extend(until(&mut stream, |kind| final_is(kind, "Needs your answer")).await);
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
        |event| matches!(&event.kind, EventKind::SubagentProgress { progress, .. }
        if progress["kind"] == "tool.ended" && progress["tool_call_id"] == "steal-restored"
            && progress["is_error"] == true && progress["result"].to_string().contains("tab_owned_by_task"))
    ));
    let opened = seen
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::SubagentProgress { progress, .. }
                if progress["kind"] == "tool.ended"
                    && progress["tool_call_id"] == "own-tab"
                    && progress["is_error"] == false =>
            {
                Some(&progress["result"])
            }
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
    seen.extend(
        until(&mut stream, |kind| {
            matches!(kind, EventKind::SubagentProgress { progress, .. }
        if progress["kind"] == "tool.ended" && progress["tool_call_id"] == "resume-own")
        })
        .await,
    );
    assert!(seen.iter().any(
        |event| matches!(&event.kind, EventKind::SubagentProgress { progress, .. }
        if progress["kind"] == "tool.ended" && progress["tool_call_id"] == "resume-own"
            && progress["is_error"] == false && progress["result"]["tab_id"] == parked)
    ));
    release(&harness, "release-sibling");
    stream.settled().await;
    harness.kill().await;
}

#[tokio::test]
async fn task_document_attaches_to_chief_message_and_fetches_by_reference() {
    let markdown = "# Comparison\n\n| Plan | Cost |\n| --- | --- |\n| Basic | Free |\n";
    let script = json!({
        "responses": [
            start("document-task", "Write a Markdown comparison"),
            tool("stay_silent", json!({}), "wait-document"),
            tool("get_tasks", json!({}), "document-status"),
            tool("send_message", json!({
                "text": "Here is the comparison.",
                "attachments": [{
                    "artifact": "{{result:document-status.tasks.0.artifacts.0}}",
                    "title": "Plan comparison",
                    "display": "panel"
                }]
            }), "deliver-document"),
        ],
        "subagents": {"document-task": [
            tool("write_document", json!({"title":"Plan comparison","markdown":markdown}), "write-comparison"),
            text("The comparison document is ready.")
        ]}
    });
    let mut harness = harness(&script, false).await;
    let session = create(&harness).await;
    let mut stream = harness.events(&session, 0).await;
    say(&harness, &session, "compare").await;
    let events = tokio::time::timeout(
        Duration::from_secs(20),
        until(&mut stream, |kind| {
            final_is(kind, "Here is the comparison.")
        }),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "document delivery stalled; events={:#?}; snapshot={}",
            durable_events(&harness, &session),
            harness.snapshot(&session),
        )
    });
    let attachment = events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::AssistantFinal { text, attachments }
                if text == "Here is the comparison." =>
            {
                assert_eq!(attachments.len(), 1);
                Some(attachments[0].clone())
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(attachment.title, "Plan comparison");
    assert_eq!(attachment.display, openside_proto::DocumentDisplay::Panel);
    assert_eq!(attachment.bytes, markdown.len() as u64);
    assert_eq!(attachment.content_type, "text/markdown");
    let (status, bytes) = harness
        .get_bytes(&format!(
            "/v1/sessions/{session}/artifacts/{}",
            attachment.path.replace('/', "%2F"),
        ))
        .await;
    assert_eq!((status, bytes), (200, markdown.as_bytes().to_vec()));
    assert!(events.iter().any(|event| matches!(&event.kind,
        EventKind::ArtifactSaved { tool, path, .. } if tool == "write_document" && path.ends_with("/write-comparison.md"))));
    tokio::time::timeout(Duration::from_secs(20), stream.settled())
        .await
        .unwrap_or_else(|_| {
            panic!(
                "document turn did not settle: {:#?}",
                durable_events(&harness, &session)
            )
        });
    harness.kill().await;
}
