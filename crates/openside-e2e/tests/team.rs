//! Team lifecycle and CLI round trips through the real daemon and scripted Python sidecar.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod ui_support;

use ui_support::send;

use openside_e2e::Harness;
use openside_proto::{ActionTarget, CardKind, EventKind, TeamCardCommand, TicketKind};
use serde_json::{json, Value};

const DAEMON: &str = env!("CARGO_BIN_EXE_e2e-opensided");
const CLI: &str = env!("CARGO_BIN_EXE_e2e-openside");

fn text(text: &str) -> Value {
    json!({"parts":[{"type":"text","text":text}]})
}

fn tool(name: &str, args: Value, id: &str) -> Value {
    json!({"parts":[{"type":"tool_call","name":name,"args":args,"id":id}]})
}

fn draft(name: &str) -> Value {
    json!({"name":name,"persona":{"role":"Price researcher","responsibilities":["Track prices"]}})
}

async fn get(harness: &Harness, path: &str) -> Value {
    let response = harness.request("GET", path, None).await;
    assert_eq!(response.status, 200, "{path}: {}", response.body);
    response.body
}

async fn post(harness: &Harness, path: &str, body: Value, expected: u16) -> Value {
    let response = harness.request("POST", path, Some(body)).await;
    assert_eq!(response.status, expected, "{path}: {}", response.body);
    response.body
}

async fn cli(harness: &Harness, args: &[&str]) -> Vec<Value> {
    let mut full = vec!["--json"];
    full.extend_from_slice(args);
    let output = harness.cli(CLI, &full, None).await;
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

async fn routine(harness: &Harness, owner: &str, name: &str) -> Value {
    post(
        harness,
        "/v1/routines",
        json!({
            "name":name,"schedule":{"type":"interval","secs":86400},
            "action":{"type":"agent","agent_id":owner,"prompt":"Track prices"},"enabled":false
        }),
        201,
    )
    .await
}

async fn memory(harness: &Harness, owner: Option<&str>, content: &str) -> Value {
    let Some(owner) = owner else {
        return post(
            harness,
            "/v1/memory",
            json!({"content":content,"tags":["team"],"retention":"permanent"}),
            201,
        )
        .await;
    };
    let agent = get(harness, &format!("/v1/agents/{owner}")).await;
    let home = agent["home_session_id"].as_str().unwrap();
    let session = get(harness, &format!("/v1/sessions/{home}")).await;
    harness.replace_script(&json!({"responses":[
        tool("memory_write", json!({"content":content,"tags":["team"],"retention":"permanent"}), "fixture-memory"),
        text("Remembered the price note.")
    ]}));
    let mut stream = harness
        .events(home, session["last_seq"].as_u64().unwrap())
        .await;
    post(
        harness,
        &format!("/v1/sessions/{home}/messages"),
        json!({"text":"Remember this price note.","mode":"now"}),
        202,
    )
    .await;
    let events = stream.settled().await;
    let memory_id = events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::ToolEnded {
                tool_call_id,
                result,
                is_error: false,
                ..
            } if tool_call_id == "fixture-memory" => result["id"].as_str(),
            _ => None,
        })
        .unwrap_or_else(|| panic!("memory_write did not create an owned note: {events:?}"));
    let memory = get(harness, &format!("/v1/memory/{memory_id}")).await;
    assert_eq!(memory["owner"], owner);
    memory
}

