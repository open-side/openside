//! Real Python sidecar scenarios: events, tools, deferred calls and crash recovery.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;

use openside_e2e::{Harness, SseReader};
use openside_proto::{Event, EventKind, MessageOutcome, TicketKind};
use serde_json::{json, Value};

fn text(text: &str) -> Value {
    json!({"parts":[{"type":"text","text":text}]})
}
fn tool(name: &str, args: Value, call: &str) -> Value {
    json!({"parts":[{"type":"tool_call","name":name,"args":args,"id":call}]})
}
async fn harness(script: Value) -> Option<Harness> {
    Harness::start(env!("CARGO_BIN_EXE_e2e-opensided"), &script).await
}
async fn create(harness: &Harness, mode: &str) -> String {
    let response = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({
                "title":"Scripted sidecar", "mode":mode, "workspace":harness.workspace()
            })),
        )
        .await;
    assert_eq!(response.status, 201, "{}", response.body);
    response.body["id"].as_str().unwrap().to_owned()
}
async fn submit(harness: &Harness, id: &str, mid: &str, mode: &str) {
    let response = harness
        .request(
            "POST",
            &format!("/v1/sessions/{id}/messages"),
            Some(json!({"message_id":mid,"text":mid,"mode":mode})),
        )
        .await;
    assert_eq!(response.status, 202, "{}", response.body);
}
fn assert_final(events: &[Event], expected: &str) {
    assert!(
        events.iter().any(
            |event| matches!(&event.kind, EventKind::AssistantFinal { text } if text == expected)
        ),
        "missing final reply: {events:?}"
    );
    assert!(matches!(
        events.last().unwrap().kind,
        EventKind::SessionSettled {}
    ));
}
async fn ticket(stream: &mut SseReader) -> openside_proto::Ticket {
    loop {
        let event = stream.next().await;
        if let EventKind::TicketOpened { ticket } = event.kind {
            return *ticket;
        }
        assert!(
            !matches!(event.kind, EventKind::SessionSettled {}),
            "settled before deferred ticket"
        );
    }
}

#[tokio::test]
async fn basic_scripted_text_reaches_assistant_final_and_settles() {
    let Some(mut harness) = harness(json!({"responses":[text("hello from Python")]})).await else {
        return;
    };
    let id = create(&harness, "guard").await;
    let mut stream = harness.events(&id, 0).await;
    submit(&harness, &id, "basic", "now").await;
    let events = stream.settled().await;
    assert_final(&events, "hello from Python");
    assert!(events.iter().any(
        |event| matches!(&event.kind, EventKind::AssistantDelta { text } if !text.is_empty())
    ));
    assert_eq!(events.iter().filter(|event| matches!(&event.kind, EventKind::MessageCompleted { message_id, outcome: MessageOutcome::Completed, .. } if message_id.as_str() == "basic")).count(), 1);
    assert_eq!(harness.snapshot(&id)["script_cursor"], 1);
    harness.kill().await;
}

#[tokio::test]
async fn scripted_read_executes_daemon_tool_and_streams_started_ended() {
    let Some(mut harness) = harness(json!({"responses":[
        tool("read", json!({"path":"fixture.txt"}), "read-call"), text("read complete")
    ]}))
    .await
    else {
        return;
    };
    std::fs::write(
        harness.workspace().join("fixture.txt"),
        "workspace fixture\n",
    )
    .unwrap();
    let id = create(&harness, "guard").await;
    let mut stream = harness.events(&id, 0).await;
    submit(&harness, &id, "read-message", "now").await;
    let events = stream.settled().await;
    let started = events.iter().position(|event| matches!(&event.kind, EventKind::ToolStarted { tool_call_id, name, .. } if tool_call_id == "read-call" && name == "read")).expect("read tool.started");
    let ended = events.iter().position(|event| matches!(&event.kind, EventKind::ToolEnded { tool_call_id, result, is_error: false, .. } if tool_call_id == "read-call" && result["content"] == "workspace fixture\n")).expect("read tool.ended with actual file contents");
    assert!(started < ended);
    assert_final(&events, "read complete");
    harness.kill().await;
}

