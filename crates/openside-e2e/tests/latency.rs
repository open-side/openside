//! PP-9: the model process is started once per session and kept warm, so the second message
//! does not pay the startup the first one did.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use openside_e2e::Harness;
use openside_proto::{Event, EventKind};
use serde_json::json;

fn latency_ms(events: &[Event], message: &str) -> i64 {
    let queued = events
        .iter()
        .find(|event| matches!(&event.kind, EventKind::MessageQueued { message_id, .. } if message_id.as_str() == message))
        .unwrap_or_else(|| panic!("{message} not queued"));
    let first = events
        .iter()
        .find(|event| {
            event.seq > queued.seq
                && matches!(
                    event.kind,
                    EventKind::AssistantDelta { .. } | EventKind::AssistantFinal { .. }
                )
        })
        .unwrap_or_else(|| panic!("{message} produced no output"));
    (first.ts - queued.ts).num_milliseconds()
}

#[tokio::test]
async fn second_message_reuses_the_warm_sidecar_and_skips_startup() {
    let Some(mut harness) = Harness::start(
        env!("CARGO_BIN_EXE_e2e-opensided"),
        &json!({"responses":[
            {"parts":[{"type":"text","text":"first reply"}]},
            {"parts":[{"type":"text","text":"second reply"}]}
        ]}),
    )
    .await
    else {
        return;
    };
    let created = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"title":"warm","mode":"guard","workspace":harness.workspace()})),
        )
        .await;
    let id = created.body["id"].as_str().unwrap().to_owned();
    let mut stream = harness.events(&id, 0).await;
    let mut events = Vec::new();
    let mut pids = Vec::new();
    for message in ["one", "two"] {
        let response = harness
            .request(
                "POST",
                &format!("/v1/sessions/{id}/messages"),
                Some(json!({"message_id":message,"text":message,"mode":"now"})),
            )
            .await;
        assert_eq!(response.status, 202, "{}", response.body);
        events.extend(stream.settled().await);
        pids.push(harness.sidecar_pid());
    }
    assert_eq!(
        pids[0], pids[1],
        "the sidecar was replaced between messages"
    );
    let starts: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::RuntimeStarted { startup_ms, .. } => Some(*startup_ms),
            _ => None,
        })
        .collect();
    assert_eq!(
        starts.len(),
        1,
        "the runtime must start once for two messages: {starts:?}"
    );

    let (first, second) = (latency_ms(&events, "one"), latency_ms(&events, "two"));
    let startup = i64::try_from(starts[0]).unwrap();
    eprintln!(
        "PP-9: first message {first} ms, second message {second} ms, measured startup {startup} ms"
    );
    assert!(
        second + startup / 2 <= first,
        "second message ({second} ms) should be at least half the startup ({startup} ms) faster than the first ({first} ms)"
    );
    harness.kill().await;
}
