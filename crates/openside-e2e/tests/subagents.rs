//! Subagents with the real Python sidecar: parent-session tickets, restart recovery,
//! cancel/steer, per-subagent models and the SSE opt-out (ADR-020).

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use openside_e2e::{Harness, SseReader};
use openside_proto::{Event, EventKind, MessageOutcome, SubagentStatus, Ticket, TicketKind};
use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_e2e-opensided");

fn text(text: &str) -> Value {
    json!({"parts":[{"type":"text","text":text}]})
}
fn tool(name: &str, args: Value, call: &str) -> Value {
    json!({"parts":[{"type":"tool_call","name":name,"args":args,"id":call}]})
}
fn delegate(call: &str, args: Value) -> Value {
    tool("delegate", args, call)
}
async fn create(harness: &Harness) -> String {
    let response = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"title":"Subagents", "mode":"guard", "workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(response.status, 201, "{}", response.body);
    response.body["id"].as_str().unwrap().to_owned()
}
async fn submit(harness: &Harness, id: &str, mid: &str) {
    let response = harness
        .request(
            "POST",
            &format!("/v1/sessions/{id}/messages"),
            Some(json!({"message_id":mid,"text":mid,"mode":"now"})),
        )
        .await;
    assert_eq!(response.status, 202, "{}", response.body);
}
async fn subagents(harness: &Harness, id: &str) -> Vec<Value> {
    let response = harness
        .request("GET", &format!("/v1/sessions/{id}/subagents"), None)
        .await;
    assert_eq!(response.status, 200, "{}", response.body);
    response.body["subagents"].as_array().unwrap().clone()
}
/// Read until a ticket opens, returning it and the events before it.
async fn ticket(stream: &mut SseReader) -> (Ticket, Vec<Event>) {
    let mut seen = Vec::new();
    loop {
        let event = stream.next().await;
        if let EventKind::TicketOpened { ticket } = &event.kind {
            return (*ticket.clone(), seen);
        }
        assert!(
            !matches!(event.kind, EventKind::SessionSettled {}),
            "settled before the child ticket: {seen:?}"
        );
        seen.push(event);
    }
}
async fn until(stream: &mut SseReader, done: impl Fn(&EventKind) -> bool) -> Vec<Event> {
    let mut events = Vec::new();
    loop {
        let event = stream.next().await;
        let stop = done(&event.kind);
        events.push(event);
        if stop {
            return events;
        }
    }
}
fn started(events: &[Event]) -> (String, String, Vec<String>) {
    events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::SubagentStarted {
                subagent_id,
                model,
                tools,
                ..
            } => Some((subagent_id.clone(), model.clone(), tools.clone())),
            _ => None,
        })
        .expect("subagent.started")
}
fn final_text(events: &[Event], expected: &str) -> bool {
    events
        .iter()
        .any(|event| matches!(&event.kind, EventKind::AssistantFinal { text } if text == expected))
}
fn ended(events: &[Event], id: &str) -> Option<SubagentStatus> {
    events.iter().find_map(|event| match &event.kind {
        EventKind::SubagentEnded {
            subagent_id,
            status,
            ..
        } if subagent_id == id => Some(*status),
        _ => None,
    })
}

fn approval_script(outside: &str) -> Value {
    json!({
        "responses": [
            delegate("delegate-call", json!({"task":"Write the report outside the workspace","tools":["write"]})),
            text("parent done"),
        ],
        "subagents": {
            "delegate-call": [
                tool("write", json!({"path":outside,"content":"from child"}), "child-write"),
                text("child wrote the report"),
            ]
        }
    })
}

async fn child_waits_on_parent_ticket(harness: &Harness) -> (String, String, Ticket) {
    let id = create(harness).await;
    let mut stream = harness.events(&id, 0).await;
    submit(harness, &id, "delegate").await;
    let (ticket, before) = ticket(&mut stream).await;
    let (subagent, model, tools) = started(&before);
    assert_eq!(model, "script", "child defaults to the session model");
    assert_eq!(tools, ["write"]);
    assert_eq!(ticket.kind, TicketKind::Confirm);
    assert_eq!(ticket.session_id.as_str(), id);
    assert_eq!(ticket.tool_call_id, "child-write");
    assert_eq!(
        ticket.origin.subagent_id.as_deref(),
        Some(subagent.as_str())
    );
    let deferred = stream.settled().await;
    assert!(deferred.iter().any(|event| matches!(
        &event.kind,
        EventKind::MessageCompleted {
            outcome: MessageOutcome::Deferred,
            ..
        }
    )));
    assert!(
        !final_text(&deferred, "parent done"),
        "parent delegate call must stay pending"
    );
    let listed = harness
        .request("GET", "/v1/tickets?status=pending", None)
        .await;
    assert_eq!(
        listed.body["tickets"][0]["origin"]["subagent_id"],
        subagent.as_str()
    );
    assert_eq!(
        harness
            .request("GET", &format!("/v1/sessions/{id}"), None)
            .await
            .body["status"],
        "waiting_human"
    );
    let children = subagents(harness, &id).await;
    assert_eq!(children.len(), 1);
    assert_eq!(children[0]["status"], "waiting_human");
    assert_eq!(children[0]["parent_tool_call_id"], "delegate-call");
    let snapshot = harness.snapshot(&id);
    assert_eq!(
        snapshot["pending_delegates"][0]["tool_call_id"],
        "delegate-call"
    );
    assert_eq!(snapshot["subagents"][0]["status"], "waiting_human");
    (id, subagent, ticket)
}

