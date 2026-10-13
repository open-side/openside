use super::*;

const CANARY: &str = "person-only-address@example.test";

#[tokio::test]
async fn person_only_ticket_cards_filter_rest_poll_sse_and_relay() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let session = create_session(&daemon, "privacy", PolicyMode::Guard, None).await;
    let ticket: Ticket = serde_json::from_value(json!({
        "id":"private-ticket","session_id":session,"tool_call_id":"private-call",
        "deferral":"external","kind":"login","summary":"Sign in","instructions":null,
        "options":[],"origin":{"subagent_id":null,"device_id":null,"driver_ids":[]},
        "tool":null,"target":{"tab_id":"tab","origin":"https://example.test","field_ref":"field"},
        "resume_when":null,"created_at":"2026-01-01T00:00:00Z","expires_at":"2030-01-01T00:00:00Z",
        "status":"pending","resolution":null,
        "login":{"origin":"https://example.test","site_name":"Example","fields":[{"id":"email","kind":"identifier","label":"Email","suggestions":[CANARY],"masked":false,"required":true}],"alternatives":[],"accounts":[{"item_id":"account-id","username_hint":CANARY}],"submit_label":"Continue","can_save":false}
    })).unwrap();
    // Store a genuinely old event; the outbound path must filter after historical hydration.
    let sid = session.clone();
    daemon
        .shared
        .db
        .store(move |store| {
            store.append(
                &sid,
                None,
                EventKind::TicketOpened {
                    ticket: Box::new(ticket),
                },
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let events = daemon.shared.db.events(&session, 0).await.unwrap();
    let card = events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::TicketOpened { ticket } => ticket.card.clone(),
            _ => None,
        })
        .unwrap();
    daemon
        .shared
        .emit(
            &session,
            None,
            EventKind::CardPresented {
                card,
                tool_call_id: "private-presentation".into(),
            },
        )
        .await
        .unwrap();
    let last_seq = daemon.shared.db.state(&session).await.unwrap().last_seq;
    for kind in [DeviceKind::AgentClient, DeviceKind::PersonDevice] {
        let (_, token) = daemon
            .shared
            .db
            .pair_as("privacy-test".into(), vec![CapabilityScope::Read], kind)
            .await
            .unwrap();
        for relay in [false, true] {
            let router = if relay {
                api::router(daemon.shared.clone(), false)
                    .layer(axum::Extension(crate::remote::RelayOrigin))
            } else {
                api::router(daemon.shared.clone(), false)
            };
            for path in [
                "/v1/tickets",
                "/v1/tickets/private-ticket",
                "/v1/sessions/privacy/events/poll?after=0&wait=0",
            ] {
                let response = router
                    .clone()
                    .oneshot(request("GET", path, Value::Null, Some(&token)))
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK, "{path}");
                let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap();
                let text = std::str::from_utf8(&bytes).unwrap();
                assert_eq!(
                    text.contains(CANARY),
                    kind == DeviceKind::PersonDevice,
                    "{path}: {text}"
                );
            }
            let response = router
                .oneshot(request(
                    "GET",
                    "/v1/sessions/privacy/events",
                    Value::Null,
                    Some(&token),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let mut body = response.into_body();
            let text = tokio::time::timeout(Duration::from_secs(5), async {
                let mut text = String::new();
                while let Some(frame) = body.frame().await {
                    if let Ok(bytes) = frame.unwrap().into_data() {
                        text.push_str(std::str::from_utf8(&bytes).unwrap());
                    }
                    if text.contains(&format!("id: {last_seq}\n")) {
                        break;
                    }
                }
                text
            })
            .await
            .unwrap();
            assert_eq!(
                text.contains(CANARY),
                kind == DeviceKind::PersonDevice,
                "SSE: {text}"
            );
        }
    }
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request(
            "GET",
            "/v1/tickets/private-ticket",
            Value::Null,
            None,
        ))
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(std::str::from_utf8(&bytes).unwrap().contains(CANARY));
    let durable = daemon.shared.db.events(&session, 0).await.unwrap();
    assert!(
        serde_json::to_string(&durable).unwrap().contains(CANARY),
        "projection must not destroy person data"
    );
    daemon.shutdown().await;
}

#[test]
fn person_only_marker_recurses_into_form_cards_and_terminal_events() {
    let caller = api::Caller(Some("agent".into()), DeviceKind::AgentClient);
    for root in ["Ticket", "Card", "Event"] {
        let card = json!({"blocks":[{"type":"input_fields","fields":[{"suggestions":[CANARY],"initial":CANARY}]}]});
        let mut value = match root {
            "Ticket" => {
                json!({"form":{"fields":[{"suggestions":[CANARY],"initial":CANARY}]},"card":card})
            }
            "Card" => card,
            _ => json!({"kind":"ticket.resolved","card":card}),
        };
        crate::person_only::strip_person_only(&mut value, root, &caller);
        assert!(!value.to_string().contains(CANARY), "{root}: {value}");
    }
}
