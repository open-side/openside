use super::*;

async fn create(daemon: &Daemon, body: Value) -> (StatusCode, Value) {
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request("POST", "/v1/sessions", body, None))
        .await
        .unwrap();
    (response.status(), json_body(response).await)
}

async fn opened_agent(daemon: &Daemon, runtime: &TestRuntime, session: &str) -> Value {
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            json!({"message_id": format!("{session}-m1"), "text": "hello", "mode": "queue"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let id = SessionId::new(session);
    until(|| {
        runtime
            .0
            .lock()
            .opened
            .iter()
            .any(|spec| spec.session == id)
    })
    .await;
    let spec = runtime
        .0
        .lock()
        .opened
        .iter()
        .find(|spec| spec.session == id)
        .cloned()
        .unwrap();
    serde_json::from_str(&spec.agent_json).unwrap()
}

#[tokio::test]
async fn session_effort_and_speed_persist_and_reach_the_sidecar_open_config() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let model = std::any::type_name::<TestRuntime>();

    let (status, body) = create(
        &daemon,
        json!({"model": model, "effort": "xhigh", "speed": "fast"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["effort"], "xhigh");
    assert_eq!(body["speed"], "fast");
    let tuned = body["id"].as_str().unwrap().to_owned();
    let state = daemon
        .shared
        .db
        .state(&SessionId::new(&tuned))
        .await
        .unwrap();
    assert_eq!(state.effort, Some(openside_proto::ThinkingEffort::Xhigh));
    assert_eq!(state.speed, Some(openside_proto::SessionSpeed::Fast));
    let agent = opened_agent(&daemon, &runtime, &tuned).await;
    assert_eq!(agent["effort"], "xhigh");
    assert_eq!(agent["speed"], "fast");

    let (status, body) = create(&daemon, json!({"model": model, "effort": "off"})).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let off = body["id"].as_str().unwrap().to_owned();
    let agent = opened_agent(&daemon, &runtime, &off).await;
    assert_eq!(agent["effort"], "off");
    assert!(agent.get("speed").is_none());

    // Sessions created without either keep their exact previous open config.
    let (status, body) = create(&daemon, json!({"model": model})).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(body.get("effort").is_none() && body.get("speed").is_none());
    let plain = body["id"].as_str().unwrap().to_owned();
    let agent = opened_agent(&daemon, &runtime, &plain).await;
    assert!(agent.get("effort").is_none() && agent.get("speed").is_none());

    for invalid in [json!({"effort": "ultrabrowse"}), json!({"speed": "slow"})] {
        let (status, _) = create(&daemon, invalid).await;
        assert!(status.is_client_error(), "{status}");
    }
    daemon.shutdown().await;
}

#[tokio::test]
async fn agent_client_tokens_cannot_pick_expensive_session_tiers() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let (_, agent) = daemon
        .shared
        .db
        .pair_as(
            "agent".into(),
            vec![CapabilityScope::Steer],
            openside_proto::DeviceKind::AgentClient,
        )
        .await
        .unwrap();
    let (_, person) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Steer])
        .await
        .unwrap();
    let remote = api::router(daemon.shared.clone(), false);
    let local = api::router(daemon.shared.clone(), true);
    let create = |router: axum::Router, token: Option<String>, body: Value| async move {
        let response = router
            .oneshot(request("POST", "/v1/sessions", body, token.as_deref()))
            .await
            .unwrap();
        let status = response.status();
        (status, json_body(response).await)
    };
    let model = std::any::type_name::<TestRuntime>();
    for body in [
        json!({"model": model, "speed": "fast"}),
        json!({"model": model, "effort": "xhigh"}),
        json!({"model": model, "effort": "max"}),
        json!({"model": model, "profile": "ultrabrowse"}),
    ] {
        let (status, error) = create(remote.clone(), Some(agent.clone()), body.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(error["code"], "local_socket_required");
        let (status, created) = create(remote.clone(), Some(person.clone()), body.clone()).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "person device: {body} {created}"
        );
        let (status, created) = create(local.clone(), None, body.clone()).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "local socket: {body} {created}"
        );
    }
    // Standard and cheaper tiers stay open to agent clients.
    for body in [
        json!({"model": model}),
        json!({"model": model, "speed": "default", "effort": "high"}),
        json!({"model": model, "effort": "off", "profile": "default"}),
    ] {
        let (status, created) = create(remote.clone(), Some(agent.clone()), body.clone()).await;
        assert_eq!(status, StatusCode::CREATED, "{body} {created}");
    }
    daemon.shutdown().await;
}