#[tokio::test]
async fn ask_user_ticket_resolved_by_cli_resumes_same_tool_call() {
    let Some(mut harness) = harness(json!({"responses":[
        tool("ask_user", json!({"question":"Choose a color", "options":["blue","green"]}), "ask-call"),
        text("blue selected")
    ]})).await else { return; };
    let id = create(&harness, "guard").await;
    let mut stream = harness.events(&id, 0).await;
    submit(&harness, &id, "question", "now").await;
    let ticket = ticket(&mut stream).await;
    assert_eq!(ticket.kind, TicketKind::Info);
    assert_eq!(ticket.tool_call_id, "ask-call");
    let deferred = stream.settled().await;
    assert!(deferred.iter().any(|event| matches!(&event.kind, EventKind::MessageCompleted { message_id, outcome: MessageOutcome::Deferred, .. } if message_id.as_str() == "question")));
    assert_eq!(
        harness
            .request("GET", &format!("/v1/sessions/{id}"), None)
            .await
            .body["status"],
        "waiting_human"
    );
    assert_eq!(
        harness.snapshot(&id)["pending_deferred"][0]["tool_call_id"],
        "ask-call"
    );
    let output = harness
        .cli(
            env!("CARGO_BIN_EXE_e2e-openside"),
            &["ticket", "resolve", ticket.id.as_str(), "blue"],
            None,
        )
        .await;
    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let resumed = stream.settled().await;
    assert_final(&resumed, "blue selected");
    assert!(resumed.iter().any(|event| matches!(&event.kind, EventKind::TicketResolved { ticket_id, resolution, .. } if ticket_id == &ticket.id && resolution["answer"] == "blue")));
    let snapshot = harness.snapshot(&id);
    assert!(snapshot["pending_deferred"].as_array().unwrap().is_empty());
    let returns: Vec<_> = snapshot["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|message| message["parts"].as_array().into_iter().flatten())
        .filter(|part| part["part_kind"] == "tool-return" && part["tool_call_id"] == "ask-call")
        .collect();
    assert_eq!(
        returns.len(),
        1,
        "deferred tool must resume exactly once with original ID"
    );
    assert_eq!(returns[0]["content"]["answer"], "blue");
    assert_eq!(
        harness
            .request(
                "POST",
                &format!("/v1/tickets/{}/resolve", ticket.id),
                Some(json!({"value":"green"}))
            )
            .await
            .status,
        409
    );
    harness.kill().await;
}

#[tokio::test]
async fn read_only_gate_hides_mutations_and_unknown_write_retries_without_touching_file() {
    let Some(mut harness) = harness(json!({"responses":[
        tool("write", json!({"path":"untouched.txt","content":"changed"}), "hidden-write"),
        text("write unavailable")
    ]}))
    .await
    else {
        return;
    };
    let file = harness.workspace().join("untouched.txt");
    std::fs::write(&file, "original").unwrap();
    let id = create(&harness, "read-only").await;
    let mut stream = harness.events(&id, 0).await;
    submit(&harness, &id, "readonly", "now").await;
    let events = stream.settled().await;
    let tools = events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::RuntimeStarted { tools, .. } => Some(tools),
            _ => None,
        })
        .expect("sidecar ready tool allowlist");
    assert!(tools.iter().any(|name| name == "read"));
    for forbidden in ["write", "edit", "bash", "browser_act"] {
        assert!(
            !tools.iter().any(|name| name == forbidden),
            "read-only exposes {forbidden}"
        );
    }
    assert!(!tools.iter().any(|name| name.starts_with("vault_")));
    assert!(
        !events.iter().any(
            |event| matches!(&event.kind, EventKind::ToolStarted { name, .. } if name == "write")
        ),
        "unknown tool reached daemon execution"
    );
    assert_final(&events, "write unavailable");
    assert_eq!(std::fs::read_to_string(file).unwrap(), "original");
    let snapshot = harness.snapshot(&id);
    let retry = snapshot["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|message| message["parts"].as_array().into_iter().flatten())
        .find(|part| part["part_kind"] == "retry-prompt" && part["tool_call_id"] == "hidden-write")
        .expect("unknown-tool ModelRetry recorded in conversation");
    let explanation = retry["content"].to_string().to_lowercase();
    assert!(
        explanation.contains("unknown tool") || explanation.contains("not available"),
        "not an unknown-tool rejection: {retry}"
    );
    harness.kill().await;
}

