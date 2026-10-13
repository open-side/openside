//! OP-2 contract: every message shape this crate puts on (or accepts from) the wire validates
//! against docs/specs/sidecar-wire.schema.json, which the Python sidecar generates from its own
//! models. A Rust-only or Python-only change to a message shape fails here or in
//! agent/tests/test_wire_schema.py.

use std::collections::BTreeSet;

use openside_core::{
    ApprovalTier, ContentKind, CredentialAction, CredentialAuth, CredentialAuthKind,
    CredentialErrorCode, CredentialOutcome, CredentialScope, CredentialUsage,
    CredentialUsageWindow, DeferKind, MessageId, ModelAccountRef, SessionId, TicketId,
};
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::protocol::{
    Config, DaemonMessage, Deferred, Envelope, ErrorCode, Event, MessageMode, Model, Outcome, Part,
    Resolution, RunOutcome, SidecarMessage, State, Tool, Versions,
};

const SCHEMA: &str = include_str!("../../../docs/specs/sidecar-wire.schema.json");

fn validator(root: &str) -> jsonschema::Validator {
    let schema: Value = serde_json::from_str(SCHEMA).unwrap();
    let rooted = json!({
        "$schema": schema["$schema"],
        "$defs": schema["$defs"],
        "$ref": format!("#/$defs/{root}"),
    });
    jsonschema::validator_for(&rooted).unwrap()
}

fn schema_types(root: &str) -> BTreeSet<String> {
    let schema: Value = serde_json::from_str(SCHEMA).unwrap();
    schema["$defs"][root]["discriminator"]["mapping"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect()
}

// Exhaustive matches: a new Rust variant does not compile until it has a wire name here and a
// sample below, so it cannot skip schema validation.
fn daemon_type(message: &DaemonMessage) -> &'static str {
    match message {
        DaemonMessage::Open { .. } => "open",
        DaemonMessage::UserMessage { .. } => "user_message",
        DaemonMessage::Abort => "abort",
        DaemonMessage::ToolResult { .. } => "tool_result",
        DaemonMessage::CredentialResponse { .. } => "credential_response",
        DaemonMessage::CredentialError { .. } => "credential_error",
        DaemonMessage::CredentialReported { .. } => "credential_reported",
        DaemonMessage::TicketResolved { .. } => "ticket_resolved",
        DaemonMessage::SubagentCancel { .. } => "subagent_cancel",
        DaemonMessage::SubagentSteer { .. } => "subagent_steer",
        DaemonMessage::DiscardDeferred => "discard_deferred",
        DaemonMessage::Shutdown => "shutdown",
        DaemonMessage::Error { .. } => "error",
    }
}

fn sidecar_type(message: &SidecarMessage) -> &'static str {
    match message {
        SidecarMessage::Hello { .. } => "hello",
        SidecarMessage::Ready { .. } => "ready",
        SidecarMessage::Event { .. } => "event",
        SidecarMessage::SubagentSteerResult { .. } => "subagent_steer_result",
        SidecarMessage::MessageAccepted { .. } => "message_accepted",
        SidecarMessage::MessageResult { .. } => "message_result",
        SidecarMessage::ToolCall { .. } => "tool_call",
        SidecarMessage::CredentialRequest { .. } => "credential_request",
        SidecarMessage::CredentialReport { .. } => "credential_report",
        SidecarMessage::Deferred { .. } => "deferred",
        SidecarMessage::StateSnapshot { .. } => "state_snapshot",
        SidecarMessage::Settled => "settled",
        SidecarMessage::ContextCompacted { .. } => "context_compacted",
        SidecarMessage::Error { .. } => "error",
        SidecarMessage::Bye => "bye",
    }
}

fn object(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}

