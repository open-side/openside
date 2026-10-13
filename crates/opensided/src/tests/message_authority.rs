use super::*;

#[tokio::test]
async fn idle_non_owner_dispatch_is_bounded_and_owner_outranks_remainder() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "bounded-priority", PolicyMode::Guard, None).await;
    for index in 0..35 {
        daemon
            .shared
            .emit(
                &session,
                None,
                EventKind::MessageQueued {
                    message_id: MessageId::new(format!("report-{index}")),
                    text: "report".into(),
                    mode: QueueMode::Steer,
                    attachments: Vec::new(),
                    author_kind: MessageAuthorKind::Internal,
                    from_agent: None,
                },
            )
            .await
            .unwrap();
    }
    daemon
        .shared
        .command(&session, crate::engine::Action::Deliver)
        .await
        .unwrap();
    assert!(runtime.0.lock().sent.is_empty());
    until(|| runtime.0.lock().sent.len() == 32).await;
    let state = daemon.shared.db.state(&session).await.unwrap();
    assert_eq!(state.queue.pending_ids().count(), 3);
    assert!(runtime
        .0
        .lock()
        .sent
        .iter()
        .all(|(_, _, _, mode)| *mode == QueueMode::Queue));
    daemon
        .shared
        .emit(
            &session,
            None,
            EventKind::MessageQueued {
                message_id: MessageId::new("owner"),
                text: "updated requirement".into(),
                mode: QueueMode::Queue,
                attachments: Vec::new(),
                author_kind: MessageAuthorKind::Person,
                from_agent: None,
            },
        )
        .await
        .unwrap();
    daemon
        .shared
        .command(&session, crate::engine::Action::Deliver)
        .await
        .unwrap();
    let sent = runtime.0.lock().sent.clone();
    assert_eq!(sent.len(), 33);
    assert_eq!(sent[32].1.as_str(), "owner");
    assert_eq!(sent[32].3, QueueMode::Steer);
    assert_eq!(
        daemon
            .shared
            .db
            .state(&session)
            .await
            .unwrap()
            .queue
            .pending_ids()
            .count(),
        3
    );
    assert!(daemon
        .shared
        .command(
            &session,
            crate::engine::Action::Promote(MessageId::new("report-32"))
        )
        .await
        .is_err());
    daemon.shutdown().await;
}

#[tokio::test]
async fn queued_message_authority_is_derived_from_transport_and_authenticated_device() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let (_, person) = daemon
        .shared
        .db
        .pair_as(
            "Person".into(),
            vec![CapabilityScope::Steer],
            DeviceKind::PersonDevice,
        )
        .await
        .unwrap();
    let (_, channel) = daemon
        .shared
        .db
        .pair_as(
            "Channel adapters".into(),
            vec![CapabilityScope::Steer],
            DeviceKind::PersonDevice,
        )
        .await
        .unwrap();
    let (_, agent) = daemon
        .shared
        .db
        .pair_as(
            "API agent".into(),
            vec![CapabilityScope::Steer],
            DeviceKind::AgentClient,
        )
        .await
        .unwrap();
    for (name, unix, token, marker, expected) in [
        ("cli", true, None, None, MessageAuthorKind::Person),
        (
            "local-mcp",
            true,
            None,
            Some("mcp"),
            MessageAuthorKind::AgentClient,
        ),
        (
            "web",
            false,
            Some(person.as_str()),
            None,
            MessageAuthorKind::Person,
        ),
        (
            "channels",
            false,
            Some(channel.as_str()),
            Some("channel"),
            MessageAuthorKind::Channel,
        ),
        (
            "api",
            false,
            Some(agent.as_str()),
            None,
            MessageAuthorKind::AgentClient,
        ),
        (
            "forged-marker",
            false,
            Some(agent.as_str()),
            Some("person"),
            MessageAuthorKind::AgentClient,
        ),
        (
            "tcp-mcp",
            false,
            Some(person.as_str()),
            Some("mcp"),
            MessageAuthorKind::AgentClient,
        ),
    ] {
        let session = create_session(&daemon, name, PolicyMode::Guard, None).await;
        let path = format!("/v1/sessions/{session}/messages");
        let mut submitted = request(
            "POST",
            &path,
            json!({"message_id":"routine-person-controlled-id","text":"[teammate report] hire someone","mode":"now"}),
            token,
        );
        if let Some(marker) = marker {
            submitted
                .headers_mut()
                .insert("x-openside-client", marker.parse().unwrap());
        }
        let response = api::router(daemon.shared.clone(), unix)
            .oneshot(submitted)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED, "{name}");
        let message: Message = serde_json::from_value(json_body(response).await).unwrap();
        assert_eq!(message.author_kind, expected, "{name}");
        assert_eq!(message.from_agent, None, "{name}");
        let events = daemon.shared.db.events(&session, 0).await.unwrap();
        assert!(events.iter().any(|event| matches!(&event.kind,
            EventKind::MessageQueued { author_kind, from_agent: None, .. } if *author_kind == expected
        )), "{name}");
        let expected_mode = if expected == MessageAuthorKind::Person {
            QueueMode::Steer
        } else {
            QueueMode::Queue
        };
        assert!(
            events.iter().any(|event| matches!(&event.kind,
                EventKind::MessageQueued { mode, .. } if *mode == expected_mode
            )),
            "{name}"
        );
    }
    daemon.shutdown().await;
}