async fn approve_and_finish(harness: &Harness, id: &str, subagent: &str, ticket: &Ticket) {
    let mut stream = harness.events(id, 0).await;
    let resolved = harness
        .human_request(
            "POST",
            &format!("/v1/tickets/{}/resolve", ticket.id),
            Some(json!({"value":true})),
        )
        .await;
    assert_eq!(resolved.status, 200, "{}", resolved.body);
    let events = until(
        &mut stream,
        |kind| matches!(kind, EventKind::AssistantFinal { text } if text == "parent done"),
    )
    .await;
    assert_eq!(ended(&events, subagent), Some(SubagentStatus::Completed));
    stream.settled().await;
    let children = subagents(harness, id).await;
    assert_eq!(children[0]["status"], "completed");
    let snapshot = harness.snapshot(id);
    assert_eq!(snapshot["pending_delegates"], json!([]));
    assert!(snapshot["messages"]
        .to_string()
        .contains("child wrote the report"));
}

#[tokio::test]
async fn child_approval_lands_in_parent_tickets_and_resumes_child_then_parent() {
    let Some(mut harness) = Harness::start(BIN, &json!({"responses":[]})).await else {
        return;
    };
    let outside = harness.root().join("outside.txt");
    harness.replace_script(&approval_script(outside.to_str().unwrap()));
    let (id, subagent, ticket) = child_waits_on_parent_ticket(&harness).await;
    assert!(!outside.exists(), "write must wait for approval");
    approve_and_finish(&harness, &id, &subagent, &ticket).await;
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "from child");
    harness.kill().await;
}

#[tokio::test]
async fn waiting_child_survives_daemon_crash_and_resumes_once() {
    let Some(mut harness) = Harness::start(BIN, &json!({"responses":[]})).await else {
        return;
    };
    let outside = harness.root().join("outside.txt");
    harness.replace_script(&approval_script(outside.to_str().unwrap()));
    let (id, subagent, ticket) = child_waits_on_parent_ticket(&harness).await;
    harness.restart().await;
    let children = subagents(&harness, &id).await;
    assert_eq!(
        children[0]["status"], "waiting_human",
        "waiting child survives restart"
    );
    approve_and_finish(&harness, &id, &subagent, &ticket).await;
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "from child");
    harness.kill().await;
}

#[tokio::test]
async fn cancel_waiting_child_rejects_its_ticket_and_returns_to_parent() {
    let script = json!({
        "responses": [delegate("delegate-call", json!({"task":"Ask the user"})), text("parent done")],
        "subagents": {"delegate-call": [
            tool("ask_user", json!({"question":"Which color?","options":["blue","green"]}), "child-ask"),
            text("child answered"),
        ]}
    });
    let Some(mut harness) = Harness::start(BIN, &script).await else {
        return;
    };
    let id = create(&harness).await;
    let mut stream = harness.events(&id, 0).await;
    submit(&harness, &id, "delegate").await;
    let (ticket, before) = ticket(&mut stream).await;
    let (subagent, _, tools) = started(&before);
    assert!(
        !tools.iter().any(|name| name == "delegate"),
        "children cannot delegate"
    );
    assert_eq!(ticket.kind, TicketKind::Info);
    stream.settled().await;
    let cancelled = harness
        .request(
            "POST",
            &format!("/v1/sessions/{id}/subagents/{subagent}/cancel"),
            None,
        )
        .await;
    assert_eq!(cancelled.status, 200, "{}", cancelled.body);
    assert_eq!(cancelled.body["status"], "cancelled");
    let events = until(
        &mut stream,
        |kind| matches!(kind, EventKind::AssistantFinal { text } if text == "parent done"),
    )
    .await;
    assert!(events.iter().any(|event| matches!(&event.kind, EventKind::TicketRejected { ticket_id, reason } if *ticket_id == ticket.id && reason == "subagent_cancelled")));
    assert_eq!(ended(&events, &subagent), Some(SubagentStatus::Cancelled));
    stream.settled().await;
    assert!(harness.snapshot(&id)["messages"]
        .to_string()
        .contains("cancelled"));
    let again = harness
        .request(
            "POST",
            &format!("/v1/sessions/{id}/subagents/{subagent}/cancel"),
            None,
        )
        .await;
    assert_eq!(
        (again.status, again.body["code"].as_str()),
        (409, Some("subagent_finished"))
    );
    let unknown = harness
        .request(
            "POST",
            &format!("/v1/sessions/{id}/subagents/unknown/steer"),
            Some(json!({"text":"x"})),
        )
        .await;
    assert_eq!(unknown.status, 404);
    let resolve = harness
        .request(
            "POST",
            &format!("/v1/tickets/{}/resolve", ticket.id),
            Some(json!({"value":"blue"})),
        )
        .await;
    assert_eq!(resolve.status, 409, "a cancelled child's ticket is closed");
    harness.kill().await;
}