#[tokio::test]
async fn sidecar_sigkill_restores_snapshot_preserves_queue_and_completes_each_id_once() {
    let Some(mut harness) = harness(json!({"responses":[
        text("checkpoint reply"), text("second reply"), text("third reply")
    ],"delay_ms":1500}))
    .await
    else {
        return;
    };
    let id = create(&harness, "guard").await;
    let mut stream = harness.events(&id, 0).await;
    submit(&harness, &id, "checkpoint", "now").await;
    let mut events = stream.settled().await;
    assert_final(&events, "checkpoint reply");
    let checkpoint = harness.snapshot(&id);
    assert_eq!(checkpoint["script_cursor"], 1);
    let original_pid = harness.sidecar_pid();
    submit(&harness, &id, "interrupted", "now").await;
    loop {
        let event = stream.next().await;
        let injected = matches!(&event.kind, EventKind::MessageInjected { message_id } if message_id.as_str() == "interrupted");
        events.push(event);
        if injected {
            break;
        }
    }
    submit(&harness, &id, "second", "queue").await;
    submit(&harness, &id, "third", "queue").await;
    let queue = harness
        .request("GET", &format!("/v1/sessions/{id}"), None)
        .await;
    assert_eq!(queue.body["queue"].as_array().unwrap().len(), 2);
    assert_eq!(harness.kill_sidecar(), original_pid);
    loop {
        let event = stream.next().await;
        let done = matches!(event.kind, EventKind::SessionSettled {}) && events.iter().any(|event| matches!(&event.kind, EventKind::MessageCompleted { message_id, outcome: MessageOutcome::Completed, .. } if message_id.as_str() == "third"));
        events.push(event);
        if done {
            break;
        }
    }
    let replacement_pid = harness.sidecar_pid();
    assert_ne!(replacement_pid, original_pid);
    let mut results: HashMap<&str, Vec<MessageOutcome>> = HashMap::new();
    let mut injected = HashMap::new();
    for event in &events {
        match &event.kind {
            EventKind::MessageCompleted {
                message_id,
                outcome,
                ..
            } => {
                results
                    .entry(message_id.as_str())
                    .or_default()
                    .push(*outcome);
            }
            EventKind::MessageInjected { message_id } => {
                *injected.entry(message_id.as_str()).or_insert(0) += 1;
            }
            _ => {}
        }
    }
    assert_eq!(
        results,
        HashMap::from([
            ("checkpoint", vec![MessageOutcome::Completed]),
            ("interrupted", vec![MessageOutcome::Failed]),
            ("second", vec![MessageOutcome::Completed]),
            ("third", vec![MessageOutcome::Completed])
        ])
    );
    assert_eq!(
        injected,
        HashMap::from([
            ("checkpoint", 1),
            ("interrupted", 1),
            ("second", 1),
            ("third", 1)
        ])
    );
    assert!(events.iter().any(
        |event| matches!(&event.kind, EventKind::RuntimeError { code, .. } if code == "interrupted")
    ));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.kind, EventKind::RuntimeStarted { .. }))
            .count(),
        2
    );
    for pair in events.windows(2) {
        assert_eq!(pair[1].seq, pair[0].seq + 1);
    }
    let finals: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::AssistantFinal { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        finals,
        ["checkpoint reply", "second reply", "third reply"],
        "saved script cursor must survive crash; interrupted request must not replay"
    );
    let restored = harness.snapshot(&id);
    assert_eq!(restored["script_cursor"], 3);
    let history = restored["messages"].to_string();
    assert!(
        history.contains("checkpoint reply"),
        "resume_state lost checkpoint history"
    );
    assert!(history.contains("second") && history.contains("third"));
    assert!(
        !history.contains("interrupted"),
        "crashed uncommitted turn leaked into resume_state"
    );
    assert!(harness
        .request("GET", &format!("/v1/sessions/{id}"), None)
        .await
        .body["queue"]
        .as_array()
        .unwrap()
        .is_empty());
    harness.kill().await;
}
