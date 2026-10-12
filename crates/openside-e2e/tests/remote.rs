//! Remote device flows against the real daemon, CLI and Python sidecar (F-3, PP-22, PP-4):
//! QR pairing, Web Push to a 127.0.0.1 push service, answering from the pushed deep link,
//! revocation, and recovery when the daemon is started the way the service unit starts it.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use openside_e2e::Harness;
use openside_push::fake_push::FakePushService;
use serde_json::{json, Value};

const CLI: &str = env!("CARGO_BIN_EXE_e2e-openside");
const DAEMON: &str = env!("CARGO_BIN_EXE_e2e-opensided");

fn stdout_json(output: &std::process::Output) -> Value {
    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // `--json` ends every command with a summary record; the payload is the first line.
    let first = output.stdout.split(|byte| *byte == b'\n').next().unwrap();
    serde_json::from_slice(first).expect("CLI JSON")
}

#[tokio::test]
async fn phone_pairs_by_qr_is_pushed_answers_from_the_deep_link_and_hears_the_task_finish() {
    let Some(mut harness) = Harness::prepare(
        DAEMON,
        &json!({"responses":[
            {"parts":[{"type":"tool_call","name":"ask_user","id":"ask-call",
                "args":{"question":"Deploy to staging?","options":["yes","no"]}}]},
            {"parts":[{"type":"text","text":"deployed"}]}
        ]}),
    ) else {
        return;
    };
    let push = FakePushService::spawn().await.unwrap();
    harness.append_config(&format!(
        "[remote]\npublic_url = \"https://desk.example.test\"\n[network]\nallow = [\"http://{}\"]\n",
        push.addr()
    ));
    harness.launch().await;

    // The terminal shows a scannable code for the HTTPS front.
    let text = harness.cli(CLI, &["pair", "--name", "phone"], None).await;
    assert!(
        text.status.success(),
        "{}",
        String::from_utf8_lossy(&text.stderr)
    );
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(
        text.contains('▀') || text.contains('█'),
        "no QR code:\n{text}"
    );
    assert!(text.contains("https://desk.example.test/pair#"), "{text}");
    let offer = stdout_json(
        &harness
            .cli(
                CLI,
                &[
                    "--json", "pair", "--scope", "read", "--scope", "approve", "--name", "phone",
                ],
                None,
            )
            .await,
    );
    let code = offer["code"].as_str().unwrap();
    assert_eq!(
        offer["url"],
        format!("https://desk.example.test/pair#{code}")
    );

    // The phone exchanges the code over TCP with no prior credential.
    let paired = harness
        .tcp_request(
            "POST",
            "/v1/pairing/exchange",
            None,
            Some(json!({"code": code})),
        )
        .await;
    assert_eq!(paired.status, 201, "{}", paired.body);
    let token = paired.body["token"].as_str().unwrap().to_owned();
    let device = paired.body["device_id"].as_str().unwrap().to_owned();
    let again = harness
        .tcp_request(
            "POST",
            "/v1/pairing/exchange",
            None,
            Some(json!({"code": code})),
        )
        .await;
    assert_eq!(again.status, 404);

    let subscription = push.subscribe();
    let vapid = harness
        .tcp_request("GET", "/v1/push/vapid", Some(&token), None)
        .await;
    assert_eq!(vapid.status, 200);
    let created = harness
        .tcp_request(
            "POST",
            "/v1/push/subscriptions",
            Some(&token),
            Some(subscription.to_json()),
        )
        .await;
    assert_eq!(created.status, 201, "{}", created.body);

    // A headless task asks a question and parks; the phone is pushed.
    let output = harness.cli(CLI, &["--json", "deploy it"], None).await;
    assert!(output.status.success());
    let summary: Value = String::from_utf8_lossy(&output.stdout)
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<Value>(line).ok())
        .unwrap();
    assert_eq!(summary["status"], "waiting_human", "{summary}");
    let session = summary["session_id"].as_str().unwrap().to_owned();
    let deliveries = push.wait_for(1, Duration::from_secs(15)).await;
    assert_eq!(deliveries.len(), 1, "no push: {:?}", push.rejections());
    let message = deliveries[0].json();
    assert_eq!(
        deliveries[0].vapid_key,
        vapid.body["public_key"].as_str().unwrap()
    );
    assert_eq!(message["kind"], "ticket");
    assert_eq!(message["title"], "Question from the agent");
    let link = message["url"].as_str().unwrap();
    let ticket = link
        .strip_prefix(&format!("/sessions/{session}?ticket="))
        .unwrap_or_else(|| panic!("deep link {link} does not target the session's ticket"));

    // Following the link: the phone reads and answers that ticket with its own token.
    let shown = harness
        .tcp_request("GET", &format!("/v1/tickets/{ticket}"), Some(&token), None)
        .await;
    assert_eq!(
        (shown.status, shown.body["status"].as_str()),
        (200, Some("pending"))
    );
    let answered = harness
        .tcp_request(
            "POST",
            &format!("/v1/tickets/{ticket}/resolve"),
            Some(&token),
            Some(json!({"value":"yes"})),
        )
        .await;
    assert_eq!(answered.status, 200, "{}", answered.body);

    let deliveries = push.wait_for(2, Duration::from_secs(30)).await;
    assert_eq!(
        deliveries.len(),
        2,
        "no completion push: {:?}",
        push.rejections()
    );
    let finished = deliveries[1].json();
    assert_eq!(finished["kind"], "settled");
    assert_eq!(finished["url"], format!("/sessions/{session}"));
    let state = harness
        .request("GET", &format!("/v1/sessions/{session}"), None)
        .await;
    assert_eq!(state.body["status"], "idle", "{}", state.body);

    // Revocation from the desk is immediate.
    let listed = stdout_json(&harness.cli(CLI, &["--json", "device", "list"], None).await);
    assert!(listed["devices"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["device_id"] == device.as_str()));
    let revoked = harness.cli(CLI, &["device", "revoke", &device], None).await;
    assert!(
        revoked.status.success(),
        "{}",
        String::from_utf8_lossy(&revoked.stderr)
    );
    assert_eq!(
        harness
            .tcp_request("GET", "/v1/sessions", Some(&token), None)
            .await
            .status,
        401
    );
    let listed = stdout_json(&harness.cli(CLI, &["--json", "device", "list"], None).await);
    assert!(!listed["devices"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["device_id"] == device.as_str()));
    harness.kill().await;
}

