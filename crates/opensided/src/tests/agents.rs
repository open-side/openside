use super::*;

fn draft() -> HireDraft {
    serde_json::from_value(json!({"name":"Mina","persona":{"role":"Watches prices","voice":"Brief and clear","responsibilities":["Track prices"],"boundaries":["Ask before purchases"]}})).unwrap()
}

#[tokio::test]
async fn agents_retirement_stops_nonhome_channel_stream_and_rejects_new_messages() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let d = daemon(root.path(), &runtime).await;
    let agent = crate::team::hire(&d.shared, &draft(), "retire-channel")
        .await
        .unwrap();
    let router = api::router(d.shared.clone(), true);
    let response = router
        .clone()
        .oneshot(request(
            "POST",
            "/v1/sessions",
            json!({"agent_id":agent.id,"model":"script:test","title":"Telegram: prices"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let session: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let id = SessionId::new(session["id"].as_str().unwrap());
    assert_ne!(id, agent.home_session_id);
    let path = format!("/v1/sessions/{id}/messages");
    let response = router
        .clone()
        .oneshot(request(
            "POST",
            &path,
            json!({"text":"Keep streaming","mode":"queue"}),
            None,
        ))
        .await
        .unwrap();
    assert!(response.status().is_success());
    tokio::time::timeout(Duration::from_secs(5), async {
        while !runtime
            .0
            .lock()
            .sent
            .iter()
            .any(|(session, _, _, _)| session == &id)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    crate::team::retire(
        &d.shared,
        &agent.id,
        &RetireAgentRequest {
            successor: None,
            pause_routines: true,
            revert_hire: false,
        },
        "retire-channel",
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if d.shared.db.state(&id).await.unwrap().status == SessionStatus::Stopped {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!runtime.0.lock().events.contains_key(&id));
    let sent = runtime.0.lock().sent.len();
    let response = router
        .oneshot(request(
            "POST",
            &path,
            json!({"text":"Must not run","mode":"queue"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(runtime.0.lock().sent.len(), sent);
    d.shutdown().await;
}

#[tokio::test]
async fn agents_bootstrap_is_idempotent_and_repairs_home() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let d = daemon(root.path(), &runtime).await;
    let first = d.shared.agents.get("chief").unwrap().unwrap();
    crate::agents::bootstrap(&d.shared).await.unwrap();
    assert_eq!(d.shared.agents.get("chief").unwrap().unwrap(), first);
    assert_eq!(
        d.shared
            .db
            .state(&first.home_session_id)
            .await
            .unwrap()
            .agent_id
            .as_deref(),
        Some("chief")
    );
    assert_eq!(
        d.shared
            .db
            .state(&first.home_session_id)
            .await
            .unwrap()
            .model,
        ""
    );
    assert_eq!(d.shared.agents.list().unwrap().len(), 1);
    let mut pending = first.clone();
    pending.id = "pending".into();
    pending.name = "Mina".into();
    pending.kind = AgentKind::Teammate;
    pending.persona = draft().persona;
    pending.home_session_id = SessionId::new("pending-home");
    d.shared.agents.put(&pending, "created").unwrap();
    assert!(d.shared.db.state(&pending.home_session_id).await.is_err());
    crate::agents::bootstrap(&d.shared).await.unwrap();
    assert_eq!(
        d.shared
            .db
            .state(&pending.home_session_id)
            .await
            .unwrap()
            .agent_id
            .as_deref(),
        Some("pending")
    );
    let mut moved = pending.clone();
    moved.home_session_id = SessionId::new("other");
    assert!(d.shared.agents.put(&moved, "updated").is_err());
    let mut retired = first;
    retired.status = AgentStatus::Retired;
    assert!(d.shared.agents.put(&retired, "retired").is_err());
}

#[tokio::test]
async fn agents_roundtrip_caps_channel_owner_and_audit() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let d = daemon(root.path(), &runtime).await;
    let mut agent = crate::agents::create(&d.shared, &draft(), "mina")
        .await
        .unwrap_or_else(|error| panic!("{}", error.1));
    assert_eq!(d.shared.agents.get("mina").unwrap(), Some(agent.clone()));
    agent.channel = Some(AgentChannelBinding {
        kind: "telegram".into(),
        account_id: "prices".into(),
    });
    d.shared.agents.put(&agent, "updated").unwrap();
    assert_eq!(
        d.shared
            .agents
            .channel_owner("telegram", "prices")
            .unwrap()
            .as_deref(),
        Some("mina")
    );
    agent.status = AgentStatus::Retired;
    d.shared.agents.put(&agent, "retired").unwrap();
    assert_eq!(
        d.shared
            .agents
            .channel_owner("telegram", "prices")
            .unwrap()
            .as_deref(),
        Some("mina")
    );
    d.shared
        .agents
        .clear_channel_binding("telegram", "prices")
        .unwrap();
    assert_eq!(
        d.shared.agents.channel_owner("telegram", "prices").unwrap(),
        None
    );
    assert!(d
        .shared
        .agents
        .get("mina")
        .unwrap()
        .unwrap()
        .channel
        .is_none());
    agent.persona.responsibilities = vec!["x".repeat(100); 6];
    agent.persona.boundaries = vec!["y".repeat(100); 6];
    agent.persona.voice = "z".repeat(100);
    assert_eq!(crate::agents::validate(&agent), Ok(()));
    assert!(crate::agents::persona(&agent).chars().count() <= 1500);
    agent.persona.responsibilities.push("extra".into());
    assert_eq!(crate::agents::validate(&agent), Err("invalid_persona"));
    assert!(crate::agents::roster(&vec![agent.clone(); 40]).is_ok());
    agent.status = AgentStatus::Active;
    assert!(crate::agents::roster(&vec![agent; 200]).is_err());
    let db = rusqlite::Connection::open(d.shared.config.data_dir.join("agents.db")).unwrap();
    let audits: i64 = db
        .query_row("SELECT count(*) FROM agent_audit", [], |r| r.get(0))
        .unwrap();
    assert!(audits >= 5);
}

#[test]
fn agents_store_migrates_v1_and_rejects_future_schema() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("agents.db");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE agents(id TEXT PRIMARY KEY,name TEXT NOT NULL UNIQUE,home_session_id TEXT NOT NULL UNIQUE,data TEXT NOT NULL); CREATE TABLE team_proposals(key TEXT PRIMARY KEY,status TEXT NOT NULL,decided_at TEXT NOT NULL); PRAGMA user_version=1;").unwrap();
    let legacy:Agent=serde_json::from_value(json!({
        "id":"mina","kind":"teammate","name":"Mina","initials":"M","color":ACCENT_COLORS[0],
        "persona":draft().persona,"skills":null,"model":null,"account_pool":null,
        "channel":{"kind":"telegram","account_id":"prices"},"mode":"guard","home_session_id":"home",
        "status":"retired","hidden":false,"pinned":false,"created_at":Utc::now(),"retired_at":Utc::now(),"persona_version":1
    })).unwrap();
    db.execute(
        "INSERT INTO agents VALUES('mina','Mina','home',?1)",
        [serde_json::to_string(&legacy).unwrap()],
    )
    .unwrap();
    drop(db);
    let service = crate::agents::Service::open(root.path()).unwrap();
    assert_eq!(service.get("mina").unwrap(), Some(legacy));
    assert_eq!(
        service
            .channel_owner("telegram", "prices")
            .unwrap()
            .as_deref(),
        Some("mina")
    );
    drop(service);
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        4
    );
    db.execute_batch("PRAGMA user_version=99;").unwrap();
    drop(db);
    assert!(crate::agents::Service::open(root.path()).is_err());
}

#[tokio::test]
async fn agents_session_binding_rejects_unknown_and_retired() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let d = daemon(root.path(), &runtime).await;
    let mut agent = crate::agents::create(&d.shared, &draft(), "mina")
        .await
        .unwrap_or_else(|error| panic!("{}", error.1));
    agent.status = AgentStatus::Retired;
    d.shared.agents.put(&agent, "retired").unwrap();
    for (id, status, code) in [
        ("absent", StatusCode::NOT_FOUND, "agent_not_found"),
        ("mina", StatusCode::CONFLICT, "agent_retired"),
    ] {
        let response = api::router(d.shared.clone(), true)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"agent_id":id,"model":"script:test"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        let body: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(body["code"], code);
    }
    let response = api::router(d.shared.clone(), true)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/sessions")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"agent_id":"chief","model":"script:test"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["agent_id"], "chief");
}

#[test]
fn agents_patch_null_is_distinct_from_omission() {
    let omitted: UpdateAgentRequest = serde_json::from_value(json!({})).unwrap();
    let clear: UpdateAgentRequest =
        serde_json::from_value(json!({"channel":null,"account_pool":null})).unwrap();
    assert_eq!(omitted.channel, None);
    assert_eq!(clear.channel, Some(None));
    assert_eq!(clear.account_pool, Some(None));
}

#[tokio::test]
async fn agents_patch_scope_matrix_and_ceiling() {
    use axum::{
        extract::{Path, State},
        Extension, Json,
    };
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let d = daemon(root.path(), &runtime).await;
    for (scope, kind, body, expected) in [
        (
            CapabilityScope::Steer,
            DeviceKind::PersonDevice,
            json!({"name":"June"}),
            None,
        ),
        (
            CapabilityScope::Steer,
            DeviceKind::PersonDevice,
            json!({"model":"script:test"}),
            Some("forbidden"),
        ),
        (
            CapabilityScope::Admin,
            DeviceKind::AgentClient,
            json!({"model":"script:test"}),
            Some("person_device_required"),
        ),
        (
            CapabilityScope::Admin,
            DeviceKind::PersonDevice,
            json!({"model":"script:test"}),
            None,
        ),
    ] {
        let result = crate::agents::api::update(
            State(d.shared.clone()),
            Extension(api::Caller(Some("device".into()), kind)),
            Extension(api::Access(vec![scope])),
            Path("chief".into()),
            Json(serde_json::from_value(body).unwrap()),
        )
        .await;
        assert_eq!(result.err().map(|error| error.1), expected);
    }
    for mode in ["read-only", "guard"] {
        let result = crate::agents::api::update(
            State(d.shared.clone()),
            Extension(api::Caller(None, DeviceKind::PersonDevice)),
            Extension(api::Access(vec![CapabilityScope::Admin])),
            Path("chief".into()),
            Json(serde_json::from_value(json!({"mode":mode})).unwrap()),
        )
        .await;
        let agent = result.unwrap_or_else(|error| panic!("{}", error.1)).0;
        assert_eq!(
            d.shared
                .db
                .state(&agent.home_session_id)
                .await
                .unwrap()
                .mode,
            agent.mode
        );
    }
    let mut chief = d.shared.agents.get("chief").unwrap().unwrap();
    chief.mode = PolicyMode::ReadOnly;
    assert_eq!(
        crate::agents::ceiling(&d.shared, &chief, PolicyMode::Full).unwrap(),
        PolicyMode::ReadOnly
    );
}

#[tokio::test]
async fn agents_list_preview_tracks_latest_final_and_home_identity() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let d = daemon(root.path(), &runtime).await;
    let chief = d.shared.agents.get("chief").unwrap().unwrap();
    d.shared
        .emit(
            &chief.home_session_id,
            None,
            EventKind::AssistantFinal {
                text: "Latest result".into(),
            },
        )
        .await
        .unwrap();
    let response = api::router(d.shared.clone(), true)
        .oneshot(
            Request::builder()
                .uri("/v1/agents")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["agents"][0]["id"], "chief");
    assert_eq!(body["agents"][0]["last_activity"], "Latest result");
    assert_eq!(body["agents"][0]["last_seq"], 2);
}

#[tokio::test]
async fn agents_http_hire_creates_home_and_legacy_filter_keeps_earlier_threads() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let d = daemon(root.path(), &runtime).await;
    create_session(&d, "earlier", PolicyMode::Guard, None).await;
    let response = api::router(d.shared.clone(), true)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/agents")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&draft()).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let agent: Agent =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(agent.name, "Mina");
    assert_eq!(
        d.shared
            .db
            .state(&agent.home_session_id)
            .await
            .unwrap()
            .agent_id
            .as_deref(),
        Some(agent.id.as_str())
    );
    let response = api::router(d.shared.clone(), true)
        .oneshot(
            Request::builder()
                .uri("/v1/sessions?agent=none")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: SessionList =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body.sessions.len(), 1);
    assert_eq!(body.sessions[0].id.as_str(), "earlier");
}

#[tokio::test]
async fn agents_credential_patch_waits_for_admission_and_advances_generation() {
    use axum::{
        extract::{Path, State},
        Extension, Json,
    };
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let d = daemon(root.path(), &runtime).await;
    let generation = d.shared.models.credentials_generation();
    let admission = d.shared.models.admit_credentials().await;
    let shared = d.shared.clone();
    let update = tokio::spawn(async move {
        crate::agents::api::update(
            State(shared),
            Extension(api::Caller(None, DeviceKind::PersonDevice)),
            Extension(api::Access(vec![CapabilityScope::Admin])),
            Path("chief".into()),
            Json(serde_json::from_value(json!({"model":"script:new"})).unwrap()),
        )
        .await
    });
    tokio::task::yield_now().await;
    assert!(!update.is_finished());
    assert_eq!(d.shared.agents.get("chief").unwrap().unwrap().model, None);
    assert_eq!(d.shared.models.credentials_generation(), generation);
    drop(admission);
    let updated = update
        .await
        .unwrap()
        .unwrap_or_else(|error| panic!("{}", error.1))
        .0;
    assert_eq!(updated.model.as_deref(), Some("script:new"));
    assert!(d.shared.models.credentials_generation() > generation);
    assert_eq!(
        d.shared
            .agents
            .get("chief")
            .unwrap()
            .unwrap()
            .model
            .as_deref(),
        Some("script:new")
    );
    let generation = d.shared.models.credentials_generation();
    let unchanged = crate::agents::api::update(
        State(d.shared.clone()),
        Extension(api::Caller(None, DeviceKind::PersonDevice)),
        Extension(api::Access(vec![CapabilityScope::Admin])),
        Path("chief".into()),
        Json(serde_json::from_value(json!({"model":"script:new"})).unwrap()),
    )
    .await
    .unwrap_or_else(|error| panic!("{}", error.1))
    .0;
    assert_eq!(unchanged.model, updated.model);
    assert_eq!(d.shared.models.credentials_generation(), generation);
}
