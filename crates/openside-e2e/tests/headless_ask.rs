//! PP-11: a headless CLI task that asks the user parks on a ticket instead of hanging, exits
//! successfully with `waiting_human`, and finishes after a remote client with the right
//! scope answers the ticket; a read-only remote client cannot.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use openside_e2e::Harness;
use serde_json::{json, Value};

const CLI: &str = env!("CARGO_BIN_EXE_e2e-openside");

fn parse(output: &std::process::Output) -> Vec<Value> {
    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).expect("CLI JSONL"))
        .collect()
}

async fn mint(harness: &Harness, scope: &str) -> String {
    let output = harness
        .cli(
            CLI,
            &["token", "create", "--scope", scope, "--name", scope],
            None,
        )
        .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let minted = String::from_utf8(output.stdout).unwrap();
    minted.trim().split_once('\t').unwrap().1.to_owned()
}

#[tokio::test]
async fn headless_ask_waits_for_a_ticket_and_a_scoped_remote_answer_resumes_it() {
    let Some(mut harness) = Harness::start(
        env!("CARGO_BIN_EXE_e2e-opensided"),
        &json!({"responses":[
            {"parts":[{"type":"tool_call","name":"ask_user","id":"ask-call",
                "args":{"question":"Choose a color","options":["blue","green"]}}]},
            {"parts":[{"type":"text","text":"blue selected"}]}
        ]}),
    )
    .await
    else {
        return;
    };

    // Headless: stdout is a pipe, no terminal to answer on.
    let output = harness.cli(CLI, &["--json", "pick a color"], None).await;
    let records = parse(&output);
    let summary = records.last().unwrap();
    assert_eq!(summary["type"], "summary");
    assert_eq!(summary["status"], "waiting_human", "{summary}");
    assert_eq!(summary["exit_code"], 0);
    let session = summary["session_id"].as_str().unwrap().to_owned();
    let opened = records
        .iter()
        .find(|record| record["kind"] == "ticket.opened")
        .expect("ticket.opened record");
    let ticket = opened["ticket"]["id"].as_str().unwrap().to_owned();
    assert_eq!(opened["ticket"]["kind"], "info");
    assert_eq!(opened["ticket"]["tool_call_id"], "ask-call");

    // The ticket is discoverable from another process.
    let listed = harness.cli(CLI, &["--json", "ticket", "list"], None).await;
    assert!(
        String::from_utf8_lossy(&listed.stdout).contains(&ticket),
        "{listed:?}"
    );
    assert_eq!(
        harness.snapshot(&session)["pending_deferred"][0]["tool_call_id"],
        "ask-call"
    );

    // A remote client with read scope sees the ticket but cannot answer it...
    let read = mint(&harness, "read").await;
    let path = format!("/v1/tickets/{ticket}/resolve");
    let refused = harness
        .tcp_request("POST", &path, Some(&read), Some(json!({"value":"blue"})))
        .await;
    assert_eq!(refused.status, 403, "{}", refused.body);
    assert_eq!(
        harness
            .tcp_request("GET", "/v1/tickets", Some(&read), None)
            .await
            .status,
        200
    );
    assert_eq!(
        harness
            .request("GET", &format!("/v1/sessions/{session}"), None)
            .await
            .body["status"],
        "waiting_human"
    );

    // ...an approve-scoped one can.
    let approve = mint(&harness, "approve").await;
    let resolved = harness
        .tcp_request("POST", &path, Some(&approve), Some(json!({"value":"blue"})))
        .await;
    assert_eq!(resolved.status, 200, "{}", resolved.body);

    // Resuming from the saved cursor shows the run continue from the answer to its end.
    let output = harness
        .cli(CLI, &["--json", "session", "resume", &session], None)
        .await;
    let records = parse(&output);
    assert_eq!(records.last().unwrap()["status"], "settled", "{records:?}");
    assert!(records
        .iter()
        .any(|record| record["kind"] == "assistant.final" && record["text"] == "blue selected"));
    let snapshot = harness.snapshot(&session);
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
        "the deferred call resumes once under its original id"
    );
    assert_eq!(returns[0]["content"]["answer"], "blue");
    harness.kill().await;
}
