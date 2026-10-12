use super::*;
use openside_core::RunOutcome;

async fn daemon_with_model(root: &Path, runtime: &TestRuntime) -> Daemon {
    let mut config = resolved_config(root);
    config.loaded.config.models.default = Some(std::any::type_name::<TestRuntime>().into());
    Daemon::open_runtime(config, runtime.clone()).await.unwrap()
}

async fn send(router: &axum::Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(request(method, path, body, None))
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, body)
}

fn interval_routine(action: Value) -> Value {
    json!({"name":"hourly","schedule":{"type":"interval","secs":3600},"action":action})
}

fn new_session_action(prompt: &str) -> Value {
    json!({"type":"new_session","prompt":prompt})
}

async fn settle(runtime: &TestRuntime, run: &Value) {
    let session = SessionId::new(run["session_id"].as_str().unwrap());
    runtime.emit(
        &session,
        RuntimeEvent::MessageResult {
            id: MessageId::new(run["message_id"].as_str().unwrap()),
            outcome: RunOutcome::Completed,
            reason: None,
        },
    );
    runtime.emit(&session, RuntimeEvent::Settled);
}

async fn run_status(router: &axum::Router, routine: &str) -> Vec<String> {
    let (_, runs) = send(
        router,
        "GET",
        &format!("/v1/routines/{routine}/runs"),
        json!({}),
    )
    .await;
    runs["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["status"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn routine_crud_validates_input_and_requires_admin_for_writes() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon_with_model(root.path(), &TestRuntime::default()).await;
    let router = api::router(daemon.shared.clone(), true);

    let (status, created) = send(
        &router,
        "POST",
        "/v1/routines",
        interval_routine(new_session_action("report")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["enabled"], true);
    assert!(created["next_run_at"].is_string());
    let id = created["id"].as_str().unwrap().to_owned();

    for (body, code) in [
        (
            json!({"name":"x","schedule":{"type":"cron","expr":"nope"},"action":new_session_action("p")}),
            "invalid_schedule",
        ),
        (
            json!({"name":"x","schedule":{"type":"cron","expr":"0 9 * * *","timezone":"Mars/Base"},"action":new_session_action("p")}),
            "invalid_timezone",
        ),
        (
            json!({"name":"x","schedule":{"type":"interval","secs":0},"action":new_session_action("p")}),
            "invalid_schedule",
        ),
        (
            json!({"name":" ","schedule":{"type":"interval","secs":5},"action":new_session_action("p")}),
            "invalid_value",
        ),
        (
            json!({"name":"x","schedule":{"type":"interval","secs":5},"action":new_session_action(" ")}),
            "invalid_action",
        ),
        (
            json!({"name":"x","schedule":{"type":"interval","secs":5},"action":{"type":"heartbeat","session_id":"ghost","prompt":"p"}}),
            "target_session_missing",
        ),
    ] {
        let (status, error) = send(&router, "POST", "/v1/routines", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
        assert_eq!(error["code"], code);
    }

    let (_, list) = send(&router, "GET", "/v1/routines", json!({})).await;
    assert_eq!(
        list["routines"].as_array().unwrap().len(),
        1,
        "rejected requests create nothing"
    );

    let (status, paused) = send(
        &router,
        "PATCH",
        &format!("/v1/routines/{id}"),
        json!({"enabled":false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(paused["enabled"], false);
    assert!(paused["next_run_at"].is_null());
    let (_, resumed) = send(
        &router,
        "PATCH",
        &format!("/v1/routines/{id}"),
        json!({"enabled":true,"name":"renamed"}),
    )
    .await;
    assert_eq!(resumed["name"], "renamed");
    assert!(resumed["next_run_at"].is_string());

    let (status, removed) = send(&router, "DELETE", &format!("/v1/routines/{id}"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(removed["id"], id);
    for method in ["GET", "PATCH", "DELETE"] {
        let (status, _) = send(&router, method, &format!("/v1/routines/{id}"), json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method}");
    }
    let (status, _) = send(
        &router,
        "POST",
        &format!("/v1/routines/{id}/run"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (_, read) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Read])
        .await
        .unwrap();
    let (_, admin) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Admin])
        .await
        .unwrap();
    let remote = api::router(daemon.shared.clone(), false);
    let call = |method: &'static str, path: &'static str, token: String| {
        let remote = remote.clone();
        async move {
            remote
                .oneshot(request(
                    method,
                    path,
                    interval_routine(new_session_action("p")),
                    Some(&token),
                ))
                .await
                .unwrap()
                .status()
        }
    };
    assert_eq!(
        call("GET", "/v1/routines", read.clone()).await,
        StatusCode::OK
    );
    assert_eq!(
        call("GET", "/v1/routines/suggestions", read.clone()).await,
        StatusCode::OK
    );
    assert_eq!(
        call("POST", "/v1/routines", read.clone()).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call("POST", "/v1/routines", admin).await,
        StatusCode::CREATED
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn run_now_creates_a_session_that_settles_and_the_run_completes() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon_with_model(root.path(), &runtime).await;
    let router = api::router(daemon.shared.clone(), true);
    let (_, routine) = send(
        &router,
        "POST",
        "/v1/routines",
        interval_routine(new_session_action("summarize inbox")),
    )
    .await;
    let id = routine["id"].as_str().unwrap();

    let (status, run) = send(
        &router,
        "POST",
        &format!("/v1/routines/{id}/run"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{run}");
    assert_eq!(run["status"], "started");
    let session = run["session_id"].as_str().unwrap().to_owned();
    assert!(run["message_id"].as_str().unwrap().starts_with("routine-"));

    // The new session exists, carries the routine's name, and its runtime got the prompt.
    let (_, sessions) = send(&router, "GET", "/v1/sessions", json!({})).await;
    assert!(sessions["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["id"] == session && s["title"] == "hourly"));
    until(|| !runtime.0.lock().sent.is_empty()).await;
    let sent = runtime.0.lock().sent[0].clone();
    assert_eq!(
        (sent.0.as_str(), sent.2.as_str(), sent.3),
        (session.as_str(), "summarize inbox", QueueMode::Queue)
    );
    assert_eq!(run_status(&router, id).await, ["started"]);

    settle(&runtime, &run).await;
    wait_event(&daemon, &SessionId::new(&session), |e| {
        matches!(e, EventKind::SessionSettled {})
    })
    .await;
    assert_eq!(run_status(&router, id).await, ["completed"]);

    // Not part of the schedule: the next occurrence is untouched.
    let (_, after) = send(&router, "GET", &format!("/v1/routines/{id}"), json!({})).await;
    assert_eq!(after["next_run_at"], routine["next_run_at"]);
    daemon.shutdown().await;
}

#[tokio::test]
async fn heartbeat_appends_a_follow_up_to_the_existing_session_and_skips_overlaps() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon_with_model(root.path(), &runtime).await;
    let session = create_session(&daemon, "target", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), true);
    let (status, routine) = send(
        &router,
        "POST",
        "/v1/routines",
        interval_routine(
            json!({"type":"heartbeat","session_id":session.as_str(),"prompt":"status check"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{routine}");
    let id = routine["id"].as_str().unwrap();

    let (_, first) = send(
        &router,
        "POST",
        &format!("/v1/routines/{id}/run"),
        json!({}),
    )
    .await;
    assert_eq!(first["status"], "started");
    assert_eq!(first["session_id"], session.as_str());
    let events = daemon.shared.db.events(&session, 0).await.unwrap();
    let queued: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::MessageQueued {
                message_id,
                text,
                mode,
                ..
            } => Some((message_id.as_str().to_owned(), text.clone(), *mode)),
            _ => None,
        })
        .collect();
    assert_eq!(queued.len(), 1);
    assert_eq!(
        (queued[0].1.as_str(), queued[0].2),
        ("status check", QueueMode::Queue),
        "follow_up lane"
    );
    assert!(queued[0].0.starts_with("routine-"));
    until(|| runtime.0.lock().sent.len() == 1).await;

    // Overlap: the first heartbeat is still being worked on.
    let (_, second) = send(
        &router,
        "POST",
        &format!("/v1/routines/{id}/run"),
        json!({}),
    )
    .await;
    assert_eq!(second["status"], "skipped_overlap");
    assert_eq!(runtime.0.lock().sent.len(), 1, "skipped runs send nothing");

    settle(&runtime, &first).await;
    wait_event(&daemon, &session, |e| {
        matches!(e, EventKind::SessionSettled {})
    })
    .await;
    let (_, third) = send(
        &router,
        "POST",
        &format!("/v1/routines/{id}/run"),
        json!({}),
    )
    .await;
    assert_eq!(third["status"], "started");
    assert_eq!(
        run_status(&router, id).await,
        ["started", "skipped_overlap", "completed"]
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn deleting_a_session_tombstones_it_rejects_tickets_and_pauses_heartbeats() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon_with_model(root.path(), &runtime).await;
    let session = create_session(&daemon, "doomed", PolicyMode::Guard, None).await;
    let keeper = create_session(&daemon, "keeper", PolicyMode::Guard, None).await;
    opened(
        &daemon,
        &session,
        TicketKind::Confirm,
        "open-ticket",
        Utc::now() + chrono::Duration::hours(1),
    )
    .await;
    let router = api::router(daemon.shared.clone(), true);
    let heartbeat = |target: &SessionId| {
        interval_routine(json!({"type":"heartbeat","session_id":target.as_str(),"prompt":"ping"}))
    };
    let (_, doomed_routine) = send(&router, "POST", "/v1/routines", heartbeat(&session)).await;
    let (_, kept_routine) = send(&router, "POST", "/v1/routines", heartbeat(&keeper)).await;

    let (status, _) = send(&router, "DELETE", "/v1/sessions/doomed", json!({})).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Hidden everywhere, and gone for good.
    let (_, list) = send(&router, "GET", "/v1/sessions", json!({})).await;
    let ids: Vec<_> = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids, ["keeper"]);
    for (method, path) in [
        ("GET", "/v1/sessions/doomed"),
        ("GET", "/v1/sessions/doomed/events"),
        ("DELETE", "/v1/sessions/doomed"),
    ] {
        let (status, _) = send(&router, method, path, json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
    }
    let (status, _) = send(
        &router,
        "POST",
        "/v1/sessions/doomed/messages",
        json!({"text":"hi","mode":"queue"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The durable log keeps a tombstone; the open ticket was rejected before it.
    let events = daemon.shared.db.events(&session, 0).await.unwrap();
    let kinds: Vec<_> = events.iter().map(|e| &e.kind).collect();
    assert!(matches!(
        kinds.last().unwrap(),
        EventKind::SessionDeleted {}
    ));
    assert!(kinds.iter().any(|k| matches!(k, EventKind::TicketRejected { ticket_id, .. } if ticket_id.as_str() == "open-ticket")));
    assert!(
        daemon
            .shared
            .emit(&session, None, EventKind::SessionSettled {})
            .await
            .is_err(),
        "nothing can be appended after deletion"
    );
    let (_, tickets) = send(&router, "GET", "/v1/tickets?status=pending", json!({})).await;
    assert!(tickets["tickets"].as_array().unwrap().is_empty());

    // Heartbeat routines aimed at it pause; others are untouched.
    let doomed_id = doomed_routine["id"].as_str().unwrap();
    let (_, paused) = send(
        &router,
        "GET",
        &format!("/v1/routines/{doomed_id}"),
        json!({}),
    )
    .await;
    assert_eq!(
        (paused["enabled"].clone(), paused["paused_reason"].clone()),
        (json!(false), json!("target_missing"))
    );
    assert_eq!(
        run_status(&router, doomed_id).await,
        ["paused_target_missing"]
    );
    let (_, kept) = send(
        &router,
        "GET",
        &format!("/v1/routines/{}", kept_routine["id"].as_str().unwrap()),
        json!({}),
    )
    .await;
    assert_eq!(kept["enabled"], true);

    // Survives restart: the tombstone is replayed, not forgotten.
    daemon.shutdown().await;
    drop(daemon);
    let reopened = daemon_with_model(root.path(), &runtime).await;
    let router = api::router(reopened.shared.clone(), true);
    let (status, _) = send(&router, "GET", "/v1/sessions/doomed", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    reopened.shutdown().await;
}

#[tokio::test]
async fn suggestions_come_from_repeated_user_messages_and_create_nothing() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    // Synthetic history written straight into the event store, before the daemon opens it:
    // the store stamps events with the current time, and repetition needs spread-out days.
    drop(openside_sessions::Store::open(data.join("events.db")).unwrap());
    let db = rusqlite::Connection::open(data.join("events.db")).unwrap();
    let insert = |seq: u32, ts: &str, payload: Value| {
        db.execute(
            "INSERT INTO events(session_id, seq, ts, run_id, payload, submission_id) VALUES('history', ?1, ?2, NULL, ?3, ?4)",
            rusqlite::params![seq, ts, payload.to_string(), payload["message_id"].as_str()],
        )
        .unwrap();
    };
    insert(
        1,
        "2026-09-01T00:00:00+00:00",
        json!({"kind":"session.created","title":"History","model":"m","mode":"guard","workspace":null}),
    );
    for day in 1..=5 {
        insert(
            day + 1,
            &format!("2026-09-{day:02}T09:00:00+00:00"),
            json!({"kind":"message.queued","message_id":format!("u{day}"),"text":format!("Check the build dashboard for failures ({day})"),"mode":"queue"}),
        );
    }
    // A stopped session is skipped by startup delivery, so these messages stay untouched.
    insert(
        7,
        "2026-09-06T00:00:00+00:00",
        json!({"kind":"session.stopped"}),
    );
    drop(db);

    let daemon = daemon_with_model(root.path(), &TestRuntime::default()).await;
    let router = api::router(daemon.shared.clone(), true);
    let (status, found) = send(&router, "GET", "/v1/routines/suggestions", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{found}");
    let drafts = found["suggestions"].as_array().unwrap();
    assert_eq!(drafts.len(), 1, "{found}");
    assert_eq!(drafts[0]["occurrences"], 5);
    assert_eq!(
        drafts[0]["prompt"],
        "Check the build dashboard for failures (5)"
    );
    assert_eq!(
        drafts[0]["schedule"],
        json!({"type":"cron","expr":"0 9 * * *","timezone":"UTC"})
    );
    assert_eq!(drafts[0]["session_ids"], json!(["history"]));
    let (_, list) = send(&router, "GET", "/v1/routines", json!({})).await;
    assert!(
        list["routines"].as_array().unwrap().is_empty(),
        "suggesting never creates"
    );

    // Turning the draft into a routine is an ordinary create, and then it stops being suggested.
    let body = json!({"name":drafts[0]["name"],"schedule":drafts[0]["schedule"],"action":new_session_action(drafts[0]["prompt"].as_str().unwrap())});
    let (status, _) = send(&router, "POST", "/v1/routines", body).await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, again) = send(&router, "GET", "/v1/routines/suggestions", json!({})).await;
    assert!(again["suggestions"].as_array().unwrap().is_empty());
    daemon.shutdown().await;
}

#[tokio::test]
async fn routines_survive_a_daemon_restart_with_their_schedule_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon_with_model(root.path(), &runtime).await;
    let router = api::router(daemon.shared.clone(), true);
    let (_, routine) = send(
        &router,
        "POST",
        "/v1/routines",
        interval_routine(new_session_action("poll")),
    )
    .await;
    let id = routine["id"].as_str().unwrap().to_owned();
    daemon.shutdown().await;
    drop(daemon);

    let reopened = daemon_with_model(root.path(), &runtime).await;
    let router = api::router(reopened.shared.clone(), true);
    let (status, found) = send(&router, "GET", &format!("/v1/routines/{id}"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        found["next_run_at"], routine["next_run_at"],
        "schedule is not reset by a restart"
    );
    assert!(
        run_status(&router, &id).await.is_empty(),
        "nothing was due while down"
    );
    reopened.shutdown().await;
}

async fn heartbeat_to(router: &axum::Router, session: &SessionId) -> String {
    let (status, routine) = send(
        router,
        "POST",
        "/v1/routines",
        interval_routine(
            json!({"type":"heartbeat","session_id":session.as_str(),"prompt":"status"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{routine}");
    routine["id"].as_str().unwrap().to_owned()
}

async fn run_now(router: &axum::Router, routine: &str) -> Value {
    send(
        router,
        "POST",
        &format!("/v1/routines/{routine}/run"),
        json!({}),
    )
    .await
    .1
}

/// A heartbeat run whose message ended in `deferred`, so its session settled without
/// finishing the work.
async fn deferred_heartbeat(
    daemon: &Daemon,
    router: &axum::Router,
    runtime: &TestRuntime,
) -> (SessionId, String, Value) {
    let session = create_session(daemon, "target", PolicyMode::Guard, None).await;
    let id = heartbeat_to(router, &session).await;
    let first = run_now(router, &id).await;
    assert_eq!(first["status"], "started");
    until(|| runtime.0.lock().sent.len() == 1).await;
    runtime.emit(
        &session,
        RuntimeEvent::MessageResult {
            id: MessageId::new(first["message_id"].as_str().unwrap()),
            outcome: RunOutcome::Deferred,
            reason: None,
        },
    );
    runtime.emit(&session, RuntimeEvent::Settled);
    wait_event(daemon, &session, |e| {
        matches!(
            e,
            EventKind::MessageCompleted {
                outcome: openside_proto::MessageOutcome::Deferred,
                ..
            }
        )
    })
    .await;
    wait_event(daemon, &session, |e| {
        matches!(e, EventKind::SessionSettled {})
    })
    .await;
    (session, id, first)
}

#[tokio::test]
async fn a_deferred_run_stays_active_until_the_carried_result_completes_it() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon_with_model(root.path(), &runtime).await;
    let router = api::router(daemon.shared.clone(), true);
    let (session, id, first) = deferred_heartbeat(&daemon, &router, &runtime).await;

    // Settled but unfinished: the session is idle, yet the run must not be reported done.
    assert_eq!(run_now(&router, &id).await["status"], "skipped_overlap");

    // The sidecar finishes the deferred run and re-reports the originating message.
    runtime.emit(
        &session,
        RuntimeEvent::MessageResult {
            id: MessageId::new(first["message_id"].as_str().unwrap()),
            outcome: RunOutcome::Completed,
            reason: None,
        },
    );
    wait_event(&daemon, &session, |e| {
        matches!(
            e,
            EventKind::MessageCompleted {
                outcome: openside_proto::MessageOutcome::Completed,
                ..
            }
        )
    })
    .await;
    assert_eq!(run_now(&router, &id).await["status"], "started");
    assert_eq!(
        run_status(&router, &id).await,
        ["started", "skipped_overlap", "completed"]
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn stopping_a_session_settles_its_deferred_run_so_the_next_one_can_start() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon_with_model(root.path(), &runtime).await;
    let router = api::router(daemon.shared.clone(), true);
    let (session, id, _) = deferred_heartbeat(&daemon, &router, &runtime).await;

    let (status, _) = send(
        &router,
        "POST",
        &format!("/v1/sessions/{}/stop", session.as_str()),
        json!({}),
    )
    .await;
    assert!(status.is_success(), "{status}");
    wait_event(&daemon, &session, |e| {
        matches!(
            e,
            EventKind::MessageCompleted {
                outcome: openside_proto::MessageOutcome::Aborted,
                ..
            }
        )
    })
    .await;
    assert_eq!(run_now(&router, &id).await["status"], "started");
    assert_eq!(run_status(&router, &id).await, ["started", "failed"]);
    daemon.shutdown().await;
}

#[tokio::test]
async fn a_deferred_run_that_nothing_can_wake_is_failed_by_the_restart_not_left_active_forever() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon_with_model(root.path(), &runtime).await;
    let router = api::router(daemon.shared.clone(), true);
    let (_, id, _) = deferred_heartbeat(&daemon, &router, &runtime).await;
    daemon.shutdown().await;
    drop(daemon);

    let reopened = daemon_with_model(root.path(), &runtime).await;
    let router = api::router(reopened.shared.clone(), true);
    assert_eq!(
        run_now(&router, &id).await["status"],
        "started",
        "the stale run no longer blocks"
    );
    assert_eq!(run_status(&router, &id).await, ["started", "failed"]);
    reopened.shutdown().await;
}

#[tokio::test]
async fn a_heartbeat_queues_behind_a_pending_human_step_and_overlaps_are_skipped() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon_with_model(root.path(), &runtime).await;
    let session = create_session(&daemon, "waiting", PolicyMode::Guard, None).await;
    opened(
        &daemon,
        &session,
        TicketKind::Confirm,
        "needs-human",
        Utc::now() + chrono::Duration::hours(1),
    )
    .await;
    let router = api::router(daemon.shared.clone(), true);
    let id = heartbeat_to(&router, &session).await;

    assert_eq!(run_now(&router, &id).await["status"], "started");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        runtime.0.lock().sent.is_empty(),
        "nothing is sent while a human step is pending"
    );
    assert_eq!(run_now(&router, &id).await["status"], "skipped_overlap");
    daemon.shutdown().await;
}

#[tokio::test]
async fn a_heartbeat_to_a_stopped_session_is_delivered_like_a_user_message() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon_with_model(root.path(), &runtime).await;
    let session = create_session(&daemon, "stopped", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), true);
    let (status, _) = send(&router, "POST", "/v1/sessions/stopped/stop", json!({})).await;
    assert!(status.is_success());
    let id = heartbeat_to(&router, &session).await;
    assert_eq!(run_now(&router, &id).await["status"], "started");
    until(|| runtime.0.lock().sent.len() == 1).await;
    daemon.shutdown().await;
}

#[tokio::test]
async fn deleting_an_active_session_races_cleanly_with_queued_work_and_heartbeats() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon_with_model(root.path(), &runtime).await;
    let session = create_session(&daemon, "busy", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), true);
    // A genuinely running turn, with more work queued behind it.
    let (status, _) = send(
        &router,
        "POST",
        "/v1/sessions/busy/messages",
        json!({"text":"work","mode":"queue"}),
    )
    .await;
    assert!(status.is_success());
    until(|| runtime.0.lock().sent.len() == 1).await;
    let (status, _) = send(
        &router,
        "POST",
        "/v1/sessions/busy/messages",
        json!({"text":"later","mode":"queue"}),
    )
    .await;
    assert!(status.is_success());
    let id = heartbeat_to(&router, &session).await;
    let opens = || {
        runtime
            .0
            .lock()
            .opened
            .iter()
            .filter(|spec| spec.session == session)
            .count()
    };
    assert_eq!(opens(), 1);

    let attempts = async {
        let mut statuses = Vec::new();
        for round in 0..4 {
            let (status, _) = send(
                &router,
                "POST",
                "/v1/sessions/busy/messages",
                json!({"text":format!("race {round}"),"mode":"queue"}),
            )
            .await;
            statuses.push(status);
            let run = run_now(&router, &id).await;
            assert!(
                [
                    "started",
                    "skipped_overlap",
                    "paused_target_missing",
                    "failed"
                ]
                .contains(&run["status"].as_str().unwrap()),
                "{run}"
            );
        }
        statuses
    };
    let deletion = send(&router, "DELETE", "/v1/sessions/busy", json!({}));
    let (statuses, (deleted, _)) = tokio::join!(attempts, deletion);
    assert_eq!(deleted, StatusCode::NO_CONTENT);
    // A message that loses the race is refused: 409 while the tombstone is being written, 404 after.
    assert!(
        statuses
            .iter()
            .all(|s| s.is_success() || *s == StatusCode::NOT_FOUND || *s == StatusCode::CONFLICT),
        "{statuses:?}"
    );

    // Once deleted: a second delete is a 404, nothing reopens, nothing more is appended.
    let (again, _) = send(&router, "DELETE", "/v1/sessions/busy", json!({})).await;
    assert_eq!(again, StatusCode::NOT_FOUND);
    let (late, _) = send(
        &router,
        "POST",
        "/v1/sessions/busy/messages",
        json!({"text":"late","mode":"queue"}),
    )
    .await;
    assert_eq!(late, StatusCode::NOT_FOUND);
    // The runtime was shut down by the delete; a straggling event must not resurrect anything.
    assert!(
        !runtime.0.lock().events.contains_key(&session),
        "the session's runtime is closed"
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    let log = daemon.shared.db.events(&session, 0).await.unwrap();
    assert!(
        matches!(log.last().unwrap().kind, EventKind::SessionDeleted {}),
        "nothing follows the tombstone"
    );
    assert_eq!(
        log.iter()
            .filter(|e| matches!(e.kind, EventKind::SessionDeleted {}))
            .count(),
        1
    );
    assert_eq!(
        opens(),
        1,
        "no runtime was reopened for the deleted session"
    );
    assert_eq!(
        runtime.0.lock().sent.len(),
        1,
        "queued work was never delivered after the delete"
    );
    let (_, paused) = send(&router, "GET", &format!("/v1/routines/{id}"), json!({})).await;
    assert_eq!(paused["paused_reason"], "target_missing");

    // And a restart does not bring it back or reopen a runtime for it.
    let log_len = log.len();
    daemon.shutdown().await;
    drop(daemon);
    let reopened = daemon_with_model(root.path(), &runtime).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(opens(), 1, "restart must not reopen the deleted session");
    assert_eq!(
        reopened.shared.db.events(&session, 0).await.unwrap().len(),
        log_len
    );
    let router = api::router(reopened.shared.clone(), true);
    let (status, _) = send(&router, "GET", "/v1/sessions/busy", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    reopened.shutdown().await;
}

#[tokio::test]
async fn deleting_a_session_or_routine_with_a_remote_token_needs_admin() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon_with_model(root.path(), &TestRuntime::default()).await;
    create_session(&daemon, "victim", PolicyMode::Guard, None).await;
    let unix = api::router(daemon.shared.clone(), true);
    let id = {
        let (_, routine) = send(
            &unix,
            "POST",
            "/v1/routines",
            interval_routine(new_session_action("p")),
        )
        .await;
        routine["id"].as_str().unwrap().to_owned()
    };
    let remote = api::router(daemon.shared.clone(), false);
    let mut statuses = Vec::new();
    for scopes in [
        vec![CapabilityScope::Read],
        vec![CapabilityScope::Steer, CapabilityScope::Approve],
    ] {
        let (_, token) = daemon.shared.db.pair(scopes).await.unwrap();
        for path in [
            "/v1/sessions/victim".to_owned(),
            format!("/v1/routines/{id}"),
        ] {
            let response = remote
                .clone()
                .oneshot(request("DELETE", &path, json!({}), Some(&token)))
                .await
                .unwrap();
            statuses.push(response.status());
        }
    }
    let anonymous = remote
        .clone()
        .oneshot(request("DELETE", "/v1/sessions/victim", json!({}), None))
        .await
        .unwrap();
    assert_eq!(statuses, [StatusCode::FORBIDDEN; 4]);
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    let (status, _) = send(&unix, "GET", "/v1/sessions/victim", json!({})).await;
    assert_eq!(status, StatusCode::OK, "nothing was deleted");

    let (_, admin) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Admin])
        .await
        .unwrap();
    let deleted = remote
        .clone()
        .oneshot(request(
            "DELETE",
            "/v1/sessions/victim",
            json!({}),
            Some(&admin),
        ))
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    let again = remote
        .oneshot(request(
            "DELETE",
            "/v1/sessions/victim",
            json!({}),
            Some(&admin),
        ))
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::NOT_FOUND);
    daemon.shutdown().await;
}

#[tokio::test]
async fn an_open_event_stream_ends_after_the_tombstone_instead_of_waiting_forever() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon_with_model(root.path(), &TestRuntime::default()).await;
    create_session(&daemon, "watched", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), true);
    let response = router
        .clone()
        .oneshot(request(
            "GET",
            "/v1/sessions/watched/events",
            json!({}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let reader =
        tokio::spawn(async move { response.into_body().collect().await.unwrap().to_bytes() });
    let (status, _) = send(&router, "DELETE", "/v1/sessions/watched", json!({})).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let body = tokio::time::timeout(Duration::from_secs(5), reader)
        .await
        .expect("the stream must end by itself")
        .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("session.deleted"));

    // Reconnecting with the cursor placed on the tombstone is also a clean 404, not a hang.
    let (status, _) = send(&router, "GET", "/v1/sessions/watched/events", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    daemon.shutdown().await;
}

#[tokio::test]
async fn a_cursor_beyond_the_log_is_refused_and_a_current_cursor_still_ends_on_delete() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon_with_model(root.path(), &TestRuntime::default()).await;
    let session = create_session(&daemon, "cursor", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), true);
    let last = daemon.shared.db.state(&session).await.unwrap().last_seq;
    let stream = |cursor: String| {
        let router = router.clone();
        async move {
            let mut request = request("GET", "/v1/sessions/cursor/events", json!({}), None);
            request
                .headers_mut()
                .insert("last-event-id", cursor.parse().unwrap());
            router.oneshot(request).await.unwrap()
        }
    };
    // Otherwise this stream would wait forever once the session is deleted.
    for cursor in [u64::MAX, last + 1] {
        assert_eq!(
            stream(cursor.to_string()).await.status(),
            StatusCode::BAD_REQUEST,
            "{cursor}"
        );
    }

    let response = stream(last.to_string()).await;
    assert_eq!(response.status(), StatusCode::OK);
    let reader =
        tokio::spawn(async move { response.into_body().collect().await.unwrap().to_bytes() });
    let (status, _) = send(&router, "DELETE", "/v1/sessions/cursor", json!({})).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let body = tokio::time::timeout(Duration::from_secs(5), reader)
        .await
        .expect("the stream must end by itself")
        .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("session.deleted"));
    daemon.shutdown().await;
}
