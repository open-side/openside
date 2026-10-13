use chrono::{TimeZone, Utc};
use serde_json::{json, Value};

use crate::*;

fn ticket() -> Ticket {
    Ticket {
        hire: None,
        id: TicketId::new("ticket1"),
        session_id: SessionId::new("s1"),
        tool_call_id: "call1".into(),
        deferral: TicketDeferral::Approval,
        kind: TicketKind::Confirm,
        summary: "Authorize a local mock tool".into(),
        instructions: Some("Review the redacted invocation".into()),
        options: vec![TicketOption {
            value: "approve".into(),
            label: "Approve".into(),
        }],
        origin: TicketOrigin {
            subagent_id: Some("child1".into()),
            device_id: None,
            driver_ids: Vec::new(),
        },
        tool: Some(TicketTool {
            name: "mock".into(),
            args: json!({"path": "/tmp/mock"}),
        }),
        target: Some(TicketTarget {
            tab_id: Some("tab1".into()),
            origin: Some("http://127.0.0.1".into()),
            field_ref: Some("e1".into()),
        }),
        resume_when: Some(json!({"manual": true})),
        created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        expires_at: Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
        status: TicketStatus::Pending,
        resolution: None,
        otp: None,
        otp_register: None,
        login: None,
        choice: None,
        approval: None,
        card: None,
        form: None,
    }
}

#[test]
fn message_authority_is_output_only_and_legacy_events_fail_closed() {
    for forged in [
        json!({"author_kind":"person"}),
        json!({"from_agent":"chief"}),
    ] {
        let mut request = json!({"text":"hire someone","mode":"now"});
        request
            .as_object_mut()
            .unwrap()
            .extend(forged.as_object().unwrap().clone());
        assert!(serde_json::from_value::<SubmitMessageRequest>(request).is_err());
    }
    let legacy: EventKind = serde_json::from_value(json!({
        "kind":"message.queued","message_id":"old","text":"hire someone","mode":"queue"
    }))
    .unwrap();
    assert!(matches!(
        legacy,
        EventKind::MessageQueued {
            author_kind: MessageAuthorKind::Internal,
            from_agent: None,
            ..
        }
    ));
    for (kind, wire) in [
        (MessageAuthorKind::Person, "person"),
        (MessageAuthorKind::AgentClient, "agent_client"),
        (MessageAuthorKind::Internal, "internal"),
    ] {
        assert_eq!(serde_json::to_value(kind).unwrap(), wire);
        let event = EventKind::MessageQueued {
            message_id: MessageId::new("new"),
            text: "hello".into(),
            mode: QueueMode::Queue,
            attachments: Vec::new(),
            author_kind: kind,
            from_agent: None,
        };
        assert_eq!(
            serde_json::from_value::<EventKind>(serde_json::to_value(&event).unwrap()).unwrap(),
            event
        );
    }
    let legacy: Message = serde_json::from_value(json!({
        "id":"old","text":"hello","status":{"queued":"queue"}
    }))
    .unwrap();
    assert_eq!(legacy.author_kind, MessageAuthorKind::Internal);
    assert_eq!(legacy.from_agent, None);
}

