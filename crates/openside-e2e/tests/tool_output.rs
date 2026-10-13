//! Private output crosses the real daemon/sidecar boundary only inside a Task.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use openside_e2e::Harness;
use openside_proto::{EventKind, SessionId};
use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_e2e-opensided");

fn tool(name: &str, args: Value, id: &str) -> Value {
    json!({"parts":[{"type":"tool_call","name":name,"args":args,"id":id}]})
}

#[tokio::test]
async fn task_spills_reads_range_and_chief_receives_only_report() {
    let expected = "X".repeat(64);
    let script = json!({
        "responses": [
            tool("create_task", json!({"goal":"Inspect the large output and verify a range","tools":["bash"]}), "spill-task"),
            tool("stay_silent", json!({}), "wait-for-task"),
            tool("send_message", json!({"text":"The requested range was verified."}), "chief-report")
        ],
        "subagents": {"spill-task": [
            tool("bash", json!({"command":"cat input.txt"}), "large-result"),
            tool("read", json!({"path":"{{result:large-result.output_ref}}","offset":4096,"limit":64}), "range-result"),
            tool("bash", json!({"command":format!("test '{{{{result:range-result.content}}}}' = '{expected}' && printf verified > range-proof.txt")}), "verify-range"),
            {"parts":[{"type":"text","text":"Verified the requested range."}]}
        ]}
    });
    let mut harness = Harness::prepare(BIN, &script).expect("uv sync --project agent is required");
    let config = harness.root().join("config/openside/config.toml");
    let original = std::fs::read_to_string(&config).unwrap();
    std::fs::write(config, original.replace("orchestration = \"single\"\n", "")).unwrap();
    std::fs::write(
        harness.workspace().join("input.txt"),
        format!(
            "PRIVATE-SPILL-BODY\n{}END-PRIVATE-SPILL",
            "X".repeat(100_000)
        ),
    )
    .unwrap();
    harness.launch().await;
    let created = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"mode":"full","workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(created.status, 201, "{}", created.body);
    let session = created.body["id"].as_str().unwrap();
    let mut events = harness.events(session, 0).await;
    let submitted = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            Some(json!({"message_id":"inspect","text":"Inspect the result","mode":"queue"})),
        )
        .await;
    assert_eq!(submitted.status, 202, "{}", submitted.body);
    loop {
        let event = events.next().await;
        if matches!(&event.kind, EventKind::AssistantFinal { text, .. } if text == "The requested range was verified.")
        {
            break;
        }
    }
    let store = openside_sessions::Store::open(harness.data_dir.join("events.db")).unwrap();
    let durable = store
        .events_after(&SessionId::new(session), 0, usize::MAX)
        .unwrap();
    let thread = serde_json::to_string(&durable).unwrap();
    assert_eq!(
        std::fs::read_to_string(harness.workspace().join("range-proof.txt"))
            .ok()
            .as_deref(),
        Some("verified"),
        "Task did not consume the requested range; durable events: {thread}",
    );
    for body in ["PRIVATE-SPILL-BODY", "END-PRIVATE-SPILL", expected.as_str()] {
        assert!(!thread.contains(body), "Task body leaked into chief thread");
    }
    assert!(thread.contains("private_output_stored"));
    assert!(thread.contains("read_private_output"));
    harness.kill().await;
}