fn daemon_samples() -> Vec<DaemonMessage> {
    let tool_result = |outcome| DaemonMessage::ToolResult {
        rpc_id: "rpc-1".into(),
        outcome,
    };
    let resolved = |resolution| DaemonMessage::TicketResolved {
        ticket_id: TicketId::new("ticket-1"),
        tool_call_id: "call-1".into(),
        resolution,
    };
    let open = |resume_state| DaemonMessage::Open {
        session: SessionId::new("session-1"),
        model: Model {
            id: "provider:model".into(),
            fallback: vec!["provider:other".into()],
        },
        system_prompt_parts: vec!["Use daemon tools only.".into()],
        tools: vec![Tool {
            name: "read".into(),
            description: "Read a file.".into(),
            parameters: object(json!({"type": "object", "properties": {}})),
            approval: ApprovalTier::Read,
            deferrable: false,
        }],
        resume_state,
        config: Config {
            credentials: object(json!({"order": ["env"]})),
            agent: object(json!({"tool_retries": 3, "profile": "full"})),
        },
        team: None,
    };
    let mut samples = vec![
        open(None),
        open(Some(State {
            format: "openside-agent-state/1".into(),
            data: "opaque".into(),
        })),
        DaemonMessage::Abort,
        tool_result(Outcome::Done {
            for_model: json!({"ok": true}),
            parts: Vec::new(),
        }),
        tool_result(Outcome::Done {
            for_model: json!({"saved": true}),
            parts: vec![png_part(), stored_only_part()],
        }),
        tool_result(Outcome::Deferred {
            ticket_id: TicketId::new("ticket-1"),
            kind: DeferKind::Approval,
        }),
        tool_result(Outcome::Denied {
            reason: "blocked".into(),
            hint: None,
        }),
        tool_result(Outcome::Denied {
            reason: "blocked".into(),
            hint: Some("ask first".into()),
        }),
        tool_result(Outcome::Error {
            message: "failed".into(),
        }),
        resolved(Resolution::Approved),
        resolved(Resolution::Denied {
            reason: "expired".into(),
        }),
        resolved(Resolution::Result {
            for_model: json!({"status": "filled"}),
        }),
        DaemonMessage::SubagentCancel {
            subagent_id: "sub-1".into(),
        },
        DaemonMessage::SubagentSteer {
            subagent_id: "sub-1".into(),
            text: "focus".into(),
            control_id: "steer-1".into(),
        },
        DaemonMessage::DiscardDeferred,
        DaemonMessage::Shutdown,
        DaemonMessage::Error {
            code: ErrorCode::Protocol,
            message: "invalid protocol message".into(),
            fatal: false,
            message_id: None,
        },
    ];
    for kind in [
        CredentialAuthKind::ApiKey,
        CredentialAuthKind::ClaudeCode,
        CredentialAuthKind::CodexCli,
        CredentialAuthKind::Bearer,
    ] {
        samples.push(DaemonMessage::CredentialResponse {
            id: "credential-rpc".into(),
            request_id: "model-request".into(),
            lease_id: "lease-1".into(),
            generation: 7,
            account: ModelAccountRef {
                source: "vault".into(),
                account_id: "opaque-1".into(),
            },
            auth: CredentialAuth {
                kind,
                value: "synthetic-wire-secret-canary".into(),
                upstream_account_id: Some("synthetic-upstream-id".into()),
                expires_at_ms: Some(1_000_000),
            },
        });
    }
    for code in [
        CredentialErrorCode::AuthUnavailable,
        CredentialErrorCode::AccountBlocked,
        CredentialErrorCode::AccountNotInPool,
        CredentialErrorCode::ProviderUnavailable,
        CredentialErrorCode::InvalidCredentialRequest,
    ] {
        samples.push(DaemonMessage::CredentialError {
            id: "credential-rpc".into(),
            request_id: "model-request".into(),
            code,
            reason: "No eligible account in the configured pool".into(),
            retry_at_ms: None,
            generation: 7,
        });
    }
    for action in [
        CredentialAction::Complete,
        CredentialAction::Refresh,
        CredentialAction::Rotate,
        CredentialAction::Stop,
    ] {
        samples.push(DaemonMessage::CredentialReported {
            id: "credential-report-rpc".into(),
            request_id: "model-request".into(),
            lease_id: "lease-1".into(),
            generation: 8,
            action,
            retry_at_ms: Some(10_000),
            reason: Some("Account report committed".into()),
        });
    }
    for mode in [MessageMode::Queue, MessageMode::Steer, MessageMode::Now] {
        samples.push(DaemonMessage::UserMessage {
            message_id: MessageId::new("message-1"),
            text: "hello".into(),
            mode,
            author_kind: openside_core::MessageAuthorKind::Person,
            parts: Vec::new(),
        });
    }
    samples.push(DaemonMessage::UserMessage {
        message_id: MessageId::new("message-2"),
        text: "see attached".into(),
        mode: MessageMode::Queue,
        author_kind: openside_core::MessageAuthorKind::Channel,
        parts: vec![png_part(), stored_only_part()],
    });
    samples
}

