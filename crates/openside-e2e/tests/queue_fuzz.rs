//! A-4 / PP-10: queue steering, editing, deleting, promoting and reordering against the real
//! Python sidecar, and a seeded random operation sequence (with a daemon crash) that must
//! deliver every message exactly once or drop it exactly once.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use openside_e2e::{Harness, SseReader};
use openside_proto::{Event, EventKind, MessageOutcome};
use serde_json::{json, Value};

fn text(text: &str) -> Value {
    json!({"parts":[{"type":"text","text":text}]})
}

async fn create(harness: &Harness) -> String {
    let response = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"title":"queue","mode":"guard","workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(response.status, 201, "{}", response.body);
    response.body["id"].as_str().unwrap().to_owned()
}

async fn submit(harness: &Harness, id: &str, mid: &str, mode: &str) -> u16 {
    harness
        .request_as(
            "POST",
            &format!("/v1/sessions/{id}/messages"),
            Some(json!({"message_id":mid,"text":mid,"mode":mode})),
            (mode == "queue" && mid != "promoted-p").then_some("mcp"),
        )
        .await
        .status
}

async fn queue_ids(harness: &Harness, id: &str) -> Vec<String> {
    let session = harness
        .request("GET", &format!("/v1/sessions/{id}"), None)
        .await;
    session.body["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| message["id"].as_str().unwrap().to_owned())
        .collect()
}

async fn until(stream: &mut SseReader, events: &mut Vec<Event>, done: impl Fn(&Event) -> bool) {
    loop {
        let event = stream.next().await;
        let stop = done(&event);
        events.push(event);
        if stop {
            return;
        }
    }
}

fn seq_of(events: &[Event], predicate: impl Fn(&EventKind) -> bool) -> u64 {
    events
        .iter()
        .find(|event| predicate(&event.kind))
        .unwrap_or_else(|| panic!("event missing in {events:#?}"))
        .seq
}

/// Texts of the snapshot's conversation as `(kind, part_kind, text)` triples, per message.
fn conversation(snapshot: &Value) -> Vec<Vec<(String, String)>> {
    snapshot["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| {
            message["parts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|part| {
                    let kind = part["part_kind"].as_str().unwrap().to_owned();
                    let content = match &part["content"] {
                        Value::String(text) => text.clone(),
                        other => other.to_string(),
                    };
                    (kind, content)
                })
                .collect()
        })
        .collect()
}