#[tokio::test]
async fn steer_reaches_running_child_at_its_next_model_request() {
    let script = json!({
        "responses": [delegate("delegate-call", json!({"task":"Summarize"})), text("parent done")],
        "subagents": {"delegate-call": [
            {"parts":[{"type":"wait_steer","timeout_ms":20000},{"type":"text","text":"first draft"}]},
            text("steered summary"),
        ]}
    });
    let Some(mut harness) = Harness::start(BIN, &script).await else {
        return;
    };
    let id = create(&harness).await;
    let mut stream = harness.events(&id, 0).await;
    submit(&harness, &id, "delegate").await;
    let events = until(&mut stream, |kind| matches!(kind, EventKind::SubagentProgress { progress, .. } if progress["kind"] == "model_request")).await;
    let (subagent, _, _) = started(&events);
    let steered = harness
        .request(
            "POST",
            &format!("/v1/sessions/{id}/subagents/{subagent}/steer"),
            Some(json!({"text":"Focus on the totals"})),
        )
        .await;
    assert_eq!(steered.status, 202, "{}", steered.body);
    assert_eq!(steered.body["status"], "running");
    let events = stream.settled().await;
    assert!(final_text(&events, "parent done"));
    assert!(events.iter().any(|event| matches!(&event.kind, EventKind::SubagentProgress { subagent_id, progress } if *subagent_id == subagent && progress["kind"] == "steer_queued")));
    assert_eq!(ended(&events, &subagent), Some(SubagentStatus::Completed));
    let snapshot = harness.snapshot(&id);
    let child = snapshot["subagents"][0]["messages"].to_string();
    assert!(
        child.contains("Focus on the totals"),
        "steer entered the child conversation"
    );
    assert!(
        !snapshot["messages"]
            .to_string()
            .contains("Focus on the totals"),
        "parent conversation is untouched"
    );
    assert!(snapshot["messages"].to_string().contains("steered summary"));
    let empty = harness
        .request(
            "POST",
            &format!("/v1/sessions/{id}/subagents/{subagent}/steer"),
            Some(json!({"text":"late"})),
        )
        .await;
    assert_eq!(empty.status, 409);
    harness.kill().await;
}

#[tokio::test]
async fn child_model_is_session_default_or_configured_fallback_and_sse_can_opt_out() {
    let script = json!({
        "responses": [
            delegate("fallback-call", json!({"task":"Use the fallback","model":"script-fallback"})),
            delegate("unknown-call", json!({"task":"Use an unknown model","model":"not-configured"})),
            text("parent done"),
        ],
        "subagents": {"fallback-call": [text("fallback child")]}
    });
    let Some(mut harness) = Harness::prepare(BIN, &script) else {
        return;
    };
    let config = harness.root().join("config/openside/config.toml");
    let text_config = std::fs::read_to_string(&config).unwrap();
    std::fs::write(
        &config,
        text_config.replacen(
            "default = \"script\"\n",
            "default = \"script\"\nfallback = [\"script-fallback\"]\n",
            1,
        ),
    )
    .unwrap();
    harness.launch().await;
    let id = create(&harness).await;
    let mut stream = harness.events(&id, 0).await;
    submit(&harness, &id, "delegate").await;
    let events = until(
        &mut stream,
        |kind| matches!(kind, EventKind::AssistantFinal { text } if text == "parent done"),
    )
    .await;
    let (subagent, model, _) = started(&events);
    assert_eq!(model, "script-fallback");
    assert_eq!(ended(&events, &subagent), Some(SubagentStatus::Completed));
    assert!(events.iter().any(|event| matches!(&event.kind, EventKind::ToolEnded { tool_call_id, result, is_error: true } if tool_call_id == "unknown-call" && result["reason"] == "model_not_allowed")));
    let children = subagents(&harness, &id).await;
    assert_eq!(children.len(), 1, "a refused delegate starts no child");
    assert_eq!(children[0]["model"], "script-fallback");
    stream.settled().await;
    let mut quiet = harness.events_query(&id, 0, "subagents=false").await;
    let replay = until(
        &mut quiet,
        |kind| matches!(kind, EventKind::AssistantFinal { text } if text == "parent done"),
    )
    .await;
    assert!(replay.iter().all(|event| !matches!(
        event.kind,
        EventKind::SubagentStarted { .. }
            | EventKind::SubagentLifecycle { .. }
            | EventKind::SubagentProgress { .. }
            | EventKind::SubagentEnded { .. }
    )));
    assert!(replay.iter().any(
        |event| matches!(&event.kind, EventKind::ToolStarted { name, .. } if name == "delegate")
    ));
    harness.kill().await;
}