#[tokio::test]
async fn autonomous_recurring_hire_sends_one_notice_and_retains_lifecycle_controls() {
    let Some(mut harness) = Harness::start(DAEMON, &json!({"responses":[]})).await else {
        return;
    };
    let chief = get(&harness, "/v1/agents/chief").await;
    let home = chief["home_session_id"].as_str().unwrap();
    let owned_routine = routine(&harness, "chief", "Price watch").await;
    let shared_note = memory(&harness, None, "Compare the same product sizes").await;
    let routine_id = owned_routine["id"].as_str().unwrap();
    let memory_id = shared_note["id"].as_str().unwrap();
    let mut hire = draft("Mina");
    hire["routine_ids"] = json!([routine_id]);
    hire["memory_ids"] = json!([memory_id]);
    harness.replace_script(&json!({"responses":[
        tool("team_hire", json!({"draft":hire}), "hire-call"),
        tool("send_message", json!({"text":"I brought in Mina to handle weekly market notes."}), "hire-notice")
    ]}));
    let mut stream = harness.events(home, 0).await;
    post(
        &harness,
        &format!("/v1/sessions/{home}/messages"),
        json!({
            "message_id":"recurring-request","text":"Keep weekly market notes and track prices for me.","mode":"now"
        }),
        202,
    )
    .await;
    let events = stream.settled().await;
    assert!(events.iter().any(|event| matches!(&event.kind,
        EventKind::ToolEnded { tool_call_id, is_error: false, .. } if tool_call_id == "hire-call"
    )), "{events:?}");
    assert_eq!(events.iter().filter(|event| matches!(&event.kind,
        EventKind::AssistantFinal { text, .. } if text == "I brought in Mina to handle weekly market notes."
    )).count(), 1);
    assert!(!events
        .iter()
        .any(|event| matches!(&event.kind, EventKind::TicketOpened { .. })));
    let (agent_id, card) = events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::TeamHired {
                agent_id,
                ticket_id: None,
                request_message_id: None,
                card: Some(card),
                ..
            } => Some((agent_id.clone(), card)),
            _ => None,
        })
        .expect("autonomous hire event with a common card");
    assert_eq!(card.kind, CardKind::Handoff);
    assert!(card.actions.iter().any(|action| matches!(&action.target,
        ActionTarget::TeamCommand { agent_id: id, command: TeamCardCommand::Retire } if id == &agent_id
    )));
    assert!(card.actions.iter().any(|action| matches!(&action.target,
        ActionTarget::OpenThread { agent_id: id, .. } if id == &agent_id
    )));
    assert_eq!(
        get(&harness, &format!("/v1/routines/{routine_id}")).await["action"]["agent_id"],
        agent_id
    );
    assert_eq!(
        get(&harness, &format!("/v1/memory/{memory_id}")).await["owner"],
        agent_id
    );

    let undone = cli(&harness, &["team", "undo", &agent_id]).await;
    assert_eq!(undone[0]["status"], "retired");
    let agent_home = undone[0]["home_session_id"].as_str().unwrap();
    assert_eq!(
        get(&harness, &format!("/v1/sessions/{agent_home}")).await["archived"],
        true
    );
    assert_eq!(
        get(&harness, &format!("/v1/routines/{routine_id}")).await["action"],
        owned_routine["action"]
    );
    assert_eq!(
        get(&harness, &format!("/v1/memory/{memory_id}")).await["owner"],
        Value::Null
    );
    let rejected = harness
        .request(
            "POST",
            &format!("/v1/sessions/{agent_home}/messages"),
            Some(json!({"text":"New work","mode":"now"})),
        )
        .await;
    assert_eq!(rejected.status, 409, "{}", rejected.body);
    assert_eq!(rejected.body["code"], "agent_retired");

    let restored = cli(&harness, &["team", "restore", &agent_id]).await;
    assert_eq!(restored[0]["status"], "active");
    assert_eq!(restored[0]["home_session_id"], agent_home);
    assert_eq!(
        get(&harness, &format!("/v1/sessions/{agent_home}")).await["archived"],
        false
    );
    assert_eq!(
        get(&harness, &format!("/v1/routines/{routine_id}")).await["action"],
        owned_routine["action"]
    );
    harness.kill().await;
}