fn png_part() -> Part {
    Part {
        kind: ContentKind::Image,
        mime: "image/png".into(),
        data_base64: Some("iVBORw0KGgo=".into()),
        artifact_ref: Some("attachments/message-2/1-chart.png".into()),
        name: Some("chart.png".into()),
    }
}

fn stored_only_part() -> Part {
    Part {
        kind: ContentKind::File,
        mime: "application/pdf".into(),
        data_base64: None,
        artifact_ref: Some("attachments/message-2/2-report.pdf".into()),
        name: None,
    }
}
fn sidecar_samples() -> Vec<SidecarMessage> {
    let deferred = |kind| Deferred {
        tool_call_id: "call-1".into(),
        ticket_id: TicketId::new("ticket-1"),
        kind,
    };
    let mut samples = vec![
        SidecarMessage::Hello {
            protocol: crate::PROTOCOL_VERSION,
            nonce: "0123456789abcdef".into(),
            pid: 42,
            versions: Versions {
                python: "3.13".into(),
                pydantic_ai: "1.0".into(),
                pydantic_ai_harness: "0.1".into(),
                openside_agent: "0.1.1".into(),
            },
        },
        SidecarMessage::Ready {
            tools: vec!["read".into()],
            pending_deferred: vec![deferred(DeferKind::External)],
            startup_ms: 7,
        },
        SidecarMessage::MessageAccepted {
            message_id: MessageId::new("message-1"),
        },
        SidecarMessage::ToolCall {
            rpc_id: "rpc-1".into(),
            tool_call_id: "call-1".into(),
            name: "read".into(),
            args: object(json!({"path": "a.txt"})),
            subagent: Some("sub-1".into()),
            approval_ticket: Some(TicketId::new("ticket-1")),
        },
        SidecarMessage::ToolCall {
            rpc_id: "rpc-2".into(),
            tool_call_id: "call-2".into(),
            name: "read".into(),
            args: Map::new(),
            subagent: None,
            approval_ticket: None,
        },
        SidecarMessage::Deferred {
            calls: vec![deferred(DeferKind::Approval)],
        },
        SidecarMessage::StateSnapshot {
            state: State {
                format: "openside-agent-state/1".into(),
                data: "opaque".into(),
            },
        },
        SidecarMessage::Settled,
        SidecarMessage::ContextCompacted { subagent: None },
        SidecarMessage::ContextCompacted {
            subagent: Some("sub-1".into()),
        },
        SidecarMessage::SubagentSteerResult {
            control_id: "steer-1".into(),
            delivered: true,
        },
        SidecarMessage::SubagentSteerResult {
            control_id: "steer-2".into(),
            delivered: false,
        },
        SidecarMessage::Bye,
    ];
    samples.push(SidecarMessage::CredentialRequest {
        id: "credential-rpc".into(),
        request_id: "model-request".into(),
        provider: "anthropic".into(),
        model: "anthropic:model-id".into(),
        attempt: 0,
        subagent: None,
    });
    samples.push(SidecarMessage::CredentialRequest {
        id: "credential-child-rpc".into(),
        request_id: "model-child-request".into(),
        provider: "anthropic".into(),
        model: "anthropic:model-id".into(),
        attempt: 63,
        subagent: Some("child-1".into()),
    });
    for outcome in [
        CredentialOutcome::Success,
        CredentialOutcome::Auth,
        CredentialOutcome::RateLimit,
        CredentialOutcome::Quota,
        CredentialOutcome::Policy,
        CredentialOutcome::Transient,
        CredentialOutcome::Cancelled,
    ] {
        for scope in [CredentialScope::Account, CredentialScope::Model] {
            samples.push(SidecarMessage::CredentialReport {
                id: "credential-report-rpc".into(),
                request_id: "model-request".into(),
                lease_id: "lease-1".into(),
                outcome,
                scope,
                stream_started: false,
                retry_after_ms: Some(60_000),
                usage: None,
                usage_windows: vec![CredentialUsageWindow {
                    name: "weekly".into(),
                    used_percent: None,
                    reset_at_ms: None,
                }],
            });
        }
    }
    samples.push(SidecarMessage::CredentialReport {
        id: "credential-stream-report-rpc".into(),
        request_id: "model-request".into(),
        lease_id: "lease-1".into(),
        outcome: CredentialOutcome::Success,
        scope: CredentialScope::Account,
        stream_started: true,
        retry_after_ms: None,
        usage: Some(CredentialUsage {
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: 80,
            cache_write_tokens: 0,
        }),
        usage_windows: vec![CredentialUsageWindow {
            name: "weekly".into(),
            used_percent: Some(25.0),
            reset_at_ms: Some(10_000.0),
        }],
    });
    for outcome in [
        RunOutcome::Completed,
        RunOutcome::Aborted,
        RunOutcome::Failed,
        RunOutcome::Deferred,
    ] {
        samples.push(SidecarMessage::MessageResult {
            message_id: MessageId::new("message-1"),
            outcome,
            reason: Some("detail".into()),
        });
    }
    for code in [
        ErrorCode::Protocol,
        ErrorCode::BadRequest,
        ErrorCode::StaleTicket,
        ErrorCode::ModelError,
        ErrorCode::AuthUnavailable,
        ErrorCode::ToolGate,
        ErrorCode::Internal,
    ] {
        samples.push(SidecarMessage::Error {
            code,
            message: "sanitized".into(),
            fatal: false,
            message_id: None,
        });
    }
    for event in [
        Event::TurnStarted,
        Event::TurnEnded,
        Event::AssistantDelta { text: "a".into() },
        Event::AssistantFinal { text: "a".into() },
        Event::SessionTitleProposed {
            title: "Compare Acme plans".into(),
        },
        Event::ThinkingDelta { text: "a".into() },
        Event::SubagentLifecycle {
            subagent_id: "sub-1".into(),
            status: "running".into(),
            output: None,
            known_subagents: None,
            error_code: None,
            diagnostic: None,
        },
        Event::SubagentLifecycle {
            subagent_id: "sub-1".into(),
            status: "completed".into(),
            output: Some("done".into()),
            known_subagents: None,
            error_code: None,
            diagnostic: None,
        },
        Event::SubagentProgress {
            subagent_id: "sub-1".into(),
            progress: json!({"kind": "model_request"}),
        },
        Event::Usage {
            model: "provider:model".into(),
            input_tokens: 1,
            output_tokens: 2,
        },
    ] {
        samples.push(SidecarMessage::Event { event });
    }
    samples
}

