//! Routines against the real daemon and the credential-free Python sidecar.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{future::Future, time::Duration};

use openside_e2e::Harness;
use openside_proto::{EventKind, RunRecord, RunStatus};
use serde_json::{json, Value};

const CLI: &str = env!("CARGO_BIN_EXE_e2e-openside");

fn reply(text: &str) -> Value {
    json!({"parts":[{"type":"text","text":text}]})
}

async fn start(replies: &[&str]) -> Option<Harness> {
    let responses: Vec<_> = replies.iter().map(|text| reply(text)).collect();
    Harness::start(
        env!("CARGO_BIN_EXE_e2e-opensided"),
        &json!({"responses": responses}),
    )
    .await
}

async fn until<T, F: Future<Output = Option<T>>>(what: &str, mut probe: impl FnMut() -> F) -> T {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(found) = probe().await {
                return found;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

async fn cli_json(harness: &Harness, args: &[&str]) -> Value {
    let mut full = vec!["--json"];
    full.extend_from_slice(args);
    let output = harness.cli(CLI, &full, None).await;
    assert!(
        output.status.success(),
        "CLI {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let line = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap()
        .to_owned();
    serde_json::from_str(&line).unwrap()
}

async fn statuses(harness: &Harness, routine: &str) -> Vec<String> {
    let runs = harness
        .request("GET", &format!("/v1/routines/{routine}/runs"), None)
        .await;
    runs.body["runs"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .map(|run| run["status"].as_str().unwrap().to_owned())
        .collect()
}

/// Full run history, oldest first.
async fn history(harness: &Harness, routine: &str) -> Vec<RunRecord> {
    let runs = harness
        .request("GET", &format!("/v1/routines/{routine}/runs"), None)
        .await;
    let mut runs: Vec<RunRecord> = serde_json::from_value(runs.body["runs"].clone()).unwrap();
    runs.reverse();
    runs
}

async fn wait_for_status(harness: &Harness, routine: &str, status: &str) {
    until(&format!("a {status} run"), || async {
        statuses(harness, routine)
            .await
            .iter()
            .any(|s| s == status)
            .then_some(())
    })
    .await;
}

#[tokio::test]
async fn run_now_from_the_cli_starts_a_real_session_and_the_run_completes() {
    let Some(mut harness) = start(&["inbox summarized"]).await else {
        return;
    };
    let added = cli_json(
        &harness,
        &[
            "routine",
            "add",
            "--name",
            "Inbox",
            "--cron",
            "0 9 * * *",
            "--prompt",
            "summarize my inbox",
        ],
    )
    .await;
    assert_eq!(added["schedule"]["expr"], "0 9 * * *");
    assert_eq!(added["enabled"], true);
    let id = added["id"].as_str().unwrap().to_owned();

    let run = cli_json(&harness, &["routine", "run", &id]).await;
    assert_eq!(run["status"], "started");
    let session = run["session_id"].as_str().unwrap().to_owned();

    wait_for_status(&harness, &id, "completed").await;
    let events = harness.events(&session, 0).await.settled().await;
    assert!(events.iter().any(
        |e| matches!(&e.kind, EventKind::AssistantFinal { text } if text == "inbox summarized")
    ));
    assert!(events.iter().any(
        |e| matches!(&e.kind, EventKind::MessageQueued { text, .. } if text == "summarize my inbox")
    ));

    // A manual run leaves the schedule alone.
    let shown = cli_json(&harness, &["routine", "show", &id]).await;
    assert_eq!(shown["next_run_at"], added["next_run_at"]);
    harness.kill().await;
}

#[tokio::test]
async fn heartbeat_follows_up_an_existing_session_and_deleting_it_pauses_the_routine() {
    let Some(mut harness) = start(&["first answer", "heartbeat answer"]).await else {
        return;
    };
    let task = harness.cli(CLI, &["a task"], None).await;
    assert!(
        task.status.success(),
        "{}",
        String::from_utf8_lossy(&task.stderr)
    );
    let first = harness.request("GET", "/v1/sessions", None).await;
    let session = first.body["sessions"][0]["id"].as_str().unwrap().to_owned();

    let added = cli_json(
        &harness,
        &[
            "routine",
            "add",
            "--name",
            "Pulse",
            "--interval-secs",
            "3600",
            "--prompt",
            "any news?",
            "--heartbeat",
            &session,
        ],
    )
    .await;
    assert_eq!(added["action"]["type"], "heartbeat");
    let id = added["id"].as_str().unwrap().to_owned();

    let run = cli_json(&harness, &["routine", "run", &id]).await;
    assert_eq!(
        (run["status"].as_str(), run["session_id"].as_str()),
        (Some("started"), Some(session.as_str()))
    );
    wait_for_status(&harness, &id, "completed").await;

    // The follow-up landed in the same session, in the queue lane, under a routine message id.
    let sessions = harness.request("GET", "/v1/sessions", None).await;
    assert_eq!(
        sessions.body["sessions"].as_array().unwrap().len(),
        1,
        "no new session"
    );
    let mut stream = harness.events(&session, 0).await;
    let mut queued = Vec::new();
    let mut finals = Vec::new();
    let last_seq = loop {
        let event = stream.next().await;
        match &event.kind {
            EventKind::MessageQueued {
                message_id,
                text,
                mode,
                author_kind,
                from_agent,
                ..
            } => {
                if message_id.as_str().starts_with("routine-") {
                    assert_eq!(*author_kind, openside_proto::MessageAuthorKind::Internal);
                    assert_eq!(*from_agent, None);
                }
                queued.push((message_id.to_string(), text.clone(), format!("{mode:?}")));
            }
            EventKind::AssistantFinal { text } => finals.push(text.clone()),
            EventKind::SessionSettled {} if finals.len() == 2 => break event.seq,
            _ => {}
        }
    };
    assert_eq!(finals, ["first answer", "heartbeat answer"]);
    let routine_message = queued.iter().find(|m| m.1 == "any news?").unwrap();
    assert!(routine_message.0.starts_with("routine-"));
    assert_eq!(routine_message.2, "Queue");

    // Live subscribers see the tombstone, then the target is gone and the routine pauses.
    let mut live = harness.events(&session, last_seq).await;
    let deleted = harness
        .request("DELETE", &format!("/v1/sessions/{session}"), None)
        .await;
    assert_eq!(deleted.status, 204);
    loop {
        if matches!(live.next().await.kind, EventKind::SessionDeleted {}) {
            break;
        }
    }
    assert!(
        live.ends_within(Duration::from_secs(10)).await,
        "the stream must close after the tombstone instead of hanging"
    );
    assert_eq!(
        harness
            .request("GET", &format!("/v1/sessions/{session}"), None)
            .await
            .status,
        404
    );
    let sessions = harness.request("GET", "/v1/sessions", None).await;
    assert!(sessions.body["sessions"].as_array().unwrap().is_empty());

    let paused = cli_json(&harness, &["routine", "show", &id]).await;
    assert_eq!(
        (paused["enabled"].clone(), paused["paused_reason"].clone()),
        (json!(false), json!("target_missing"))
    );
    assert_eq!(
        statuses(&harness, &id).await.last().map(String::as_str),
        Some("paused_target_missing")
    );

    harness.kill().await;
}

#[tokio::test]
async fn a_due_routine_fires_by_itself_and_a_restart_records_what_it_missed() {
    let Some(mut harness) = start(&["tick one", "tick two", "tick three", "tick four"]).await
    else {
        return;
    };
    let added = cli_json(
        &harness,
        &[
            "routine",
            "add",
            "--name",
            "Tick",
            "--interval-secs",
            "1",
            "--prompt",
            "tick",
        ],
    )
    .await;
    let id = added["id"].as_str().unwrap().to_owned();
    wait_for_status(&harness, &id, "completed").await;
    let before = history(&harness, &id).await;
    let last_before = before.iter().map(|run| run.scheduled_at).max().unwrap();

    // Down for several periods: recovery must record them as missed instead of replaying.
    harness.crash_daemon().await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    harness.launch().await;
    let listed = harness.request("GET", "/v1/routines", None).await;
    assert_eq!(
        listed.body["routines"][0]["id"],
        id.as_str(),
        "routine survived the crash"
    );
    wait_for_status(&harness, &id, "missed").await;
    // Stop the routine so no further runs start while the harness inspects the history.
    let paused = cli_json(&harness, &["routine", "pause", &id]).await;
    assert_eq!(paused["enabled"], false);

    let after = history(&harness, &id).await;
    let identity = |runs: &[RunRecord]| {
        runs.iter()
            .map(|run| (run.id, run.scheduled_at))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        identity(&after[..before.len()]),
        identity(&before),
        "earlier history is untouched"
    );
    let new: Vec<_> = after[before.len()..].iter().collect();
    let missed: Vec<_> = new
        .iter()
        .filter(|run| run.status == RunStatus::Missed)
        .collect();
    assert!(
        missed.len() >= 2,
        "three seconds of downtime miss at least two occurrences: {missed:?}"
    );
    assert!(
        missed.iter().all(|run| run.scheduled_at > last_before),
        "only occurrences after the last run"
    );
    for pair in missed.windows(2) {
        assert_eq!(
            (pair[1].scheduled_at - pair[0].scheduled_at).num_seconds(),
            1,
            "every missed occurrence is accounted for exactly once, with no gap and no duplicate"
        );
    }
    let replayed = new
        .iter()
        .filter(|run| run.status != RunStatus::Missed)
        .count();
    assert!(replayed <= 2, "no burst replay of the missed runs: {new:?}");
    harness.kill().await;
}

fn tool(name: &str, args: Value, call: &str) -> Value {
    json!({"parts":[{"type":"tool_call","name":name,"args":args,"id":call}]})
}

/// A session with one finished turn and a heartbeat routine whose first run is parked on an
/// `ask_user` ticket: its message ends `deferred`. Returns `(session, routine, ticket)`.
async fn parked_heartbeat(harness: &Harness) -> (String, String, String) {
    let task = harness.cli(CLI, &["a task"], None).await;
    assert!(
        task.status.success(),
        "{}",
        String::from_utf8_lossy(&task.stderr)
    );
    let sessions = harness.request("GET", "/v1/sessions", None).await;
    let session = sessions.body["sessions"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let added = cli_json(
        harness,
        &[
            "routine",
            "add",
            "--name",
            "Parked",
            "--interval-secs",
            "3600",
            "--prompt",
            "pick one",
            "--heartbeat",
            &session,
        ],
    )
    .await;
    let id = added["id"].as_str().unwrap().to_owned();
    let run = cli_json(harness, &["routine", "run", &id]).await;
    assert_eq!(run["status"], "started");
    let ticket = until("the ask_user ticket", || async {
        let tickets = harness
            .request("GET", "/v1/tickets?status=pending", None)
            .await;
        tickets.body["tickets"][0]["id"].as_str().map(str::to_owned)
    })
    .await;
    until("the deferred session to settle", || async {
        let state = harness
            .request("GET", &format!("/v1/sessions/{session}"), None)
            .await;
        (state.body["status"] == "waiting_human").then_some(())
    })
    .await;
    (session, id, ticket)
}

fn parked_script() -> Vec<Value> {
    vec![
        reply("first answer"),
        tool(
            "ask_user",
            json!({"question":"Pick one","options":["a","b"]}),
            "ask-call",
        ),
        reply("resolved"),
        reply("again"),
    ]
}

async fn run_status(harness: &Harness, routine: &str) -> String {
    let run = cli_json(harness, &["routine", "run", routine]).await;
    run["status"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn a_run_parked_on_a_ticket_stays_active_until_it_is_resolved_and_then_the_next_run_starts() {
    let Some(mut harness) = Harness::start(
        env!("CARGO_BIN_EXE_e2e-opensided"),
        &json!({"responses": parked_script()}),
    )
    .await
    else {
        return;
    };
    let (_, id, ticket) = parked_heartbeat(&harness).await;
    assert_eq!(
        statuses(&harness, &id).await,
        ["started"],
        "deferred is not completion"
    );
    assert_eq!(run_status(&harness, &id).await, "skipped_overlap");

    let resolved = harness
        .cli(CLI, &["ticket", "resolve", &ticket, "a"], None)
        .await;
    assert!(
        resolved.status.success(),
        "{}",
        String::from_utf8_lossy(&resolved.stderr)
    );
    wait_for_status(&harness, &id, "completed").await;
    assert_eq!(
        run_status(&harness, &id).await,
        "started",
        "the next run goes ahead"
    );
    harness.kill().await;
}

#[tokio::test]
async fn stopping_the_session_ends_a_run_parked_on_a_ticket() {
    let Some(mut harness) = Harness::start(
        env!("CARGO_BIN_EXE_e2e-opensided"),
        &json!({"responses": parked_script()}),
    )
    .await
    else {
        return;
    };
    let (session, id, _) = parked_heartbeat(&harness).await;
    let stopped = harness
        .request("POST", &format!("/v1/sessions/{session}/stop"), None)
        .await;
    assert!(stopped.status < 300, "{}", stopped.body);
    wait_for_status(&harness, &id, "failed").await;
    assert_eq!(
        run_status(&harness, &id).await,
        "started",
        "the next run goes ahead"
    );
    harness.kill().await;
}

#[tokio::test]
async fn a_run_parked_on_a_ticket_survives_a_daemon_restart_and_completes_after_resolution() {
    let Some(mut harness) = Harness::start(
        env!("CARGO_BIN_EXE_e2e-opensided"),
        &json!({"responses": parked_script()}),
    )
    .await
    else {
        return;
    };
    let (_, id, ticket) = parked_heartbeat(&harness).await;
    harness.crash_daemon().await;
    harness.launch().await;
    assert_eq!(
        run_status(&harness, &id).await,
        "skipped_overlap",
        "the pending ticket still owns the run"
    );

    let resolved = harness
        .cli(CLI, &["ticket", "resolve", &ticket, "b"], None)
        .await;
    assert!(
        resolved.status.success(),
        "{}",
        String::from_utf8_lossy(&resolved.stderr)
    );
    wait_for_status(&harness, &id, "completed").await;
    assert_eq!(run_status(&harness, &id).await, "started");
    harness.kill().await;
}
