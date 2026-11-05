use super::*;
use crate::tools::DaemonTool;

fn legacy_ticket(kind: TicketKind) -> Ticket {
    serde_json::from_value(json!({"id":"ticket-card","session_id":"cards","tool_call_id":"card-call","deferral":"external","kind":kind,"summary":"A human step","instructions":null,"options":[],"origin":{"subagent_id":null,"device_id":null,"driver_ids":[]},"tool":null,"target":{"tab_id":"tab","origin":"https://example.test","field_ref":"field"},"resume_when":null,"created_at":"2026-01-01T00:00:00Z","expires_at":"2030-01-01T00:00:00Z","status":"pending","resolution":null})).unwrap()
}
fn draft() -> Value {
    json!({"card":{"id":"","kind":"table","site":{"origin":"https://example.test","name":"Example","favicon_ref":null},"title":"Results","subtitle":null,"status":"done","blocks":[{"type":"table","id":"table","columns":[{"id":"name","label":"Name"}],"rows":[[{"type":"text","text":"Example"}]],"layout":"grid"}],"actions":[],"interaction":"none","collapsed_summary":"One result"},"images":[]})
}

#[tokio::test]
async fn cards_public_trending_table_ignores_cookie_preferences_but_rejects_credentials() {
    let root = tempfile::tempdir().unwrap();
    let daemon = Daemon::open_runtime(resolved_config(root.path()), TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "trending", PolicyMode::Full, Some(root.path())).await;
    let state = daemon.shared.db.state(&session).await.unwrap();
    let env = crate::tools::ToolEnv::new(daemon.shared.clone(), &state);
    let mut input = draft();
    input["card"]["site"] = json!({"name":"GitHub","origin":"https://github.com"});
    input["card"]["title"] = json!("Trending repositories");
    input["card"]["collapsed_summary"] = json!("Two public repositories");
    input["card"]["blocks"] = json!([{
        "type":"table","id":"trending","layout":"grid",
        "columns":[{"id":"repo","label":"Repository"},{"id":"language","label":"Language"},{"id":"stars","label":"Stars"}],
        "rows":[
            [{"type":"link","link":{"label":"mksglu/context-mode","url":"https://github.com/mksglu/context-mode"}},{"type":"text","text":"TypeScript"},{"type":"text","text":"12,345"}],
            [{"type":"link","link":{"label":"vercel-labs/skills","url":"https://github.com/vercel-labs/skills"}},{"type":"text","text":"JavaScript"},{"type":"text","text":"98,765"}]
        ]
    }]);
    input["card"]["actions"] = json!([{"id":"open","label":"View trending","style":"primary","target":{"type":"link","url":"https://github.com/trending"}}]);
    // The former all-cookie filter rejected even the mandatory interaction="none".
    let preference = openside_vault::Secret::new("no".into());
    assert_ne!(
        openside_vault::Redactor::new([&preference]).redact("none"),
        "none"
    );
    for value in [
        "no",
        "true",
        "1",
        "light",
        "Asia/Seoul",
        "session-cookie-canary-58f29d04a1",
    ] {
        if let Some(filter) = openside_vault::Redactor::for_cookie(value) {
            daemon.shared.registry.redactors.write().push(filter);
        }
    }
    assert!(crate::tools::present::admit(&input, Some(&env)).is_ok());
    for (value, reason) in [
        ("session-cookie-canary-58f29d04a1", "known secret value"),
        ("sk-1234567890abcdefghijklmnop", "secret-shaped content"),
        (
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ1c2VyIn0.signature123456",
            "secret-shaped content",
        ),
    ] {
        let mut secret_input = input.clone();
        secret_input["card"]["blocks"][0]["rows"][1][1]["text"] = json!(value);
        let error = crate::tools::present::admit(&secret_input, Some(&env))
            .unwrap_err()
            .0;
        assert!(
            error.contains("$.card.blocks[0].rows[1][1].text"),
            "{error}"
        );
        assert!(error.contains(reason), "{error}");
        assert!(!error.contains(value));
    }
    daemon.shutdown().await;
}
#[test]
fn cards_project_every_current_ticket_kind_and_legacy_metadata() {
    for (kind, expected) in [
        (TicketKind::Confirm, CardKind::Confirmation),
        (TicketKind::SecretInput, CardKind::SecretInput),
        (TicketKind::DeviceApproval, CardKind::Confirmation),
        (TicketKind::Info, CardKind::Form),
        (TicketKind::Takeover, CardKind::Takeover),
        (TicketKind::Login, CardKind::SecureLogin),
        (TicketKind::Choice, CardKind::LoginChoice),
    ] {
        let source = legacy_ticket(kind);
        assert!(source.card.is_none());
        assert!(source.form.is_none());
        let card = crate::cards::ticket(&source);
        assert_eq!(card.kind, expected);
        assert_eq!(card.id, "ticket:ticket-card");
        let wire = serde_json::to_value(&card).unwrap();
        assert_eq!(serde_json::from_value::<Card>(wire).unwrap(), card);
    }
}
#[test]
fn cards_lifecycle_receipts_never_reopen_inputs() {
    let mut ticket = legacy_ticket(TicketKind::SecretInput);
    ticket.otp = Some(TicketOtp {
        channel: OtpChannel::Sms,
        can_register: false,
    });
    assert_eq!(crate::cards::ticket(&ticket).kind, CardKind::Otp);
    for (status, expected) in [
        (TicketStatus::Resolved, CardStatus::Done),
        (TicketStatus::Rejected, CardStatus::Failed),
        (TicketStatus::Expired, CardStatus::Expired),
    ] {
        ticket.status = status;
        ticket.resolution = Some(json!({"status":"filled"}));
        let card = crate::cards::ticket(&ticket);
        assert_eq!(card.status, expected);
        assert!(card.actions.is_empty());
        assert!(!card
            .blocks
            .iter()
            .any(|b| matches!(b, Block::InputFields { .. } | Block::ChoiceOptions { .. })));
    }
    ticket.status = TicketStatus::Resolved;
    ticket.resolution = Some(json!({"status":"indeterminate"}));
    assert_eq!(crate::cards::ticket(&ticket).status, CardStatus::Failed);
}
#[test]
fn cards_push_approval_cannot_claim_completion_and_missing_target_is_safe() {
    let mut ticket = legacy_ticket(TicketKind::DeviceApproval);
    ticket.approval = Some(TicketApproval {
        origin: "https://example.test".into(),
        site_name: "Example".into(),
        icon: None,
        message: "Approve on your phone".into(),
        number: Some("42".into()),
    });
    let card = crate::cards::ticket(&ticket);
    assert_eq!(card.actions.len(), 1);
    assert!(matches!(card.actions[0].target, ActionTarget::TicketReject));
    ticket = legacy_ticket(TicketKind::SecretInput);
    ticket.target = None;
    let card = crate::cards::ticket(&ticket);
    assert!(!card
        .blocks
        .iter()
        .any(|b| matches!(b, Block::InputFields { .. })));
}
#[test]
fn cards_present_admission_limits_secrets_and_urls() {
    let good = draft();
    assert!(crate::tools::present::admit(&good, None).is_ok());
    for (path, value, code) in [
        ("/card/title", json!("x".repeat(161)), "card_too_large"),
        (
            "/card/title",
            json!("4111 1111 1111 1111"),
            "card_secret_detected",
        ),
        ("/card/kind", json!("secure_login"), "invalid_card"),
        ("/card/ticket_id", json!("forged"), "invalid_card"),
        (
            "/card/blocks",
            json!([{"type":"input_fields","id":"fields","fields":[]}]),
            "card_action_forbidden",
        ),
        (
            "/card/actions",
            json!([{"id":"resolve","label":"Approve","style":"primary","target":{"type":"ticket_reject"}}]),
            "card_action_forbidden",
        ),
        (
            "/card/actions",
            json!([{"id":"link","label":"Open","style":"secondary","target":{"type":"link","url":"javascript:alert(1)"}}]),
            "invalid_card",
        ),
        (
            "/card/actions",
            json!([{"id":"link","label":"Open","style":"secondary","target":{"type":"link","url":"https://example.test/?token=private"}}]),
            "card_secret_detected",
        ),
        (
            "/card/site/favicon_ref",
            json!({"type":"data","data_url":"data:image/svg+xml;base64,AA=="}),
            "invalid_card",
        ),
    ] {
        let mut input = good.clone();
        if path == "/card/ticket_id" {
            input["card"]["ticket_id"] = value;
        } else {
            *input.pointer_mut(path).unwrap() = value;
        }
        let err = crate::tools::present::admit(&input, None).err().unwrap().0;
        assert!(err.starts_with(code), "{path}: {err}");
        assert!(!err.contains("4111"));
    }
    let mut bad = good.clone();
    bad["card"]["extra"] = json!(true);
    assert!(crate::tools::present::admit(&bad, None).is_err());
    let mut bad = good.clone();
    bad["card"]["blocks"][0]["rows"] =
        json!([[{"type":"text","text":"a"},{"type":"text","text":"b"}]]);
    assert!(crate::tools::present::admit(&bad, None).is_err());
    assert_eq!(
        crate::tools::present::Present.redact_args(&good),
        json!({"kind":"table","blocks":1,"actions":0,"images":0})
    );
}
#[tokio::test]
async fn cards_present_replay_and_known_secret_canary() {
    let root = tempfile::tempdir().unwrap();
    let daemon = Daemon::open_runtime(resolved_config(root.path()), TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "cards", PolicyMode::Full, Some(root.path())).await;
    let state = daemon.shared.db.state(&session).await.unwrap();
    let mut env = crate::tools::ToolEnv::new(daemon.shared.clone(), &state);
    env.tool_call_id = "presentation".into();
    let tool = crate::tools::present::Present;
    for _ in 0..2 {
        assert!(matches!(
            tool.call(draft(), &env).await,
            crate::tools::ToolExec::Done { .. }
        ));
    }
    let events = daemon.shared.db.events(&session, 0).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::CardPresented { .. }))
            .count(),
        1
    );
    let mut input = draft();
    input["card"]["title"] = json!("4111 1111 1111 1111");
    assert!(matches!(
        tool.call(input, &env).await,
        crate::tools::ToolExec::Failed(_)
    ));
    let secret = openside_vault::Secret::new("canary-private-field-9281".into());
    daemon
        .shared
        .registry
        .redactors
        .write()
        .push(openside_vault::Redactor::new([&secret]));
    let mut input = draft();
    input["card"]["title"] = json!("canary-private-field-9281");
    assert!(matches!(
        tool.call(input, &env).await,
        crate::tools::ToolExec::Failed(_)
    ));
    let logged =
        serde_json::to_string(&daemon.shared.db.events(&session, 0).await.unwrap()).unwrap();
    assert!(!logged.contains("4111"));
    assert!(!logged.contains("canary-private-field-9281"));
    daemon.shutdown().await;
    drop(env);
    drop(daemon);
    let restarted = Daemon::open_runtime(resolved_config(root.path()), TestRuntime::default())
        .await
        .unwrap();
    let state = restarted.shared.db.state(&session).await.unwrap();
    let mut env = crate::tools::ToolEnv::new(restarted.shared.clone(), &state);
    env.tool_call_id = "presentation".into();
    assert!(matches!(
        tool.call(draft(), &env).await,
        crate::tools::ToolExec::Done { .. }
    ));
    let replay = restarted.shared.db.events(&session, 0).await.unwrap();
    assert_eq!(
        replay
            .iter()
            .filter(|e| matches!(e.kind, EventKind::CardPresented { .. }))
            .count(),
        1
    );
    restarted.shutdown().await;
}

