//! Hermetic process tests for controlled same-PID restart and launcher authority continuity.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use openside_e2e::Harness;
use serde_json::{json, Value};
use std::time::Duration;

#[tokio::test]
async fn controlled_restart_preserves_pid_and_launcher_secret_but_changes_boot_id() {
    let mut harness = Harness::start(
        env!("CARGO_BIN_EXE_e2e-opensided"),
        &json!({"responses":[]}),
    )
    .await
    .expect("scripted Python runtime is required");
    let pid = harness.daemon_pid();
    let before = harness.request("GET", "/v1/daemon", None).await.body;
    let local = harness
        .request("GET", "/v1/remote/local-access", None)
        .await
        .body;
    let response = harness
        .request("POST", "/v1/daemon/restart", Some(json!({})))
        .await;
    assert_eq!(response.status, 202, "{}", response.body);
    let client = reqwest::Client::builder()
        .unix_socket(harness.socket.clone())
        .build()
        .unwrap();
    let after: Value = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if let Ok(response) = client.get("http://localhost/v1/daemon").send().await {
                if let Ok(body) = response.json::<Value>().await {
                    if body["boot_id"].is_string() && body["boot_id"] != before["boot_id"] {
                        break body;
                    }
                }
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(harness.daemon_pid(), pid);
    assert_ne!(after["boot_id"], before["boot_id"]);
    assert_eq!(after["restart_method"], "re_exec");
    let next = harness
        .request("GET", "/v1/remote/local-access", None)
        .await
        .body;
    assert_eq!(next["secret"], local["secret"]);
    assert!(!harness.data_dir.join("restart-handoff.json").exists());
    harness.kill().await;
    harness.launch().await;
    let ordinary = harness
        .request("GET", "/v1/remote/local-access", None)
        .await
        .body;
    assert_ne!(ordinary["secret"], local["secret"]);
    harness.kill().await;
}