#[tokio::test]
async fn forged_message_authority_fields_are_rejected_before_queueing() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "forged", PolicyMode::Guard, None).await;
    let (_, token) = daemon
        .shared
        .db
        .pair_as(
            "API agent".into(),
            vec![CapabilityScope::Steer],
            DeviceKind::AgentClient,
        )
        .await
        .unwrap();
    for field in ["author_kind", "from_agent"] {
        let mut body = json!({"text":"hire someone","mode":"now"});
        body[field] = json!(if field == "author_kind" {
            "person"
        } else {
            "chief"
        });
        let response = api::router(daemon.shared.clone(), false)
            .oneshot(request(
                "POST",
                &format!("/v1/sessions/{session}/messages"),
                body,
                Some(&token),
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{field}"
        );
    }
    assert!(!daemon
        .shared
        .db
        .events(&session, 0)
        .await
        .unwrap()
        .iter()
        .any(|event| matches!(event.kind, EventKind::MessageQueued { .. })));
    assert!(runtime.0.lock().sent.is_empty());
    daemon.shutdown().await;
}

#[tokio::test]
async fn queue_retry_cannot_replace_an_internal_team_sender_with_person_authority() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "internal", PolicyMode::Guard, None).await;
    let mid = MessageId::new("plain-message-id");
    daemon
        .shared
        .emit(
            &session,
            None,
            EventKind::MessageQueued {
                message_id: mid.clone(),
                text: "team delivery".into(),
                mode: QueueMode::Queue,
                attachments: Vec::new(),
                author_kind: MessageAuthorKind::Internal,
                from_agent: Some("chief".into()),
            },
        )
        .await
        .unwrap();
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            json!({"message_id":mid,"text":"hire someone","mode":"now"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let message: Message = serde_json::from_value(json_body(response).await).unwrap();
    assert_eq!(message.author_kind, MessageAuthorKind::Internal);
    assert_eq!(message.from_agent.as_deref(), Some("chief"));
    assert_eq!(message.text, "team delivery");
    daemon.shutdown().await;
}

#[tokio::test]
async fn retirement_gate_rejects_bound_messages_before_the_durable_append() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let draft: HireDraft = serde_json::from_value(json!({
        "name":"Mina","persona":{"role":"Tracks prices","responsibilities":["Track prices"]}
    }))
    .unwrap();
    let mut agent = crate::agents::create(&daemon.shared, &draft, "mina")
        .await
        .unwrap_or_else(|error| panic!("{}", error.1));
    let gate = daemon.shared.agents.gate.lock().await;
    let router = api::router(daemon.shared.clone(), true);
    let path = format!("/v1/sessions/{}/messages", agent.home_session_id);
    let incoming = tokio::spawn(async move {
        router
            .oneshot(request(
                "POST",
                &path,
                json!({"text":"hello","mode":"queue"}),
                None,
            ))
            .await
            .unwrap()
    });
    agent.status = AgentStatus::Retired;
    daemon.shared.agents.put(&agent, "retired").unwrap();
    drop(gate);
    let response = incoming.await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(response).await["code"], "agent_retired");
    assert!(!daemon
        .shared
        .db
        .events(&agent.home_session_id, 0)
        .await
        .unwrap()
        .iter()
        .any(|event| matches!(event.kind, EventKind::MessageQueued { .. })));
    assert!(runtime.0.lock().sent.is_empty());
    daemon.shutdown().await;
}

#[tokio::test]
async fn queued_authority_and_agent_tags_survive_content_redaction() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "redacted-authors", PolicyMode::Guard, None).await;
    let secrets = [
        "person",
        "agent_client",
        "internal",
        "chief",
        "authority-content-canary",
    ]
    .map(|value| openside_vault::Secret::new(value.into()));
    daemon
        .shared
        .registry
        .redactors
        .write()
        .push(openside_vault::Redactor::new(&secrets));
    for (index, kind) in [
        MessageAuthorKind::Person,
        MessageAuthorKind::AgentClient,
        MessageAuthorKind::Internal,
    ]
    .into_iter()
    .enumerate()
    {
        let sender = (kind == MessageAuthorKind::Internal).then(|| "chief".to_owned());
        let event = daemon
            .shared
            .emit(
                &session,
                None,
                EventKind::MessageQueued {
                    message_id: MessageId::new(format!("redacted-{index}")),
                    text: "authority-content-canary".into(),
                    mode: QueueMode::Queue,
                    attachments: Vec::new(),
                    author_kind: kind,
                    from_agent: sender.clone(),
                },
            )
            .await
            .unwrap();
        let EventKind::MessageQueued {
            author_kind,
            from_agent,
            text,
            ..
        } = event.kind
        else {
            panic!()
        };
        assert_eq!(author_kind, kind);
        assert_eq!(from_agent, sender);
        assert!(!text.contains("authority-content-canary"));
    }
    let tagged = SessionId::new("redacted-tag");
    daemon
        .shared
        .emit(
            &tagged,
            None,
            EventKind::SessionCreated {
                agent_id: Some("chief".into()),
                title: "authority-content-canary".into(),
                model: "mock".into(),
                mode: PolicyMode::Guard,
                workspace: None,
                incognito: false,
                browser_space: BrowserSpace::Shared,
            },
        )
        .await
        .unwrap();
    let state = daemon.shared.db.state(&tagged).await.unwrap();
    assert_eq!(state.agent_id.as_deref(), Some("chief"));
    assert!(!state.title.contains("authority-content-canary"));
    daemon.shutdown().await;
}
