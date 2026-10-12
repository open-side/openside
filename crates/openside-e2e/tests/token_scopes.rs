//! PP-27 over the real loopback listener: a read-scoped remote client cannot steer.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use openside_e2e::Harness;
use openside_proto::{CapabilityScope, ROUTES};
use serde_json::{json, Value};

const CLI: &str = env!("CARGO_BIN_EXE_e2e-openside");

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
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .split_once('\t')
        .unwrap()
        .1
        .to_owned()
}

#[tokio::test]
async fn read_token_over_tcp_cannot_reach_any_steer_route_and_changes_nothing() {
    let Some(mut harness) = Harness::start(
        env!("CARGO_BIN_EXE_e2e-opensided"),
        &json!({"responses":[{"parts":[{"type":"text","text":"done"}]}],"delay_ms":1500}),
    )
    .await
    else {
        return;
    };
    let created = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"title":"scopes","mode":"guard","workspace":harness.workspace()})),
        )
        .await;
    let id = created.body["id"].as_str().unwrap().to_owned();
    let read = mint(&harness, "read").await;
    let steer = mint(&harness, "steer").await;

    // A run is in flight with one pending message, so every steer route has a live target.
    for (mid, mode) in [("running", "now"), ("pending", "queue")] {
        let response = harness
            .tcp_request(
                "POST",
                &format!("/v1/sessions/{id}/messages"),
                Some(&steer),
                Some(json!({"message_id":mid,"text":mid,"mode":mode})),
            )
            .await;
        assert_eq!(response.status, 202, "{}", response.body);
    }
    let state = |token: String| {
        let (harness, id) = (&harness, id.clone());
        async move {
            let session = harness
                .tcp_request("GET", &format!("/v1/sessions/{id}"), Some(&token), None)
                .await;
            assert_eq!(session.status, 200);
            (
                session.body["last_seq"].clone(),
                session.body["queue"].clone(),
            )
        }
    };
    let before = state(read.clone()).await;

    let mut probed = 0;
    for route in ROUTES
        .iter()
        .filter(|route| route.scope == CapabilityScope::Steer)
    {
        let path = route
            .path
            .replace("{id}", &id)
            .replace("{mid}", "pending")
            .replace("{sid}", "child");
        let body: Option<Value> = match route.path {
            "/v1/sessions/{id}/messages" => Some(json!({"text":"hijack","mode":"now"})),
            "/v1/sessions/{id}/queue/{mid}" => Some(json!({"text":"hijack"})),
            "/v1/sessions/{id}/queue/{mid}/move" => Some(json!({"position":0})),
            _ => Some(json!({})),
        };
        let response = harness
            .tcp_request(route.method.as_str(), &path, Some(&read), body)
            .await;
        assert_eq!(
            response.status,
            403,
            "{} {path} accepted a read token",
            route.method.as_str()
        );
        probed += 1;
    }
    assert!(probed >= 8, "only {probed} steer routes probed");
    assert_eq!(
        state(read.clone()).await,
        before,
        "a refused request changed the session"
    );
    harness.kill().await;
}
