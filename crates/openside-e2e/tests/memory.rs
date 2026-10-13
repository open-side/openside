//! Real Python sidecar: a fact written with `memory_write` in one session is recalled by
//! `memory_search` in a later session after a daemon restart; a secret write is refused.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use openside_e2e::Harness;
use openside_proto::{Event, EventKind};
use serde_json::{json, Value};

const FACT: &str = "The team offsite is booked at the lakeside cabin in October";
const CANARY: &str = "ghp_e2ememorycanary0123456789abcdefABCD";

fn text(text: &str) -> Value {
    json!({"parts":[{"type":"text","text":text}]})
}
fn tool(name: &str, args: Value, call: &str) -> Value {
    json!({"parts":[{"type":"tool_call","name":name,"args":args,"id":call}]})
}
async fn session(harness: &Harness) -> String {
    let response = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"title":"Memory", "mode":"guard", "workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(response.status, 201, "{}", response.body);
    response.body["id"].as_str().unwrap().to_owned()
}
async fn turn(harness: &Harness, id: &str, mid: &str) -> Vec<Event> {
    let mut stream = harness.events(id, 0).await;
    let response = harness
        .request(
            "POST",
            &format!("/v1/sessions/{id}/messages"),
            Some(json!({"message_id":mid,"text":mid,"mode":"now"})),
        )
        .await;
    assert_eq!(response.status, 202, "{}", response.body);
    stream.settled().await
}
fn result<'a>(events: &'a [Event], call: &str) -> (&'a Value, bool) {
    events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::ToolEnded {
                card: _,
                tool_call_id,
                result,
                is_error,
            } if tool_call_id == call => Some((result, *is_error)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no tool.ended for {call}: {events:?}"))
}

#[tokio::test]
async fn memory_written_in_one_session_is_found_by_a_later_session() {
    let script = json!({"responses":[
        tool("memory_write", json!({"content":format!("deploy token {CANARY}")}), "write-secret"),
        tool("memory_write", json!({"content":FACT,"tags":["team"]}), "write-fact"),
        text("noted"),
    ]});
    let Some(mut harness) = Harness::start(env!("CARGO_BIN_EXE_e2e-opensided"), &script).await
    else {
        return;
    };
    let first = session(&harness).await;
    let events = turn(&harness, &first, "remember").await;
    let (refused, is_error) = result(&events, "write-secret");
    assert!(is_error);
    assert_eq!(refused["reason"], "secret_detected", "{refused}");
    let started = events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::ToolStarted {
                tool_call_id, args, ..
            } if tool_call_id == "write-secret" => Some(args),
            _ => None,
        })
        .unwrap();
    assert_eq!(started["content"], "[REDACTED]", "{started}");
    let (stored, is_error) = result(&events, "write-fact");
    assert!(!is_error, "{stored}");
    assert_eq!(stored["stored"], true);

    // The later session runs on a fresh daemon process over the same data directory.
    harness.replace_script(&json!({"responses":[
        tool("memory_search", json!({"query":"where is the offsite"}), "recall"),
        text("found it"),
    ]}));
    harness.restart().await;
    let second = session(&harness).await;
    assert_ne!(first, second);
    let events = turn(&harness, &second, "recall").await;
    let (found, is_error) = result(&events, "recall");
    assert!(!is_error, "{found}");
    assert_eq!(found["results"][0]["content"], FACT, "{found}");
    assert_eq!(found["results"][0]["tags"], json!(["team"]));

    let listed = harness.request("GET", "/v1/memory?q=offsite", None).await;
    assert_eq!(listed.status, 200);
    assert_eq!(
        listed.body["hits"].as_array().unwrap().len(),
        1,
        "{}",
        listed.body
    );
    let history = harness
        .request(
            "GET",
            &format!("/v1/memory/{}/history", stored["id"].as_str().unwrap()),
            None,
        )
        .await;
    assert_eq!(
        history.body["versions"][0]["actor"],
        format!("agent:{first}")
    );

    harness.kill().await;
    // The model's own tool-call arguments live in the sidecar transcript; memory storage
    // and the durable tool.started event must never hold the refused value.
    for entry in std::fs::read_dir(harness.root().join("data")).unwrap() {
        let path = entry.unwrap().path();
        if path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("memory.db")
        {
            let bytes = std::fs::read(&path).unwrap();
            assert!(
                !bytes
                    .windows(CANARY.len())
                    .any(|part| part == CANARY.as_bytes()),
                "canary reached {}",
                path.display()
            );
        }
    }
}
