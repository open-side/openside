//! Credential-free real CLI round trips through the Python sidecar daemon.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use openside_e2e::Harness;
use openside_proto::{Event, EventKind};
use serde_json::{json, Value};

async fn start() -> Option<Harness> {
    Harness::start(
        env!("CARGO_BIN_EXE_e2e-opensided"),
        &json!({"responses":[
            {"parts":[{"type":"text","text":"hello from the real sidecar"}]},
            {"parts":[{"type":"text","text":"restored conversation reply"}]}
        ]}),
    )
    .await
}
fn records(output: &std::process::Output) -> Vec<Value> {
    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.stdout.contains(&0x1b));
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).expect("CLI JSONL"))
        .collect()
}

/// PP-5: `openside "x" | cat` — stdout is a pipe, not a TTY — prints JSONL with a final
/// summary even though `--json` was not passed.
#[tokio::test]
async fn cli_task_with_piped_stdout_emits_jsonl_without_json_flag() {
    let Some(mut harness) = start().await else {
        return;
    };
    let output = harness
        .cli(env!("CARGO_BIN_EXE_e2e-openside"), &["x"], None)
        .await;
    let records = records(&output);
    let (summary, events) = records.split_last().unwrap();
    assert_eq!(summary["type"], "summary");
    assert_eq!(summary["status"], "settled");
    assert!(events.iter().any(|event| event["kind"] == "assistant.final"
        && event["text"] == "hello from the real sidecar"));
    harness.kill().await;
}

#[tokio::test]
async fn cli_task_prints_real_sidecar_reply_and_json_roundtrip_settles() {
    let Some(mut harness) = start().await else {
        return;
    };
    // A terminal keeps human text; no JSON, no summary line.
    let (status, plain) = harness
        .cli_on_tty(env!("CARGO_BIN_EXE_e2e-openside"), &["task"])
        .await;
    assert!(status.success(), "{plain}");
    assert!(plain.contains("hello from the real sidecar"), "{plain}");
    assert!(
        !plain.contains("\"type\":\"summary\"") && !plain.trim_start().starts_with('{'),
        "{plain}"
    );
    let output = harness
        .cli(
            env!("CARGO_BIN_EXE_e2e-openside"),
            &["--json", "another task"],
            None,
        )
        .await;
    let records = records(&output);
    let (summary, events) = records.split_last().unwrap();
    assert_eq!(summary["type"], "summary");
    assert_eq!(summary["status"], "settled");
    assert_eq!(summary["exit_code"], 0);
    let events: Vec<Event> = events
        .iter()
        .map(|event| serde_json::from_value(event.clone()).unwrap())
        .collect();
    assert!(events.iter().any(|event| matches!(&event.kind, EventKind::AssistantFinal { text, .. } if text == "hello from the real sidecar")));
    assert!(events.windows(2).all(|pair| pair[1].seq == pair[0].seq + 1));
    assert!(matches!(
        events.last().unwrap().kind,
        EventKind::SessionSettled {}
    ));
    assert_eq!(summary["last_seq"], events.last().unwrap().seq);
    let id = summary["session_id"].as_str().unwrap();
    let listed = records_for(&harness, &["--json", "session", "list"]).await;
    assert!(listed[0]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|session| session["id"] == id));
    let stopped = records_for(&harness, &["--json", "session", "stop", id]).await;
    assert_eq!(stopped[0]["status"], "stopped");
    harness.kill().await;
}

async fn records_for(harness: &Harness, args: &[&str]) -> Vec<Value> {
    records(
        &harness
            .cli(env!("CARGO_BIN_EXE_e2e-openside"), args, None)
            .await,
    )
}

#[tokio::test]
async fn cli_resume_after_daemon_restart_restores_history_and_script_cursor() {
    let Some(mut harness) = start().await else {
        return;
    };
    let first = records_for(&harness, &["--json", "first task"]).await;
    let summary = first.last().unwrap();
    let id = summary["session_id"].as_str().unwrap();
    let previous_seq = summary["last_seq"].as_u64().unwrap();
    assert_eq!(harness.snapshot(id)["script_cursor"], 1);
    if !first
        .iter()
        .any(|record| record["kind"] == "session.title_proposed")
    {
        let mut metadata = harness.events(id, previous_seq).await;
        while !matches!(
            metadata.next().await.kind,
            EventKind::SessionTitleProposed { .. }
        ) {}
    }
    // Pinning supplies a deterministic metadata tail even if naming beat the settled marker.
    let pinned = harness
        .request(
            "PATCH",
            &format!("/v1/sessions/{id}"),
            Some(json!({"pinned":true})),
        )
        .await;
    assert_eq!(pinned.status, 200);
    assert_eq!(pinned.body["status"], "idle");
    let metadata_seq = pinned.body["last_seq"].as_u64().unwrap();
    assert!(metadata_seq > previous_seq);
    harness.restart().await;
    let idle = records_for(&harness, &["--json", "session", "resume", id]).await;
    let (idle_summary, metadata) = idle.split_last().unwrap();
    assert_eq!(idle_summary["last_seq"], metadata_seq);
    assert!(metadata.iter().all(|record| matches!(
        record["kind"].as_str(),
        Some("usage" | "session.title_proposed" | "session.updated")
    )));
    let seen: Vec<_> = metadata
        .iter()
        .map(|record| record["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seen, (previous_seq + 1..=metadata_seq).collect::<Vec<_>>());
    assert_eq!(
        harness.snapshot(id)["script_cursor"],
        1,
        "idle replay must not run the model again"
    );
    let current = records_for(&harness, &["--json", "session", "resume", id]).await;
    assert_eq!(
        current.len(),
        1,
        "the replay advances the saved event cursor"
    );
    assert_eq!(current[0]["last_seq"], metadata_seq);
    let resumed = records_for(
        &harness,
        &["--json", "session", "resume", id, "second task"],
    )
    .await;
    let (summary, events) = resumed.split_last().unwrap();
    assert_eq!(summary["status"], "settled");
    let events: Vec<Event> = events
        .iter()
        .map(|event| serde_json::from_value(event.clone()).unwrap())
        .collect();
    assert!(events.iter().all(|event| event.seq > metadata_seq));
    assert!(events.iter().any(|event| matches!(&event.kind, EventKind::AssistantFinal { text, .. } if text == "restored conversation reply")));
    let snapshot = harness.snapshot(id);
    assert_eq!(snapshot["script_cursor"], 2);
    assert!(snapshot["messages"]
        .to_string()
        .contains("hello from the real sidecar"));
    harness.kill().await;
}

/// Every tool the daemon lets API callers invoke has a `openside browser` verb and the other
/// way round, so the CLI cannot drift from the daemon's catalog.
#[tokio::test]
async fn cli_browser_verbs_match_the_daemon_tool_catalog() {
    let Some(mut harness) = start().await else {
        return;
    };
    let catalog = harness.request("GET", "/v1/tools", None).await;
    assert_eq!(catalog.status, 200);
    let mut daemon: Vec<&str> = catalog.body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    daemon.sort_unstable();
    assert_eq!(openside_cli::browser_verb_tools(), daemon);
    harness.kill().await;
}