/// A-4: a steer message reaches the model at the next model-request boundary of the run that
/// is already in flight (after the tool result, before the next answer); a queued message
/// waits for the run to settle and starts its own. Edit, delete, promote and reorder act on
/// the pending queue and every mutation of a delivered message is a conflict.
#[tokio::test]
async fn steer_lands_at_the_next_request_boundary_and_queue_edits_are_honored() {
    let Some(mut harness) = Harness::start(
        env!("CARGO_BIN_EXE_e2e-opensided"),
        &json!({"responses":[
            json!({"parts":[{"type":"tool_call","name":"read","args":{"path":"fixture.txt"},"id":"read-call"}]}),
            text("first done"), text("second done"), text("third done"), text("fourth done"),
        ],"delay_ms":1500}),
    )
    .await
    else {
        return;
    };
    std::fs::write(harness.workspace().join("fixture.txt"), "fixture body\n").unwrap();
    let id = create(&harness).await;
    let mut stream = harness.events(&id, 0).await;
    let mut events = Vec::new();

    assert_eq!(submit(&harness, &id, "a", "now").await, 202);
    until(&mut stream, &mut events, |event| {
        matches!(event.kind, EventKind::TurnStarted {})
    })
    .await;
    // Request 1 is now waiting on the (slow) model.
    assert_eq!(submit(&harness, &id, "steer-s", "now").await, 202);
    for mid in ["q1", "q2", "q3", "promoted-p"] {
        assert_eq!(submit(&harness, &id, mid, "queue").await, 202);
    }
    let promote = harness
        .request(
            "POST",
            &format!("/v1/sessions/{id}/queue/promoted-p/promote"),
            None,
        )
        .await;
    assert_eq!(
        promote.status, 409,
        "owner messages already steer: {}",
        promote.body
    );
    let edit = harness
        .request(
            "PATCH",
            &format!("/v1/sessions/{id}/queue/q1"),
            Some(json!({"text":"q1-edited"})),
        )
        .await;
    assert_eq!(
        (edit.status, edit.body["text"].as_str()),
        (200, Some("q1-edited"))
    );
    let delete = harness
        .request("DELETE", &format!("/v1/sessions/{id}/queue/q2"), None)
        .await;
    assert_eq!(delete.status, 200, "{}", delete.body);
    let moved = harness
        .request(
            "POST",
            &format!("/v1/sessions/{id}/queue/q3/move"),
            Some(json!({"position":0})),
        )
        .await;
    assert_eq!(moved.status, 200, "{}", moved.body);
    assert_eq!(queue_ids(&harness, &id).await, ["q3", "q1"]);

    // Delivered (steer, promoted) and unknown messages cannot be edited or reordered.
    for (method, path, body) in [
        ("PATCH", "queue/steer-s", json!({"text":"late"})),
        ("POST", "queue/steer-s/move", json!({"position":0})),
        ("POST", "queue/promoted-p/move", json!({"position":0})),
        ("POST", "queue/never-existed/move", json!({"position":0})),
        ("DELETE", "queue/promoted-p", Value::Null),
    ] {
        let response = harness
            .request(
                method,
                &format!("/v1/sessions/{id}/{path}"),
                Some(body).filter(|b| !b.is_null()),
            )
            .await;
        assert_eq!(response.status, 409, "{method} {path}: {}", response.body);
    }

    until(&mut stream, &mut events, |event| {
        matches!(&event.kind, EventKind::MessageCompleted { message_id, .. } if message_id.as_str() == "q1")
    })
    .await;
    until(&mut stream, &mut events, |event| {
        matches!(event.kind, EventKind::SessionSettled {})
    })
    .await;

    // Daemon-side order of events.
    let injected = |mid: &'static str| move |kind: &EventKind| matches!(kind, EventKind::MessageInjected { message_id } if message_id.as_str() == mid);
    let tool_started = seq_of(&events, |kind| {
        matches!(kind, EventKind::ToolStarted { .. })
    });
    let first_final = seq_of(
        &events,
        |kind| matches!(kind, EventKind::AssistantFinal { text, .. } if text == "first done"),
    );
    let (steer, promoted) = (
        seq_of(&events, injected("steer-s")),
        seq_of(&events, injected("promoted-p")),
    );
    assert!(
        steer < promoted && promoted < tool_started,
        "steer lanes are injected into the running run"
    );
    let (q3, q1) = (
        seq_of(&events, injected("q3")),
        seq_of(&events, injected("q1")),
    );
    assert!(
        first_final < q3 && q3 < q1,
        "queue messages wait for the settled run, in queue order"
    );
    assert!(!events.iter().any(|event| matches!(&event.kind, EventKind::MessageInjected { message_id } if message_id.as_str() == "q2")));
    assert!(events.iter().any(|event| matches!(&event.kind, EventKind::MessageDropped { message_id, .. } if message_id.as_str() == "q2")));

    // What the model actually received: both steers sit in the request that carries the tool
    // result, after it, before the answer to it; the queued messages open later runs.
    let messages = conversation(&harness.snapshot(&id));
    let carrier = messages
        .iter()
        .position(|parts| parts.iter().any(|(kind, _)| kind == "tool-return"))
        .expect("tool result request");
    let parts = &messages[carrier];
    let at = |needle: &str| {
        parts
            .iter()
            .position(|(kind, content)| kind == "user-prompt" && content.contains(needle))
    };
    let tool_return = parts
        .iter()
        .position(|(kind, _)| kind == "tool-return")
        .unwrap();
    let (s, p) = (
        at("steer-s").expect("steer in tool-return request"),
        at("promoted-p").expect("promoted in same request"),
    );
    assert!(
        tool_return < s && s < p,
        "tool result, then steer, then promoted: {parts:?}"
    );
    assert!(
        messages[carrier + 1]
            .iter()
            .any(|(kind, content)| kind == "text" && content == "first done"),
        "the steered request is answered by the next response: {messages:#?}"
    );
    let prompts: Vec<_> = messages
        .iter()
        .flatten()
        .filter(|(kind, _)| kind == "user-prompt")
        .map(|(_, content)| content.as_str())
        .collect();
    assert_eq!(prompts.len(), 5, "{prompts:?}");
    for (prompt, expected) in prompts
        .iter()
        .zip(["a", "steer-s", "promoted-p", "q3", "q1-edited"])
    {
        assert!(prompt.contains(expected), "{prompts:?}");
    }
    assert!(prompts.iter().all(|prompt| !prompt.contains("q2")));
    assert!(prompts
        .iter()
        .all(|prompt| !prompt.contains("q1\"") && *prompt != "q1"));
    harness.kill().await;
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, bound: usize) -> usize {
        usize::try_from(self.next() % bound as u64).unwrap()
    }
}

