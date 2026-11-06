//! Native computer use over the in-memory desktop: the setting gate, per-app and per-action
//! approval, out-of-window refusal, Stop, and the app-facing routes.

use super::*;
use openside_computer::fake::Fake;
use openside_computer::{Input, Rect, Window};

fn window(id: &str, app: &str) -> Window {
    Window {
        id: id.into(),
        app: app.into(),
        title: format!("{app} main"),
        pid: 4100,
        bounds: Rect {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        },
        visible: true,
        program: Some(app.into()),
        instance: format!("i{id}"),
    }
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&vec![200; (width * height * 3) as usize])
            .unwrap();
    }
    out
}

async fn desktop(daemon: &Daemon) -> Fake {
    let fake = Fake::new(vec![window("w1", "calc"), window("w2", "firefox")]);
    fake.set_frame(png(800, 600), 800, 600);
    daemon.shared.computer.set_backend(Arc::new(fake.clone()));
    fake
}

fn enable(daemon: &Daemon, on: bool) {
    let mut preferences = daemon.shared.preferences.get();
    preferences.computer_use = on;
    daemon.shared.preferences.replace(preferences);
}

fn question(state: &openside_sessions::SessionState, ticket: &TicketId) -> String {
    state.tickets[ticket].summary.clone()
}

async fn approval(host: &Host, invocation: &ToolCall) -> TicketId {
    match host.call(invocation.clone()).await {
        ToolOutcome::Deferred {
            ticket,
            kind: DeferKind::Approval,
        } => ticket,
        other => panic!("{} should ask first: {other:?}", invocation.name),
    }
}

async fn approved(daemon: &Daemon, host: &Host, mut invocation: ToolCall) -> ToolOutcome {
    let ticket = approval(host, &invocation).await;
    assert_eq!(
        person_resolve(daemon, &ticket, json!(true)).await.status(),
        StatusCode::OK
    );
    invocation.approval_ticket = Some(ticket);
    host.call(invocation).await
}

fn failure(outcome: &ToolOutcome) -> String {
    match outcome {
        ToolOutcome::Failed { message } => message.clone(),
        ToolOutcome::Denied { reason, .. } => reason.clone(),
        other => panic!("expected a refusal: {other:?}"),
    }
}