#[tokio::test]
async fn cli_roster_profiles_retirement_handover_pause_and_restore_match_api() {
    let Some(mut harness) = Harness::start(DAEMON, &json!({"responses":[]})).await else {
        return;
    };
    let hired = cli(
        &harness,
        &[
            "team",
            "hire",
            "--name",
            "Mina",
            "--role",
            "Price researcher",
        ],
    )
    .await;
    let id = hired[0]["id"].as_str().unwrap();
    assert_eq!(
        hired[0]["persona"]["responsibilities"],
        json!(["Price researcher"])
    );
    let successor = cli(
        &harness,
        &["team", "hire", "--name", "Nora", "--role", "Travel planner"],
    )
    .await;
    let successor_id = successor[0]["id"].as_str().unwrap();
    let listed = cli(&harness, &["team"]).await;
    assert_eq!(listed[0], get(&harness, "/v1/agents").await);
    assert_eq!(cli(&harness, &["team", "list"]).await[0], listed[0]);
    let (status, plain) = harness.cli_on_tty(CLI, &["team", "list"]).await;
    assert!(status.success(), "{plain}");
    for expected in [
        "NAME",
        "ROLE",
        "STATUS",
        "OWNS",
        "Mina",
        "Price researcher",
        "active",
        "Nora",
    ] {
        assert!(plain.contains(expected), "{expected}: {plain}");
    }
    assert_eq!(cli(&harness, &["team", "show", "mInA"]).await[0], hired[0]);
    assert_eq!(
        cli(&harness, &["team", "hide", id]).await[0]["hidden"],
        true
    );
    assert_eq!(
        cli(&harness, &["team", "show", id]).await[0]["hidden"],
        true,
        "inspection must not unhide"
    );
    assert_eq!(
        cli(&harness, &["team", "unhide", id]).await[0]["hidden"],
        false
    );
    let renamed = cli(&harness, &["team", "rename", id, "Mira"]).await;
    assert_eq!(renamed[0]["name"], "Mira");
    assert_eq!(renamed[0]["home_session_id"], hired[0]["home_session_id"]);

    let schedule = routine(&harness, id, "Owned price watch").await;
    let note = memory(&harness, Some(id), "Watch the total including shipping").await;
    let schedule_id = schedule["id"].as_str().unwrap();
    let note_id = note["id"].as_str().unwrap();
    assert_eq!(
        cli(&harness, &["team", "retire", id, "--hand-to", successor_id]).await[0]["status"],
        "retired"
    );
    assert_eq!(
        get(&harness, &format!("/v1/routines/{schedule_id}")).await["action"]["agent_id"],
        successor_id
    );
    assert_eq!(
        get(&harness, &format!("/v1/memory/{note_id}")).await["owner"],
        successor_id
    );
    let successor = get(&harness, &format!("/v1/agents/{successor_id}")).await;
    assert!(successor["persona"]["responsibilities"]
        .as_array()
        .unwrap()
        .contains(&json!("Price researcher")));
    assert_eq!(
        cli(&harness, &["team", "restore", id]).await[0]["status"],
        "active"
    );
    assert_eq!(
        get(&harness, &format!("/v1/memory/{note_id}")).await["owner"],
        successor_id,
        "restore does not reclaim transferred work"
    );

    let paused = routine(&harness, id, "Paused on retirement").await;
    let paused_id = paused["id"].as_str().unwrap();
    cli(&harness, &["team", "retire", id, "--pause"]).await;
    let paused = get(&harness, &format!("/v1/routines/{paused_id}")).await;
    assert_eq!(paused["action"]["agent_id"], "chief");
    assert_eq!(paused["enabled"], false);
    assert_eq!(paused["paused_reason"], "agent_retired");
    let bad = harness
        .cli(
            CLI,
            &["team", "retire", id, "--pause", "--hand-to", successor_id],
            None,
        )
        .await;
    assert_eq!(bad.status.code(), Some(2));
    harness.kill().await;
}

#[tokio::test]
async fn cli_proposals_lists_suppression_and_allows_the_topic_again() {
    let script = json!({"responses":[
        tool("team_propose_hire", json!({"draft":{
            "name":"Mina","persona":{"role":"Price researcher","responsibilities":["Track prices"]},"proposal_key":"price-watch"
        }}), "proposal-call"),
        send("I will not suggest that topic again.")
    ]});
    let Some(mut harness) = Harness::start(DAEMON, &script).await else {
        return;
    };
    let chief = get(&harness, "/v1/agents/chief").await;
    let home = chief["home_session_id"].as_str().unwrap();
    let mut stream = harness.events(home, 0).await;
    post(
        &harness,
        &format!("/v1/sessions/{home}/messages"),
        json!({"text":"Could a teammate track prices?","mode":"now"}),
        202,
    )
    .await;
    let ticket = loop {
        let event = stream.next().await;
        match event.kind {
            EventKind::TicketOpened { ticket } => break ticket,
            EventKind::SessionSettled {} => panic!("settled before hire proposal"),
            _ => {}
        }
    };
    assert_eq!(ticket.kind, TicketKind::Hire);
    stream.settled().await;
    let response = harness
        .human_request(
            "POST",
            &format!("/v1/tickets/{}/resolve", ticket.id),
            Some(json!({"value":"never"})),
        )
        .await;
    assert_eq!(response.status, 200, "{}", response.body);
    stream.settled().await;
    let proposals = cli(&harness, &["team", "proposals"]).await;
    assert_eq!(
        proposals[0],
        get(&harness, "/v1/team/proposals/suppressed").await
    );
    assert_eq!(proposals[0][0]["key"], "price researcher");
    assert_eq!(proposals[0][0]["label"], "Price researcher");
    let cleared = cli(
        &harness,
        &["team", "proposals", "allow", "price researcher"],
    )
    .await;
    assert_eq!(
        cleared.len(),
        1,
        "204 produces only the existing CLI summary"
    );
    assert_eq!(cli(&harness, &["team", "proposals"]).await[0], json!([]));
    harness.kill().await;
}