fn assert_valid<T: Serialize>(validator: &jsonschema::Validator, message: T) -> Value {
    let wire = serde_json::to_value(Envelope::new(message)).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&wire)
        .map(|error| format!("{} at {}", error, error.instance_path()))
        .collect();
    assert!(
        errors.is_empty(),
        "{wire} violates the wire schema: {errors:#?}"
    );
    wire
}

#[test]
fn every_daemon_message_validates_against_the_generated_schema() {
    let validator = validator("DaemonToSidecar");
    let mut seen = BTreeSet::new();
    for message in daemon_samples() {
        seen.insert(daemon_type(&message).to_owned());
        assert_valid(&validator, message);
    }
    assert_eq!(seen, schema_types("DaemonToSidecar"));
}

#[test]
fn every_sidecar_message_shape_validates_against_the_generated_schema() {
    let validator = validator("SidecarToDaemon");
    let mut seen = BTreeSet::new();
    for message in sidecar_samples() {
        seen.insert(sidecar_type(&message).to_owned());
        let wire = assert_valid(&validator, message);
        // Rust must also read back what the schema accepts.
        serde_json::from_value::<Envelope<SidecarMessage>>(wire).unwrap();
    }
    assert_eq!(seen, schema_types("SidecarToDaemon"));
}

#[test]
fn schema_rejects_shapes_outside_the_contract() {
    let validator = validator("SidecarToDaemon");
    for bad in [
        json!({"v": 2, "type": "settled"}),
        json!({"type": "settled"}),
        json!({"v": 1, "type": "hello", "protocol": 2, "nonce": "n", "pid": 1,
            "versions": {"python": "", "pydantic_ai": "", "pydantic_ai_harness": "", "openside_agent": ""}}),
        json!({"v": 1, "type": "ready", "tools": [], "pending_deferred": [], "startup_ms": 1, "extra": 1}),
        json!({"v": 1, "type": "unknown"}),
    ] {
        assert!(!validator.is_valid(&bad), "{bad}");
    }
}

