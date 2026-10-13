use super::*;

fn draft() -> HireDraft {
    serde_json::from_value(json!({"name":"Mina","persona":{"role":"Price researcher","voice":"Brief","responsibilities":["Track prices"],"boundaries":["Ask before purchases"]},"proposal_key":"price researcher"})).unwrap()
}

async fn propose(d: &Daemon, id: &str) -> Result<TicketId, crate::persistence::Error> {
    let chief = d.shared.agents.get("chief").unwrap().unwrap();
    let state = d.shared.db.state(&chief.home_session_id).await.unwrap();
    let ctx = crate::tools::ToolEnv::new(d.shared.clone(), &state);
    let crate::tools::ToolExec::Defer(request) = crate::tickets::propose_hire(&ctx, draft()).await
    else {
        panic!("expected proposal")
    };
    crate::tickets::open(
        &d.shared,
        &chief.home_session_id,
        &call("team_propose_hire", json!({}), id),
        TicketDeferral::External,
        *request,
        None,
        None,
        None,
    )
    .await
}

async fn post(d: &Daemon, path: &str, body: Value) -> (StatusCode, Value) {
    let (_, token) = d
        .shared
        .db
        .pair_as(
            "Person".into(),
            vec![CapabilityScope::Admin],
            DeviceKind::PersonDevice,
        )
        .await
        .unwrap();
    let response = api::router(d.shared.clone(), false)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    (status, value)
}