#[tokio::test]
async fn run_and_chat_default_to_chief_but_explicit_session_is_preserved() {
    let script = json!({"responses":[send("first reply"),send("second reply")]});
    let Some(mut harness) = Harness::start(DAEMON, &script).await else {
        return;
    };
    let chief = get(&harness, "/v1/agents/chief").await;
    let home = chief["home_session_id"].as_str().unwrap();
    for (verb, prompt, reply) in [
        ("run", "First request", "first reply"),
        ("chat", "Second request", "second reply"),
    ] {
        let output = cli(&harness, &[verb, prompt, "--wait", "--timeout", "30"]).await;
        assert_eq!(output.last().unwrap()["session_id"], home);
        assert!(
            output
                .iter()
                .any(|record| record["kind"] == "assistant.final" && record["text"] == reply),
            "{output:?}"
        );
    }
    assert_eq!(
        get(&harness, "/v1/sessions").await["sessions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let legacy = post(
        &harness,
        "/v1/sessions",
        json!({"title":"Explicit target"}),
        201,
    )
    .await;
    let legacy_id = legacy["id"].as_str().unwrap();
    harness.replace_script(&json!({"responses":[text("third reply")]}));
    assert_eq!(
        cli(
            &harness,
            &[
                "run",
                "Third request",
                "--session",
                legacy_id,
                "--wait",
                "--timeout",
                "30"
            ]
        )
        .await
        .last()
        .unwrap()["session_id"],
        legacy_id
    );
    let rejected = harness
        .cli(
            CLI,
            &["--model", "script", "chat", "No ignored flags"],
            None,
        )
        .await;
    assert_eq!(rejected.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("openside session new"));
    harness.kill().await;
}

#[tokio::test]
async fn cli_hire_from_session_carries_bounded_context_without_a_summary_model_call() {
    let long_reply = format!("{}latest useful context", "界".repeat(6000));
    let Some(mut harness) = Harness::start(DAEMON, &json!({"responses":[text(&long_reply)]})).await
    else {
        return;
    };
    let source = post(
        &harness,
        "/v1/sessions",
        json!({"title":"Price history"}),
        201,
    )
    .await;
    let source_id = source["id"].as_str().unwrap();
    let mut stream = harness.events(source_id, 0).await;
    post(
        &harness,
        &format!("/v1/sessions/{source_id}/messages"),
        json!({"text":"Compare prices","mode":"now"}),
        202,
    )
    .await;
    stream.settled().await;
    let before = harness.snapshot(source_id)["script_cursor"].clone();
    let hired = cli(
        &harness,
        &[
            "team",
            "hire",
            "--name",
            "Mina",
            "--role",
            "Price researcher",
            "--from-session",
            source_id,
        ],
    )
    .await;
    let home = hired[0]["home_session_id"].as_str().unwrap();
    let page = get(
        &harness,
        &format!("/v1/sessions/{home}/events/poll?after=0&wait=0"),
    )
    .await;
    let notice = page["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["kind"] == "message.queued")
        .expect("onboarding context queued");
    let context = notice["text"].as_str().unwrap();
    assert!(
        context.starts_with("[team notice]\nRecent conversation excerpt from"),
        "{context}"
    );
    assert!(context.contains(source_id));
    assert!(context.contains("latest useful context"));
    assert!(context.len() <= 12_000 + "[team notice]\n".len());
    assert_eq!(
        harness.snapshot(source_id)["script_cursor"],
        before,
        "the CLI does not ask the source model to summarize"
    );
    let roster_before = get(&harness, "/v1/agents").await["agents"]
        .as_array()
        .unwrap()
        .len();
    let missing = harness
        .cli(
            CLI,
            &[
                "team",
                "hire",
                "--name",
                "Nora",
                "--role",
                "Travel planner",
                "--from-session",
                "missing-session",
            ],
            None,
        )
        .await;
    assert!(!missing.status.success());
    assert_eq!(
        get(&harness, "/v1/agents").await["agents"]
            .as_array()
            .unwrap()
            .len(),
        roster_before
    );
    harness.kill().await;
}