/// PP-4: the generated systemd unit runs the daemon with no arguments; started that way
/// from the configuration file alone, a crashed daemon comes back with its sessions.
#[tokio::test]
async fn daemon_started_like_the_service_unit_recovers_its_session_after_a_crash() {
    let Some(mut harness) = Harness::prepare(
        DAEMON,
        &json!({"responses":[
            {"parts":[{"type":"text","text":"never finished"}]},
            {"parts":[{"type":"text","text":"back after the restart"}]}
        ],"delay_ms":1500}),
    ) else {
        return;
    };
    let unit = harness
        .cli(
            CLI,
            &[
                "service",
                "unit",
                "--platform",
                "systemd",
                "--daemon",
                DAEMON,
            ],
            None,
        )
        .await;
    assert!(
        unit.status.success(),
        "{}",
        String::from_utf8_lossy(&unit.stderr)
    );
    let unit = String::from_utf8(unit.stdout).unwrap();
    assert!(unit.contains("Restart=on-failure"));
    let exec = unit
        .lines()
        .find_map(|line| line.strip_prefix("ExecStart="))
        .unwrap();
    assert_eq!(
        exec, DAEMON,
        "the unit passes no arguments; configuration supplies them"
    );
    harness.append_config(&format!(
        "[daemon]\ndata_dir = {}\nsocket = {}\nlisten = \"127.0.0.1:0\"\n",
        json!(harness.data_dir),
        json!(harness.socket)
    ));
    harness.launch_with(Vec::new()).await;

    let created = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"title":"boot","mode":"guard","workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(created.status, 201, "{}", created.body);
    let session = created.body["id"].as_str().unwrap().to_owned();
    let sent = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            Some(json!({"message_id":"m1","text":"keep going","mode":"queue"})),
        )
        .await;
    assert_eq!(sent.status, 202, "{}", sent.body);
    tokio::time::sleep(Duration::from_millis(300)).await;

    // What Restart=on-failure does: the process dies, the manager runs ExecStart again.
    harness.crash_daemon().await;
    harness.launch_with(Vec::new()).await;
    // The interrupted turn is reported, not lost, and the session takes new work.
    let mut events = harness.events(&session, 0).await;
    let recovered = events.settled().await;
    assert!(
        recovered.iter().any(|event| matches!(
            &event.kind,
            openside_proto::EventKind::MessageCompleted { message_id, reason: Some(reason), .. }
                if message_id.as_str() == "m1" && reason == "interrupted"
        )),
        "{recovered:#?}"
    );
    let sent = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            Some(json!({"message_id":"m2","text":"again","mode":"queue"})),
        )
        .await;
    assert_eq!(sent.status, 202, "{}", sent.body);
    let settled = events.settled().await;
    let text: String = settled
        .iter()
        .filter_map(|event| match &event.kind {
            openside_proto::EventKind::AssistantFinal { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    let kinds: Vec<String> = settled
        .iter()
        .map(|event| format!("{:?}", event.kind))
        .collect();
    assert!(
        !text.is_empty(),
        "restarted daemon did not run the session: {kinds:#?}"
    );
    harness.kill().await;
}
