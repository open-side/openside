//! Real Python sidecar and real CLI: skills listed in the prompt can be mentioned as
//! `/name` in chat; the sidecar loads them through the daemon's `skill` tool before the
//! model runs. Disabled and unknown skills are never loaded. `openside skills install`
//! writes a skill another agent can use.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use openside_e2e::Harness;
use openside_proto::{Event, EventKind};
use serde_json::{json, Value};

const BODY: &str = "Run pdftotext on the file, then summarize the first page.";

fn text(text: &str) -> Value {
    json!({"parts":[{"type":"text","text":text}]})
}

async fn start(script: &Value) -> Option<Harness> {
    let mut harness = Harness::prepare(env!("CARGO_BIN_EXE_e2e-opensided"), script)?;
    // The CLI and the daemon resolve the default library from different data dirs in this
    // harness (the daemon gets `--data-dir`), so both are pointed at one explicit library.
    let library = harness.root().join("skill-library");
    harness.append_config(&format!(
        "[skills]\ndir = {:?}\n",
        library.to_str().unwrap()
    ));
    harness.launch().await;
    Some(harness)
}

async fn session(harness: &Harness) -> String {
    let response = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"title":"Skills", "mode":"guard", "workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(response.status, 201, "{}", response.body);
    response.body["id"].as_str().unwrap().to_owned()
}

async fn turn(harness: &Harness, id: &str, message: &str, mid: &str, after: u64) -> Vec<Event> {
    let mut stream = harness.events(id, after).await;
    let response = harness
        .request(
            "POST",
            &format!("/v1/sessions/{id}/messages"),
            Some(json!({"message_id":mid,"text":message,"mode":"now"})),
        )
        .await;
    assert_eq!(response.status, 202, "{}", response.body);
    stream.settled().await
}

fn skill_calls(events: &[Event]) -> Vec<(&str, &Value, bool)> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::ToolEnded {
                tool_call_id,
                result,
                is_error,
            } if tool_call_id.starts_with("skill-mention-") => {
                Some((tool_call_id.as_str(), result, *is_error))
            }
            _ => None,
        })
        .collect()
}

async fn cli(harness: &Harness, args: &[&str]) -> (bool, String, String) {
    let output = harness
        .cli(env!("CARGO_BIN_EXE_e2e-openside"), args, None)
        .await;
    (
        output.status.success(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

#[tokio::test]
async fn mentioning_a_skill_loads_it_before_the_model_and_disabled_skills_are_not_offered() {
    let script = json!({"responses":[text("on it"), text("second"), text("third")]});
    let Some(harness) = start(&script).await else {
        return;
    };

    // Create through the CLI (the daemon API), as a user would.
    let (ok, out, err) = cli(
        &harness,
        &[
            "skills",
            "new",
            "pdf-tools",
            "--description",
            "Extract text from PDFs",
            "--body",
            BODY,
        ],
    )
    .await;
    assert!(ok, "{err}");
    assert!(out.starts_with("pdf-tools (user, enabled)"), "{out}");
    let (ok, ..) = cli(
        &harness,
        &[
            "skills",
            "new",
            "retired",
            "--description",
            "No longer used",
            "--body",
            "OLD",
        ],
    )
    .await;
    assert!(ok);
    let (ok, ..) = cli(&harness, &["skills", "disable", "retired"]).await;
    assert!(ok);

    let (ok, out, _) = cli(&harness, &["--json", "skills", "list", "pdf"]).await;
    assert!(ok);
    let list: Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(list["skills"][0]["name"], "pdf-tools", "{list}");

    let id = session(&harness).await;
    let events = turn(
        &harness,
        &id,
        "Please use /pdf-tools on report.pdf, and also /retired and /ghost.",
        "m1",
        0,
    )
    .await;
    let calls = skill_calls(&events);
    assert_eq!(
        calls.len(),
        1,
        "only the enabled, known skill is loaded: {calls:?}"
    );
    let (call_id, result, is_error) = calls[0];
    assert!(!is_error, "{result}");
    assert!(result.to_string().contains(BODY), "{result}");
    let started = events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::ToolStarted {
                tool_call_id,
                name,
                args,
                ..
            } if tool_call_id == call_id => Some((name.clone(), args.clone())),
            _ => None,
        })
        .expect("tool.started for the mention");
    assert_eq!(started, ("skill".to_owned(), json!({"name":"pdf-tools"})));
    let model_ran_after = events
        .iter()
        .position(|e| matches!(e.kind, EventKind::ToolEnded { .. }))
        .unwrap()
        < events
            .iter()
            .position(|e| matches!(e.kind, EventKind::AssistantDelta { .. }))
            .unwrap_or(usize::MAX);
    assert!(model_ran_after, "the skill loads before the model answers");

    let last = events.last().unwrap().seq;
    // The loaded skill is part of the persisted conversation.
    assert!(harness.snapshot(&id).to_string().contains("Run pdftotext"));

    // A message with no mention causes no load.
    let events = turn(&harness, &id, "thanks, no skills needed", "m2", last).await;
    let last = events.last().unwrap().seq;
    assert!(skill_calls(&events).is_empty());

    // A skill disabled mid-session is still refused: the daemon re-checks on every load.
    let (ok, ..) = cli(&harness, &["skills", "disable", "pdf-tools"]).await;
    assert!(ok);
    let events = turn(&harness, &id, "once more /pdf-tools", "m3", last).await;
    let calls = skill_calls(&events);
    assert_eq!(calls.len(), 1, "{calls:?}");
    let (_, result, is_error) = calls[0];
    assert!(
        is_error && result["reason"] == "skill_not_found",
        "{result}"
    );
    assert!(!result.to_string().contains(BODY), "{result}");
    let (ok, out, _) = cli(&harness, &["skills", "list"]).await;
    assert!(ok);
    assert!(out.contains("pdf-tools\tuser\tdisabled"), "{out}");

    let mut harness = harness;
    harness.kill().await;
}

#[tokio::test]
async fn install_makes_the_browser_skill_available_to_another_agent() {
    let script = json!({"responses":[text("unused")]});
    let Some(mut harness) = Harness::start(env!("CARGO_BIN_EXE_e2e-opensided"), &script).await
    else {
        return;
    };
    let target = harness.root().join("claude-skills");
    let (ok, out, err) = cli(
        &harness,
        &[
            "--json",
            "skills",
            "install",
            "claude",
            "--to-dir",
            target.to_str().unwrap(),
        ],
    )
    .await;
    assert!(ok, "{err}");
    let report: Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(report["skill"], "openside-browser");
    let installed = std::fs::read_to_string(target.join("openside-browser/SKILL.md")).unwrap();
    assert!(installed.contains("name: openside-browser"));
    assert!(
        installed.contains("openside"),
        "teaches the agent to call openside"
    );

    // The same file works as a library skill: importing it back is accepted.
    let (ok, _, err) = cli(
        &harness,
        &[
            "skills",
            "import",
            "openside-browser",
            "--from-dir",
            target.to_str().unwrap(),
        ],
    )
    .await;
    assert!(ok, "{err}");
    harness.kill().await;
}