#[test]
fn every_event_kind_round_trips() {
    let id = || MessageId::new("m1");
    let kinds = vec![
        EventKind::MessageQueued {
            message_id: id(),
            text: "hi".into(),
            mode: QueueMode::Queue,
            attachments: Vec::new(),
            author_kind: MessageAuthorKind::Person,
            from_agent: None,
        },
        EventKind::MessageQueued {
            message_id: MessageId::new("m-attached"),
            text: "see attached".into(),
            mode: QueueMode::Queue,
            attachments: vec![Attachment {
                path: "attachments/m-attached/1-chart.png".into(),
                kind: ContentKind::Image,
                mime: "image/png".into(),
                size: 68,
                sha256: "ab".repeat(32),
                name: Some("chart.png".into()),
            }],
            author_kind: MessageAuthorKind::Internal,
            from_agent: Some("chief".into()),
        },
        EventKind::MessageEdited {
            message_id: id(),
            text: "edit".into(),
        },
        EventKind::MessagePromoted { message_id: id() },
        EventKind::MessageMoved {
            message_id: id(),
            position: 2,
        },
        EventKind::MessageInjected { message_id: id() },
        EventKind::MessageCompleted {
            message_id: id(),
            outcome: MessageOutcome::Completed,
            reason: None,
        },
        EventKind::MessageCompleted {
            message_id: id(),
            outcome: MessageOutcome::Aborted,
            reason: Some("stopped".into()),
        },
        EventKind::MessageCompleted {
            message_id: id(),
            outcome: MessageOutcome::Failed,
            reason: Some("interrupted".into()),
        },
        EventKind::MessageCompleted {
            message_id: id(),
            outcome: MessageOutcome::Deferred,
            reason: None,
        },
        EventKind::MessageDropped {
            message_id: id(),
            reason: DropReason::Removed,
        },
        EventKind::TurnStarted {},
        EventKind::TurnEnded {},
        EventKind::AssistantDelta {
            text: "part".into(),
        },
        EventKind::AssistantFinal {
            text: "full".into(),
            attachments: Vec::new(),
        },
        EventKind::ThinkingDelta {
            text: "thinking".into(),
        },
        EventKind::Usage {
            model: "model".into(),
            input_tokens: 10,
            output_tokens: 5,
        },
        EventKind::ToolStarted {
            tool_call_id: "t1".into(),
            name: "mock".into(),
            args: json!({}),
        },
        EventKind::ToolUpdated {
            tool_call_id: "t1".into(),
            update: json!({"progress": 50}),
        },
        EventKind::ToolEnded {
            card: None,
            tool_call_id: "t1".into(),
            result: json!("done"),
            is_error: false,
        },
        EventKind::ArtifactSaved {
            path: "files/report.md".into(),
            size: 12,
            mime: "text/markdown".into(),
            tool_call_id: "t1".into(),
            tool: "write".into(),
        },
        EventKind::TicketOpened {
            ticket: Box::new(ticket()),
        },
        EventKind::TicketResolved {
            card: None,
            ticket_id: TicketId::new("ticket1"),
            resolution: json!("approve"),
        },
        EventKind::TicketRejected {
            card: None,
            ticket_id: TicketId::new("ticket1"),
            reason: "stopped".into(),
        },
        EventKind::TicketExpired {
            card: None,
            ticket_id: TicketId::new("ticket1"),
        },
        EventKind::SubagentStarted {
            subagent_id: "child1".into(),
            parent_tool_call_id: "call1".into(),
            task: "Inspect the fixture".into(),
            model: "mock/model".into(),
            tools: vec!["mock".into()],
            background: true,
        },
        EventKind::SubagentEnded {
            subagent_id: "child1".into(),
            status: SubagentStatus::Completed,
            reason: None,
            output: Some("done".into()),
        },
        EventKind::BrowserState(Box::new(BrowserState {
            task_id: Some("child1".into()),
            tab_id: "tab1".into(),
            url: "https://example.test/login".into(),
            host: "example.test".into(),
            title: "Sign in".into(),
            favicon: None,
            phase: BrowserPhase::WaitingHuman,
            action: Some(BrowserActionState {
                tool: "browser_act".into(),
                tool_call_id: "call2".into(),
                phrase: "Working on “Continue”".into(),
                target: Some("Continue".into()),
            }),
            page_kind: PageKind::Login,
            facts: PageFacts {
                heading: Some("Sign in".into()),
                fields: vec![PageField {
                    label: "Password".into(),
                    kind: "password".into(),
                }],
                result_count: None,
                error_text: None,
            },
            thumbnail: Some("thumbnail1".into()),
        })),
        EventKind::SubagentLifecycle {
            subagent_id: "child1".into(),
            status: "running".into(),
        },
        EventKind::SubagentProgress {
            subagent_id: "child1".into(),
            progress: json!({"text": "working"}),
        },
        EventKind::TabOpened {
            tab_id: "tab1".into(),
            url: "http://127.0.0.1/".into(),
        },
        EventKind::TabClosed {
            tab_id: "tab1".into(),
        },
        EventKind::TabAsleep {
            tab_id: "tab1".into(),
        },
        EventKind::SessionCreated {
            agent_id: None,
            title: "Task".into(),
            model: "mock/model".into(),
            mode: PolicyMode::Guard,
            workspace: None,
            incognito: false,
            browser_space: BrowserSpace::Isolated,
        },
        EventKind::SessionSettled {},
        EventKind::SessionStopped {},
        EventKind::SessionUpdated {
            mode: None,
            title: Some("Renamed".into()),
            pinned: Some(true),
            archived: None,
        },
        EventKind::RuntimeStarted {
            tools: vec!["mock".into()],
            startup_ms: 12,
        },
        EventKind::runtime_error("mock_failure", "sanitized"),
        EventKind::SessionTitleProposed {
            title: "Compare Acme plans".into(),
        },
    ];
    for (index, kind) in kinds.into_iter().enumerate() {
        let event = Event {
            session_id: SessionId::new("s1"),
            seq: index as u64 + 1,
            ts: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            run_id: (index % 2 == 0).then(|| "run1".into()),
            kind,
        };
        let encoded = serde_json::to_value(&event).unwrap();
        let wire_kind = encoded["kind"].as_str().unwrap();
        assert!(wire_kind.contains('.') || wire_kind == "usage");
        assert!(encoded["session_id"].is_string());
        assert!(encoded["ts"].as_str().unwrap().ends_with('Z'));
        assert_eq!(serde_json::from_value::<Event>(encoded).unwrap(), event);
    }
}

