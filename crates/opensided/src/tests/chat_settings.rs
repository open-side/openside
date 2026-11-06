//! Person preferences, per-chat options, the read marker, context usage, About and the
//! notification kinds (`/v1/settings/preferences`, `PATCH /v1/sessions/{id}`, `/v1/about`).

use std::time::Duration;

use axum::{http::StatusCode, Router};
use openside_push::fake_push::FakePushService;
use serde_json::{json, Value};
use tower::ServiceExt;

use super::*;

async fn call(
    router: &Router,
    method: &str,
    path: &str,
    body: Value,
    token: Option<&str>,
) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(request(method, path, body, token))
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, body)
}

fn unix(daemon: &Daemon) -> Router {
    api::router(daemon.shared.clone(), true)
}

async fn new_chat(daemon: &Daemon, body: Value) -> String {
    let mut body = body;
    body["model"] = json!(std::any::type_name::<TestRuntime>());
    let (status, created) = call(&unix(daemon), "POST", "/v1/sessions", body, None).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn preferences_default_persist_validate_and_apply_without_restart() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let unix = unix(&daemon);

    let (status, prefs) = call(&unix, "GET", "/v1/settings/preferences", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        prefs,
        json!({
            "name": null,
            "notifications": ["needs_attention", "task_completed", "routine_completed"],
            "completion_sound": true,
            "follow_up": "queue",
            "confirmation": "skip",
            "agent_tab_cleanup_minutes": 15,
            "save_sessions": false,
            "mcp_server": true,
            "computer_use": false,
        })
    );

    let (status, prefs) = call(
        &unix,
        "PATCH",
        "/v1/settings/preferences",
        json!({
            "name": "  Ada  ",
            "notifications": ["task_completed"],
            "completion_sound": false,
            "follow_up": "steer",
            "confirmation": "ask_first",
            "agent_tab_cleanup_minutes": 60,
            "save_sessions": true,
            "mcp_server": false,
        }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{prefs}");
    assert_eq!(prefs["name"], "Ada");
    assert_eq!(prefs["notifications"], json!(["task_completed"]));
    assert_eq!(prefs["follow_up"], "steer");
    assert_eq!(prefs["confirmation"], "ask_first");
    assert_eq!(prefs["mcp_server"], false);
    let file = std::fs::read_to_string(root.path().join("config.toml")).unwrap();
    let stored = openside_config::parse_document(&root.path().join("config.toml"), &file).unwrap();
    assert_eq!(stored.preferences.name.as_deref(), Some("Ada"));
    assert_eq!(stored.preferences.agent_tab_cleanup_minutes, 60);
    assert!(stored.preferences.save_sessions);
    // Applied live: the running copy already carries the new preferences.
    assert_eq!(daemon.shared.preferences.get(), stored.preferences);
    assert_eq!(
        call(&unix, "GET", "/v1/settings/preferences", Value::Null, None)
            .await
            .1["name"],
        "Ada"
    );

    // An empty name clears it.
    let (_, prefs) = call(
        &unix,
        "PATCH",
        "/v1/settings/preferences",
        json!({"name": ""}),
        None,
    )
    .await;
    assert_eq!(prefs["name"], Value::Null);

    for invalid in [
        json!({"agent_tab_cleanup_minutes": 7}),
        json!({"notifications": ["task_completed", "task_completed"]}),
        json!({"name": "x".repeat(81)}),
        json!({"name": "two\nlines"}),
    ] {
        let (status, body) = call(&unix, "PATCH", "/v1/settings/preferences", invalid, None).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::BAD_REQUEST, Some("invalid_preferences"))
        );
    }
    let (status, body) = call(&unix, "PATCH", "/v1/settings/preferences", json!({}), None).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_value"))
    );
    let (status, _) = call(
        &unix,
        "PATCH",
        "/v1/settings/preferences",
        json!({"unknown": true}),
        None,
    )
    .await;
    assert!(status.is_client_error());
}

#[tokio::test]
async fn paired_devices_change_preferences_but_not_the_mcp_switch() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let tcp = api::router(daemon.shared.clone(), false);
    let (_, person) = daemon
        .shared
        .db
        .pair_named("phone".into(), vec![CapabilityScope::Admin])
        .await
        .unwrap();
    let (_, agent) = daemon
        .shared
        .db
        .pair_as(
            "bot".into(),
            vec![CapabilityScope::Admin],
            openside_proto::DeviceKind::AgentClient,
        )
        .await
        .unwrap();
    let (status, prefs) = call(
        &tcp,
        "PATCH",
        "/v1/settings/preferences",
        json!({"confirmation": "ask_first"}),
        Some(&person),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{prefs}");
    let (status, body) = call(
        &tcp,
        "PATCH",
        "/v1/settings/preferences",
        json!({"mcp_server": false}),
        Some(&person),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("local_access_required"))
    );
    let (status, _) = call(
        &tcp,
        "PATCH",
        "/v1/settings/preferences",
        json!({"name": "Bot"}),
        Some(&agent),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, prefs) = call(
        &tcp,
        "GET",
        "/v1/settings/preferences",
        Value::Null,
        Some(&agent),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (
            prefs["confirmation"].as_str(),
            prefs["mcp_server"].as_bool()
        ),
        (Some("ask_first"), Some(true))
    );
}

