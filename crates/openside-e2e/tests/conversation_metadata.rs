//! Conversation identity and activity through the public API, with a real scripted sidecar.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use openside_e2e::Harness;
use openside_proto::EventKind;
use serde_json::json;

#[tokio::test]
async fn scripted_title_and_activity_follow_question_resolution_and_survive_restart() {
    let script = json!({"title":"Compare Acme plans", "responses":[
        {"parts":[{"type":"tool_call","name":"ask_user","id":"choose-plan","args":{"question":"Which plan should I compare?","options":["Team","Business"]}}]},
        {"parts":[{"type":"text","text":"The Team plan fits your needs."}]},
        {"parts":[{"type":"text","text":"I kept your conversation name."}]}
    ]});
    let mut harness = Harness::start(env!("CARGO_BIN_EXE_e2e-opensided"), &script)
        .await
        .expect("scripted sidecar environment is required");
    let created = harness
        .request("POST", "/v1/sessions", Some(json!({})))
        .await;
    assert_eq!(created.status, 201, "{}", created.body);
    let id = created.body["id"].as_str().unwrap();
    let mut stream = harness.events(id, 0).await;
    let sent = harness.request("POST", &format!("/v1/sessions/{id}/messages"), Some(json!({
        "message_id":"first", "text":"https://shop.example/plans?token=private", "mode":"now"
    }))).await;
    assert_eq!(sent.status, 202);
    let ticket = loop {
        let event = stream.next().await;
        if let EventKind::TicketOpened { ticket } = event.kind {
            break ticket;
        }
    };
    assert_eq!(ticket.tool_call_id, "choose-plan");
    let waiting = harness
        .request("GET", &format!("/v1/sessions/{id}"), None)
        .await;
    assert_eq!(waiting.body["activity"], "Waiting for you");
    assert_eq!(
        waiting.body["title"], "New session",
        "raw URL prompts must never name a conversation"
    );
    let answer = harness
        .request(
            "POST",
            &format!("/v1/tickets/{}/resolve", ticket.id),
            Some(json!({"value":"Team"})),
        )
        .await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    let mut events = stream.settled().await;
    if !events
        .iter()
        .any(|event| matches!(event.kind, EventKind::AssistantFinal { .. }))
    {
        events.extend(stream.settled().await);
    }
    while !events
        .iter()
        .any(|event| matches!(event.kind, EventKind::SessionTitleProposed { .. }))
    {
        events.push(stream.next().await);
    }
    assert!(events.iter().any(|event| matches!(&event.kind, EventKind::SessionTitleProposed { title } if title == "Compare Acme plans")), "{events:?}");
    let listed = harness.request("GET", "/v1/sessions", None).await;
    let row = listed.body["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == id)
        .unwrap();
    assert_eq!(row["title"], "Compare Acme plans");
    assert_eq!(row["activity"], "Done");
    assert_eq!(harness.snapshot(id)["title_attempted"], true);
    let renamed = harness
        .request(
            "PATCH",
            &format!("/v1/sessions/{id}"),
            Some(json!({"title":"My plan comparison"})),
        )
        .await;
    assert_eq!(renamed.status, 200);
    harness.kill().await;
    harness.launch().await;
    let mut stream = harness
        .events(id, renamed.body["last_seq"].as_u64().unwrap())
        .await;
    let sent = harness
        .request(
            "POST",
            &format!("/v1/sessions/{id}/messages"),
            Some(json!({"message_id":"follow-up","text":"Keep going","mode":"now"})),
        )
        .await;
    assert_eq!(sent.status, 202);
    let events = stream.settled().await;
    assert!(!events
        .iter()
        .any(|event| matches!(event.kind, EventKind::SessionTitleProposed { .. })));
    let final_state = harness
        .request("GET", &format!("/v1/sessions/{id}"), None)
        .await;
    assert_eq!(final_state.body["title"], "My plan comparison");
    assert_eq!(final_state.body["activity"], "Done");
    harness.kill().await;
}