fn check_refs(value: &Value, document: &Value) {
    match value {
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                let pointer = reference.strip_prefix('#').unwrap();
                assert!(
                    document.pointer(pointer).is_some(),
                    "unresolved {reference}"
                );
                assert!(pointer.starts_with("/components/schemas/"));
            }
            for child in object.values() {
                check_refs(child, document);
            }
        }
        Value::Array(array) => {
            for child in array {
                check_refs(child, document);
            }
        }
        _ => {}
    }
}

#[test]
fn openapi_is_json_and_all_schema_references_resolve() {
    let document = openapi();
    let json = serde_json::to_string(&document).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&json).unwrap(), document);
    assert_eq!(document["openapi"], "3.1.0");
    assert!(document["components"]["schemas"]["Event"].is_object());
    assert!(document["components"]["schemas"]["EventKind"].is_object());
    assert!(document["components"]["schemas"]["Settings"].is_object());
    check_refs(&document, &document);
    for route in ROUTES {
        for name in [route.request_schema, route.response_schema]
            .into_iter()
            .flatten()
        {
            assert!(
                document["components"]["schemas"][name].is_object(),
                "missing {name}"
            );
        }
        let operation = &document["paths"][route.path][route.method.as_str().to_ascii_lowercase()];
        assert!(operation.is_object());
        assert_eq!(
            operation["x-required-capability"],
            serde_json::to_value(route.scope).unwrap()
        );
    }
}

#[test]
fn subagent_routes_scopes_and_stream_opt_out_are_documented() {
    let document = openapi();
    let scope = |method: HttpMethod, path: &str| {
        ROUTES
            .iter()
            .find(|route| route.method == method && route.path == path)
            .map(|route| route.scope)
    };
    assert_eq!(
        scope(HttpMethod::Get, "/v1/sessions/{id}/subagents"),
        Some(CapabilityScope::Read)
    );
    for action in ["cancel", "steer"] {
        let path = format!("/v1/sessions/{{id}}/subagents/{{sid}}/{action}");
        assert_eq!(scope(HttpMethod::Post, &path), Some(CapabilityScope::Steer));
        let parameters = document["paths"][path.as_str()]["post"]["parameters"]
            .as_array()
            .unwrap();
        assert!(parameters.iter().any(|p| p["name"] == "sid"));
    }
    assert_eq!(
        document["paths"]["/v1/sessions/{id}/subagents/{sid}/steer"]["post"]["requestBody"]
            ["content"]["application/json"]["schema"],
        json!({"$ref": "#/components/schemas/SteerSubagentRequest"})
    );
    let events = document["paths"]["/v1/sessions/{id}/events"]["get"]["parameters"]
        .as_array()
        .unwrap();
    assert!(events.iter().any(|p| p["name"] == "subagents"));
    let status = serde_json::to_value(SubagentStatus::WaitingHuman).unwrap();
    assert_eq!(status, "waiting_human");
    assert!(SubagentStatus::Cancelled.is_terminal() && !SubagentStatus::Running.is_terminal());
}