#[tokio::test]
async fn only_a_person_flips_the_mcp_server_and_computer_use() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let tcp = api::router(daemon.shared.clone(), false);
    let (_, phone) = daemon
        .shared
        .db
        .pair_named("phone".into(), vec![CapabilityScope::Admin])
        .await
        .unwrap();
    let mcp = |method: &str, path: &str, body: Value| {
        let mut request = request(method, path, body, None);
        request.headers_mut().insert(
            "x-openside-client",
            axum::http::HeaderValue::from_static("mcp"),
        );
        let router = unix(&daemon);
        async move {
            let response = router.oneshot(request).await.unwrap();
            let status = response.status();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            (
                status,
                serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
            )
        }
    };
    let person_required = (StatusCode::FORBIDDEN, Some("person_access_required"));

    // An MCP client on the socket keeps ordinary preferences but cannot widen its own reach.
    for body in [json!({"computer_use": true}), json!({"mcp_server": false})] {
        let (status, refused) = mcp("PATCH", "/v1/settings/preferences", body).await;
        assert_eq!(
            (status, refused["code"].as_str()),
            person_required,
            "{refused}"
        );
    }
    let (status, prefs) = mcp(
        "PATCH",
        "/v1/settings/preferences",
        json!({"completion_sound": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{prefs}");
    let (_, document) = call(
        &unix(&daemon),
        "GET",
        "/v1/settings/config",
        Value::Null,
        None,
    )
    .await;
    let mut config = document["config"].clone();
    config["preferences"]["computer_use"] = json!(true);
    let (status, refused) = mcp(
        "PUT",
        "/v1/settings/config",
        json!({"config": config.clone()}),
    )
    .await;
    assert_eq!(
        (status, refused["code"].as_str()),
        person_required,
        "{refused}"
    );
    assert!(!daemon.shared.preferences.get().computer_use);

    // A paired device without local authority is still refused for locality first.
    let (status, refused) = call(
        &tcp,
        "PATCH",
        "/v1/settings/preferences",
        json!({"computer_use": true}),
        Some(&phone),
    )
    .await;
    assert_eq!(
        (status, refused["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("local_access_required"))
    );

    // The person at this computer, on the plain socket, may change both.
    let (status, prefs) = call(
        &unix(&daemon),
        "PATCH",
        "/v1/settings/preferences",
        json!({"computer_use": true, "mcp_server": false}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{prefs}");
    assert_eq!(
        (
            prefs["computer_use"].as_bool(),
            prefs["mcp_server"].as_bool()
        ),
        (Some(true), Some(false))
    );
    config["preferences"]["computer_use"] = json!(false);
    config["preferences"]["mcp_server"] = json!(false);
    let (status, written) = call(
        &unix(&daemon),
        "PUT",
        "/v1/settings/config",
        json!({"config": config}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{written}");
    assert!(!daemon.shared.preferences.get().computer_use);
}

#[tokio::test]
async fn chats_take_the_default_confirmation_and_change_their_options_later() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let unix = unix(&daemon);

    let plain = new_chat(&daemon, json!({})).await;
    let state = daemon
        .shared
        .db
        .state(&SessionId::new(&plain))
        .await
        .unwrap();
    assert_eq!(
        state.session().confirmation,
        openside_proto::ChatConfirmation::Skip
    );
    assert!(!host(&daemon, &state.id).await.env.policy.confirm_first);

    call(
        &unix,
        "PATCH",
        "/v1/settings/preferences",
        json!({"confirmation": "ask_first"}),
        None,
    )
    .await;
    let asking = new_chat(&daemon, json!({})).await;
    let (_, listed) = call(
        &unix,
        "GET",
        &format!("/v1/sessions/{asking}"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(listed["confirmation"], "ask_first", "{listed}");
    assert!(
        host(&daemon, &SessionId::new(&asking))
            .await
            .env
            .policy
            .confirm_first
    );
    let explicit = new_chat(&daemon, json!({"confirmation": "skip"})).await;
    let state = daemon
        .shared
        .db
        .state(&SessionId::new(&explicit))
        .await
        .unwrap();
    assert_eq!(state.confirmation, openside_proto::ChatConfirmation::Skip);

    let (status, updated) = call(
        &unix,
        "PATCH",
        &format!("/v1/sessions/{plain}"),
        json!({
            "model": "anthropic:claude-sonnet-5-5",
            "effort": "high",
            "mode": "read-only",
            "confirmation": "ask_first",
        }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(
        (
            updated["model"].as_str(),
            updated["effort"].as_str(),
            updated["mode"].as_str(),
            updated["confirmation"].as_str()
        ),
        (
            Some("anthropic:claude-sonnet-5-5"),
            Some("high"),
            Some("read-only"),
            Some("ask_first")
        )
    );
    let events = daemon
        .shared
        .db
        .events(&SessionId::new(&plain), 0)
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.kind, EventKind::SessionOptionsUpdated { .. }))
            .count(),
        1
    );
    // Unchanged values append nothing.
    call(
        &unix,
        "PATCH",
        &format!("/v1/sessions/{plain}"),
        json!({"effort": "high"}),
        None,
    )
    .await;
    assert_eq!(
        daemon
            .shared
            .db
            .events(&SessionId::new(&plain), 0)
            .await
            .unwrap()
            .len(),
        events.len()
    );
    let (status, _) = call(
        &unix,
        "PATCH",
        &format!("/v1/sessions/{plain}"),
        json!({"model": "  "}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn paired_devices_cannot_raise_a_chats_mode_and_agent_clients_not_its_cost() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let tcp = api::router(daemon.shared.clone(), false);
    let chat = new_chat(&daemon, json!({})).await;
    let (_, person) = daemon
        .shared
        .db
        .pair_named("phone".into(), vec![CapabilityScope::Steer])
        .await
        .unwrap();
    let (_, agent) = daemon
        .shared
        .db
        .pair_as(
            "bot".into(),
            vec![CapabilityScope::Steer],
            openside_proto::DeviceKind::AgentClient,
        )
        .await
        .unwrap();
    let path = format!("/v1/sessions/{chat}");
    let (status, body) = call(&tcp, "PATCH", &path, json!({"mode": "full"}), Some(&person)).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("local_socket_required"))
    );
    let (status, _) = call(
        &tcp,
        "PATCH",
        &path,
        json!({"mode": "read-only"}),
        Some(&person),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(&tcp, "PATCH", &path, json!({"effort": "max"}), Some(&agent)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &tcp,
        "PATCH",
        &path,
        json!({"effort": "max"}),
        Some(&person),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn an_effort_change_reopens_the_runtime_with_the_new_effort() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let unix = unix(&daemon);
    let chat = new_chat(&daemon, json!({"effort": "low"})).await;
    let id = SessionId::new(&chat);
    let send = |n: u32| {
        let unix = unix.clone();
        let chat = chat.clone();
        async move {
            call(
                &unix,
                "POST",
                &format!("/v1/sessions/{chat}/messages"),
                json!({"message_id": format!("m{n}"), "text": "hello", "mode": "queue"}),
                None,
            )
            .await
            .0
        }
    };
    assert_eq!(send(1).await, StatusCode::ACCEPTED);
    until(|| {
        runtime
            .0
            .lock()
            .opened
            .iter()
            .any(|spec| spec.session == id)
    })
    .await;
    runtime.emit(&id, RuntimeEvent::Settled);
    wait_event(&daemon, &id, |kind| {
        matches!(kind, EventKind::SessionSettled {})
    })
    .await;
    call(
        &unix,
        "PATCH",
        &format!("/v1/sessions/{chat}"),
        json!({"effort": "high"}),
        None,
    )
    .await;
    assert_eq!(send(2).await, StatusCode::ACCEPTED);
    until(|| {
        runtime
            .0
            .lock()
            .opened
            .iter()
            .filter(|spec| spec.session == id)
            .count()
            == 2
    })
    .await;
    let efforts: Vec<Value> = runtime
        .0
        .lock()
        .opened
        .iter()
        .filter(|spec| spec.session == id)
        .map(|spec| serde_json::from_str::<Value>(&spec.agent_json).unwrap()["effort"].clone())
        .collect();
    assert_eq!(efforts, vec![json!("low"), json!("high")]);
}

#[tokio::test]
async fn replies_make_a_chat_unread_until_marked_read_or_answered() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let unix = unix(&daemon);
    let chat = new_chat(&daemon, json!({})).await;
    let id = SessionId::new(&chat);
    let path = format!("/v1/sessions/{chat}");
    let unread = || async {
        call(&unix, "GET", &path, Value::Null, None).await.1["unread"]
            .as_bool()
            .unwrap()
    };
    assert!(!unread().await);
    let reply = EventKind::AssistantFinal {
        text: "Done.".into(),
        attachments: Vec::new(),
    };
    daemon.shared.emit(&id, None, reply.clone()).await.unwrap();
    assert!(unread().await);
    let (status, body) = call(&unix, "PATCH", &path, json!({"read": true}), None).await;
    assert_eq!(
        (status, body["unread"].as_bool()),
        (StatusCode::OK, Some(false))
    );
    // Marking an already read chat read appends nothing.
    let before = daemon.shared.db.events(&id, 0).await.unwrap().len();
    call(&unix, "PATCH", &path, json!({"read": true}), None).await;
    assert_eq!(daemon.shared.db.events(&id, 0).await.unwrap().len(), before);
    call(&unix, "PATCH", &path, json!({"read": false}), None).await;
    assert!(unread().await);
    call(&unix, "PATCH", &path, json!({"read": true}), None).await;
    daemon.shared.emit(&id, None, reply).await.unwrap();
    assert!(unread().await);
    // Writing in the chat means its replies were seen.
    let (status, _) = call(
        &unix,
        "POST",
        &format!("{path}/messages"),
        json!({"message_id": "m1", "text": "thanks", "mode": "queue"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert!(!unread().await);
    // Read state changes do not reorder the list.
    let state = daemon.shared.db.state(&id).await.unwrap();
    call(&unix, "PATCH", &path, json!({"read": false}), None).await;
    assert_eq!(
        daemon.shared.db.state(&id).await.unwrap().last_activity,
        state.last_activity
    );
}

#[tokio::test]
async fn the_context_meter_follows_only_the_chats_own_agent() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let chat = new_chat(&daemon, json!({})).await;
    let id = SessionId::new(&chat);
    let usage = |input: u64, window: Option<u64>| EventKind::Usage {
        model: "m".into(),
        input_tokens: input,
        output_tokens: 1,
        effort: None,
        context_window: window,
    };
    assert_eq!(
        daemon.shared.db.state(&id).await.unwrap().session().context,
        None
    );
    daemon
        .shared
        .emit(&id, None, usage(12_000, Some(200_000)))
        .await
        .unwrap();
    // A subagent or title request reports no window and leaves the meter alone.
    daemon
        .shared
        .emit(&id, None, usage(90_000, None))
        .await
        .unwrap();
    let (_, session) = call(
        &unix(&daemon),
        "GET",
        &format!("/v1/sessions/{chat}"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(
        session["context"],
        json!({"used_tokens": 12_000, "window_tokens": 200_000})
    );
    // An own-agent request on a model with an unknown window (0) still moves the used tokens.
    daemon
        .shared
        .emit(&id, None, usage(30_000, Some(0)))
        .await
        .unwrap();
    assert_eq!(
        daemon.shared.db.state(&id).await.unwrap().context,
        Some(openside_proto::ContextUsage {
            used_tokens: 30_000,
            window_tokens: None
        })
    );
}

#[tokio::test]
async fn the_session_list_can_be_limited_to_the_most_recent() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    for _ in 0..3 {
        new_chat(&daemon, json!({})).await;
    }
    let (_, all) = call(
        &unix(&daemon),
        "GET",
        "/v1/sessions?agent=none",
        Value::Null,
        None,
    )
    .await;
    let (_, two) = call(
        &unix(&daemon),
        "GET",
        "/v1/sessions?agent=none&limit=2",
        Value::Null,
        None,
    )
    .await;
    assert_eq!(all["sessions"].as_array().unwrap().len(), 3);
    assert_eq!(two["sessions"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn the_session_list_puts_pinned_first_then_the_most_recent_activity() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let unix = unix(&daemon);
    let mut chats = Vec::new();
    for _ in 0..3 {
        chats.push(new_chat(&daemon, json!({})).await);
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let (oldest, middle, newest) = (&chats[0], &chats[1], &chats[2]);
    call(
        &unix,
        "PATCH",
        &format!("/v1/sessions/{oldest}"),
        json!({"pinned": true}),
        None,
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    daemon
        .shared
        .emit(
            &SessionId::new(middle),
            None,
            EventKind::Usage {
                model: "m".into(),
                input_tokens: 1,
                output_tokens: 1,
                effort: None,
                context_window: None,
            },
        )
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    // A rename is list metadata, not activity: it must not lift the chat above newer work.
    call(
        &unix,
        "PATCH",
        &format!("/v1/sessions/{newest}"),
        json!({"title": "Renamed"}),
        None,
    )
    .await;
    let ids = |list: &Value| -> Vec<String> {
        list["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap().to_owned())
            .collect()
    };
    let (_, all) = call(&unix, "GET", "/v1/sessions?agent=none", Value::Null, None).await;
    assert_eq!(
        ids(&all),
        vec![oldest.clone(), middle.clone(), newest.clone()]
    );
    let (_, two) = call(
        &unix,
        "GET",
        "/v1/sessions?agent=none&limit=2",
        Value::Null,
        None,
    )
    .await;
    assert_eq!(ids(&two), vec![oldest.clone(), middle.clone()]);
    assert!(
        all["sessions"][1]["last_activity_at"].as_str().unwrap()
            > all["sessions"][2]["last_activity_at"].as_str().unwrap()
    );
}

#[tokio::test]
async fn about_reports_installed_versions() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let (status, about) = call(&unix(&daemon), "GET", "/v1/about", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(about["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        about["runtime_versions"],
        openside_agent_sidecar::supported_agent_versions()
    );
    assert_eq!(
        about["releases_url"],
        format!("{}/releases", env!("CARGO_PKG_REPOSITORY"))
    );
}

#[tokio::test]
async fn notification_kinds_gate_ticket_and_completion_pushes() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let push = FakePushService::spawn().await.unwrap();
    let mut config = resolved_config(root.path());
    config.loaded.config.network.allow = vec![format!("http://{}", push.addr())];
    config.loaded.config.preferences.notifications = Vec::new();
    let daemon = Daemon::open_runtime(config, runtime.clone()).await.unwrap();
    let tcp = api::router(daemon.shared.clone(), false);
    let (_, phone) = daemon
        .shared
        .db
        .pair_named("phone".into(), vec![CapabilityScope::Read])
        .await
        .unwrap();
    let subscription = push.subscribe();
    let (status, _) = call(
        &tcp,
        "POST",
        "/v1/push/subscriptions",
        subscription.to_json(),
        Some(&phone),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let session = create_session(&daemon, "s1", PolicyMode::Guard, None).await;
    opened(
        &daemon,
        &session,
        TicketKind::Confirm,
        "t1",
        Utc::now() + chrono::Duration::minutes(10),
    )
    .await;
    let quiet = create_session(&daemon, "s2", PolicyMode::Guard, None).await;
    daemon
        .shared
        .emit(&quiet, None, EventKind::SessionSettled {})
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(push.deliveries().is_empty(), "every kind is off");

    daemon
        .shared
        .preferences
        .replace(openside_config::PreferencesConfig {
            notifications: vec![openside_config::NotificationKindSetting::TaskCompleted],
            ..Default::default()
        });
    opened(
        &daemon,
        &session,
        TicketKind::Confirm,
        "t2",
        Utc::now() + chrono::Duration::minutes(10),
    )
    .await;
    daemon
        .shared
        .emit(&quiet, None, EventKind::SessionSettled {})
        .await
        .unwrap();
    let deliveries = push.wait_for(1, Duration::from_secs(5)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(push.deliveries().len(), 1, "only the completion is pushed");
    assert_eq!(deliveries[0].json()["kind"], "settled");
}

#[test]
fn routine_prompts_are_internal_messages_without_a_sender() {
    let dir = tempfile::tempdir().unwrap();
    let state = |author: MessageAuthorKind, sender: Option<&str>| {
        let path = dir
            .path()
            .join(format!("{author:?}-{}", sender.unwrap_or("none")));
        let mut queued = openside_sessions::Store::open(path).unwrap();
        queued
            .create_session(SessionId::new("s"), "t", "m", PolicyMode::Guard, None)
            .unwrap();
        queued
            .append(
                &SessionId::new("s"),
                None,
                EventKind::MessageQueued {
                    message_id: MessageId::new("m"),
                    text: "run".into(),
                    mode: QueueMode::Queue,
                    attachments: Vec::new(),
                    author_kind: author,
                    from_agent: sender.map(str::to_owned),
                },
            )
            .unwrap();
        queued.session(&SessionId::new("s")).unwrap().clone()
    };
    let id = MessageId::new("m");
    assert!(crate::push::routine_message(
        &state(MessageAuthorKind::Internal, None),
        &id
    ));
    assert!(!crate::push::routine_message(
        &state(MessageAuthorKind::Internal, Some("a1")),
        &id
    ));
    assert!(!crate::push::routine_message(
        &state(MessageAuthorKind::Person, None),
        &id
    ));
}

/// Writes one finished exchange into `session`'s log without a runtime.
async fn exchange(daemon: &Daemon, session: &SessionId, id: &str, text: &str, reply: &str) {
    for kind in [
        EventKind::MessageQueued {
            message_id: MessageId::new(id),
            text: text.into(),
            mode: QueueMode::Queue,
            attachments: Vec::new(),
            author_kind: MessageAuthorKind::Person,
            from_agent: None,
        },
        EventKind::MessageInjected {
            message_id: MessageId::new(id),
        },
        EventKind::AssistantFinal {
            text: reply.into(),
            attachments: Vec::new(),
        },
        EventKind::MessageCompleted {
            message_id: MessageId::new(id),
            outcome: openside_proto::MessageOutcome::Completed,
            reason: None,
        },
    ] {
        daemon.shared.emit(session, None, kind).await.unwrap();
    }
}

#[tokio::test]
async fn a_branch_copies_the_conversation_up_to_a_reply_and_gives_it_to_the_agent() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let unix = unix(&daemon);
    let chat = new_chat(
        &daemon,
        json!({"title": "Plans", "effort": "low", "confirmation": "ask_first"}),
    )
    .await;
    let source = SessionId::new(&chat);
    exchange(
        &daemon,
        &source,
        "q1",
        "Compare plan A and B",
        "Plan B is cheaper.",
    )
    .await;
    exchange(
        &daemon,
        &source,
        "q2",
        "What about plan C?",
        "Plan C has no support.",
    )
    .await;
    let fork = |from: &str| {
        let unix = unix.clone();
        let path = format!("/v1/sessions/{chat}/fork");
        let body = json!({"from_message": from});
        async move { call(&unix, "POST", &path, body, None).await }
    };

    let (status, branch) = fork("q1").await;
    assert_eq!(status, StatusCode::CREATED, "{branch}");
    assert_eq!(branch["title"], "Plans (branch)");
    assert_eq!(
        (branch["effort"].as_str(), branch["confirmation"].as_str()),
        (Some("low"), Some("ask_first"))
    );
    assert_eq!(branch["status"], "idle");
    assert!(branch["queue"].as_array().unwrap().is_empty());
    let id = SessionId::new(branch["id"].as_str().unwrap());
    let events = daemon.shared.db.events(&id, 0).await.unwrap();
    let texts: Vec<&str> = events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::MessageQueued { text, .. } | EventKind::AssistantFinal { text, .. } => {
                Some(text.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["Compare plan A and B", "Plan B is cheaper."]);
    let marker = events
        .iter()
        .find(|event| matches!(event.kind, EventKind::SessionBranched { .. }))
        .expect("session.branched");
    assert!(matches!(
        &marker.kind,
        EventKind::SessionBranched { from_session, from_message }
            if from_session == &source && from_message.as_str() == "q1"
    ));
    // The marker closes the copied conversation.
    assert!(events
        .iter()
        .filter(|event| event.seq > marker.seq)
        .all(|event| !matches!(
            event.kind,
            EventKind::MessageQueued { .. } | EventKind::AssistantFinal { .. }
        )));
    assert_eq!(
        daemon.shared.db.state(&id).await.unwrap().branched_at,
        Some(marker.seq)
    );
    // The source is untouched.
    assert_eq!(
        daemon
            .shared
            .db
            .state(&source)
            .await
            .unwrap()
            .completions
            .len(),
        2
    );

    // The copied messages are never delivered; the agent gets them as context instead.
    let (status, _) = call(
        &unix,
        "POST",
        &format!("/v1/sessions/{}/messages", id.as_str()),
        json!({"message_id": "next", "text": "And plan D?", "mode": "queue"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    until(|| runtime.0.lock().sent.iter().any(|sent| sent.0 == id)).await;
    {
        let state = runtime.0.lock();
        let sent: Vec<&str> = state
            .sent
            .iter()
            .filter(|sent| sent.0 == id)
            .map(|sent| sent.1.as_str())
            .collect();
        assert_eq!(sent, vec!["next"]);
        let spec = state.opened.iter().find(|spec| spec.session == id).unwrap();
        let context = spec
            .system_prompt_parts
            .iter()
            .find(|part| part.contains("<earlier_conversation>"))
            .expect("branch context");
        assert!(context.contains(
            "{\"from\":\"person\",\"text\":\"Compare plan A and B\"}\n\n{\"from\":\"you\",\"text\":\"Plan B is cheaper.\"}"
        ));
        assert!(!context.contains("plan C"));
    }

    let (status, body) = fork("missing").await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::NOT_FOUND, Some("message_not_found"))
    );
    daemon
        .shared
        .emit(
            &source,
            None,
            EventKind::MessageQueued {
                message_id: MessageId::new("pending"),
                text: "later".into(),
                mode: QueueMode::Queue,
                attachments: Vec::new(),
                author_kind: MessageAuthorKind::Person,
                from_agent: None,
            },
        )
        .await
        .unwrap();
    let (status, body) = fork("pending").await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("message_not_finished"))
    );
    let (status, _) = call(
        &unix,
        "POST",
        "/v1/sessions/nope/fork",
        json!({"from_message": "q1"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // A branch of the whole conversation, from the later reply.
    let (status, later) = fork("q2").await;
    assert_eq!(status, StatusCode::CREATED);
    let later = daemon
        .shared
        .db
        .state(&SessionId::new(later["id"].as_str().unwrap()))
        .await
        .unwrap();
    assert_eq!(later.completions.len(), 2);
    daemon.shutdown().await;
}

/// One finished exchange as raw log events, with optional attachment and document names.
fn raw_exchange(
    id: &str,
    text: String,
    attached: Option<&str>,
    reply: String,
    document: Option<&str>,
) -> [EventKind; 4] {
    let attachments = attached
        .map(|name| {
            serde_json::from_value(json!({
                "path": format!("uploads/{id}"), "kind": "text", "mime": "text/plain",
                "size": 1, "sha256": "0".repeat(64), "name": name,
            }))
            .unwrap()
        })
        .into_iter()
        .collect();
    let documents = document
        .map(|title| {
            serde_json::from_value(json!({
                "path": format!("documents/{id}.md"), "title": title, "display": "panel",
                "bytes": 1, "content_type": "text/markdown",
            }))
            .unwrap()
        })
        .into_iter()
        .collect();
    [
        EventKind::MessageQueued {
            message_id: MessageId::new(id),
            text,
            mode: QueueMode::Queue,
            attachments,
            author_kind: MessageAuthorKind::Person,
            from_agent: None,
        },
        EventKind::MessageInjected {
            message_id: MessageId::new(id),
        },
        EventKind::AssistantFinal {
            text: reply,
            attachments: documents,
        },
        EventKind::MessageCompleted {
            message_id: MessageId::new(id),
            outcome: openside_proto::MessageOutcome::Completed,
            reason: None,
        },
    ]
}

/// A chat whose log is `history`, written in one commit so large histories stay cheap.
async fn seeded(daemon: &Daemon, name: &str, history: Vec<EventKind>) -> SessionId {
    let id = SessionId::new(name);
    let mut kinds = vec![EventKind::SessionCreated {
        agent_id: None,
        title: "Seeded".into(),
        model: std::any::type_name::<TestRuntime>().into(),
        mode: PolicyMode::Full,
        workspace: None,
        incognito: false,
        browser_space: Default::default(),
        effort: None,
        speed: None,
        profile: None,
    }];
    kinds.extend(history);
    let stored = id.clone();
    daemon
        .shared
        .db
        .store(move |db| Ok(db.create_with_history(&stored, kinds)?))
        .await
        .unwrap();
    id
}

async fn fork_at(daemon: &Daemon, source: &SessionId, from: &str) -> (StatusCode, Value) {
    let path = format!("/v1/sessions/{}/fork", source.as_str());
    call(
        &unix(daemon),
        "POST",
        &path,
        json!({"from_message": from}),
        None,
    )
    .await
}

#[tokio::test]
async fn a_branch_masks_secrets_registered_after_the_source_was_written() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    const CANARY: &str = "fork-canary-7f3a91";
    let source = seeded(
        &daemon,
        "fork-secret",
        raw_exchange(
            "q1",
            format!("my key is {CANARY}"),
            Some(&format!("{CANARY}.txt")),
            format!("Stored {CANARY} for you."),
            Some(&format!("Notes on {CANARY}")),
        )
        .into(),
    )
    .await;
    daemon
        .shared
        .registry
        .redactors
        .write()
        .push(openside_vault::Redactor::new([
            &openside_vault::Secret::new(CANARY.into()),
        ]));

    let (status, branch) = fork_at(&daemon, &source, "q1").await;
    assert_eq!(status, StatusCode::CREATED, "{branch}");
    let id = SessionId::new(branch["id"].as_str().unwrap());
    let events = daemon.shared.db.events(&id, 0).await.unwrap();
    let log = serde_json::to_string(&events).unwrap();
    assert!(!log.contains(CANARY), "{log}");
    let texts: Vec<&str> = events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::MessageQueued { text, .. } | EventKind::AssistantFinal { text, .. } => {
                Some(text.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(texts.len(), 2);
    assert!(
        texts.iter().all(|text| text.contains("[REDACTED]")),
        "{texts:?}"
    );
    assert!(
        texts[0].contains("[Attached: [REDACTED].txt]"),
        "{}",
        texts[0]
    );
    assert!(
        texts[1].contains("[Document: Notes on [REDACTED]]"),
        "{}",
        texts[1]
    );
    let state = daemon.shared.db.state(&id).await.unwrap();
    let context = crate::fork::context(&daemon.shared, &state)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !context.contains(CANARY) && context.contains("[REDACTED]"),
        "{context}"
    );
}

#[tokio::test]
async fn a_branch_refuses_more_retained_text_than_its_budget_names_included() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let budget = crate::fork::MAX_RETAINED_BYTES;
    // Text and reply fill the budget to 100 bytes short of it.
    let text = || "a".repeat(budget - 102);
    let fits = seeded(
        &daemon,
        "fork-budget-fits",
        raw_exchange("q1", text(), None, "ok".into(), None).into(),
    )
    .await;
    let (status, body) = fork_at(&daemon, &fits, "q1").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    for (name, attached, document) in [
        ("fork-budget-attachment", Some("n".repeat(120)), None),
        ("fork-budget-document", None, Some("d".repeat(120))),
    ] {
        let over = seeded(
            &daemon,
            name,
            raw_exchange(
                "q1",
                text(),
                attached.as_deref(),
                "ok".into(),
                document.as_deref(),
            )
            .into(),
        )
        .await;
        let (status, body) = fork_at(&daemon, &over, "q1").await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::CONFLICT, Some("history_too_long")),
            "{name}"
        );
    }
}

#[tokio::test]
async fn a_long_chat_branches_with_its_newest_turns_as_context_and_a_huge_one_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let history = |count: usize| -> Vec<EventKind> {
        (0..count)
            .flat_map(|n| {
                raw_exchange(
                    &format!("m{n}"),
                    format!("message {n:05}"),
                    None,
                    format!("reply {n:05}"),
                    None,
                )
            })
            .collect()
    };
    let long = seeded(&daemon, "fork-long", history(3_000)).await;
    let (status, branch) = fork_at(&daemon, &long, "m2999").await;
    assert_eq!(status, StatusCode::CREATED, "{branch}");
    let state = daemon
        .shared
        .db
        .state(&SessionId::new(branch["id"].as_str().unwrap()))
        .await
        .unwrap();
    assert_eq!(state.completions.len(), 3_000);
    let context = crate::fork::context(&daemon.shared, &state)
        .await
        .unwrap()
        .unwrap();
    assert!(context.contains("[Earlier messages omitted]"));
    assert!(context.contains("reply 02999") && !context.contains("message 00000"));

    // Past the event cap the chat is refused instead of scanned to its end.
    let huge = seeded(&daemon, "fork-huge", history(12_600)).await;
    let (status, body) = fork_at(&daemon, &huge, "m12599").await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("history_too_long"))
    );
    let (status, _) = fork_at(&daemon, &huge, "m10").await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "an early reply of a huge chat still branches"
    );
}

/// Turning Enable MCP server off refuses every request `openside mcp` sends from then on, on
/// the socket and over TCP with any token, while other callers and the switch itself stay
/// readable.
#[tokio::test]
async fn the_daemon_refuses_openside_mcp_while_the_mcp_server_is_off() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let tcp = api::router(daemon.shared.clone(), false);
    let (_, agent) = daemon
        .shared
        .db
        .pair_as(
            "mcp".into(),
            vec![CapabilityScope::Read, CapabilityScope::Steer],
            openside_proto::DeviceKind::AgentClient,
        )
        .await
        .unwrap();
    let (_, person) = daemon
        .shared
        .db
        .pair_named(
            "phone".into(),
            vec![CapabilityScope::Read, CapabilityScope::Steer],
        )
        .await
        .unwrap();
    let marked = |router: Router, method: &str, path: &str, token: Option<String>| {
        let mut request = request(method, path, Value::Null, token.as_deref());
        request.headers_mut().insert(
            "x-openside-client",
            axum::http::HeaderValue::from_static("mcp"),
        );
        async move {
            let response = router.oneshot(request).await.unwrap();
            let status = response.status();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            (
                status,
                serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
            )
        }
    };
    let refused = (
        StatusCode::FORBIDDEN,
        Some("mcp_server_disabled".to_owned()),
    );
    let code =
        |(status, body): (StatusCode, Value)| (status, body["code"].as_str().map(str::to_owned));

    // A running `openside mcp` works while the switch is on.
    let (status, _) = marked(unix(&daemon), "GET", "/v1/sessions", None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = marked(tcp.clone(), "GET", "/v1/sessions", Some(agent.clone())).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = call(
        &unix(&daemon),
        "PATCH",
        "/v1/settings/preferences",
        json!({"mcp_server": false}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        code(marked(unix(&daemon), "GET", "/v1/sessions", None).await),
        refused
    );
    assert_eq!(
        code(marked(unix(&daemon), "POST", "/v1/sessions", None).await),
        refused
    );
    for token in [&agent, &person] {
        assert_eq!(
            code(marked(tcp.clone(), "GET", "/v1/sessions", Some(token.clone())).await),
            refused
        );
    }
    // The switch stays readable so the CLI can say why, and unmarked callers are unaffected.
    let (status, prefs) = marked(unix(&daemon), "GET", "/v1/settings/preferences", None).await;
    assert_eq!(
        (status, prefs["mcp_server"].as_bool()),
        (StatusCode::OK, Some(false))
    );
    let (status, _) = call(&unix(&daemon), "GET", "/v1/sessions", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(&tcp, "GET", "/v1/sessions", Value::Null, Some(&agent)).await;
    assert_eq!(status, StatusCode::OK);
    // An unauthenticated marked request learns nothing about the switch.
    let (status, _) = marked(tcp.clone(), "GET", "/v1/sessions", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = call(
        &unix(&daemon),
        "PATCH",
        "/v1/settings/preferences",
        json!({"mcp_server": true}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = marked(unix(&daemon), "GET", "/v1/sessions", None).await;
    assert_eq!(status, StatusCode::OK);
}