async fn local(daemon: &Daemon, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request(method, path, body, None))
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn guard_asks_for_the_app_and_for_every_action_inside_its_window() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let fake = desktop(&daemon).await;
    let session = create_session(&daemon, "guard", PolicyMode::Guard, None).await;

    // Off by default: hidden from the agent and refused by name.
    let names: Vec<String> = host(&daemon, &session)
        .await
        .specs()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(
        !names.iter().any(|name| name.starts_with("computer_use_")),
        "{names:?}"
    );
    let refused = host(&daemon, &session)
        .await
        .call(call("computer_use_apps", json!({}), "off"))
        .await;
    assert!(!matches!(refused, ToolOutcome::Done { .. }), "{refused:?}");

    enable(&daemon, true);
    let host = host(&daemon, &session).await;
    let ToolOutcome::Done { for_model_json } = host
        .call(call("computer_use_apps", json!({}), "apps"))
        .await
    else {
        panic!("apps listing failed");
    };
    let apps: Value = serde_json::from_str(&for_model_json).unwrap();
    assert_eq!(apps["apps"][0]["app"], "calc");
    assert_eq!(apps["apps"][0]["title"], "calc main");
    assert_eq!(
        apps["apps"].as_array().map(Vec::len),
        Some(1),
        "blocked windows are left out entirely: {apps}"
    );

    // A browser can never be connected; nobody is even asked.
    let blocked = host
        .call(call(
            "computer_use_connect",
            json!({"window_id": "w2"}),
            "browser",
        ))
        .await;
    assert!(
        failure(&blocked).contains("computer_app_blocked"),
        "{blocked:?}"
    );

    // Connecting asks about this app by name.
    let connect = call(
        "computer_use_connect",
        json!({"window_id": "w1"}),
        "connect",
    );
    let ticket = approval(&host, &connect).await;
    let state = daemon.shared.db.state(&session).await.unwrap();
    assert_eq!(
        question(&state, &ticket),
        "Allow openside to see and control calc?"
    );
    let mut connect_approved = connect.clone();
    connect_approved.approval_ticket = Some(ticket.clone());
    person_resolve(&daemon, &ticket, json!(true)).await;
    assert!(matches!(
        host.call(connect_approved).await,
        ToolOutcome::Done { .. }
    ));
    let first = daemon
        .shared
        .computer
        .computer()
        .connection("guard")
        .unwrap();

    // Pointer actions need a frame first, and points outside it never reach the person.
    let early = host
        .call(call(
            "computer_use_act",
            json!({"action":"click","x":10,"y":10}),
            "early",
        ))
        .await;
    assert!(
        failure(&early).contains("computer_frame_required"),
        "{early:?}"
    );
    let ToolOutcome::DoneWithParts {
        for_model_json,
        parts,
    } = host
        .call(call("computer_use_screenshot", json!({}), "shot"))
        .await
    else {
        panic!("screenshot failed");
    };
    let shot: Value = serde_json::from_str(&for_model_json).unwrap();
    assert_eq!(
        (shot["width"].as_u64(), shot["height"].as_u64()),
        (Some(800), Some(600))
    );
    assert_eq!(parts.len(), 1);
    let tickets_before = daemon
        .shared
        .db
        .state(&session)
        .await
        .unwrap()
        .tickets
        .len();
    let outside = host
        .call(call(
            "computer_use_act",
            json!({"action":"click","x":900,"y":10}),
            "outside",
        ))
        .await;
    assert!(
        failure(&outside).contains("computer_outside_window"),
        "{outside:?}"
    );
    assert_eq!(
        daemon
            .shared
            .db
            .state(&session)
            .await
            .unwrap()
            .tickets
            .len(),
        tickets_before
    );

    // Every write asks, names the action, and runs only after approval.
    let click = call(
        "computer_use_act",
        json!({"action":"click","x":400,"y":300}),
        "click",
    );
    let click_ticket = approval(&host, &click).await;
    let state = daemon.shared.db.state(&session).await.unwrap();
    assert_eq!(
        question(&state, &click_ticket),
        "Allow openside to click at 400, 300 in calc?"
    );
    assert!(fake.inputs().is_empty());
    person_resolve(&daemon, &click_ticket, json!(true)).await;
    let mut click_approved = click.clone();
    click_approved.approval_ticket = Some(click_ticket.clone());
    assert!(matches!(
        host.call(click_approved.clone()).await,
        ToolOutcome::DoneWithParts { .. }
    ));
    assert_eq!(fake.inputs().len(), 1);
    assert!(matches!(fake.inputs()[0].1, Input::Click { .. }));

    // The next write asks again; an earlier approval does not cover different input.
    let typed = call(
        "computer_use_act",
        json!({"action":"type","text":"secret words"}),
        "type",
    );
    let type_ticket = approval(&host, &typed).await;
    let state = daemon.shared.db.state(&session).await.unwrap();
    assert_eq!(
        question(&state, &type_ticket),
        "Allow openside to type 12 characters in calc?"
    );
    let mut reused = typed.clone();
    reused.approval_ticket = Some(click_ticket.clone());
    assert!(!matches!(
        host.call(reused).await,
        ToolOutcome::Done { .. } | ToolOutcome::DoneWithParts { .. }
    ));
    assert_eq!(fake.inputs().len(), 1);

    // Stop ends the connection at once; an approval given before it no longer acts.
    person_resolve(&daemon, &type_ticket, json!(true)).await;
    let (status, stopped) = local(
        &daemon,
        "POST",
        &format!("/v1/sessions/{session}/computer-use/stop"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{stopped}");
    assert_eq!(stopped["stopped"], true);
    assert_eq!(stopped["connection"]["app"], "calc");
    let mut late = typed.clone();
    late.approval_ticket = Some(type_ticket);
    let late = host.call(late).await;
    assert!(
        !matches!(
            late,
            ToolOutcome::Done { .. } | ToolOutcome::DoneWithParts { .. }
        ),
        "{late:?}"
    );
    assert_eq!(fake.inputs().len(), 1);
    assert_eq!(fake.releases(), 1);

    // A new connection gets a new id: the old click approval does not carry over.
    assert!(matches!(
        approved(
            &daemon,
            &host,
            call("computer_use_connect", json!({"window_id": "w1"}), "again")
        )
        .await,
        ToolOutcome::Done { .. }
    ));
    let second = daemon
        .shared
        .computer
        .computer()
        .connection("guard")
        .unwrap();
    assert_ne!(first.id, second.id);
    host.call(call("computer_use_screenshot", json!({}), "shot-2"))
        .await;
    let mut stale = click.clone();
    stale.approval_ticket = Some(click_ticket);
    assert!(!matches!(
        host.call(stale).await,
        ToolOutcome::Done { .. } | ToolOutcome::DoneWithParts { .. }
    ));
    assert_eq!(fake.inputs().len(), 1);

    // The app sees the connection and its eligibility; turning the setting off stops it.
    let (status, status_body) = local(&daemon, "GET", "/v1/computer-use", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(status_body["enabled"], true);
    assert_eq!(status_body["eligible"], true);
    assert_eq!(status_body["backend"], "fake");
    let (_, current) = local(
        &daemon,
        "GET",
        &format!("/v1/sessions/{session}/computer-use"),
        Value::Null,
    )
    .await;
    assert_eq!(current["connection"]["id"], json!(second.id));
    assert_eq!(current["connection"]["frame_width"], 800);
    let (status, prefs) = local(
        &daemon,
        "PATCH",
        "/v1/settings/preferences",
        json!({"computer_use": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{prefs}");
    assert_eq!(prefs["computer_use"], false);
    assert!(daemon
        .shared
        .computer
        .computer()
        .connection("guard")
        .is_none());
    let audit = std::fs::read_to_string(
        openside_config::data_layout::DataPath::Audit.path(&daemon.shared.config.data_dir),
    )
    .unwrap();
    assert!(audit.contains("\"tool\":\"computer_use\""), "{audit}");
    assert!(audit.contains("\"decision\":\"connected\""));
    assert!(audit.contains("\"decision\":\"stopped\""));
    assert!(
        !audit.contains("secret words"),
        "typed text must never be logged"
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn full_access_still_asks_for_the_app_and_read_only_never_connects() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let fake = desktop(&daemon).await;
    enable(&daemon, true);

    let full = create_session(&daemon, "full", PolicyMode::Full, None).await;
    let host_full = host(&daemon, &full).await;
    assert!(matches!(
        approved(
            &daemon,
            &host_full,
            call("computer_use_connect", json!({"window_id": "w1"}), "c")
        )
        .await,
        ToolOutcome::Done { .. }
    ));
    host_full
        .call(call("computer_use_screenshot", json!({}), "s"))
        .await;
    assert!(matches!(
        host_full
            .call(call(
                "computer_use_act",
                json!({"action":"key","keys":"ctrl+s"}),
                "k"
            ))
            .await,
        ToolOutcome::DoneWithParts { .. }
    ));
    assert_eq!(fake.inputs().len(), 1);

    let read_only = create_session(&daemon, "ro", PolicyMode::ReadOnly, None).await;
    let refused = host(&daemon, &read_only)
        .await
        .call(call(
            "computer_use_connect",
            json!({"window_id": "w1"}),
            "ro",
        ))
        .await;
    assert!(matches!(refused, ToolOutcome::Denied { .. }), "{refused:?}");
    assert!(daemon.shared.computer.computer().connection("ro").is_none());
    daemon.shutdown().await;
}

#[tokio::test]
async fn stop_cancels_an_action_in_flight_and_a_stopped_chat_disconnects() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let fake = desktop(&daemon).await;
    enable(&daemon, true);
    let session = create_session(&daemon, "flight", PolicyMode::Full, None).await;
    let host_full = Arc::new(host(&daemon, &session).await);
    approved(
        &daemon,
        &host_full,
        call("computer_use_connect", json!({"window_id": "w1"}), "c"),
    )
    .await;
    fake.hold_input(true);
    let started = fake.input_started.clone();
    let running = {
        let host_full = host_full.clone();
        tokio::spawn(async move {
            host_full
                .call(call(
                    "computer_use_act",
                    json!({"action":"type","text":"slow"}),
                    "slow",
                ))
                .await
        })
    };
    started.notified().await;
    let (status, stopped) = local(
        &daemon,
        "POST",
        &format!("/v1/sessions/{session}/computer-use/stop"),
        Value::Null,
    )
    .await;
    assert_eq!(
        (status, stopped["stopped"].as_bool()),
        (StatusCode::OK, Some(true))
    );
    let outcome = tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .expect("stop must end the action")
        .unwrap();
    assert!(
        matches!(&outcome, ToolOutcome::Denied { reason, .. } if reason == "computer_use_stopped"),
        "{outcome:?}"
    );
    assert!(fake.inputs().is_empty());

    // Stopping the chat itself also disconnects.
    fake.hold_input(false);
    approved(
        &daemon,
        &host_full,
        call("computer_use_connect", json!({"window_id": "w1"}), "c2"),
    )
    .await;
    assert!(daemon
        .shared
        .computer
        .computer()
        .connection("flight")
        .is_some());
    daemon
        .shared
        .emit(&session, None, EventKind::SessionStopped {})
        .await
        .unwrap();
    assert!(daemon
        .shared
        .computer
        .computer()
        .connection("flight")
        .is_none());
    daemon.shutdown().await;
}

#[tokio::test]
async fn a_private_chat_never_sees_or_connects_the_desktop() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let fake = desktop(&daemon).await;
    enable(&daemon, true);
    let (status, private) = local(
        &daemon,
        "POST",
        "/v1/sessions",
        json!({"title": "Private", "incognito": true, "model": std::any::type_name::<TestRuntime>()}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{private}");
    let private = SessionId::new(private["id"].as_str().unwrap());
    // With the setting on, a public chat lists the tools; a private one must not.
    let public = create_session(&daemon, "public", PolicyMode::Guard, None).await;
    let public_names: Vec<String> = host(&daemon, &public)
        .await
        .specs()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(public_names.iter().any(|name| name == "computer_use_apps"));
    let host = host(&daemon, &private).await;
    let names: Vec<String> = host.specs().into_iter().map(|s| s.name).collect();
    assert!(
        !names.iter().any(|name| name.starts_with("computer_use_")),
        "{names:?}"
    );
    for (name, args) in [
        ("computer_use_apps", json!({})),
        ("computer_use_connect", json!({"window_id": "w1"})),
        ("computer_use_screenshot", json!({})),
        ("computer_use_screenshot", json!({"output": true})),
        ("computer_use_act", json!({"action": "key", "key": "x"})),
    ] {
        let outcome = host.call(call(name, args, name)).await;
        assert!(
            !matches!(
                outcome,
                ToolOutcome::Done { .. }
                    | ToolOutcome::DoneWithParts { .. }
                    | ToolOutcome::Deferred { .. }
            ),
            "{name}: {outcome:?}"
        );
    }
    assert!(daemon
        .shared
        .computer
        .computer()
        .connection(private.as_str())
        .is_none());
    let (status, _) = local(
        &daemon,
        "GET",
        &format!("/v1/sessions/{private}/computer-use/frames"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(fake.inputs().is_empty());
    daemon.shutdown().await;
}

#[tokio::test]
async fn frames_are_for_the_person_and_a_few_viewers_only() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let _fake = desktop(&daemon).await;
    enable(&daemon, true);
    let session = create_session(&daemon, "frames", PolicyMode::Guard, None).await;
    let path = format!("/v1/sessions/{session}/computer-use/frames");
    let tcp = api::router(daemon.shared.clone(), false);
    let state_path = format!("/v1/sessions/{session}/computer-use");
    let remote_at = |path: &str, token: &str| {
        let mut request = request("GET", path, Value::Null, Some(token));
        request
            .headers_mut()
            .insert("host", "127.0.0.1".parse().unwrap());
        request
    };
    let remote = |token: &str| remote_at(&path, token);

    let agent = daemon
        .shared
        .db
        .pair_as(
            "agent".into(),
            vec![CapabilityScope::Browse, CapabilityScope::Read],
            openside_proto::DeviceKind::AgentClient,
        )
        .await
        .unwrap()
        .1;
    let refused = tcp.clone().oneshot(remote(&agent)).await.unwrap();
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_body(refused).await["code"], "person_device_required");
    // The connection's app, title and pid are the person's too.
    let refused = tcp
        .clone()
        .oneshot(remote_at(&state_path, &agent))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_body(refused).await["code"], "person_device_required");

    // The person's own phone may watch on purpose.
    let phone = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Browse, CapabilityScope::Read])
        .await
        .unwrap()
        .1;
    let watching = tcp.clone().oneshot(remote(&phone)).await.unwrap();
    assert_eq!(watching.status(), StatusCode::OK);
    let state = tcp
        .clone()
        .oneshot(remote_at(&state_path, &phone))
        .await
        .unwrap();
    assert_eq!(state.status(), StatusCode::OK);

    let missing = local(
        &daemon,
        "GET",
        "/v1/sessions/nope/computer-use/frames",
        Value::Null,
    )
    .await;
    assert_eq!(missing.0, StatusCode::NOT_FOUND);

    let unix = api::router(daemon.shared.clone(), true);
    let mut open = vec![watching];
    for _ in 1..4 {
        let response = unix
            .clone()
            .oneshot(request("GET", &path, Value::Null, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        open.push(response);
    }
    let fifth = unix
        .clone()
        .oneshot(request("GET", &path, Value::Null, None))
        .await
        .unwrap();
    assert_eq!(fifth.status(), StatusCode::TOO_MANY_REQUESTS);
    drop(open.pop());
    let again = unix
        .oneshot(request("GET", &path, Value::Null, None))
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::OK);
    drop(open);
    drop(again);
    daemon.shutdown().await;
}