#[tokio::test]
async fn cards_atomic_terminal_projection_and_historical_hydration() {
    let root = tempfile::tempdir().unwrap();
    let daemon = Daemon::open_runtime(resolved_config(root.path()), TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "cards", PolicyMode::Full, Some(root.path())).await;
    let mut source = legacy_ticket(TicketKind::Confirm);
    source.session_id = session.clone();
    source.deferral = TicketDeferral::Approval;
    source.tool = Some(TicketTool {
        name: "browser_act".into(),
        args: json!({}),
    });
    let original = source.clone();
    let sid = session.clone();
    // Bypass the new append adapter only to model a genuine old log.
    daemon
        .shared
        .db
        .store(move |store| {
            store.append(
                &sid,
                None,
                EventKind::TicketOpened {
                    ticket: Box::new(original),
                },
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let replay = daemon.shared.db.events(&session, 0).await.unwrap();
    assert!(replay.iter().any(|e| matches!(&e.kind, EventKind::TicketOpened { ticket } if ticket.card.as_ref().is_some_and(|c| c.status == CardStatus::Pending))));
    let event = daemon
        .shared
        .emit(
            &session,
            None,
            EventKind::TicketResolved {
                ticket_id: source.id.clone(),
                resolution: json!({"approved":true}),
                card: None,
            },
        )
        .await
        .unwrap();
    assert!(
        matches!(event.kind, EventKind::TicketResolved { card: Some(card), .. } if card.status == CardStatus::Pending && card.actions.is_empty())
    );
    let event = daemon
        .shared
        .emit(
            &session,
            None,
            EventKind::ToolEnded {
                tool_call_id: source.tool_call_id,
                result: json!({"status":"done"}),
                is_error: false,
                card: None,
            },
        )
        .await
        .unwrap();
    assert!(
        matches!(event.kind, EventKind::ToolEnded { card: Some(card), .. } if card.id == "ticket:ticket-card" && card.kind == CardKind::Activity && card.status == CardStatus::Done)
    );
    let stored = daemon.shared.db.state(&session).await.unwrap();
    assert_eq!(
        stored.tickets[&source.id].card.as_ref().unwrap().status,
        CardStatus::Done
    );
    daemon.shutdown().await;
}

#[test]
fn cards_catalog_schema_resolves_definitions_and_restricts_drafts() {
    let spec = crate::tools::present::Present.spec();
    assert!(
        spec.parameters_json.len() < 12_000,
        "model schema must stay compact"
    );
    assert!(!spec.parameters_json.contains("x-person-only"));
    assert!(!spec.parameters_json.contains("TicketAnswer"));
    assert!(!spec.parameters_json.contains("InputField"));
    let schema: Value = serde_json::from_str(&spec.parameters_json).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(validator.is_valid(&draft()));
    for kind in [
        "product_list",
        "table",
        "place_list",
        "order_summary",
        "payment",
        "form",
        "connect_account",
    ] {
        let mut input = draft();
        input["card"]["kind"] = json!(kind);
        assert!(validator.is_valid(&input), "{kind}");
    }
    for kind in [
        "secure_login",
        "otp",
        "payment_card",
        "hire",
        "handoff",
        "activity",
    ] {
        let mut input = draft();
        input["card"]["kind"] = json!(kind);
        assert!(!validator.is_valid(&input), "{kind}");
    }
    let mut input = draft();
    input["card"]["actions"] = json!([{"id":"reject","label":"Reject","style":"secondary","target":{"type":"ticket_reject"}}]);
    assert!(!validator.is_valid(&input));
    let mut input = draft();
    input["card"]["blocks"][0]["row_sites"] = json!([{"origin":"https://example.test","name":"Example","favicon_ref":{"type":"origin","origin":"https://example.test"}}]);
    assert!(!validator.is_valid(&input));
    let mut input = draft();
    input["card"]["blocks"] = json!([{"type":"list_items","id":"list","items":[{"id":"item","name":"Item","facts":[],"image":null,"image_alt":null,"price":null,"rating":null,"link":null,"quantity":null,"site":{"origin":"https://example.test","name":"Example","favicon_ref":{"type":"origin","origin":"https://example.test"}}}]}]);
    assert!(!validator.is_valid(&input));
}

fn about(card: &Card, id: &str) -> Vec<(String, String)> {
    card.blocks
        .iter()
        .find_map(|b| match b {
            Block::KvRows {
                id: block, rows, ..
            } if block == id => Some(
                rows.iter()
                    .map(|r| match &r.value {
                        CardValue::Text { text } => (r.label.clone(), text.clone()),
                        other => panic!("unexpected value {other:?}"),
                    })
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

#[test]
fn cards_pending_asks_explain_target_consequence_and_deadline() {
    let mut login = legacy_ticket(TicketKind::Login);
    login.login = Some(serde_json::from_value(json!({"origin":"https://example.test","site_name":"Example","icon":null,"fields":[{"id":"f0","kind":"identifier","label":"Email","masked":false,"required":true,"autocomplete":"email","placeholder":null,"suggestions":[]},{"id":"f1","kind":"password","label":"Password","masked":true,"required":true,"autocomplete":"current-password","placeholder":null,"suggestions":[]}],"alternatives":[{"id":"a0","label":"Continue with Google","kind":"provider","provider":"google"}],"submit_label":"Sign in","can_save":false,"accounts":null,"error_text":null})).unwrap());
    login.instructions = Some(format!(
        "Enter your sign-in details for example.test.{}The invoice is behind the sign-in",
        crate::tools::login::WHY_REQUESTED
    ));
    let card = crate::cards::ticket(&login);
    assert_eq!(card.expires_at, Some(login.expires_at));
    let rows = about(&card, "about");
    assert_eq!(rows[0], ("Tab".into(), "The agent's browser tab".into()));
    assert_eq!(
        rows[1],
        (
            "Agent's reason".into(),
            "The invoice is behind the sign-in".into()
        )
    );
    assert!(!card
        .blocks
        .iter()
        .any(|b| matches!(b, Block::Notice { id, .. } if id == "instructions")));
    assert!(rows.iter().any(|(label, text)| label == "Next step"
        && text.contains("Enter your sign-in details")
        && text.contains("another way to sign in")));
    assert!(rows.iter().any(|(label, _)| label == "What happens"));
    login.status = TicketStatus::Resolved;
    login.resolution = Some(json!({"status":"submitted","next":"done"}));
    let receipt = crate::cards::ticket(&login);
    assert_eq!(receipt.expires_at, None);
    assert!(about(&receipt, "about").is_empty());

    let mut otp = legacy_ticket(TicketKind::SecretInput);
    otp.otp = Some(TicketOtp {
        channel: OtpChannel::Email,
        can_register: false,
    });
    let card = crate::cards::ticket(&otp);
    assert!(card.blocks.iter().any(|b| matches!(b, Block::Notice { id, text, .. } if id == "channel" && text == "Enter the code sent by email.")));
    otp.instructions = Some("Enter the code the site sent you by email.".into());
    assert!(!crate::cards::ticket(&otp)
        .blocks
        .iter()
        .any(|b| matches!(b, Block::Notice { id, .. } if id == "instructions")));
    assert!(about(&card, "about")
        .iter()
        .any(|(label, text)| label == "What happens" && text.contains("isn't sent to the agent")));

    let mut takeover = legacy_ticket(TicketKind::Takeover);
    let rows = about(&crate::cards::ticket(&takeover), "about");
    assert!(rows
        .iter()
        .any(|(l, t)| l == "Next step" && t.contains("in the agent's tab")));
    takeover.target = None;
    let rows = about(&crate::cards::ticket(&takeover), "about");
    assert!(!rows.iter().any(|(l, _)| l == "Tab"));
    assert!(rows
        .iter()
        .any(|(l, t)| l == "Next step" && t.contains("on the computer")));

    let mut confirm = legacy_ticket(TicketKind::Confirm);
    confirm.deferral = TicketDeferral::Approval;
    confirm.target = None;
    let card = crate::cards::ticket(&confirm);
    let rows = about(&card, "about");
    assert!(rows
        .iter()
        .any(|(l, t)| l == "What happens" && t.contains("once")));
    assert!(rows.iter().any(|(l, _)| l == "If you don't answer"));
    let labels: Vec<_> = card.actions.iter().map(|a| a.label.as_str()).collect();
    assert_eq!(labels, ["Approve", "Not now"]);

    let mut device = legacy_ticket(TicketKind::DeviceApproval);
    device.approval = Some(TicketApproval {
        origin: "https://example.test".into(),
        site_name: "Example".into(),
        icon: None,
        message: "Check your phone".into(),
        number: Some("42".into()),
    });
    let rows = about(&crate::cards::ticket(&device), "about");
    assert!(rows.contains(&("Number to match".into(), "42".into())));
}

#[test]
fn cards_routine_proposal_states_schedule_zone_target_and_prompt() {
    let mut ticket = legacy_ticket(TicketKind::Confirm);
    ticket.target = None;
    ticket.instructions = Some("Approve this exact routine configuration.\n{}".into());
    ticket.tool = Some(TicketTool {
        name: crate::routines::proposals::PROPOSE_ROUTINE.into(),
        args: json!({"name":"Morning digest","schedule":{"type":"cron","expr":"0 8 * * 1-5","timezone":"Asia/Seoul"},"action":{"type":"new_session","prompt":"Summarize overnight email"},"enabled":true,"catch_up":false}),
    });
    let card = crate::cards::ticket(&ticket);
    assert!(!card
        .blocks
        .iter()
        .any(|b| matches!(b, Block::Notice { id, .. } if id == "instructions")));
    assert_eq!(
        about(&card, "routine"),
        [
            ("Schedule".to_string(), "Weekdays at 08:00".to_string()),
            ("Time zone".into(), "Asia/Seoul".into()),
            ("Runs in".into(), "A new conversation each time".into()),
            ("Message sent".into(), "Summarize overnight email".into()),
        ]
    );
    let labels: Vec<_> = card.actions.iter().map(|a| a.label.as_str()).collect();
    assert_eq!(labels, ["Create routine", "Decline"]);
    // The routine rows precede the explanation and survive into the receipt.
    let order: Vec<_> = card
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::KvRows { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(order, ["routine", "about"]);
    ticket.status = TicketStatus::Resolved;
    ticket.resolution = Some(json!({"approved":false}));
    let receipt = crate::cards::ticket(&ticket);
    assert_eq!(about(&receipt, "routine").len(), 4);
    assert!(about(&receipt, "about").is_empty());

    for (schedule, words) in [
        (
            json!({"type":"cron","expr":"30 7 * * *"}),
            "Every day at 07:30",
        ),
        (
            json!({"type":"cron","expr":"5 * * * *"}),
            "Every hour at :05",
        ),
        (
            json!({"type":"cron","expr":"0 9 * * 1"}),
            "Every Monday at 09:00",
        ),
        (
            json!({"type":"cron","expr":"0 9 1 * *"}),
            "On day 1 of every month at 09:00",
        ),
        (
            json!({"type":"cron","expr":"*/15 * * * *"}),
            "On the cron schedule */15 * * * *",
        ),
        (
            json!({"type":"interval","secs":21600}),
            "Every 6 hours, counted from when you approve",
        ),
    ] {
        let mut t = legacy_ticket(TicketKind::Confirm);
        t.tool = Some(TicketTool {
            name: crate::routines::proposals::PROPOSE_ROUTINE.into(),
            args: json!({"name":"R","schedule":schedule,"action":{"type":"new_session","prompt":"p"}}),
        });
        let rows = about(&crate::cards::ticket(&t), "routine");
        assert_eq!(rows[0].1, words);
        let zone = rows
            .iter()
            .find(|(l, _)| l == "Time zone")
            .map(|(_, z)| z.as_str());
        assert_eq!(zone, (!words.starts_with("Every 6")).then_some("UTC"));
    }

    // A message too long for a row is shortened there, and the full instructions stay.
    let mut long = legacy_ticket(TicketKind::Confirm);
    long.instructions = Some("Approve this exact routine configuration.".into());
    long.tool = Some(TicketTool {
        name: crate::routines::proposals::PROPOSE_ROUTINE.into(),
        args: json!({"name":"R","schedule":{"type":"interval","secs":3600},"action":{"type":"new_session","prompt":"x".repeat(1_500)}}),
    });
    let card = crate::cards::ticket(&long);
    let shown = about(&card, "routine")
        .into_iter()
        .find(|(l, _)| l == "Message sent")
        .unwrap()
        .1;
    assert_eq!(shown.chars().count(), 1_001);
    assert!(shown.ends_with('…'));
    assert!(card
        .blocks
        .iter()
        .any(|b| matches!(b, Block::Notice { id, .. } if id == "instructions")));
}

#[test]
fn cards_fill_only_receipt_lists_labels_without_values_and_offers_submit_path() {
    let mut ticket = legacy_ticket(TicketKind::Info);
    ticket.form = Some(serde_json::from_value(json!({"origin":"https://example.test","site_name":"Example","payment":false,"submit_label":"Fill fields","fields":[
        {"id":"name","label":"Full name","kind":"text","secret":false,"otp":false,"card":null,"required":true,"autocomplete":"name","placeholder":null,"validation":"text","max_length":80,"initial":null,"suggestions":[],"target_origin":"https://example.test"},
        {"id":"city","label":"City","kind":"text","secret":false,"otp":false,"card":null,"required":false,"autocomplete":"address-level2","placeholder":null,"validation":"text","max_length":80,"initial":null,"suggestions":[],"target_origin":"https://example.test"}
    ]})).unwrap());
    let pending = crate::cards::ticket(&ticket);
    assert!(about(&pending, "about")
        .iter()
        .any(|(l, t)| l == "What happens" && t.contains("Nothing is submitted")));
    ticket.status = TicketStatus::Resolved;
    ticket.resolution = Some(json!({"status":"filled","field_count":2,"next":"review_page"}));
    let card = crate::cards::ticket(&ticket);
    assert_eq!(
        card.collapsed_summary,
        "Details filled — page not submitted"
    );
    let rows = about(&card, "filled");
    assert_eq!(rows[0], ("Fields filled".into(), "Full name, City".into()));
    assert!(rows
        .iter()
        .any(|(l, t)| l == "Page" && t.starts_with("Not submitted")));
    assert!(rows.iter().any(|(l, _)| l == "To submit"));
    assert_eq!(card.actions.len(), 1);
    assert!(
        matches!(&card.actions[0].target, ActionTarget::FollowUp { text } if text == "Submit the form you just filled in.")
    );
    ticket.resolution = Some(json!({"status":"indeterminate"}));
    let failed = crate::cards::ticket(&ticket);
    assert!(about(&failed, "filled").is_empty());
    assert!(failed.actions.is_empty());
}

#[test]
fn cards_presentations_cannot_claim_a_deadline() {
    let mut input = draft();
    input["card"]["expires_at"] = json!("2030-01-01T00:00:00Z");
    assert!(crate::tools::present::admit(&input, None)
        .unwrap_err()
        .0
        .starts_with("invalid_card"));
    let spec = crate::tools::present::Present.spec();
    assert!(!spec.parameters_json.contains("expires_at"));
}