async fn fuzz(seed: u64) {
    let responses: Vec<_> = (0..400).map(|n| text(&format!("reply {n}"))).collect();
    let Some(mut harness) = Harness::start(
        env!("CARGO_BIN_EXE_e2e-opensided"),
        &json!({"responses":responses,"delay_ms":150}),
    )
    .await
    else {
        return;
    };
    let id = create(&harness).await;
    let mut rng = Rng(seed);
    let mut submitted: Vec<String> = Vec::new();
    let restart_at = 8 + rng.below(24);
    for step in 0..40 {
        tokio::time::sleep(Duration::from_millis(rng.below(30) as u64)).await;
        if step == restart_at {
            harness.restart().await;
        }
        let pick = |rng: &mut Rng, submitted: &[String]| {
            if submitted.is_empty() || rng.below(10) == 0 {
                "never-submitted".to_owned()
            } else {
                submitted[rng.below(submitted.len())].clone()
            }
        };
        match rng.below(10) {
            0..=4 => {
                let mid = format!("m{step}");
                let mode = ["now", "steer", "queue", "queue"][rng.below(4)];
                assert_eq!(submit(&harness, &id, &mid, mode).await, 202, "{mid}");
                submitted.push(mid.clone());
                if rng.below(4) == 0 {
                    // A retry of the same idempotency key is accepted and changes nothing.
                    assert_eq!(
                        submit(&harness, &id, &mid, mode).await,
                        202,
                        "duplicate {mid}"
                    );
                }
            }
            5 => {
                let mid = pick(&mut rng, &submitted);
                let response = harness
                    .request(
                        "PATCH",
                        &format!("/v1/sessions/{id}/queue/{mid}"),
                        Some(json!({"text": format!("{mid}-edit")})),
                    )
                    .await;
                assert!(
                    [200, 409].contains(&response.status),
                    "edit {mid}: {}",
                    response.body
                );
            }
            6 => {
                let mid = pick(&mut rng, &submitted);
                let response = harness
                    .request("DELETE", &format!("/v1/sessions/{id}/queue/{mid}"), None)
                    .await;
                assert!(
                    [200, 409].contains(&response.status),
                    "delete {mid}: {}",
                    response.body
                );
            }
            7 => {
                let mid = pick(&mut rng, &submitted);
                let response = harness
                    .request(
                        "POST",
                        &format!("/v1/sessions/{id}/queue/{mid}/promote"),
                        None,
                    )
                    .await;
                assert!(
                    [200, 409].contains(&response.status),
                    "promote {mid}: {}",
                    response.body
                );
            }
            8 => {
                let mid = pick(&mut rng, &submitted);
                let response = harness
                    .request(
                        "POST",
                        &format!("/v1/sessions/{id}/queue/{mid}/move"),
                        Some(json!({"position": rng.below(4)})),
                    )
                    .await;
                assert!(
                    [200, 409].contains(&response.status),
                    "move {mid}: {}",
                    response.body
                );
            }
            _ => {
                if rng.below(3) == 0 {
                    let response = harness
                        .request("POST", &format!("/v1/sessions/{id}/stop"), None)
                        .await;
                    assert_eq!(response.status, 200, "stop: {}", response.body);
                }
            }
        }
    }
    // Final queue message: after everything else it must run, proving the queue still drains.
    assert_eq!(submit(&harness, &id, "drain", "queue").await, 202);
    submitted.push("drain".into());
    // An idle session with an empty queue is only final once the log shows the drain message
    // finished: right after submission the status can still read idle.
    let events = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let session = harness
                .request("GET", &format!("/v1/sessions/{id}"), None)
                .await;
            if session.body["status"] == "idle"
                && session.body["queue"].as_array().unwrap().is_empty()
            {
                let last_seq = session.body["last_seq"].as_u64().unwrap();
                let mut stream = harness.events(&id, 0).await;
                let mut events = Vec::new();
                until(&mut stream, &mut events, |event| event.seq >= last_seq).await;
                let drained = events.iter().any(|event| {
                    matches!(&event.kind,
                    EventKind::MessageCompleted { message_id, outcome, .. }
                        if message_id.as_str() == "drain" && *outcome != MessageOutcome::Deferred)
                });
                if drained {
                    return events;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("seed {seed}: queue never drained"));
    for pair in events.windows(2) {
        assert_eq!(pair[1].seq, pair[0].seq + 1, "seed {seed}: sequence gap");
    }

    let mut completed: BTreeMap<String, Vec<MessageOutcome>> = BTreeMap::new();
    let mut dropped: BTreeMap<String, usize> = BTreeMap::new();
    let mut injected: BTreeMap<String, usize> = BTreeMap::new();
    let mut queued: BTreeMap<String, usize> = BTreeMap::new();
    for event in &events {
        match &event.kind {
            EventKind::MessageCompleted {
                message_id,
                outcome,
                ..
            } if *outcome != MessageOutcome::Deferred => {
                completed
                    .entry(message_id.to_string())
                    .or_default()
                    .push(*outcome);
            }
            EventKind::MessageDropped { message_id, .. } => {
                *dropped.entry(message_id.to_string()).or_default() += 1
            }
            EventKind::MessageInjected { message_id } => {
                *injected.entry(message_id.to_string()).or_default() += 1
            }
            EventKind::MessageQueued { message_id, .. } => {
                *queued.entry(message_id.to_string()).or_default() += 1
            }
            _ => {}
        }
    }
    let known: BTreeSet<_> = submitted.iter().cloned().collect();
    for mid in &known {
        assert_eq!(
            queued.get(mid),
            Some(&1),
            "seed {seed}: {mid} accepted once"
        );
        let done = completed.get(mid).map_or(0, Vec::len);
        let gone = dropped.get(mid).copied().unwrap_or(0);
        assert_eq!(done + gone, 1, "seed {seed}: {mid} must end exactly once (completed {done}, dropped {gone})\n{events:#?}");
        assert!(
            injected.get(mid).copied().unwrap_or(0) <= 1,
            "seed {seed}: {mid} injected twice"
        );
        assert_eq!(
            injected.get(mid).copied().unwrap_or(0),
            usize::from(done == 1),
            "seed {seed}: {mid} delivered iff it completed"
        );
    }
    assert!(
        completed.contains_key("drain"),
        "seed {seed}: the final queued message never ran: {completed:?}"
    );
    eprintln!(
        "seed {seed:#x}: {} messages, {} completed, {} dropped, {} events",
        known.len(),
        completed.len(),
        dropped.len(),
        events.len()
    );

    // The model saw each delivered message at most once, including recovery notices.
    let prompts: Vec<String> = harness.snapshot(&id)["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|message| message["parts"].as_array().unwrap().clone())
        .filter(|part| part["part_kind"] == "user-prompt")
        .map(|part| part["content"].to_string())
        .collect();
    let distinct: BTreeSet<_> = prompts.iter().collect();
    assert_eq!(
        distinct.len(),
        prompts.len(),
        "seed {seed}: duplicate prompts in the conversation: {prompts:?}"
    );
    harness.kill().await;
}

/// PP-10: random submit / duplicate / edit / delete / promote / move / stop, plus one daemon
/// crash, against the real sidecar for several seeds.
#[tokio::test]
async fn seeded_random_queue_operations_with_a_crash_deliver_each_message_once() {
    for seed in [
        0x9E37_79B9_7F4A_7C15,
        0xD1B5_4A32_D192_ED03,
        0x2545_F491_4F6C_DD1D,
    ] {
        fuzz(seed).await;
    }
}