#[test]
fn published_version_range_and_protocol_match_the_daemon_pin() {
    let schema: Value = serde_json::from_str(SCHEMA).unwrap();
    assert_eq!(
        schema["x-openside-agent-versions"],
        crate::supported_agent_versions()
    );
    assert_eq!(schema["x-protocol"], crate::PROTOCOL_VERSION);
    assert_eq!(
        schema["$defs"]["Hello"]["properties"]["protocol"]["const"],
        crate::PROTOCOL_VERSION
    );
}

#[test]
fn credential_attempt_bound_matches_the_daemon_owner() {
    let schema: Value = serde_json::from_str(SCHEMA).unwrap();
    assert_eq!(
        schema["$defs"]["CredentialRequest"]["properties"]["attempt"]["maximum"],
        openside_core::MAX_CREDENTIAL_ATTEMPTS - 1,
    );
}

#[cfg(unix)]
#[test]
fn team_context_wire_caps_and_roles() {
    for role in [
        openside_core::TeamRole::Chief,
        openside_core::TeamRole::Teammate,
    ] {
        let mut spec = crate::tests::spec();
        spec.team = Some(openside_core::TeamContext {
            role,
            agent_id: "owner".into(),
            persona: "🦉".repeat(1500),
            roster: "界".repeat(2000),
        });
        let wire = crate::open_wire(spec.clone()).unwrap();
        assert!(validator("DaemonToSidecar").is_valid(&wire));
        assert_eq!(
            wire["team"]["persona"].as_str().unwrap().chars().count(),
            1500
        );
        let mut invalid = wire.clone();
        invalid["team"]["role"] = json!("admin");
        assert!(!validator("DaemonToSidecar").is_valid(&invalid));
        spec.team.as_mut().unwrap().persona.push('x');
        assert!(crate::open_wire(spec.clone()).is_err());
        spec.team.as_mut().unwrap().persona.pop();
        spec.team.as_mut().unwrap().roster.push('x');
        assert!(crate::open_wire(spec).is_err());
    }
}