#[tokio::test]
async fn hire_approval_uses_edited_name_and_role_and_undo_restores_lifecycle() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let d = daemon(root.path(), &runtime).await;
    let id = propose(&d, "proposal").await.unwrap();
    let mut edited = draft();
    edited.name = "Nora".into();
    edited.persona.role = "Travel researcher".into();
    let (status, ticket) = post(
        &d,
        &format!("/v1/tickets/{id}/resolve"),
        json!({"value":"hire","hire":edited}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ticket}");
    let agent_id = ticket["resolution"]["agent_id"].as_str().unwrap();
    let agent = d.shared.agents.get(agent_id).unwrap().unwrap();
    assert_eq!(agent.name, "Nora");
    assert_eq!(agent.persona.role, "Travel researcher");
    assert_eq!(ticket["card"]["status"], "done");
    assert_eq!(ticket["card"]["actions"], json!([]));
    let (status, routine) = post(&d, "/v1/routines", json!({"name":"Fare watch","schedule":{"type":"interval","secs":3600},"action":{"type":"agent","agent_id":agent_id,"prompt":"Compare fares"}})).await;
    assert_eq!(status, StatusCode::CREATED, "{routine}");
    let routine_id = routine["id"].as_str().unwrap();
    let (status, retired) = post(
        &d,
        &format!("/v1/agents/{agent_id}/retire"),
        json!({"revert_hire":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{retired}");
    assert_eq!(retired["status"], "retired");
    assert!(
        d.shared
            .db
            .state(&agent.home_session_id)
            .await
            .unwrap()
            .archived
    );
    assert_eq!(
        d.shared
            .routines
            .get()
            .unwrap()
            .get(routine_id)
            .unwrap()
            .action
            .owner_agent_id(),
        Some("chief")
    );
    let (status, restored) = post(&d, &format!("/v1/agents/{agent_id}/restore"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{restored}");
    assert_eq!(restored["status"], "active");
}

#[tokio::test]
async fn hire_later_sets_thirty_day_cooldown() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let d = daemon(root.path(), &runtime).await;
    let id = propose(&d, "proposal").await.unwrap();
    assert!(propose(&d, "duplicate")
        .await
        .unwrap_err()
        .to_string()
        .contains("proposal_pending"));
    let (status, ticket) = post(
        &d,
        &format!("/v1/tickets/{id}/resolve"),
        json!({"value":"not_now"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ticket}");
    assert!(propose(&d, "global-cooldown")
        .await
        .unwrap_err()
        .to_string()
        .contains("proposal_global_cooldown"));
    d.shared
        .agents
        .put_proposal(
            "price researcher",
            "not_now",
            Utc::now() - chrono::Duration::days(8),
        )
        .unwrap();
    assert!(propose(&d, "cooldown")
        .await
        .unwrap_err()
        .to_string()
        .contains("proposal_cooldown"));
    d.shared
        .agents
        .put_proposal(
            "price researcher",
            "not_now",
            Utc::now() - chrono::Duration::days(31),
        )
        .unwrap();
    assert!(propose(&d, "after-cooldown").await.is_ok());
    assert_eq!(d.shared.agents.list().unwrap().len(), 1);
}

#[tokio::test]
async fn hire_never_stays_suppressed_until_person_clears_it() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let d = daemon(root.path(), &runtime).await;
    let id = propose(&d, "proposal").await.unwrap();
    let (status, _) = post(
        &d,
        &format!("/v1/tickets/{id}/resolve"),
        json!({"value":"never"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    d.shared
        .agents
        .put_proposal(
            "price researcher",
            "never",
            Utc::now() - chrono::Duration::days(365),
        )
        .unwrap();
    assert!(propose(&d, "suppressed")
        .await
        .unwrap_err()
        .to_string()
        .contains("proposal_suppressed"));
    let (_, reader) = d
        .shared
        .db
        .pair_as(
            "Reader".into(),
            vec![CapabilityScope::Read],
            DeviceKind::PersonDevice,
        )
        .await
        .unwrap();
    let response = api::router(d.shared.clone(), false)
        .oneshot(
            Request::builder()
                .uri("/v1/team/proposals/suppressed")
                .header("authorization", format!("Bearer {reader}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let list: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(list[0]["key"], "price researcher");
    assert_eq!(list[0]["label"], "Price researcher");
    assert_eq!(list[0]["status"], "never");
    for (kind, scope, expected) in [
        (
            DeviceKind::PersonDevice,
            CapabilityScope::Read,
            StatusCode::FORBIDDEN,
        ),
        (
            DeviceKind::AgentClient,
            CapabilityScope::Admin,
            StatusCode::FORBIDDEN,
        ),
        (
            DeviceKind::PersonDevice,
            CapabilityScope::Admin,
            StatusCode::NO_CONTENT,
        ),
    ] {
        let (_, token) = d
            .shared
            .db
            .pair_as("Caller".into(), vec![scope], kind)
            .await
            .unwrap();
        let response = api::router(d.shared.clone(), false)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/v1/team/proposals/price%20researcher")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        if expected == StatusCode::FORBIDDEN {
            assert_eq!(d.shared.agents.suppressed_proposals().unwrap().len(), 1);
        }
    }
    assert!(d.shared.agents.suppressed_proposals().unwrap().is_empty());
    assert!(propose(&d, "after-clear").await.is_ok());
}

#[tokio::test]
async fn hire_card_cannot_authorize_without_an_actual_ticket_and_draft() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let d = daemon(root.path(), &runtime).await;
    let (status, _) = post(
        &d,
        "/v1/tickets/fabricated/resolve",
        json!({"value":"hire","hire":draft()}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let chief = d.shared.agents.get("chief").unwrap().unwrap();
    opened(
        &d,
        &chief.home_session_id,
        TicketKind::Hire,
        "missing-draft",
        Utc::now() + chrono::Duration::hours(1),
    )
    .await;
    let (status, _) = post(
        &d,
        "/v1/tickets/missing-draft/resolve",
        json!({"value":"hire","hire":draft()}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(d.shared.agents.list().unwrap().len(), 1);
}

#[tokio::test]
async fn hire_cards_project_every_terminal_state_and_explicit_commands() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let d = daemon(root.path(), &runtime).await;
    let id = propose(&d, "proposal").await.unwrap();
    let mut ticket = crate::tickets::get(&d.shared, &id).await.unwrap().unwrap();
    let pending = crate::cards::ticket(&ticket);
    assert_eq!(pending.kind, CardKind::Hire);
    assert_eq!(pending.actions.len(), 3);
    assert!(pending.blocks.iter().any(|b| matches!(b, Block::InputFields { id, fields } if id == "hire-edit" && fields.iter().all(|f| !f.secret))));
    for (status, result, expected) in [
        (TicketStatus::Resolved, "hired", CardStatus::Done),
        (TicketStatus::Resolved, "declined", CardStatus::Done),
        (TicketStatus::Resolved, "later", CardStatus::Done),
        (TicketStatus::Rejected, "rejected", CardStatus::Failed),
        (TicketStatus::Expired, "expired", CardStatus::Expired),
    ] {
        ticket.status = status;
        ticket.resolution = Some(json!({"status":result}));
        let card = crate::cards::ticket(&ticket);
        assert_eq!(card.status, expected);
        assert!(card.actions.is_empty());
        assert!(!card
            .blocks
            .iter()
            .any(|b| matches!(b, Block::InputFields { .. })));
    }
    let chief = d.shared.agents.get("chief").unwrap().unwrap();
    let state = d.shared.db.state(&chief.home_session_id).await.unwrap();
    for explicit in [false, true] {
        let mut event = EventKind::TeamHired {
            agent_id: "mina".into(),
            name: "Mina".into(),
            ticket_id: (!explicit).then(|| id.to_string()),
            request_message_id: explicit.then(|| MessageId::new("person-hire")),
            card: None,
        };
        crate::cards::enrich(&mut event, &state);
        let EventKind::TeamHired {
            card: Some(card), ..
        } = event
        else {
            panic!("missing card")
        };
        assert_eq!(
            card.actions
                .iter()
                .filter(|a| matches!(
                    a.target,
                    ActionTarget::TeamCommand {
                        command: TeamCardCommand::Retire,
                        ..
                    }
                ))
                .count(),
            1
        );
        assert_eq!(
            card.actions
                .iter()
                .filter(|a| matches!(
                    a.target,
                    ActionTarget::TeamCommand {
                        command: TeamCardCommand::UndoHire,
                        ..
                    }
                ))
                .count(),
            usize::from(explicit)
        );
        assert!(!card
            .actions
            .iter()
            .any(|a| matches!(a.target, ActionTarget::FollowUp { .. })));
    }
}

#[tokio::test]
async fn hire_team_events_project_handoff_and_teammate_states_atomically() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let d = daemon(root.path(), &runtime).await;
    let chief = d.shared.agents.get("chief").unwrap().unwrap();
    let session = &chief.home_session_id;
    for kind in [
        EventKind::TeamHandoff {
            handoff_id: "work".into(),
            from_agent: "chief".into(),
            to_agent: "mina".into(),
            handoff_kind: HandoffKind::Task,
            goal: "Compare fares".into(),
            dedupe_key: "fares".into(),
            card: None,
        },
        EventKind::TeamQuestion {
            question_id: "question".into(),
            from_agent: "mina".into(),
            text: "Which dates?".into(),
            options: vec!["Next week".into()],
            card: None,
        },
        EventKind::TeamAnswered {
            question_id: "question".into(),
            card: None,
        },
        EventKind::TeamTransferred {
            from_agent: "chief".into(),
            to_agent: "mina".into(),
            routine_ids: vec!["routine".into()],
            responsibilities: vec!["Compare fares".into()],
            card: None,
        },
        EventKind::TeamRetired {
            agent_id: "mina".into(),
            successor: Some("chief".into()),
            card: None,
        },
        EventKind::TeamRestored {
            agent_id: "mina".into(),
            card: None,
        },
    ] {
        d.shared.emit(session, None, kind).await.unwrap();
    }
    let state = d.shared.db.state(session).await.unwrap();
    assert_eq!(state.cards["handoff:work"].status, CardStatus::Pending);
    assert_eq!(state.cards["question:question"].status, CardStatus::Done);
    assert_eq!(state.cards["teammate:mina"].title, "Teammate restored");
    for (status, expected) in [
        (HandoffStatus::Done, CardStatus::Done),
        (HandoffStatus::Failed, CardStatus::Failed),
        (HandoffStatus::Cancelled, CardStatus::Failed),
    ] {
        let mut event = EventKind::TeamHandoffEnded {
            handoff_id: "work".into(),
            status,
            summary: Some("Outcome".into()),
            card: None,
        };
        crate::cards::enrich(&mut event, &state);
        let EventKind::TeamHandoffEnded {
            card: Some(card), ..
        } = event
        else {
            panic!("missing projection")
        };
        assert_eq!(card.id, "handoff:work");
        assert_eq!(card.status, expected);
        assert!(card
            .blocks
            .iter()
            .any(|block| matches!(block, Block::People { .. })));
    }
    d.shared
        .emit(
            session,
            None,
            EventKind::TeamHandoffEnded {
                handoff_id: "work".into(),
                status: HandoffStatus::Done,
                summary: None,
                card: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        d.shared.db.state(session).await.unwrap().cards["handoff:work"].status,
        CardStatus::Done
    );
    let events = d.shared.db.events(session, 0).await.unwrap();
    assert!(events.iter().any(|event| matches!(&event.kind, EventKind::TeamHandoff { card:Some(card), .. } if card.status == CardStatus::Pending)));
    let legacy_session = session.clone();
    d.shared
        .db
        .store(move |store| {
            for _ in 0..2 {
                store.append(
                    &legacy_session,
                    None,
                    EventKind::TeamTransferred {
                        from_agent: "chief".into(),
                        to_agent: "mina".into(),
                        routine_ids: vec![],
                        responsibilities: vec![],
                        card: None,
                    },
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
    let mut previous = None;
    for _ in 0..2 {
        let ids: Vec<String> = d
            .shared
            .db
            .events(session, 0)
            .await
            .unwrap()
            .into_iter()
            .filter_map(|event| match event.kind {
                EventKind::TeamTransferred {
                    card: Some(card), ..
                } => Some(card.id),
                _ => None,
            })
            .collect();
        assert_eq!(ids.len(), 3);
        assert_eq!(
            ids.iter().collect::<std::collections::HashSet<_>>().len(),
            3
        );
        if let Some(previous) = previous {
            assert_eq!(ids, previous);
        }
        previous = Some(ids);
    }
}

#[tokio::test]
async fn hire_resolution_requires_person_approve_and_refuses_self_approval() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let d = daemon(root.path(), &runtime).await;
    let id = propose(&d, "proposal").await.unwrap();
    let chief = d.shared.agents.get("chief").unwrap().unwrap();
    for (kind, scopes, driver, code) in [
        (
            DeviceKind::AgentClient,
            vec![CapabilityScope::Approve],
            false,
            "person_approval_required",
        ),
        (
            DeviceKind::PersonDevice,
            vec![CapabilityScope::Read],
            false,
            "forbidden",
        ),
        (
            DeviceKind::PersonDevice,
            vec![CapabilityScope::Approve],
            true,
            "self_approval_forbidden",
        ),
    ] {
        let (device, token) = d
            .shared
            .db
            .pair_as("Caller".into(), scopes, kind)
            .await
            .unwrap();
        if driver {
            d.shared
                .emit(
                    &chief.home_session_id,
                    None,
                    EventKind::SessionDriverRecorded { device_id: device },
                )
                .await
                .unwrap();
        }
        let response = api::router(d.shared.clone(), false)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/tickets/{id}/resolve"))
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(json!({"value":"hire"}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let value: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(value["code"], code);
    }
    assert_eq!(d.shared.agents.list().unwrap().len(), 1);
    assert_eq!(
        crate::tickets::get(&d.shared, &id)
            .await
            .unwrap()
            .unwrap()
            .status,
        TicketStatus::Pending
    );
}

#[test]
fn hire_suppression_migrates_legacy_labels_and_clears_with_audit() {
    let root = tempfile::tempdir().unwrap();
    let db = rusqlite::Connection::open(root.path().join("agents.db")).unwrap();
    db.execute_batch("CREATE TABLE team_proposals(key TEXT PRIMARY KEY,status TEXT NOT NULL,decided_at TEXT NOT NULL);
        CREATE TABLE agent_audit(seq INTEGER PRIMARY KEY AUTOINCREMENT,agent_id TEXT NOT NULL,operation TEXT NOT NULL,at TEXT NOT NULL);
        INSERT INTO team_proposals VALUES('legacy','never','2025-01-01T00:00:00Z');
        PRAGMA user_version=2;").unwrap();
    drop(db);
    let store = crate::agents::Service::open(root.path()).unwrap();
    let rows = store.suppressed_proposals().unwrap();
    assert_eq!(rows[0].key, "legacy");
    assert_eq!(rows[0].label, None);
    store.clear_proposal("legacy").unwrap();
    store.clear_proposal("legacy").unwrap();
    assert!(store.suppressed_proposals().unwrap().is_empty());
    let db = rusqlite::Connection::open(root.path().join("agents.db")).unwrap();
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        4
    );
    assert_eq!(db.query_row("SELECT count(*) FROM agent_audit WHERE agent_id='chief' AND operation='proposal_cleared'", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
}

#[tokio::test]
async fn hire_lifecycle_routes_transfer_work_and_delegate_from_chief() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let d = daemon(root.path(), &runtime).await;
    let (status, first) = post(&d, "/v1/agents", json!(draft())).await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    let mut second_draft = draft();
    second_draft.name = "Nora".into();
    let (status, second) = post(&d, "/v1/agents", json!(second_draft)).await;
    assert_eq!(status, StatusCode::CREATED, "{second}");
    let from = first["id"].as_str().unwrap();
    let to = second["id"].as_str().unwrap();
    let (status, routine) = post(&d, "/v1/routines", json!({"name":"Price watch","schedule":{"type":"interval","secs":3600},"action":{"type":"agent","agent_id":from,"prompt":"Compare prices"}})).await;
    assert_eq!(status, StatusCode::CREATED, "{routine}");
    let routine_id = routine["id"].as_str().unwrap();
    for _ in 0..2 {
        let (status, value) = post(
            &d,
            &format!("/v1/agents/{from}/transfer"),
            json!({"to":to,"routine_ids":[routine_id]}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }
    assert_eq!(
        d.shared
            .routines
            .get()
            .unwrap()
            .get(routine_id)
            .unwrap()
            .action
            .owner_agent_id(),
        Some(to)
    );
    let (status, handoff) = post(&d, &format!("/v1/agents/{to}/handoffs"), json!({"kind":"task","packet":{"goal":"Compare prices","context":"Current list","acceptance":"Return three prices","constraints":"Do not purchase"},"dedupe_key":"price-work"})).await;
    assert_eq!(status, StatusCode::OK, "{handoff}");
    assert_eq!(handoff["from_agent"], "chief");
    assert_eq!(handoff["to_agent"], to);
    let response = api::router(d.shared.clone(), true)
        .oneshot(
            Request::builder()
                .uri(format!("/v1/agents/{to}/handoffs"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let listed: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(listed["handoffs"][0]["id"], handoff["id"]);
}

#[tokio::test]
async fn hire_after_page_output_requires_ticket_and_bypasses_only_cooldown() {
    use crate::tools::DaemonTool;
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let d = daemon(root.path(), &runtime).await;
    let chief = d.shared.agents.get("chief").unwrap().unwrap();
    let session = &chief.home_session_id;
    d.shared
        .emit(
            session,
            None,
            EventKind::MessageQueued {
                message_id: MessageId::new("hire-request"),
                text: "Hire a price researcher".into(),
                mode: QueueMode::Queue,
                attachments: vec![],
                author_kind: MessageAuthorKind::Person,
                from_agent: None,
            },
        )
        .await
        .unwrap();
    d.shared
        .emit(
            session,
            None,
            EventKind::ToolEnded {
                tool_call_id: "page-read".into(),
                result: json!({"text":"Untrusted page content"}),
                is_error: false,
                card: None,
            },
        )
        .await
        .unwrap();
    d.shared
        .agents
        .suppress_proposal("price researcher", "Price researcher", Utc::now())
        .unwrap();
    let state = d.shared.db.state(session).await.unwrap();
    let ctx = crate::tools::ToolEnv::new(d.shared.clone(), &state);
    let args = json!({"draft":draft(),"request_message_id":"hire-request"});
    let crate::tools::ToolExec::Defer(request) =
        crate::team::TeamTool::Hire.call(args.clone(), &ctx).await
    else {
        panic!("tainted hire must defer")
    };
    assert_eq!(d.shared.agents.list().unwrap().len(), 1);
    let id = crate::tickets::open(
        &d.shared,
        session,
        &call("team_hire", args.clone(), "tainted-hire"),
        TicketDeferral::External,
        *request,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let ticket = crate::tickets::get(&d.shared, &id).await.unwrap().unwrap();
    assert_eq!(ticket.kind, TicketKind::Hire);
    assert_eq!(ticket.status, TicketStatus::Pending);
    assert_eq!(ticket.card.unwrap().kind, CardKind::Hire);
    let crate::tools::ToolExec::Defer(request) =
        crate::team::TeamTool::Hire.call(args.clone(), &ctx).await
    else {
        panic!("expected deferred hire")
    };
    assert!(crate::tickets::open(
        &d.shared,
        session,
        &call("team_hire", args, "second-hire"),
        TicketDeferral::External,
        *request,
        None,
        None,
        None
    )
    .await
    .unwrap_err()
    .to_string()
    .contains("proposal_pending"));
    assert_eq!(d.shared.agents.list().unwrap().len(), 1);
}

#[tokio::test]
async fn hire_canonical_topics_and_global_backstop_prevent_renamed_proposals() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let d = daemon(root.path(), &runtime).await;
    let id = propose(&d, "original").await.unwrap();
    assert_eq!(
        post(
            &d,
            &format!("/v1/tickets/{id}/resolve"),
            json!({"value":"never"})
        )
        .await
        .0,
        StatusCode::OK
    );
    let chief = d.shared.agents.get("chief").unwrap().unwrap();
    let state = d.shared.db.state(&chief.home_session_id).await.unwrap();
    let ctx = crate::tools::ToolEnv::new(d.shared.clone(), &state);
    for (role, expected) in [
        ("The ＰＲＩＣＥ---Researcher", "proposal_suppressed"),
        ("Fare analyst", "proposal_global_cooldown"),
    ] {
        let mut changed = draft();
        changed.proposal_key = Some("fresh-model-key".into());
        changed.persona.role = role.into();
        let crate::tools::ToolExec::Defer(request) =
            crate::tickets::propose_hire(&ctx, changed).await
        else {
            panic!("expected draft")
        };
        let error = crate::tickets::open(
            &d.shared,
            &chief.home_session_id,
            &call("team_propose_hire", json!({}), role),
            TicketDeferral::External,
            *request,
            None,
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
    d.shared
        .agents
        .put_proposal(
            "price researcher",
            "never",
            Utc::now() - chrono::Duration::days(8),
        )
        .unwrap();
    let mut different = draft();
    different.persona.role = "Fare analyst".into();
    let crate::tools::ToolExec::Defer(request) =
        crate::tickets::propose_hire(&ctx, different).await
    else {
        panic!("expected draft")
    };
    assert!(crate::tickets::open(
        &d.shared,
        &chief.home_session_id,
        &call("team_propose_hire", json!({}), "after-global"),
        TicketDeferral::External,
        *request,
        None,
        None,
        None
    )
    .await
    .is_ok());
}

#[test]
fn hire_legacy_topic_key_migration_is_atomic_and_one_time() {
    let root = tempfile::tempdir().unwrap();
    let store = crate::agents::Service::open(root.path()).unwrap();
    store
        .suppress_proposal("old-key", "Price researcher", Utc::now())
        .unwrap();
    store
        .suppress_proposal("unresolved-key", "Unknown", Utc::now())
        .unwrap();
    store
        .migrate_proposal_keys(&[("old-key".into(), "price researcher".into())])
        .unwrap();
    assert!(store.get_proposal("old-key").unwrap().is_none());
    assert_eq!(
        store.get_proposal("price researcher").unwrap().unwrap().0,
        "never"
    );
    assert!(store.get_proposal("unresolved-key").unwrap().is_some());
    store
        .migrate_proposal_keys(&[("price researcher".into(), "attacker-key".into())])
        .unwrap();
    assert!(store.get_proposal("price researcher").unwrap().is_some());
    assert!(store.get_proposal("attacker-key").unwrap().is_none());
}

#[tokio::test]
async fn hire_after_agent_client_message_is_a_pending_card_not_an_agent() {
    use crate::tools::DaemonTool;
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let d = daemon(root.path(), &runtime).await;
    let chief = d.shared.agents.get("chief").unwrap().unwrap();
    for (id, author) in [
        ("person-request", MessageAuthorKind::Person),
        ("agent-followup", MessageAuthorKind::AgentClient),
    ] {
        d.shared
            .emit(
                &chief.home_session_id,
                None,
                EventKind::MessageQueued {
                    message_id: MessageId::new(id),
                    text: "Hire a researcher".into(),
                    mode: QueueMode::Queue,
                    attachments: vec![],
                    author_kind: author,
                    from_agent: None,
                },
            )
            .await
            .unwrap();
    }
    let state = d.shared.db.state(&chief.home_session_id).await.unwrap();
    let ctx = crate::tools::ToolEnv::new(d.shared.clone(), &state);
    let args = json!({"draft":draft(),"request_message_id":"person-request"});
    let crate::tools::ToolExec::Defer(request) =
        crate::team::TeamTool::Hire.call(args.clone(), &ctx).await
    else {
        panic!("agent follow-up must require approval")
    };
    let id = crate::tickets::open(
        &d.shared,
        &chief.home_session_id,
        &call("team_hire", args, "client-tainted-hire"),
        TicketDeferral::External,
        *request,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        crate::tickets::get(&d.shared, &id)
            .await
            .unwrap()
            .unwrap()
            .status,
        TicketStatus::Pending
    );
    assert_eq!(d.shared.agents.list().unwrap().len(), 1);
}