#[test]
fn sse_resume_and_ticket_status_filter_are_documented() {
    let document = openapi();
    let events = &document["paths"]["/v1/sessions/{id}/events"]["get"];
    assert!(events["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["name"] == "Last-Event-ID"));
    assert!(events["responses"]["200"]["content"]["text/event-stream"].is_object());
    let tickets = &document["paths"]["/v1/tickets"]["get"];
    assert_eq!(tickets["parameters"][0]["name"], "status");
    assert_eq!(
        tickets["parameters"][0]["schema"],
        json!({"$ref": "#/components/schemas/TicketStatus"})
    );
}

#[test]
fn settings_mirror_policy_modes_and_model_configuration() {
    for (mode, wire) in [
        (PolicyMode::ReadOnly, "read-only"),
        (PolicyMode::Guard, "guard"),
        (PolicyMode::Full, "full"),
    ] {
        assert_eq!(PolicyMode::from(openside_policy::Mode::from(mode)), mode);
        assert_eq!(serde_json::to_value(mode).unwrap(), wire);
    }
    let settings = Settings {
        default_model: None,
        fallback_models: vec![],
        policy_mode: PolicyMode::Guard,
    };
    assert_eq!(
        serde_json::from_value::<Settings>(serde_json::to_value(&settings).unwrap()).unwrap(),
        settings
    );
    assert_eq!(
        serde_json::from_value::<SubmitMessageRequest>(json!({"text":"hi","mode":"queue"}))
            .unwrap()
            .message_id,
        None
    );
    assert_eq!(QueueMode::from(MessageMode::Now), QueueMode::Steer);
}

#[test]
fn ticket_types_round_trip_with_all_wire_variants() {
    for (kind, wire) in [
        (TicketKind::Confirm, "confirm"),
        (TicketKind::SecretInput, "secret_input"),
        (TicketKind::DeviceApproval, "device_approval"),
        (TicketKind::Info, "info"),
        (TicketKind::Takeover, "takeover"),
        (TicketKind::Login, "login"),
        (TicketKind::Choice, "choice"),
        (TicketKind::Hire, "hire"),
    ] {
        assert_eq!(serde_json::to_value(kind).unwrap(), wire);
        for (status, status_wire) in [
            (TicketStatus::Pending, "pending"),
            (TicketStatus::Resolved, "resolved"),
            (TicketStatus::Rejected, "rejected"),
            (TicketStatus::Expired, "expired"),
            (TicketStatus::NeedsDesktop, "needs_desktop"),
        ] {
            assert_eq!(serde_json::to_value(status).unwrap(), status_wire);
            for (deferral, deferral_wire) in [
                (TicketDeferral::Approval, "approval"),
                (TicketDeferral::External, "external"),
            ] {
                assert_eq!(serde_json::to_value(deferral).unwrap(), deferral_wire);
                let mut value = ticket();
                value.kind = kind;
                value.status = status;
                value.deferral = deferral;
                value.resolution =
                    (status == TicketStatus::Resolved).then(|| json!({"status": "filled"}));
                let list = TicketList {
                    tickets: vec![value],
                };
                let encoded = serde_json::to_value(&list).unwrap();
                assert_eq!(serde_json::from_value::<TicketList>(encoded).unwrap(), list);
            }
        }
    }
    let schemas = schemas();
    for name in [
        "Ticket",
        "TicketList",
        "TicketKind",
        "TicketStatus",
        "TicketDeferral",
        "TicketOrigin",
        "TicketOption",
        "TicketTool",
        "TicketTarget",
        "TicketId",
        "ResolveTicketRequest",
        "RejectTicketRequest",
        "TicketLogin",
        "TicketChoice",
    ] {
        assert!(schemas[name].is_object(), "missing {name}");
    }
    assert_eq!(
        schema_variants(&schemas["TicketKind"]),
        vec![
            json!("confirm"),
            json!("secret_input"),
            json!("device_approval"),
            json!("info"),
            json!("takeover"),
            json!("login"),
            json!("choice"),
            json!("hire")
        ]
    );
    assert_eq!(
        schema_variants(&schemas["TicketStatus"]),
        vec![
            json!("pending"),
            json!("resolved"),
            json!("rejected"),
            json!("expired"),
            json!("needs_desktop")
        ]
    );
    assert_eq!(
        schema_variants(&schemas["TicketDeferral"]),
        vec![json!("approval"), json!("external")]
    );
    assert_eq!(schemas["TicketId"]["type"], "string");
}

#[test]
fn ticket_requests_reject_unknown_fields_and_round_trip() {
    for value in [
        json!(true),
        json!(false),
        json!("approve"),
        json!("reject"),
        json!("answer"),
    ] {
        let request = ResolveTicketRequest { hire: None, value };
        let encoded = serde_json::to_value(&request).unwrap();
        assert_eq!(
            serde_json::from_value::<ResolveTicketRequest>(encoded).unwrap(),
            request
        );
    }
    assert!(serde_json::from_value::<ResolveTicketRequest>(
        json!({"value": true, "unexpected": true})
    )
    .is_err());
    for reason in [None, Some("cancelled".into())] {
        let request = RejectTicketRequest { reason };
        let encoded = serde_json::to_value(&request).unwrap();
        assert_eq!(
            serde_json::from_value::<RejectTicketRequest>(encoded).unwrap(),
            request
        );
    }
    assert!(serde_json::from_value::<RejectTicketRequest>(
        json!({"reason": null, "unexpected": true})
    )
    .is_err());
    for name in ["ResolveTicketRequest", "RejectTicketRequest"] {
        assert_eq!(schemas()[name]["additionalProperties"], false);
    }
}

#[test]
fn session_modes_waiting_status_and_deferred_outcome_round_trip() {
    let request: CreateSessionRequest = serde_json::from_value(json!({})).unwrap();
    assert_eq!(request.mode, None);
    assert_eq!(request.model, None);
    assert_eq!(request.title, None);
    assert_eq!(request.agent_id, None);
    assert!(
        !request.incognito,
        "sessions are recorded unless incognito is asked for"
    );
    assert_eq!(request.workspace, None);
    assert_eq!(request.browser_space, BrowserSpace::Shared);
    assert_eq!(
        serde_json::to_value(BrowserSpace::Isolated).unwrap(),
        "isolated"
    );
    for mode in [PolicyMode::ReadOnly, PolicyMode::Guard, PolicyMode::Full] {
        let space = if mode == PolicyMode::Guard {
            BrowserSpace::Isolated
        } else {
            BrowserSpace::Shared
        };
        let request = CreateSessionRequest {
            agent_id: None,
            title: None,
            model: None,
            mode: Some(mode),
            workspace: Some("/tmp/workspace".into()),
            incognito: mode == PolicyMode::Full,
            browser_space: space,
        };
        assert_eq!(
            serde_json::from_value::<CreateSessionRequest>(serde_json::to_value(&request).unwrap())
                .unwrap(),
            request
        );
        let event = EventKind::SessionCreated {
            agent_id: None,
            title: "Task".into(),
            model: "model".into(),
            mode,
            workspace: Some("/tmp/workspace".into()),
            incognito: mode == PolicyMode::Full,
            browser_space: space,
        };
        assert_eq!(
            serde_json::from_value::<EventKind>(serde_json::to_value(&event).unwrap()).unwrap(),
            event
        );
        let session = Session {
            agent_id: None,
            id: SessionId::new("s1"),
            title: "Task".into(),
            model: "model".into(),
            mode,
            workspace: Some("/tmp/workspace".into()),
            status: SessionStatus::WaitingHuman,
            activity: "Waiting for you".into(),
            last_seq: 1,
            queue: vec![],
            pinned: true,
            archived: false,
            incognito: mode == PolicyMode::Full,
            browser_space: space,
        };
        assert_eq!(
            serde_json::from_value::<Session>(serde_json::to_value(&session).unwrap()).unwrap(),
            session
        );
    }
    assert_eq!(
        serde_json::from_value::<EventKind>(
            json!({"kind": "session.created", "title": "Task", "model": "model"})
        )
        .unwrap(),
        EventKind::SessionCreated {
            agent_id: None,
            title: "Task".into(),
            model: "model".into(),
            mode: PolicyMode::Guard,
            workspace: None,
            incognito: false,
            browser_space: BrowserSpace::Shared,
        }
    );
    assert_eq!(
        serde_json::to_value(SessionStatus::WaitingHuman).unwrap(),
        "waiting_human"
    );
    assert_eq!(
        serde_json::to_value(MessageOutcome::Deferred).unwrap(),
        "deferred"
    );
    assert_eq!(
        serde_json::to_value(CapabilityScope::HumanStep).unwrap(),
        "human_step"
    );
    for (name, wire) in [
        ("SessionStatus", "waiting_human"),
        ("MessageOutcome", "deferred"),
        ("CapabilityScope", "human_step"),
    ] {
        assert!(schema_variants(&schemas()[name]).contains(&json!(wire)));
    }
}

#[test]
fn ticket_routes_document_exact_capability_and_desktop_contracts() {
    let document = openapi();
    let mut operations = std::collections::BTreeSet::new();
    for route in ROUTES {
        assert!(operations.insert((route.path, route.method.as_str())));
        let operation = &document["paths"][route.path][route.method.as_str().to_ascii_lowercase()];
        assert!(operation["responses"][route.success_status.to_string()].is_object());
    }
    assert_eq!(
        document["paths"]
            .as_object()
            .unwrap()
            .values()
            .map(|path| path.as_object().unwrap().len())
            .sum::<usize>(),
        ROUTES.len()
    );
    for path in ["/v1/tickets/{id}/resolve", "/v1/tickets/{id}/reject"] {
        let operation = &document["paths"][path]["post"];
        assert_eq!(operation["x-required-capability"], "approve");
        assert_eq!(
            operation["x-capability-by-ticket-kind"],
            json!({
                "confirm": "approve", "info": "approve", "secret_input": "human_step",
                "device_approval": "human_step", "takeover": "human_step",
                "login": "human_step", "choice": "approve",
            })
        );
        assert_eq!(
            operation["parameters"][0]["schema"],
            json!({"$ref": "#/components/schemas/TicketId"})
        );
    }
    assert_eq!(
        document["paths"]["/v1/tickets/{id}/done"]["post"]["x-required-capability"],
        "human_step"
    );
    let takeover = &document["paths"]["/v1/tickets/{id}/takeover"]["get"];
    assert_eq!(takeover["x-required-capability"], "human_step");
    assert_eq!(
        takeover["responses"]["101"]["x-websocket-server-message"]["$ref"],
        "#/components/schemas/TakeoverServerMessage"
    );
    assert_eq!(
        takeover["responses"]["101"]["x-websocket-client-message"]["$ref"],
        "#/components/schemas/TakeoverInput"
    );
    assert_eq!(
        takeover["responses"]["409"]["content"]["application/json"]["example"]["code"],
        "takeover_in_use"
    );
    // Wire shapes clients depend on: tagged JSON text messages in both directions.
    assert_eq!(
        serde_json::to_value(TakeoverServerMessage::Ended {
            reason: TakeoverEndReason::Done
        })
        .unwrap(),
        json!({"type":"ended","reason":"done"})
    );
    assert_eq!(
        serde_json::from_value::<TakeoverInput>(
            json!({"type":"mouse","event":"down","x":10.5,"y":20,"button":"left"})
        )
        .unwrap(),
        TakeoverInput::Mouse {
            event: TakeoverMouseEvent::Down,
            x: 10.5,
            y: 20.0,
            button: Some(TakeoverMouseButton::Left),
            click_count: None,
            modifiers: None
        }
    );
    assert!(
        serde_json::from_value::<TakeoverInput>(json!({"type":"text","text":"a","x":1})).is_err()
    );
}

fn schema_variants(schema: &Value) -> Vec<Value> {
    schema["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|variant| variant["const"].clone())
        .collect()
}

#[test]
fn committed_openapi_snapshot_matches_generated_document() {
    // The web client types are generated from this file, so drift means a stale client.
    let committed: Value =
        serde_json::from_str(include_str!("../../../docs/specs/openapi.json")).unwrap();
    assert!(
        committed == openapi(),
        "docs/specs/openapi.json is stale: run `cargo run -q -p openside-proto --example openapi > docs/specs/openapi.json` then `pnpm -C web gen:api`"
    );
}

#[test]
fn skill_routes_require_read_for_listing_and_admin_for_writes() {
    let scope = |method: HttpMethod, path: &str| {
        ROUTES
            .iter()
            .find(|route| route.method == method && route.path == path)
            .map(|route| route.scope)
    };
    assert_eq!(
        scope(HttpMethod::Get, "/v1/skills"),
        Some(CapabilityScope::Read)
    );
    assert_eq!(
        scope(HttpMethod::Get, "/v1/skills/{name}"),
        Some(CapabilityScope::Read)
    );
    for (method, path) in [
        (HttpMethod::Post, "/v1/skills"),
        (HttpMethod::Patch, "/v1/skills/{name}"),
        (HttpMethod::Delete, "/v1/skills/{name}"),
    ] {
        assert_eq!(scope(method, path), Some(CapabilityScope::Admin), "{path}");
    }
    let document = openapi();
    let query: Vec<_> = document["paths"]["/v1/skills"]["get"]["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(query, ["q", "session", "enabled"]);
    let created: CreateSkillRequest = serde_json::from_value(json!({
        "name": "a", "description": "d"
    }))
    .unwrap();
    assert!(created.body.is_empty() && created.resources.is_empty());
    assert!(serde_json::from_value::<CreateSkillRequest>(json!({
        "name": "a", "description": "d", "path": "/etc"
    }))
    .is_err());
}

#[test]
fn capability_scopes_have_stable_wire_names_and_minimal_implication() {
    use CapabilityScope::*;
    let all = [
        Read, Steer, Approve, HumanStep, Admin, Browse, VaultUse, OtpIngest,
    ];
    let wire: Vec<Value> = all
        .iter()
        .map(|s| serde_json::to_value(s).unwrap())
        .collect();
    assert_eq!(
        wire,
        [
            "read",
            "steer",
            "approve",
            "human_step",
            "admin",
            "browse",
            "vault_use",
            "otp_ingest"
        ]
        .map(Value::from)
    );
    for held in all {
        for required in all {
            let expected =
                held == required || held == Admin || (held == Steer && required == Browse);
            assert_eq!(held.grants(required), expected, "{held:?} -> {required:?}");
        }
    }
}

#[test]
fn policy_routes_and_browse_scope_are_documented() {
    let scope = |method: HttpMethod, path: &str| {
        ROUTES
            .iter()
            .find(|route| route.method == method && route.path == path)
            .map(|route| route.scope)
    };
    assert_eq!(
        scope(HttpMethod::Get, "/v1/policy"),
        Some(CapabilityScope::Read)
    );
    assert_eq!(
        scope(HttpMethod::Put, "/v1/policy"),
        Some(CapabilityScope::Admin)
    );
    assert_eq!(
        scope(HttpMethod::Post, "/v1/policy/test"),
        Some(CapabilityScope::Read)
    );
    assert_eq!(
        scope(HttpMethod::Post, "/v1/sessions/{id}/repl"),
        Some(CapabilityScope::Browse)
    );
    assert_eq!(
        scope(HttpMethod::Post, "/v1/sessions/{id}/tools/{name}"),
        Some(CapabilityScope::Browse)
    );
    assert_eq!(
        scope(HttpMethod::Get, "/v1/tools"),
        Some(CapabilityScope::Read)
    );
    let document = openapi();
    assert_eq!(
        document["paths"]["/v1/policy"]["put"]["requestBody"]["content"]["application/json"]
            ["schema"],
        json!({"$ref": "#/components/schemas/PolicySettings"})
    );
    assert_eq!(
        document["components"]["schemas"]["CapabilityScope"]["oneOf"]
            .as_array()
            .map(Vec::len)
            .or_else(
                || document["components"]["schemas"]["CapabilityScope"]["enum"]
                    .as_array()
                    .map(Vec::len)
            ),
        Some(8)
    );
    let settings: PolicySettings = serde_json::from_value(json!({
        "default_mode": "guard",
        "deny_origins": ["https://*.evil.example"],
        "rules": [{"effect": "deny", "capability": "fs_write", "pattern": "/etc/**"}]
    }))
    .unwrap();
    assert!(settings.allow_origins.is_empty());
    assert_eq!(settings.rules[0].capability, PolicyCapability::FsWrite);
    assert!(
        serde_json::from_value::<PolicySettings>(json!({"default_mode":"guard","extra":1}))
            .is_err()
    );
}

#[test]
fn vault_value_requests_reject_unknown_fields_and_redact_debug() {
    let request: CreateVaultItemRequest = serde_json::from_value(json!({
        "id": "mail",
        "origins": ["https://mail.example.com"],
        "username": "user-canary-41",
        "password": "password-canary-41",
        "totp": "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP"
    }))
    .unwrap();
    assert_eq!(request.policy, None);
    let unlock: VaultUnlockRequest =
        serde_json::from_value(json!({"passphrase": "passphrase-canary-41"})).unwrap();
    let debug = format!("{request:?} {unlock:?}");
    for canary in [
        "user-canary-41",
        "password-canary-41",
        "JBSWY3DPEHPK3PXP",
        "passphrase-canary-41",
    ] {
        assert!(!debug.contains(canary), "{debug}");
    }
    assert!(serde_json::from_value::<VaultUnlockRequest>(json!({}))
        .unwrap()
        .passphrase
        .is_none());
    assert!(serde_json::from_value::<CreateVaultItemRequest>(
        json!({"id": "a", "origins": [], "extra": 1})
    )
    .is_err());
    for name in ["CreateVaultItemRequest", "VaultUnlockRequest"] {
        assert_eq!(schemas()[name]["additionalProperties"], false);
    }
    assert_eq!(
        schema_variants(&schemas()["VaultState"]),
        vec![
            json!("uninitialized"),
            json!("locked"),
            json!("unlocked"),
            json!("unavailable")
        ]
    );
}

#[test]
fn runtime_error_copy_is_calm_and_retryability_is_explicit() {
    let error = EventKind::runtime_error("interrupted", "process EOF and internal dispatch state");
    let value = serde_json::to_value(error).unwrap();
    assert_eq!(value["user_message"], "The agent stopped. Try again.");
    assert_eq!(value["retryable"], true);
    assert!(value["message"].as_str().unwrap().contains("EOF"));
    for code in ["auth_unavailable", "tool_gate", "unknown_code"] {
        let value =
            serde_json::to_value(EventKind::runtime_error(code, "technical details")).unwrap();
        assert_eq!(value["retryable"], false);
        assert!(!value["user_message"]
            .as_str()
            .unwrap()
            .contains("technical"));
    }
    let historical: EventKind = serde_json::from_value(
        json!({"kind":"runtime.error", "code":"interrupted", "message":"old diagnostic"}),
    )
    .unwrap();
    assert!(
        matches!(historical, EventKind::RuntimeError { user_message, retryable: false, .. } if user_message.is_empty())
    );
}

#[test]
fn cards_all_kinds_and_legacy_events_round_trip() {
    let kinds = [
        CardKind::ProductList,
        CardKind::Table,
        CardKind::PlaceList,
        CardKind::OrderSummary,
        CardKind::Payment,
        CardKind::SecureLogin,
        CardKind::LoginChoice,
        CardKind::Otp,
        CardKind::PaymentCard,
        CardKind::Confirmation,
        CardKind::Form,
        CardKind::ConnectAccount,
        CardKind::Activity,
        CardKind::Handoff,
        CardKind::Hire,
        CardKind::SecretInput,
        CardKind::Takeover,
    ];
    for kind in kinds {
        let card = Card {
            id: "present:fixture".into(),
            kind,
            site: CardSite {
                origin: None,
                name: "Local".into(),
                favicon_ref: None,
            },
            title: "Fixture".into(),
            subtitle: None,
            status: CardStatus::Done,
            blocks: vec![],
            actions: vec![],
            interaction: CardInteraction::None,
            ticket_id: None,
            collapsed_summary: "Fixture".into(),
        };
        assert_eq!(
            serde_json::from_value::<Card>(serde_json::to_value(&card).unwrap()).unwrap(),
            card
        );
        let event = EventKind::CardPresented {
            card,
            tool_call_id: "fixture".into(),
        };
        assert_eq!(
            serde_json::from_value::<EventKind>(serde_json::to_value(&event).unwrap()).unwrap(),
            event
        );
    }
    for event in [
        json!({"kind":"ticket.resolved","ticket_id":"old","resolution":true}),
        json!({"kind":"ticket.rejected","ticket_id":"old","reason":"cancelled"}),
        json!({"kind":"ticket.expired","ticket_id":"old"}),
        json!({"kind":"tool.ended","tool_call_id":"old","result":null,"is_error":false}),
    ] {
        let parsed: EventKind = serde_json::from_value(event.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), event);
    }
    let wire = serde_json::to_value(ticket()).unwrap();
    assert!(wire.get("card").is_none());
    assert!(wire.get("form").is_none());
    let old: Ticket = serde_json::from_value(wire).unwrap();
    assert!(old.card.is_none() && old.form.is_none());
}

#[test]
fn cards_normative_block_and_action_fixtures_round_trip() {
    let fixtures: Vec<Value> = serde_json::from_str(include_str!("cards_fixtures.json")).unwrap();
    assert_eq!(fixtures.len(), 17);
    for fixture in fixtures {
        let card: Card = serde_json::from_value(fixture).unwrap();
        let encoded = serde_json::to_value(&card).unwrap();
        assert_eq!(
            serde_json::from_value::<Card>(encoded.clone()).unwrap(),
            card
        );
        let mut unknown = encoded;
        unknown["arbitrary_html"] = json!("<script>forbidden</script>");
        assert!(serde_json::from_value::<Card>(unknown).is_err());
    }
}

#[test]
fn assistant_final_document_serde_back_compat() {
    let old: EventKind = serde_json::from_value(serde_json::json!({
        "kind": "assistant.final", "text": "A short answer"
    }))
    .unwrap();
    assert!(matches!(old, EventKind::AssistantFinal { attachments, .. } if attachments.is_empty()));
    let wire = serde_json::json!({
        "kind": "assistant.final", "text": "Here is the report.",
        "attachments": [{
            "path": "documents/delivered/report.md", "title": "Report",
            "display": "panel", "bytes": 42, "content_type": "text/markdown"
        }]
    });
    let event: EventKind = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(event).unwrap(), wire);
}
