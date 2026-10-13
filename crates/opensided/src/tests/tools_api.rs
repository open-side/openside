//! `GET /v1/tools` and `POST /v1/sessions/{id}/tools/{name}`: direct tool calls for API
//! clients go through the agent's tool pipeline, follow token scopes over TCP, and keep their
//! tickets away from the agent runtime.

use super::*;

#[tokio::test]
async fn direct_tool_calls_follow_scopes_and_return_the_human_answer() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let mut config = resolved_config(root.path());
    // In front orchestration the front agent may not act; API callers are not the front agent.
    config.loaded.config.agent.orchestration = openside_config::Orchestration::Front;
    let daemon = Daemon::open_runtime(config, runtime.clone()).await.unwrap();
    let session = create_session(&daemon, "direct", PolicyMode::Guard, None).await;
    let local = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let send =
        |router: &axum::Router, method: &str, path: String, body: Value, token: Option<&str>| {
            router.clone().oneshot(request(method, &path, body, token))
        };

    let catalog = send(&local, "GET", "/v1/tools".into(), json!({}), None)
        .await
        .unwrap();
    assert_eq!(catalog.status(), StatusCode::OK);
    let catalog = json_body(catalog).await;
    let names: Vec<&str> = catalog["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"ask_user"), "{names:?}");
    assert!(names.contains(&"browser_act"), "{names:?}");
    for agent_only in [
        "bash",
        "write",
        "delegate",
        "create_task",
        "browser_repl",
        "memory_search",
    ] {
        assert!(
            !names.contains(&agent_only),
            "{agent_only} must stay agent-only"
        );
    }
    let vault = catalog["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "vault_login")
        .unwrap();
    assert_eq!(vault["scope"], "vault_use");

    let path = |tool: &str| format!("/v1/sessions/{session}/tools/{tool}");
    let ask = json!({"args":{"question":"Which size?","options":["S","M"]}});

    // Scopes over TCP: read cannot call, browse can, vault tools need vault_use too.
    let (_, read) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Read])
        .await
        .unwrap();
    let (_, browse) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Read, CapabilityScope::Browse])
        .await
        .unwrap();
    let denied = send(&tcp, "POST", path("ask_user"), ask.clone(), Some(&read))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let vault = send(
        &tcp,
        "POST",
        path("vault_login"),
        json!({"args":{"tab_id":"t"}}),
        Some(&browse),
    )
    .await
    .unwrap();
    assert_eq!(vault.status(), StatusCode::FORBIDDEN);
    let unknown = send(&local, "POST", path("bash"), json!({"args":{}}), None)
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

    // A human step opens an ordinary ticket; its answer is the caller's result, not the agent's.
    let deferred = send(&tcp, "POST", path("ask_user"), ask.clone(), Some(&browse))
        .await
        .unwrap();
    assert_eq!(deferred.status(), StatusCode::OK);
    let deferred = json_body(deferred).await;
    assert_eq!(deferred["status"], "deferred", "{deferred}");
    assert_eq!(deferred["deferral"], "external");
    assert!(deferred["tool_call_id"]
        .as_str()
        .unwrap()
        .starts_with("api-tool-"));
    let ticket = deferred["ticket_id"].as_str().unwrap().to_owned();
    let pending = json_body(
        send(
            &local,
            "GET",
            "/v1/tickets?status=pending".into(),
            json!({}),
            None,
        )
        .await
        .unwrap(),
    )
    .await;
    assert!(
        pending["tickets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|listed| listed["id"] == ticket.as_str() && listed["kind"] == "info"),
        "{pending}"
    );
    let resolved = send(
        &local,
        "POST",
        format!("/v1/tickets/{ticket}/resolve"),
        json!({"value":"M"}),
        None,
    )
    .await
    .unwrap();
    assert_eq!(resolved.status(), StatusCode::OK);
    let answered = json_body(
        send(
            &local,
            "GET",
            format!("/v1/tickets/{ticket}"),
            json!({}),
            None,
        )
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(answered["status"], "resolved", "{answered}");
    assert!(!runtime
        .0
        .lock()
        .resolved
        .iter()
        .any(|(id, _, _)| id.as_str() == ticket));

    // An approval rerun only resumes this route's own ticket for the same tool.
    let wrong_tool = send(
        &local,
        "POST",
        path("request_choice"),
        json!({"args":{"tab_id":"t"},"approval_ticket":ticket}),
        None,
    )
    .await
    .unwrap();
    assert_eq!(wrong_tool.status(), StatusCode::CONFLICT);
    daemon.shutdown().await;
}

async fn authority_request(
    router: &axum::Router,
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
    (response.status(), json_body(response).await)
}

#[tokio::test]
async fn direct_confirm_tickets_refuse_self_approval_but_allow_denial_and_other_devices() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "direct-confirm", PolicyMode::Guard, None).await;
    let tcp = api::router(daemon.shared.clone(), false);
    let local = api::router(daemon.shared.clone(), true);
    let scopes = vec![
        CapabilityScope::Read,
        CapabilityScope::Browse,
        CapabilityScope::Approve,
    ];
    let (device, token) = daemon
        .shared
        .db
        .pair_as(
            "external agent".into(),
            scopes,
            openside_proto::DeviceKind::AgentClient,
        )
        .await
        .unwrap();
    let (_, person) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Approve])
        .await
        .unwrap();
    let path = format!("/v1/sessions/{session}/tools/request_human_step");
    let args = json!({"args":{"kind":"confirm","summary":"Approve this sensitive action?"}});

    for answer in [json!(true), json!("approve"), json!(false), json!("reject")] {
        let (status, deferred) =
            authority_request(&tcp, "POST", &path, args.clone(), Some(&token)).await;
        assert_eq!(status, StatusCode::OK, "{deferred}");
        let ticket = deferred["ticket_id"].as_str().unwrap();
        let resolve = format!("/v1/tickets/{ticket}/resolve");
        let (status, result) = authority_request(
            &tcp,
            "POST",
            &resolve,
            json!({"value":answer}),
            Some(&token),
        )
        .await;
        if answer == true || answer == "approve" {
            assert_eq!(status, StatusCode::FORBIDDEN);
            assert_eq!(result["code"], "person_approval_required");
            let (_, pending) = authority_request(
                &local,
                "GET",
                &format!("/v1/tickets/{ticket}"),
                json!({}),
                None,
            )
            .await;
            assert_eq!(pending["status"], "pending");
            assert_eq!(pending["origin"]["device_id"], device);
            assert_eq!(pending["origin"]["driver_ids"], json!([device]));
            let (status, resolved) =
                authority_request(&tcp, "POST", &resolve, json!({"value":true}), Some(&person))
                    .await;
            assert_eq!(status, StatusCode::OK, "{resolved}");
            assert_eq!(resolved["resolution"]["approved"], true);
        } else {
            assert_eq!(status, StatusCode::OK, "{result}");
            assert_eq!(result["resolution"]["approved"], false);
        }
    }
    let (_, deferred) = authority_request(&local, "POST", &path, args.clone(), None).await;
    let ticket = deferred["ticket_id"].as_str().unwrap();
    let resolve = format!("/v1/tickets/{ticket}/resolve");
    let (status, refused) =
        authority_request(&local, "POST", &resolve, json!({"value":true}), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert_eq!(refused["code"], "person_approval_required");
    let (status, denied) = authority_request(
        &local,
        "POST",
        &format!("/v1/tickets/{ticket}/reject"),
        json!({}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{denied}");
    assert_eq!(denied["status"], "rejected");
    daemon.shutdown().await;
}

#[tokio::test]
async fn session_agent_confirmations_keep_all_driver_exclusions_across_restart() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let first = daemon(root.path(), &runtime).await;
    let scopes = vec![
        CapabilityScope::Read,
        CapabilityScope::Steer,
        CapabilityScope::Browse,
        CapabilityScope::Approve,
    ];
    let (creator_id, creator) = first
        .shared
        .db
        .pair_as(
            "creator".into(),
            scopes.clone(),
            openside_proto::DeviceKind::AgentClient,
        )
        .await
        .unwrap();
    let (driver_id, driver) = first
        .shared
        .db
        .pair_as(
            "driver".into(),
            scopes.clone(),
            openside_proto::DeviceKind::AgentClient,
        )
        .await
        .unwrap();
    let (_, person) = first.shared.db.pair(scopes).await.unwrap();
    let tcp = api::router(first.shared.clone(), false);
    let (status, created) = authority_request(
        &tcp,
        "POST",
        "/v1/sessions",
        json!({"model":std::any::type_name::<TestRuntime>()}),
        Some(&creator),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let session = SessionId::new(created["id"].as_str().unwrap());
    let (status, driven) = authority_request(
        &tcp,
        "POST",
        &format!("/v1/sessions/{session}/messages"),
        json!({"text":"Do the work","mode":"now"}),
        Some(&driver),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{driven}");
    let outcome = host(&first, &session)
        .await
        .call(call(
            "write",
            json!({"path":root.path().join("outside.txt"),"content":"must not run"}),
            "internal-sensitive",
        ))
        .await;
    let ToolOutcome::Deferred {
        ticket,
        kind: DeferKind::Approval,
    } = outcome
    else {
        panic!("{outcome:?}");
    };
    let snapshot = first.shared.db.state(&session).await.unwrap();
    assert_eq!(
        snapshot.driver_ids,
        [creator_id.clone(), driver_id.clone()]
            .into_iter()
            .collect()
    );
    assert_eq!(snapshot.tickets[&ticket].origin.driver_ids.len(), 2);
    assert!(snapshot.tickets[&ticket].origin.device_id.is_none());
    let stored = session.clone();
    first
        .shared
        .db
        .store(move |store| {
            Ok(store.put_agent_state(
                &stored,
                &AgentState {
                    format: "test-state".into(),
                    data: "approval pending".into(),
                },
            )?)
        })
        .await
        .unwrap();
    runtime.0.lock().ready_deferred.push(DeferredCall {
        ticket: ticket.clone(),
        tool_call_id: snapshot.tickets[&ticket].tool_call_id.clone(),
        kind: DeferKind::Approval,
    });
    first.shutdown().await;
    drop(tcp);
    drop(first);

    let second = daemon(root.path(), &runtime).await;
    let tcp = api::router(second.shared.clone(), false);
    for token in [&creator, &driver] {
        let (status, refused) = authority_request(
            &tcp,
            "POST",
            &format!("/v1/tickets/{ticket}/resolve"),
            json!({"value":true}),
            Some(token),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
        assert_eq!(refused["code"], "person_approval_required");
    }
    assert!(!root.path().join("outside.txt").exists());
    let (status, approved) = authority_request(
        &tcp,
        "POST",
        &format!("/v1/tickets/{ticket}/resolve"),
        json!({"value":true}),
        Some(&person),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert_eq!(approved["resolution"]["approved"], true);
    second.shutdown().await;
}

#[tokio::test]
async fn person_device_conversations_do_not_gain_agent_driver_exclusions() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let (_, person) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Steer, CapabilityScope::Approve])
        .await
        .unwrap();
    let tcp = api::router(daemon.shared.clone(), false);
    let (status, created) = authority_request(
        &tcp,
        "POST",
        "/v1/sessions",
        json!({"model":std::any::type_name::<TestRuntime>()}),
        Some(&person),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let session = SessionId::new(created["id"].as_str().unwrap());
    let (status, sent) = authority_request(
        &tcp,
        "POST",
        &format!("/v1/sessions/{session}/messages"),
        json!({"text":"Work for me","mode":"now"}),
        Some(&person),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{sent}");
    let outcome = host(&daemon, &session)
        .await
        .call(call(
            "request_human_step",
            json!({"kind":"confirm","summary":"Approve?"}),
            "person-request",
        ))
        .await;
    let ToolOutcome::Deferred { ticket, .. } = outcome else {
        panic!("{outcome:?}");
    };
    let (status, approved) = authority_request(
        &tcp,
        "POST",
        &format!("/v1/tickets/{ticket}/resolve"),
        json!({"value":true}),
        Some(&person),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert!(daemon
        .shared
        .db
        .state(&session)
        .await
        .unwrap()
        .driver_ids
        .is_empty());
    daemon.shutdown().await;
}

#[tokio::test]
async fn pairing_roles_are_fixed_by_the_operator_and_not_the_redeemer() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let local = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    for (kind, attempted) in [
        ("agent_client", "person_device"),
        ("person_device", "agent_client"),
    ] {
        let (status, offer) = authority_request(
            &local,
            "POST",
            "/v1/pairing",
            json!({
                "name":"operator-selected","scopes":["read","steer","approve"],"kind":kind
            }),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{offer}");
        assert_eq!(offer["kind"], kind);
        let (status, refused) = authority_request(
            &tcp,
            "POST",
            "/v1/pairing/exchange",
            json!({
                "code":offer["code"],"kind":attempted
            }),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
        let (status, paired) = authority_request(
            &tcp,
            "POST",
            "/v1/pairing/exchange",
            json!({"code":offer["code"]}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{paired}");
        assert_eq!(paired["kind"], kind);
        let devices = daemon.shared.db.devices().await.unwrap();
        let paired_kind = devices
            .iter()
            .find(|device| device.device_id == paired["device_id"].as_str().unwrap())
            .unwrap()
            .kind;
        assert_eq!(serde_json::to_value(paired_kind).unwrap(), kind);
    }
    daemon.shutdown().await;
}

#[tokio::test]
async fn local_session_agent_cannot_approve_its_own_confirmation() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let local = api::router(daemon.shared.clone(), true);
    let (status, created) = authority_request(
        &local,
        "POST",
        "/v1/sessions",
        json!({"model":std::any::type_name::<TestRuntime>()}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let session = SessionId::new(created["id"].as_str().unwrap());
    let outcome = host(&daemon, &session)
        .await
        .call(call(
            "request_human_step",
            json!({"kind":"confirm","summary":"Approve?"}),
            "local-agent-request",
        ))
        .await;
    let ToolOutcome::Deferred { ticket, .. } = outcome else {
        panic!("{outcome:?}");
    };
    let (status, refused) = authority_request(
        &local,
        "POST",
        &format!("/v1/tickets/{ticket}/resolve"),
        json!({"value":true}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert_eq!(refused["code"], "person_approval_required");
    let state = daemon.shared.db.state(&session).await.unwrap();
    assert_eq!(state.tickets[&ticket].origin.device_id, None);
    assert_eq!(state.tickets[&ticket].origin.driver_ids, vec!["local"]);
    assert_eq!(state.tickets[&ticket].status, TicketStatus::Pending);
    let (status, denied) = authority_request(
        &local,
        "POST",
        &format!("/v1/tickets/{ticket}/resolve"),
        json!({"value":false}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{denied}");
    assert_eq!(denied["resolution"]["approved"], false);
    daemon.shutdown().await;
}

#[tokio::test]
async fn every_agent_identity_is_refused_for_person_decisions_but_may_reject() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "person-decisions", PolicyMode::Guard, None).await;
    let scopes = vec![
        CapabilityScope::Read,
        CapabilityScope::Approve,
        CapabilityScope::HumanStep,
    ];
    let (creator_id, creator) = daemon
        .shared
        .db
        .pair_as(
            "creator".into(),
            scopes.clone(),
            openside_proto::DeviceKind::AgentClient,
        )
        .await
        .unwrap();
    let (_, other_agent) = daemon
        .shared
        .db
        .pair_as(
            "fresh agent".into(),
            scopes,
            openside_proto::DeviceKind::AgentClient,
        )
        .await
        .unwrap();
    let local = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    for (id, kind, value) in [
        ("confirm-gate", TicketKind::Confirm, json!(true)),
        ("choice-gate", TicketKind::Choice, json!({"option":"o1"})),
        (
            "enroll-gate",
            TicketKind::SecretInput,
            json!({"totp":"GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"}),
        ),
        (
            "save-gate",
            TicketKind::Login,
            json!({"values":{"f0":"gate-user","f1":"gate-canary-password"},"save":true}),
        ),
    ] {
        let mut opened: Ticket = serde_json::from_value(json!({
            "id":id,"session_id":session,"tool_call_id":format!("call-{id}"),
            "deferral":"external",
            "kind":kind,"summary":"Person decision","options":[],
            "origin":{"device_id":creator_id,"driver_ids":[]},
            "target":{"tab_id":"tab","origin":"https://example.test","field_ref":"e1"},
            "created_at":Utc::now(),"expires_at":Utc::now() + chrono::Duration::minutes(10),
            "status":"pending"
        }))
        .unwrap();
        if kind == TicketKind::Choice {
            opened.choice = Some(serde_json::from_value(json!({
                "origin":"https://example.test","site_name":"Example","question":"Choose an account",
                "multiple":false,"options":[{"id":"o1","label":"Account"}]
            })).unwrap());
        }
        if kind == TicketKind::SecretInput {
            opened.otp = Some(openside_proto::TicketOtp {
                channel: openside_proto::OtpChannel::Unknown,
                can_register: true,
            });
        }
        if kind == TicketKind::Login {
            opened.login = Some(serde_json::from_value(json!({
                "origin":"https://example.test","site_name":"Example","fields":[
                    {"id":"f0","kind":"identifier","label":"Account","masked":false,"required":true},
                    {"id":"f1","kind":"password","label":"Password","masked":true,"required":true}
                ],"alternatives":[],"submit_label":"Sign in","can_save":true
            })).unwrap());
        }
        daemon
            .shared
            .emit(
                &session,
                None,
                EventKind::TicketOpened {
                    ticket: Box::new(opened),
                },
            )
            .await
            .unwrap();
        let path = format!("/v1/tickets/{id}/resolve");
        for (router, token) in [
            (&tcp, Some(creator.as_str())),
            (&tcp, Some(other_agent.as_str())),
            (&local, None),
        ] {
            let redactors = daemon.shared.registry.redactors.read().len();
            let (status, refused) =
                authority_request(router, "POST", &path, json!({"value":value}), token).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{id}: {refused}");
            assert_eq!(refused["code"], "person_approval_required");
            assert!(!refused.to_string().contains("gate-canary-password"));
            assert_eq!(daemon.shared.registry.redactors.read().len(), redactors);
            assert_eq!(
                daemon.shared.db.state(&session).await.unwrap().tickets[&TicketId::new(id)].status,
                TicketStatus::Pending
            );
        }
        let (status, rejected) = authority_request(
            &tcp,
            "POST",
            &format!("/v1/tickets/{id}/reject"),
            json!({}),
            Some(&other_agent),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{id}: {rejected}");
        assert_eq!(rejected["status"], "rejected");
    }
    assert!(daemon
        .shared
        .db
        .state(&session)
        .await
        .unwrap()
        .driver_ids
        .is_empty());
    assert!(runtime
        .0
        .lock()
        .resolved
        .iter()
        .all(|(_, _, resolution)| !matches!(resolution, TicketResolution::Approved)));
    daemon.shutdown().await;
}

#[tokio::test]
async fn a_person_direct_tool_creator_still_needs_another_person_for_approval() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "direct-person", PolicyMode::Guard, None).await;
    let (_, creator) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Browse, CapabilityScope::Approve])
        .await
        .unwrap();
    let (_, other) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Approve])
        .await
        .unwrap();
    let tcp = api::router(daemon.shared.clone(), false);
    let (_, deferred) = authority_request(
        &tcp,
        "POST",
        &format!("/v1/sessions/{session}/tools/request_human_step"),
        json!({"args":{"kind":"confirm","summary":"Approve?"}}),
        Some(&creator),
    )
    .await;
    let id = deferred["ticket_id"].as_str().unwrap();
    let path = format!("/v1/tickets/{id}/resolve");
    let (status, refused) =
        authority_request(&tcp, "POST", &path, json!({"value":true}), Some(&creator)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert_eq!(refused["code"], "self_approval_forbidden");
    let (status, approved) =
        authority_request(&tcp, "POST", &path, json!({"value":true}), Some(&other)).await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert_eq!(approved["resolution"]["approved"], true);
    daemon.shutdown().await;
}
