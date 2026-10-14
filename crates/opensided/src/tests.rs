use parking_lot::Mutex;
use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};

use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::Response,
};
use chrono::Utc;
use clap::Parser;
use futures_util::future::BoxFuture;
use http_body_util::BodyExt;
use openside_core::{
    AgentRuntime, AgentSession, AgentState, DeferKind, DeferredCall, HostToolHandler, MessageId,
    QueueMode, RunOutcome, RuntimeError, RuntimeEvent, SessionInfo, SessionSpec, TicketId,
    TicketResolution, ToolCall, ToolOutcome,
};
use openside_proto::*;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tower::ServiceExt;
use zeroize::Zeroizing;

use crate::{
    api,
    config::Resolved,
    host::Host,
    tools::{SecretSink, SecretSinkError},
    Config, Daemon,
};
mod access_requests;
mod agents;
mod artifacts;
mod browser;
mod browser_snap;
mod browser_space;
mod browser_tabs;
mod cards;
mod channels;
mod connection_tools;
mod connections;
mod core_gaps;
mod credential_paths;
mod favicons;
mod hire;
mod import_delivery;
mod login;
#[path = "managed_subscriptions/tests.rs"]
mod managed_subscriptions;
mod memory;
mod message_authority;
mod models;
mod onboarding;
mod operator;
mod otp;
mod person_only;
mod policy;
mod preview;
mod profiles;
mod remote;
mod remote_setup;
mod remote_transport;
mod repl;
mod resource_limits;
mod routines;
mod session_help;
mod skills;
mod subagents;
mod sync;
mod takeover;
mod tasks;
mod team;
mod tool_output;
mod tools_api;
mod vault;
mod web;

#[derive(Default)]
struct TestState {
    opened: Vec<SessionSpec>,
    sent: Vec<(SessionId, MessageId, String, QueueMode)>,
    resolved: Vec<(TicketId, String, TicketResolution)>,
    events: HashMap<SessionId, mpsc::UnboundedSender<RuntimeEvent>>,
    ready_deferred: Vec<DeferredCall>,
    auth_failure: bool,
    delivery_store: Option<std::path::PathBuf>,
    discarded: usize,
    parts: Vec<(MessageId, Vec<openside_core::ContentPart>)>,
    controls: Vec<(String, openside_core::SubagentControl)>,
}
#[derive(Clone, Default)]
struct TestRuntime(Arc<Mutex<TestState>>);
struct TestSession {
    runtime: TestRuntime,
    id: SessionId,
    info: SessionInfo,
    events: mpsc::UnboundedReceiver<RuntimeEvent>,
}

impl AgentRuntime for TestRuntime {
    type Session = TestSession;
    async fn open_session(
        &self,
        spec: SessionSpec,
        _: Arc<dyn HostToolHandler>,
    ) -> std::result::Result<TestSession, RuntimeError> {
        let mut state = self.0.lock();
        if state.auth_failure {
            return Err(RuntimeError::AuthUnavailable(
                "test sources unavailable".into(),
            ));
        }
        let (tx, rx) = mpsc::unbounded_channel();
        let info = SessionInfo {
            tools: spec.tools.iter().map(|t| t.name.clone()).collect(),
            pending_deferred: state.ready_deferred.clone(),
            startup_ms: 1,
        };
        let id = spec.session.clone();
        state.opened.push(spec);
        state.events.insert(id.clone(), tx);
        Ok(TestSession {
            runtime: self.clone(),
            id,
            info,
            events: rx,
        })
    }
}
impl AgentSession for TestSession {
    fn info(&self) -> &SessionInfo {
        &self.info
    }
    async fn send(
        &mut self,
        id: MessageId,
        text: String,
        mode: QueueMode,
    ) -> std::result::Result<(), RuntimeError> {
        self.runtime
            .0
            .lock()
            .sent
            .push((self.id.clone(), id.clone(), text, mode));
        self.runtime.emit(&self.id, RuntimeEvent::TurnStarted);
        self.runtime
            .emit(&self.id, RuntimeEvent::MessageAccepted { id });
        Ok(())
    }
    async fn send_with_parts(
        &mut self,
        id: MessageId,
        text: String,
        parts: Vec<openside_core::ContentPart>,
        mode: QueueMode,
        _author_kind: MessageAuthorKind,
    ) -> std::result::Result<(), RuntimeError> {
        if !parts.is_empty() {
            self.runtime.0.lock().parts.push((id.clone(), parts));
        }
        self.send(id, text, mode).await
    }
    async fn abort(&mut self) -> std::result::Result<(), RuntimeError> {
        Ok(())
    }
    async fn resolve_ticket(
        &mut self,
        ticket: TicketId,
        call: String,
        resolution: TicketResolution,
    ) -> std::result::Result<(), RuntimeError> {
        if let Some(path) = self.runtime.0.lock().delivery_store.clone() {
            let mut store = openside_sessions::Store::open(path).unwrap();
            assert!(!store.mark_ticket_delivered(&self.id, &ticket).unwrap());
        }
        self.runtime
            .0
            .lock()
            .resolved
            .push((ticket, call, resolution));
        self.runtime.emit(&self.id, RuntimeEvent::Settled);
        Ok(())
    }
    async fn discard_deferred(&mut self) -> std::result::Result<(), RuntimeError> {
        self.runtime.0.lock().discarded += 1;
        Ok(())
    }
    async fn control_subagent(
        &mut self,
        subagent: String,
        control: openside_core::SubagentControl,
    ) -> std::result::Result<(), RuntimeError> {
        self.runtime.0.lock().controls.push((subagent, control));
        Ok(())
    }
    async fn next_event(&mut self) -> Option<RuntimeEvent> {
        self.events.recv().await
    }
    async fn shutdown(&mut self) -> std::result::Result<(), RuntimeError> {
        self.runtime.0.lock().events.remove(&self.id);
        Ok(())
    }
}
impl TestRuntime {
    fn emit(&self, id: &SessionId, event: RuntimeEvent) {
        self.0.lock().events.get(id).unwrap().send(event).unwrap();
    }
    fn crash(&self, id: &SessionId) {
        self.0.lock().events.remove(id);
    }
}

pub(crate) fn resolved_config(root: &Path) -> Resolved {
    let mut product = openside_config::Config::default();
    product.agent.command = Some(vec!["unused-test-command".into()]);
    product.agent.orchestration = openside_config::Orchestration::Single;
    product.browser.executable = Some(root.join("missing-browser"));
    let loaded = openside_config::Loaded {
        config: product,
        paths: openside_config::Paths {
            config_file: root.join("config.toml"),
            data_dir: root.join("data"),
            state_dir: root.join("state"),
            runtime_dir: root.join("run"),
        },
        source: None,
        warnings: Vec::new(),
    };
    let mut config = Config::default().resolve_loaded(loaded).unwrap();
    // No host sandbox discovery controls a unit test's execution path.
    config.bwrap = None;
    config
}
/// Chromium for in-process daemon tests. The daemon's browser is detached (it survives a
/// daemon restart), which used to mean a test process killed mid-test — abort, SIGKILL, a
/// harness timeout — orphaned Chromium under the init process. `launch_config` therefore
/// sets `die_with_spawner` under cfg(test), asking the launcher for kernel parent-death
/// supervision (PR_SET_PDEATHSIG) tied to the spawning thread; a daemon restart inside one
/// test is unaffected.
fn test_browser() -> std::result::Result<std::path::PathBuf, String> {
    openside_config::discover_browser(&openside_config::Config::default())
        .map_err(|error| error.to_string())
}
async fn daemon(root: &Path, runtime: &TestRuntime) -> Daemon {
    Daemon::open_runtime(resolved_config(root), runtime.clone())
        .await
        .unwrap()
}
async fn create_session(
    daemon: &Daemon,
    name: &str,
    mode: PolicyMode,
    workspace: Option<&Path>,
) -> SessionId {
    let id = SessionId::new(name);
    let stored = id.clone();
    let workspace = workspace.map(|path| {
        std::fs::canonicalize(path)
            .unwrap()
            .to_string_lossy()
            .into_owned()
    });
    daemon
        .shared
        .db
        .store(move |store| {
            store.create_session(
                stored,
                "Test",
                std::any::type_name::<TestRuntime>(),
                mode,
                workspace,
            )?;
            Ok(())
        })
        .await
        .unwrap();
    id
}
async fn host(daemon: &Daemon, session: &SessionId) -> Host {
    Host::new(
        daemon.shared.clone(),
        &daemon.shared.db.state(session).await.unwrap(),
    )
}
fn call(name: &str, args: Value, id: &str) -> ToolCall {
    ToolCall {
        tool_call_id: id.into(),
        name: name.into(),
        arguments_json: args.to_string(),
        subagent: None,
        approval_ticket: None,
    }
}
async fn opened(
    daemon: &Daemon,
    session: &SessionId,
    kind: TicketKind,
    id: &str,
    deadline: chrono::DateTime<Utc>,
) -> Ticket {
    let ticket = Ticket {
        hire: None,
        id: TicketId::new(id),
        session_id: session.clone(),
        tool_call_id: format!("call-{id}"),
        deferral: TicketDeferral::External,
        kind,
        summary: "Human step".into(),
        instructions: None,
        options: Vec::new(),
        origin: TicketOrigin {
            subagent_id: None,
            device_id: None,
            driver_ids: Vec::new(),
        },
        tool: None,
        target: (kind == TicketKind::SecretInput).then(|| TicketTarget {
            tab_id: Some("tab".into()),
            origin: Some("https://example.test".into()),
            field_ref: Some("e1".into()),
        }),
        resume_when: None,
        created_at: Utc::now(),
        expires_at: deadline,
        status: TicketStatus::Pending,
        resolution: None,
        otp: None,
        otp_register: None,
        login: None,
        choice: None,
        approval: None,
        card: None,
        form: None,
    };
    daemon
        .shared
        .emit(
            session,
            None,
            EventKind::TicketOpened {
                ticket: Box::new(ticket.clone()),
            },
        )
        .await
        .unwrap();
    ticket
}
fn request(method: &str, path: &str, body: Value, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::from(body.to_string())).unwrap()
}
async fn json_body(response: Response) -> Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

async fn person_resolve(daemon: &Daemon, id: &TicketId, value: Value) -> Response {
    let (_, token) = daemon
        .shared
        .db
        .pair(vec![
            CapabilityScope::Read,
            CapabilityScope::Approve,
            CapabilityScope::HumanStep,
        ])
        .await
        .unwrap();
    api::router(daemon.shared.clone(), false)
        .oneshot(request(
            "POST",
            &format!("/v1/tickets/{id}/resolve"),
            json!({"value":value}),
            Some(&token),
        ))
        .await
        .unwrap()
}
async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
async fn wait_event(daemon: &Daemon, id: &SessionId, predicate: impl Fn(&EventKind) -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if daemon
                .shared
                .db
                .events(id, 0)
                .await
                .unwrap()
                .iter()
                .any(|event| predicate(&event.kind))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn every_protocol_method_and_path_is_routed() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    daemon
        .shared
        .emit(
            &session,
            None,
            EventKind::MessageQueued {
                message_id: MessageId::new("message"),
                text: "pending".into(),
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
        .emit(
            &session,
            None,
            EventKind::SubagentStarted {
                subagent_id: "child".into(),
                parent_tool_call_id: "delegate-call".into(),
                task: "routed".into(),
                model: "test/model".into(),
                tools: Vec::new(),
                background: false,
            },
        )
        .await
        .unwrap();
    opened(
        &daemon,
        &session,
        TicketKind::Takeover,
        "ticket",
        Utc::now() + chrono::Duration::hours(1),
    )
    .await;
    let doomed_session = create_session(&daemon, "doomed-session", PolicyMode::Guard, None).await;
    let scheduler = daemon.shared.routines.get().unwrap();
    let seed = |name: &'static str| {
        let scheduler = scheduler.clone();
        async move {
            scheduler
                .create(CreateRoutineRequest {
                    name: name.into(),
                    schedule: RoutineSchedule::Interval { secs: 3600 },
                    action: RoutineAction::NewSession {
                        prompt: "routed".into(),
                        mode: None,
                        workspace: None,
                        model: None,
                    },
                    enabled: Some(false),
                    catch_up: None,
                })
                .await
                .unwrap()
                .id
        }
    };
    let (routine, doomed_routine) = (seed("routed").await, seed("doomed").await);
    let remember = |text: &'static str| {
        let memory = daemon.shared.memory.clone();
        async move {
            memory
                .run(move |store| store.create(openside_memory::NewMemory::note(text), "test"))
                .await
                .unwrap()
                .id
        }
    };
    let (memory, doomed_memory) = (remember("routed").await, remember("doomed").await);
    // Handlers answer 404 for unknown resources too; only a matched route carries this marker.
    let router = api::router(daemon.shared.clone(), true).route_layer(
        axum::middleware::map_response(|mut response: axum::response::Response| async move {
            response
                .headers_mut()
                .insert("x-test-routed", axum::http::HeaderValue::from_static("1"));
            response
        }),
    );
    for route in ROUTES {
        // Destructive routes get their own victim so later routes still see live resources.
        let deleting = route.method == HttpMethod::Delete;
        let id = if route.path.starts_with("/v1/tickets/") {
            "ticket"
        } else if route.path.starts_with("/v1/routines/") {
            if deleting {
                doomed_routine.as_str()
            } else {
                routine.as_str()
            }
        } else if route.path.starts_with("/v1/memory/") {
            if deleting {
                doomed_memory.as_str()
            } else {
                memory.as_str()
            }
        } else if deleting && route.path == "/v1/sessions/{id}" {
            doomed_session.as_str()
        } else {
            "session"
        };
        let path = route
            .path
            .replace("{mid}", "message")
            .replace("{thumbnail_id}", "thumbnail")
            .replace("{sid}", "child")
            .replace("{name}", "skill")
            .replace("{key}", "proposal")
            .replace("{origin}", "https%3A%2F%2Fexample.test")
            .replace("{path}", "transcript.jsonl")
            .replace("{provider}", "anthropic")
            .replace("{source}", "claude-code")
            .replace("{kind}", "telegram")
            .replace("{account_id}", "unknown-account")
            .replace("{key_id}", "unknown-key")
            .replace("{space_id}", "unknown-space")
            .replace("{ticket}", "ticket")
            .replace("{id}", id);
        assert!(
            !path.contains('{'),
            "unsubstituted path parameter in {path}"
        );
        let response = router
            .clone()
            .oneshot(request(route.method.as_str(), &path, json!({}), None))
            .await
            .unwrap();
        assert!(
            response.headers().contains_key("x-test-routed"),
            "{} {path} reached no route: {}",
            route.method.as_str(),
            response.status()
        );
        assert_ne!(
            response.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "{} {path}",
            route.method.as_str()
        );
    }
    daemon.shutdown().await;
}

#[tokio::test]
async fn model_is_required_and_workspace_is_canonical() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let router = api::router(daemon.shared.clone(), true);
    let response = router
        .clone()
        .oneshot(request("POST", "/v1/sessions", json!({}), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(response).await["code"], "model_not_configured");
    let body = json!({"model":std::any::type_name::<TestRuntime>(),"mode":"read-only","workspace":root.path().join(".")});
    let response = router
        .clone()
        .oneshot(request("POST", "/v1/sessions", body, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_body(response).await;
    assert_eq!(body["mode"], "read-only");
    assert_eq!(
        body["workspace"],
        std::fs::canonicalize(root.path())
            .unwrap()
            .to_string_lossy()
            .as_ref()
    );
    let response = router
        .oneshot(request(
            "POST",
            "/v1/sessions",
            json!({"model":std::any::type_name::<TestRuntime>(),"workspace":"relative"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    daemon.shutdown().await;
}

#[test]
fn flags_have_no_legacy_runtime_defaults_and_listener_is_loopback_only() {
    assert!(Config::try_parse_from(["opensided"]).is_ok());
    for legacy in ["--approval-mode", "--rpc-mode", "--model"] {
        assert!(Config::try_parse_from(["opensided", legacy, "value"]).is_err());
    }
    let root = tempfile::tempdir().unwrap();
    let resolved = resolved_config(root.path());
    let bad = Config::parse_from(["opensided", "--listen", "0.0.0.0:1234"]);
    assert!(bad.resolve_loaded(resolved.loaded.clone()).is_err());
    for address in ["127.0.0.1:0", "[::1]:0"] {
        assert!(Config::parse_from(["opensided", "--listen", address])
            .resolve_loaded(resolved.loaded.clone())
            .is_ok());
    }
}

#[tokio::test]
async fn capabilities_are_hashed_scoped_persistent_and_revocable() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let (device, token) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Read])
        .await
        .unwrap();
    let router = api::router(daemon.shared.clone(), false);
    assert_eq!(
        router
            .clone()
            .oneshot(request("GET", "/v1/health", json!({}), None))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        router
            .clone()
            .oneshot(request("GET", "/v1/health", json!({}), Some(&token)))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        router
            .clone()
            .oneshot(request("POST", "/v1/sessions", json!({}), Some(&token)))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert!(
        !std::fs::read(daemon.shared.config.data_dir.join("devices.db"))
            .unwrap()
            .windows(token.len())
            .any(|bytes| bytes == token.as_bytes())
    );
    daemon.shared.db.revoke(device).await.unwrap();
    assert_eq!(
        router
            .oneshot(request("GET", "/v1/health", json!({}), Some(&token)))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn ticket_lifecycle_and_scope_by_kind_hs7() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let (_, read) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Read])
        .await
        .unwrap();
    let (_, approve) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Approve])
        .await
        .unwrap();
    let (_, human) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::HumanStep])
        .await
        .unwrap();
    let router = api::router(daemon.shared.clone(), false);
    opened(
        &daemon,
        &session,
        TicketKind::Confirm,
        "confirm",
        Utc::now() + chrono::Duration::hours(1),
    )
    .await;
    opened(
        &daemon,
        &session,
        TicketKind::DeviceApproval,
        "device",
        Utc::now() + chrono::Duration::hours(1),
    )
    .await;
    opened(
        &daemon,
        &session,
        TicketKind::Info,
        "info",
        Utc::now() + chrono::Duration::hours(1),
    )
    .await;
    assert_eq!(
        router
            .clone()
            .oneshot(request(
                "GET",
                "/v1/tickets?status=pending",
                json!({}),
                Some(&read)
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    for (path, token, body) in [
        ("/v1/tickets/confirm/resolve", &read, json!({"value":true})),
        ("/v1/tickets/confirm/resolve", &human, json!({"value":true})),
        ("/v1/tickets/device/done", &approve, json!({})),
        ("/v1/tickets/device/reject", &approve, json!({})),
    ] {
        assert_eq!(
            router
                .clone()
                .oneshot(request("POST", path, body, Some(token)))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let response = router
        .clone()
        .oneshot(request(
            "POST",
            "/v1/tickets/confirm/resolve",
            json!({"value":"approve"}),
            Some(&approve),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await["resolution"],
        json!({"approved":true})
    );
    assert_eq!(
        router
            .clone()
            .oneshot(request(
                "POST",
                "/v1/tickets/confirm/resolve",
                json!({"value":true}),
                Some(&approve)
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    let response = router
        .clone()
        .oneshot(request(
            "POST",
            "/v1/tickets/device/done",
            json!({}),
            Some(&human),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await["resolution"],
        json!({"status":"done"})
    );
    let response = router
        .oneshot(request(
            "POST",
            "/v1/tickets/info/reject",
            json!({"reason":"not now"}),
            Some(&approve),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["status"], "rejected");
    assert_eq!(runtime.0.lock().resolved.len(), 3);
    daemon.shutdown().await;
}

#[tokio::test]
async fn expiry_emits_event_and_delivers_external_expired_result() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    opened(
        &daemon,
        &session,
        TicketKind::Info,
        "expired",
        Utc::now() - chrono::Duration::seconds(1),
    )
    .await;
    crate::tickets::expire(&daemon.shared).await.unwrap();
    let state = daemon.shared.db.state(&session).await.unwrap();
    assert_eq!(
        state.tickets[&TicketId::new("expired")].status,
        TicketStatus::Expired
    );
    assert!(state.tickets[&TicketId::new("expired")]
        .resolution
        .is_none());
    until(|| !runtime.0.lock().resolved.is_empty()).await;
    assert_eq!(
        runtime.0.lock().resolved[0].2,
        TicketResolution::Result {
            for_model_json: json!({"status":"expired"}).to_string()
        }
    );
    wait_event(&daemon, &session, |kind| {
        matches!(kind, EventKind::TicketExpired { .. })
    })
    .await;
    crate::tickets::expire(&daemon.shared).await.unwrap();
    assert_eq!(runtime.0.lock().resolved.len(), 1);
    daemon.shutdown().await;
}

#[tokio::test]
async fn restart_keeps_pending_tickets_and_resolution_resumes_once_hs1() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let first = daemon(root.path(), &runtime).await;
    let session = create_session(&first, "session", PolicyMode::Guard, None).await;
    let ticket = opened(
        &first,
        &session,
        TicketKind::Info,
        "pending",
        Utc::now() + chrono::Duration::hours(1),
    )
    .await;
    let stored = session.clone();
    first
        .shared
        .db
        .store(move |store| {
            Ok(store.put_agent_state(
                &stored,
                &AgentState {
                    format: "test-state".into(),
                    data: "consistent".into(),
                },
            )?)
        })
        .await
        .unwrap();
    first.shutdown().await;
    drop(first);
    let second_runtime = TestRuntime::default();
    second_runtime.0.lock().ready_deferred.push(DeferredCall {
        tool_call_id: ticket.tool_call_id.clone(),
        ticket: ticket.id.clone(),
        kind: DeferKind::External,
    });
    let second = daemon(root.path(), &second_runtime).await;
    assert_eq!(
        second
            .shared
            .db
            .state(&session)
            .await
            .unwrap()
            .session()
            .status,
        SessionStatus::WaitingHuman
    );
    assert!(second_runtime.0.lock().opened.is_empty());
    let router = api::router(second.shared.clone(), true);
    let response = router
        .clone()
        .oneshot(request(
            "POST",
            "/v1/tickets/pending/resolve",
            json!({"value":"answer"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    {
        let state = second_runtime.0.lock();
        assert_eq!(state.resolved.len(), 1);
        assert_eq!(state.resolved[0].1, ticket.tool_call_id);
        assert_eq!(
            state.opened[0].resume_state.as_ref().unwrap().data,
            "consistent"
        );
    }
    assert_eq!(
        router
            .oneshot(request(
                "POST",
                "/v1/tickets/pending/resolve",
                json!({"value":"again"}),
                None
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(second_runtime.0.lock().resolved.len(), 1);
    second.shutdown().await;
}

#[tokio::test]
async fn startup_resumes_terminal_undelivered_ticket_without_queued_messages() {
    let root = tempfile::tempdir().unwrap();
    let first = daemon(root.path(), &TestRuntime::default()).await;
    let mut active_ticket = None;
    for name in ["active", "stopped", "auth-failed"] {
        let session = create_session(&first, name, PolicyMode::Guard, None).await;
        let ticket = opened(
            &first,
            &session,
            TicketKind::Info,
            name,
            Utc::now() + chrono::Duration::hours(1),
        )
        .await;
        first
            .shared
            .emit(
                &session,
                None,
                EventKind::TicketResolved {
                    card: None,
                    ticket_id: ticket.id.clone(),
                    resolution: json!({"answer": "ready"}),
                },
            )
            .await
            .unwrap();
        let stored = session.clone();
        first
            .shared
            .db
            .store(move |store| {
                Ok(store.put_agent_state(
                    &stored,
                    &AgentState {
                        format: "test-state".into(),
                        data: "pending".into(),
                    },
                )?)
            })
            .await
            .unwrap();
        match name {
            "stopped" => {
                first
                    .shared
                    .emit(&session, None, EventKind::SessionStopped {})
                    .await
                    .unwrap();
            }
            "auth-failed" => {
                first
                    .shared
                    .emit(
                        &session,
                        None,
                        EventKind::runtime_error("auth_unavailable", "Unavailable"),
                    )
                    .await
                    .unwrap();
            }
            _ => active_ticket = Some(ticket),
        }
    }
    let ticket = active_ticket.unwrap();
    first.shutdown().await;
    drop(first);
    let runtime = TestRuntime::default();
    runtime.0.lock().ready_deferred.push(DeferredCall {
        ticket: ticket.id.clone(),
        tool_call_id: ticket.tool_call_id.clone(),
        kind: DeferKind::External,
    });
    let second = daemon(root.path(), &runtime).await;
    {
        let state = runtime.0.lock();
        assert_eq!(state.opened.len(), 1);
        assert_eq!(state.opened[0].session, SessionId::new("active"));
        assert!(state.sent.is_empty());
        assert_eq!(
            state.resolved,
            vec![(
                ticket.id,
                ticket.tool_call_id,
                TicketResolution::Result {
                    for_model_json: json!({"answer":"ready"}).to_string(),
                }
            )]
        );
    }
    second.shutdown().await;
}

#[tokio::test]
async fn crash_after_ticket_delivery_recovers_only_interrupted_resolutions() {
    for approval in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let runtime = TestRuntime::default();
        runtime.0.lock().delivery_store = Some(root.path().join("data/events.db"));
        let first = daemon(root.path(), &runtime).await;
        let session = create_session(&first, "session", PolicyMode::Guard, None).await;
        let mut ticket = opened(
            &first,
            &session,
            TicketKind::Info,
            "delivered",
            Utc::now() + chrono::Duration::hours(1),
        )
        .await;
        if approval {
            // Use a second ticket because public ticket identity is immutable.
            ticket.id = TicketId::new("approval");
            ticket.deferral = TicketDeferral::Approval;
            ticket.kind = TicketKind::Confirm;
            ticket.tool = Some(TicketTool {
                name: "write".into(),
                args: json!({}),
            });
            first
                .shared
                .emit(
                    &session,
                    None,
                    EventKind::TicketOpened {
                        ticket: Box::new(ticket.clone()),
                    },
                )
                .await
                .unwrap();
        }
        first
            .shared
            .emit(
                &session,
                None,
                EventKind::TicketResolved {
                    card: None,
                    ticket_id: ticket.id.clone(),
                    resolution: if approval {
                        json!({"approved":true})
                    } else {
                        json!({"status":"filled"})
                    },
                },
            )
            .await
            .unwrap();
        let stored = session.clone();
        first
            .shared
            .db
            .store(move |store| {
                Ok(store.put_agent_state(
                    &stored,
                    &AgentState {
                        format: "test-state".into(),
                        data: "still pending".into(),
                    },
                )?)
            })
            .await
            .unwrap();
        runtime.0.lock().ready_deferred.push(DeferredCall {
            ticket: ticket.id.clone(),
            tool_call_id: ticket.tool_call_id.clone(),
            kind: if approval {
                DeferKind::Approval
            } else {
                DeferKind::External
            },
        });
        first
            .shared
            .command(&session, crate::engine::Action::Resume(ticket.id.clone()))
            .await
            .unwrap();
        assert_eq!(runtime.0.lock().resolved.len(), 1);
        assert_eq!(
            runtime.0.lock().resolved[0].2,
            if approval {
                TicketResolution::Approved
            } else {
                TicketResolution::Result {
                    for_model_json: json!({"status":"filled"}).to_string(),
                }
            }
        );
        runtime.crash(&session);
        wait_event(&first, &session, |kind| {
            matches!(kind,
            EventKind::RuntimeError { code, .. } if code == "interrupted")
        })
        .await;
        first.shutdown().await;
        drop(first);
        let recovered = TestRuntime::default();
        recovered.0.lock().ready_deferred = runtime.0.lock().ready_deferred.clone();
        let second = daemon(root.path(), &recovered).await;
        second
            .shared
            .command(&session, crate::engine::Action::Resume(ticket.id.clone()))
            .await
            .unwrap();
        let expected = if approval {
            TicketResolution::Denied {
                reason: "interrupted_after_resume".into(),
            }
        } else {
            TicketResolution::Result { for_model_json: json!({
                "status":"interrupted",
                "message":"The previous resumption was interrupted; verify state before retrying."
            }).to_string() }
        };
        assert_eq!(
            recovered.0.lock().resolved,
            vec![(ticket.id.clone(), ticket.tool_call_id.clone(), expected)]
        );
        wait_event(&second, &session, |kind| {
            matches!(kind,
            EventKind::RuntimeError { code, .. } if code == "interrupted")
        })
        .await;
        second
            .shared
            .command(&session, crate::engine::Action::Resume(ticket.id))
            .await
            .unwrap();
        assert_eq!(recovered.0.lock().resolved.len(), 1);
        second.shutdown().await;
    }
}

#[tokio::test]
async fn interruption_reports_coalesce_and_survive_partial_recovery() {
    for partial in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let runtime = TestRuntime::default();
        let first = daemon(root.path(), &runtime).await;
        let session = create_session(&first, "session", PolicyMode::Guard, None).await;
        for mid in ["a", "b"] {
            first
                .shared
                .emit(
                    &session,
                    None,
                    EventKind::MessageQueued {
                        message_id: MessageId::new(mid),
                        text: mid.into(),
                        mode: QueueMode::Steer,
                        attachments: Vec::new(),
                        author_kind: MessageAuthorKind::Person,
                        from_agent: None,
                    },
                )
                .await
                .unwrap();
            first
                .shared
                .emit(
                    &session,
                    None,
                    EventKind::MessageInjected {
                        message_id: MessageId::new(mid),
                    },
                )
                .await
                .unwrap();
        }
        if partial {
            first
                .shared
                .emit(
                    &session,
                    None,
                    EventKind::MessageQueued {
                        message_id: MessageId::new("interrupted-report-a"),
                        text: "Recovery covering a, b".into(),
                        mode: QueueMode::Queue,
                        attachments: Vec::new(),
                        author_kind: MessageAuthorKind::Internal,
                        from_agent: None,
                    },
                )
                .await
                .unwrap();
            first
                .shared
                .emit(
                    &session,
                    None,
                    EventKind::MessageCompleted {
                        message_id: MessageId::new("a"),
                        outcome: openside_proto::MessageOutcome::Failed,
                        reason: Some("interrupted".into()),
                    },
                )
                .await
                .unwrap();
        }
        first.shutdown().await;
        drop(first);
        for _ in 0..2 {
            let recovered = daemon(root.path(), &runtime).await;
            let events = recovered.shared.db.events(&session, 0).await.unwrap();
            let reports: Vec<_> = events
                .iter()
                .filter_map(|event| match &event.kind {
                    EventKind::MessageQueued {
                        message_id,
                        text,
                        author_kind: MessageAuthorKind::Internal,
                        ..
                    } if message_id.as_str().starts_with("interrupted-report-") => {
                        Some((message_id, text))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(reports.len(), 1);
            assert_eq!(reports[0].0.as_str(), "interrupted-report-a");
            assert!(reports[0].1.contains("a, b"));
            for mid in ["a", "b"] {
                assert_eq!(
                    events
                        .iter()
                        .filter(|event| matches!(&event.kind,
                            EventKind::MessageCompleted { message_id, reason: Some(reason), .. }
                                if message_id.as_str() == mid && reason == "interrupted"
                        ))
                        .count(),
                    1
                );
            }
            recovered.shutdown().await;
        }
    }
}

#[tokio::test]
async fn restart_reconciles_inflight_once_and_expires_overdue_tickets() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let first = daemon(root.path(), &runtime).await;
    let session = create_session(&first, "session", PolicyMode::Guard, None).await;
    first
        .shared
        .emit(
            &session,
            None,
            EventKind::MessageQueued {
                message_id: MessageId::new("inflight"),
                text: "task".into(),
                mode: QueueMode::Queue,
                attachments: Vec::new(),
                author_kind: MessageAuthorKind::Person,
                from_agent: None,
            },
        )
        .await
        .unwrap();
    first
        .shared
        .emit(
            &session,
            None,
            EventKind::MessageInjected {
                message_id: MessageId::new("inflight"),
            },
        )
        .await
        .unwrap();
    opened(
        &first,
        &session,
        TicketKind::Confirm,
        "overdue",
        Utc::now() - chrono::Duration::seconds(1),
    )
    .await;
    first.shutdown().await;
    drop(first);
    let second = daemon(root.path(), &runtime).await;
    let events = second.shared.db.events(&session, 0).await.unwrap();
    assert_eq!(events.iter().filter(|e| matches!(&e.kind, EventKind::MessageCompleted { reason: Some(reason), .. } if reason == "interrupted")).count(), 1);
    assert_eq!(
        second.shared.db.state(&session).await.unwrap().tickets[&TicketId::new("overdue")].status,
        TicketStatus::Expired
    );
    assert!(runtime.0.lock().sent.is_empty());
    second.shutdown().await;
    drop(second);
    let third = daemon(root.path(), &runtime).await;
    assert_eq!(
        third
            .shared
            .db
            .events(&session, 0)
            .await
            .unwrap()
            .iter()
            .filter(|e| matches!(e.kind, EventKind::MessageCompleted { .. }))
            .count(),
        1
    );
    third.shutdown().await;
}

#[tokio::test]
async fn ask_user_defers_to_info_and_options_validate_resolution() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let result = host(&daemon, &session)
        .await
        .call(call(
            "ask_user",
            json!({"question":"Choose","options":["a","b"]}),
            "ask",
        ))
        .await;
    let ToolOutcome::Deferred {
        ticket,
        kind: DeferKind::External,
    } = result
    else {
        panic!("expected deferral");
    };
    let path = format!("/v1/tickets/{ticket}/resolve");
    let router = api::router(daemon.shared.clone(), true);
    assert_eq!(
        router
            .clone()
            .oneshot(request("POST", &path, json!({"value":"invalid"}), None))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let response = router
        .oneshot(request("POST", &path, json!({"value":"a"}), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await["resolution"],
        json!({"answer":"a"})
    );
    assert_eq!(
        runtime.0.lock().resolved[0].2,
        TicketResolution::Result {
            for_model_json: json!({"answer":"a"}).to_string()
        }
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn approval_recall_requires_session_call_tool_and_resolved_approval() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, Some(root.path())).await;
    let other = create_session(&daemon, "other", PolicyMode::Guard, Some(root.path())).await;
    let path = outside.path().join("file");
    std::fs::write(&path, "before").unwrap();
    let host = host(&daemon, &session).await;
    let mut invocation = call(
        "write",
        json!({"path":path,"content":"after"}),
        "write-call",
    );
    let ToolOutcome::Deferred {
        ticket,
        kind: DeferKind::Approval,
    } = host.call(invocation.clone()).await
    else {
        panic!("expected approval");
    };
    invocation.approval_ticket = Some(ticket.clone());
    assert!(matches!(
        host.call(invocation.clone()).await,
        ToolOutcome::Denied { .. }
    ));
    assert_eq!(
        person_resolve(&daemon, &ticket, json!(true)).await.status(),
        StatusCode::OK
    );
    let mut wrong = invocation.clone();
    wrong.tool_call_id = "other-call".into();
    assert!(matches!(host.call(wrong).await, ToolOutcome::Denied { .. }));
    let mut wrong = invocation.clone();
    wrong.name = "read".into();
    wrong.arguments_json = json!({"path":path}).to_string();
    assert!(matches!(host.call(wrong).await, ToolOutcome::Denied { .. }));
    assert!(matches!(
        crate::tests::host(&daemon, &other)
            .await
            .call(invocation.clone())
            .await,
        ToolOutcome::Denied { .. }
    ));
    assert!(matches!(
        host.call(invocation).await,
        ToolOutcome::Done { .. }
    ));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "after");
    daemon.shutdown().await;
}

#[tokio::test]
async fn approval_binding_rejects_changed_args_and_is_single_use_across_restart() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let first = daemon(root.path(), &runtime).await;
    let session = create_session(&first, "session", PolicyMode::Guard, Some(root.path())).await;
    let path = outside.path().join("approved-file");
    std::fs::write(&path, "before").unwrap();
    let host = host(&first, &session).await;
    let mut invocation = call(
        "write",
        json!({
            "path": path, "content": "approved"
        }),
        "bound-write",
    );
    let ToolOutcome::Deferred { ticket, .. } = host.call(invocation.clone()).await else {
        panic!("expected approval");
    };
    first
        .shared
        .emit(
            &session,
            None,
            EventKind::TicketResolved {
                card: None,
                ticket_id: ticket.clone(),
                resolution: json!({"approved": true}),
            },
        )
        .await
        .unwrap();
    invocation.approval_ticket = Some(ticket);
    let mut changed = invocation.clone();
    changed.arguments_json = json!({
        "path": path, "content": "changed"
    })
    .to_string();
    assert!(matches!(host.call(changed).await,
        ToolOutcome::Denied { reason, .. } if reason == "approval_args_mismatch"));
    let mut changed = invocation.clone();
    changed.arguments_json = json!({
        "path": outside.path().join("unapproved-file"), "content": "approved"
    })
    .to_string();
    assert!(matches!(host.call(changed).await,
        ToolOutcome::Denied { reason, .. } if reason == "approval_args_mismatch"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "before");
    invocation.arguments_json = format!(
        "{{\"content\":\"approved\",\"path\":{}}}",
        serde_json::to_string(&path).unwrap()
    );
    let (a, b) = tokio::join!(host.call(invocation.clone()), host.call(invocation.clone()));
    assert!(matches!((&a, &b),
        (ToolOutcome::Done { .. }, ToolOutcome::Denied { reason, .. })
        | (ToolOutcome::Denied { reason, .. }, ToolOutcome::Done { .. })
        if reason == "approval_already_used"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "approved");
    drop(host);
    first.shutdown().await;
    drop(first);
    std::fs::write(&path, "changed after execution").unwrap();
    let second = daemon(root.path(), &TestRuntime::default()).await;
    assert!(
        matches!(crate::tests::host(&second, &session).await.call(invocation).await,
        ToolOutcome::Denied { reason, .. } if reason == "approval_already_used")
    );
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "changed after execution"
    );
    second.shutdown().await;
}

#[tokio::test]
async fn approval_binding_rejects_changed_policy_action_target() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, Some(root.path())).await;
    let original = outside.path().join("original");
    let replacement = outside.path().join("replacement");
    let link = outside.path().join("link");
    std::fs::write(&original, "original").unwrap();
    std::fs::write(&replacement, "replacement").unwrap();
    std::os::unix::fs::symlink(&original, &link).unwrap();
    let host = host(&daemon, &session).await;
    let mut invocation = call(
        "write",
        json!({"path":link,"content":"changed"}),
        "retarget",
    );
    let ToolOutcome::Deferred { ticket, .. } = host.call(invocation.clone()).await else {
        panic!("expected approval");
    };
    daemon
        .shared
        .emit(
            &session,
            None,
            EventKind::TicketResolved {
                card: None,
                ticket_id: ticket.clone(),
                resolution: json!({"approved": true}),
            },
        )
        .await
        .unwrap();
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&replacement, &link).unwrap();
    invocation.approval_ticket = Some(ticket);
    assert!(matches!(host.call(invocation).await,
        ToolOutcome::Denied { reason, .. } if reason == "approval_args_mismatch"));
    assert_eq!(std::fs::read_to_string(original).unwrap(), "original");
    assert_eq!(std::fs::read_to_string(replacement).unwrap(), "replacement");
    daemon.shutdown().await;
}

#[tokio::test]
async fn policy_deny_cannot_be_approved() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, Some(root.path())).await;
    let path = outside.path().join("file");
    std::fs::write(&path, "before").unwrap();
    let mut host = host(&daemon, &session).await;
    let mut invocation = call("write", json!({"path":path,"content":"after"}), "write");
    let ToolOutcome::Deferred { ticket, .. } = host.call(invocation.clone()).await else {
        panic!("expected deferral");
    };
    daemon
        .shared
        .emit(
            &session,
            None,
            EventKind::TicketResolved {
                card: None,
                ticket_id: ticket.clone(),
                resolution: json!({"approved":true}),
            },
        )
        .await
        .unwrap();
    invocation.approval_ticket = Some(ticket);
    host.env.policy.layers.push(openside_policy::RuleLayer {
        rules: vec![openside_policy::Rule::new(
            openside_policy::Effect::Deny,
            openside_policy::Capability::FsWrite,
            "**",
        )],
    });
    assert!(matches!(
        host.call(invocation).await,
        ToolOutcome::Denied { .. }
    ));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "before");
    daemon.shutdown().await;
}

#[tokio::test]
async fn readonly_hides_write_edit_bash_and_denies_unexposed_calls() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::ReadOnly, Some(root.path())).await;
    let host = host(&daemon, &session).await;
    let names: Vec<_> = host.specs().into_iter().map(|tool| tool.name).collect();
    for hidden in ["write", "edit", "bash"] {
        assert!(!names.iter().any(|name| name == hidden));
        assert!(matches!(
            host.call(call(hidden, json!({}), "call")).await,
            ToolOutcome::Denied { .. }
        ));
    }
    assert!(names.iter().any(|name| name == "read"));
    assert!(names.iter().any(|name| name == "ask_user"));
    assert!(matches!(
        host.call(call("unknown", json!({}), "call")).await,
        ToolOutcome::Denied { .. }
    ));
    daemon.shutdown().await;
}

#[tokio::test]
async fn required_missing_bwrap_hides_bash() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let mut config = resolved_config(root.path());
    config.loaded.config.sandbox.required = true;
    config.bwrap = None;
    let daemon = Daemon::open_runtime(config, runtime).await.unwrap();
    let session = create_session(&daemon, "session", PolicyMode::Full, Some(root.path())).await;
    assert!(!host(&daemon, &session)
        .await
        .specs()
        .iter()
        .any(|tool| tool.name == "bash"));
    daemon.shutdown().await;
}

#[tokio::test]
async fn selected_bwrap_start_failure_never_runs_unsandboxed() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let mut config = resolved_config(root.path());
    config.bwrap = Some(root.path().join("missing-bwrap"));
    let daemon = Daemon::open_runtime(config, runtime).await.unwrap();
    let session = create_session(&daemon, "session", PolicyMode::Full, Some(root.path())).await;
    let result = host(&daemon, &session)
        .await
        .call(call(
            "bash",
            json!({"command":"touch should-not-exist"}),
            "shell",
        ))
        .await;
    assert!(matches!(result, ToolOutcome::Denied { .. }));
    assert!(!root.path().join("should-not-exist").exists());
    daemon.shutdown().await;
}

#[test]
fn output_limit_counts_escaped_json_bytes_and_keeps_retrieval_hint() {
    for value in [
        json!({"content":"x".repeat(65536)}),
        json!({"content":"\"\n한".repeat(20000)}),
    ] {
        let output = crate::host::limit_output(value);
        assert!(output.len() <= crate::host::OUTPUT_LIMIT);
        let value: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["truncated"], true);
        assert!(value["how_to_get_more"]
            .as_str()
            .unwrap()
            .contains("offset/limit"));
    }
}

#[tokio::test]
async fn file_tools_validate_edit_and_redact_content_in_events_and_audit() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, Some(root.path())).await;
    let host = host(&daemon, &session).await;
    assert!(matches!(
        host.call(call(
            "write",
            json!({"path":"file","content":"before"}),
            "write"
        ))
        .await,
        ToolOutcome::Done { .. }
    ));
    assert!(matches!(
        host.call(call(
            "edit",
            json!({"path":"file","old_text":"before","new_text":"after"}),
            "edit"
        ))
        .await,
        ToolOutcome::Done { .. }
    ));
    assert!(matches!(
        host.call(call(
            "edit",
            json!({"path":"file","old_text":"missing","new_text":"x"}),
            "edit-bad"
        ))
        .await,
        ToolOutcome::Denied { .. }
    ));
    let ToolOutcome::Done { for_model_json } = host
        .call(call("read", json!({"path":"file"}), "read"))
        .await
    else {
        panic!("expected read");
    };
    assert!(for_model_json.contains("after"));
    let audit = std::fs::read_to_string(daemon.shared.config.data_dir.join("audit.jsonl")).unwrap();
    assert!(!audit.contains("before"));
    assert!(!audit.contains("after"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(daemon.shared.config.data_dir.join("audit.jsonl"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    daemon.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_escape_requires_approval_and_dangling_symlink_is_denied() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, Some(root.path())).await;
    std::fs::write(outside.path().join("file"), "outside").unwrap();
    std::os::unix::fs::symlink(outside.path().join("file"), root.path().join("link")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("absent"), root.path().join("dangling"))
        .unwrap();
    let host = host(&daemon, &session).await;
    assert!(matches!(
        host.call(call("read", json!({"path":"link"}), "read"))
            .await,
        ToolOutcome::Deferred {
            kind: DeferKind::Approval,
            ..
        }
    ));
    assert!(matches!(
        host.call(call(
            "write",
            json!({"path":"dangling","content":"x"}),
            "write"
        ))
        .await,
        ToolOutcome::Denied { .. }
    ));
    assert!(!outside.path().join("absent").exists());
    daemon.shutdown().await;
}

struct CanarySink {
    filled: Arc<Mutex<usize>>,
    expected: &'static str,
    mismatch: bool,
}
impl SecretSink for CanarySink {
    fn fill<'a>(
        &'a self,
        _: &'a TicketId,
        _: &'a SessionId,
        target: &'a TicketTarget,
        value: Zeroizing<String>,
    ) -> BoxFuture<'a, std::result::Result<(), SecretSinkError>> {
        Box::pin(async move {
            if self.mismatch {
                return Err(SecretSinkError {
                    code: "origin_mismatch",
                });
            }
            assert_eq!(target.origin.as_deref(), Some("https://example.test"));
            assert_eq!(value.as_str(), self.expected);
            *self.filled.lock() += 1;
            Ok(())
        })
    }
}

#[tokio::test]
async fn secret_input_canary_never_reaches_events_database_audit_or_runtime() {
    const CANARY: &str = "canary-secret-do-not-persist-937";
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    opened(
        &daemon,
        &session,
        TicketKind::SecretInput,
        "secret",
        Utc::now() + chrono::Duration::minutes(5),
    )
    .await;
    let filled = Arc::new(Mutex::new(0));
    *daemon.shared.registry.secret_sink.write() = Some(Arc::new(CanarySink {
        filled: filled.clone(),
        expected: CANARY,
        mismatch: false,
    }));
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request(
            "POST",
            "/v1/tickets/secret/resolve",
            json!({"value":CANARY}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = json_body(response).await;
    assert_eq!(response["resolution"], json!({"status":"filled"}));
    assert!(!response.to_string().contains(CANARY));
    assert_eq!(*filled.lock(), 1);
    let events = daemon.shared.db.events(&session, 0).await.unwrap();
    assert!(!serde_json::to_string(&events).unwrap().contains(CANARY));
    assert_eq!(
        runtime.0.lock().resolved[0].2,
        TicketResolution::Result {
            for_model_json: json!({"status":"filled"}).to_string()
        }
    );
    assert!(!format!("{:?}", runtime.0.lock().opened).contains(CANARY));
    for entry in std::fs::read_dir(&daemon.shared.config.data_dir).unwrap() {
        let entry = entry.unwrap();
        if entry.path().is_file() {
            let bytes = std::fs::read(entry.path()).unwrap();
            assert!(!bytes
                .windows(CANARY.len())
                .any(|bytes| bytes == CANARY.as_bytes()));
        }
    }
    daemon.shutdown().await;
}

#[tokio::test]
async fn secret_input_without_sink_is_501_and_origin_mismatch_stays_pending() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    *daemon.shared.registry.secret_sink.write() = None;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    opened(
        &daemon,
        &session,
        TicketKind::SecretInput,
        "secret",
        Utc::now() + chrono::Duration::minutes(5),
    )
    .await;
    let router = api::router(daemon.shared.clone(), true);
    let response = router
        .clone()
        .oneshot(request(
            "POST",
            "/v1/tickets/secret/resolve",
            json!({"value":"canary"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(
        json_body(response).await["code"],
        "secret_input_unavailable"
    );
    *daemon.shared.registry.secret_sink.write() = Some(Arc::new(CanarySink {
        filled: Default::default(),
        expected: "canary",
        mismatch: true,
    }));
    let response = router
        .oneshot(request(
            "POST",
            "/v1/tickets/secret/resolve",
            json!({"value":"canary"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(response).await["code"], "origin_mismatch");
    assert_eq!(
        daemon.shared.db.state(&session).await.unwrap().tickets[&TicketId::new("secret")].status,
        TicketStatus::Pending
    );
    assert!(runtime.0.lock().resolved.is_empty());
    daemon.shutdown().await;
}

/// A site that keeps rejecting codes cannot keep the human typing forever: the fourth request
/// for one exact field is refused, while another field of the same page is still allowed.
#[tokio::test]
async fn a_fourth_secret_input_request_for_the_same_field_is_refused_but_another_field_is_not() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), true);
    let step = |field: &str, id: &str| {
        call(
            "request_human_step",
            json!({"kind":"secret_input","summary":"Enter the code","tab_id":"tab",
                "origin":"https://example.test","field_ref":field}),
            id,
        )
    };
    for attempt in 0..3 {
        let ToolOutcome::Deferred { ticket, .. } = host(&daemon, &session)
            .await
            .call(step("e1", &format!("attempt-{attempt}")))
            .await
        else {
            panic!("attempt {attempt} must open a ticket");
        };
        let response = router
            .clone()
            .oneshot(request(
                "POST",
                &format!("/v1/tickets/{ticket}/reject"),
                json!({"reason":"the site refused the code"}),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    let fourth = host(&daemon, &session)
        .await
        .call(step("e1", "attempt-3"))
        .await;
    assert!(matches!(fourth, ToolOutcome::Failed { .. }), "{fourth:?}");
    assert_eq!(
        daemon
            .shared
            .db
            .state(&session)
            .await
            .unwrap()
            .tickets
            .len(),
        3
    );
    assert!(matches!(
        host(&daemon, &session)
            .await
            .call(step("e2", "other-field"))
            .await,
        ToolOutcome::Deferred { .. }
    ));
    daemon.shutdown().await;
}

/// Nobody answering a code or device-approval request ends it on the kind's own lifetime and
/// tells the agent; a code typed after that is refused before it can reach the page.
#[tokio::test]
async fn unanswered_secret_input_and_device_approval_expire_on_their_lifetime_and_a_late_code_is_refused(
) {
    const LATE: &str = "late-code-canary-5521";
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let mut config = resolved_config(root.path());
    config.loaded.config.tickets.secret_input_ttl_secs = 1;
    config.loaded.config.tickets.device_approval_ttl_secs = 1;
    let daemon = Daemon::open_runtime(config, runtime.clone()).await.unwrap();
    let filled = Arc::new(Mutex::new(0));
    *daemon.shared.registry.secret_sink.write() = Some(Arc::new(CanarySink {
        filled: filled.clone(),
        expected: LATE,
        mismatch: false,
    }));
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let mut tickets = Vec::new();
    for (id, args) in [
        (
            "code",
            json!({"kind":"secret_input","summary":"Enter the code","tab_id":"tab",
                "origin":"https://example.test","field_ref":"e1"}),
        ),
        (
            "phone",
            json!({"kind":"device_approval","summary":"Approve on your phone",
                "resume_when":{"manual":true}}),
        ),
    ] {
        let ToolOutcome::Deferred { ticket, .. } = host(&daemon, &session)
            .await
            .call(call("request_human_step", args, id))
            .await
        else {
            panic!("{id} must open a ticket");
        };
        tickets.push(ticket);
    }
    let state = daemon.shared.db.state(&session).await.unwrap();
    for ticket in &tickets {
        let ticket = &state.tickets[ticket];
        assert_eq!(
            ticket.expires_at - ticket.created_at,
            chrono::Duration::seconds(1)
        );
    }
    tokio::time::sleep(Duration::from_millis(1100)).await;
    crate::tickets::expire(&daemon.shared).await.unwrap();
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request(
            "POST",
            &format!("/v1/tickets/{}/resolve", tickets[0]),
            json!({"value":LATE}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(*filled.lock(), 0);
    until(|| runtime.0.lock().resolved.len() == 2).await;
    let expired = TicketResolution::Result {
        for_model_json: json!({"status":"expired"}).to_string(),
    };
    assert!(runtime
        .0
        .lock()
        .resolved
        .iter()
        .all(|resolved| resolved.2 == expired));
    let state = daemon.shared.db.state(&session).await.unwrap();
    assert!(tickets
        .iter()
        .all(|ticket| state.tickets[ticket].status == TicketStatus::Expired));
    daemon.shutdown().await;
}

#[tokio::test]
async fn takeover_without_tab_returns_needs_desktop_and_done_can_finish_it() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let ToolOutcome::Deferred { ticket, .. } = host(&daemon, &session)
        .await
        .call(call(
            "request_human_step",
            json!({"kind":"takeover","summary":"Use desktop"}),
            "takeover",
        ))
        .await
    else {
        panic!("expected ticket");
    };
    let router = api::router(daemon.shared.clone(), true);
    let response = router
        .clone()
        .oneshot(request(
            "GET",
            &format!("/v1/tickets/{ticket}/takeover"),
            json!({}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(response).await["code"], "needs_desktop");
    assert_eq!(
        router
            .oneshot(request(
                "POST",
                &format!("/v1/tickets/{ticket}/done"),
                json!({}),
                None
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn pending_tickets_hold_queue_and_stop_rejects_them() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    opened(
        &daemon,
        &session,
        TicketKind::Info,
        "pending",
        Utc::now() + chrono::Duration::hours(1),
    )
    .await;
    let router = api::router(daemon.shared.clone(), true);
    assert_eq!(
        router
            .clone()
            .oneshot(request(
                "POST",
                "/v1/sessions/session/messages",
                json!({"text":"waiting","mode":"steer"}),
                None
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::ACCEPTED
    );
    assert!(runtime.0.lock().sent.is_empty());
    assert_eq!(
        router
            .oneshot(request(
                "POST",
                "/v1/sessions/session/stop",
                json!({}),
                None
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let state = daemon.shared.db.state(&session).await.unwrap();
    assert_eq!(state.status, SessionStatus::Stopped);
    assert_eq!(state.queue.pending().len(), 1);
    assert_eq!(
        state.tickets[&TicketId::new("pending")].resolution,
        Some(json!({"status":"rejected","reason":"stopped"}))
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn steer_is_sent_while_running_and_followup_waits_for_settled() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), true);
    for (id, mode) in [("first", "queue"), ("later", "queue"), ("steer", "steer")] {
        let mut submission = request(
            "POST",
            "/v1/sessions/session/messages",
            json!({"message_id":id,"text":id,"mode":mode}),
            None,
        );
        if id == "later" {
            submission
                .headers_mut()
                .insert("x-openside-client", "mcp".parse().unwrap());
        }
        assert_eq!(
            router.clone().oneshot(submission).await.unwrap().status(),
            StatusCode::ACCEPTED
        );
    }
    assert_eq!(
        runtime
            .0
            .lock()
            .sent
            .iter()
            .map(|entry| entry.1.as_str())
            .collect::<Vec<_>>(),
        ["first", "steer"]
    );
    runtime.emit(
        &session,
        RuntimeEvent::MessageResult {
            id: MessageId::new("first"),
            outcome: RunOutcome::Completed,
            reason: None,
        },
    );
    runtime.emit(
        &session,
        RuntimeEvent::MessageResult {
            id: MessageId::new("steer"),
            outcome: RunOutcome::Completed,
            reason: None,
        },
    );
    runtime.emit(&session, RuntimeEvent::Settled);
    until(|| runtime.0.lock().sent.len() == 3).await;
    assert_eq!(runtime.0.lock().sent[2].1.as_str(), "later");
    daemon.shutdown().await;
}

#[tokio::test]
async fn process_crash_closes_inflight_and_lazily_reopens_with_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), true);
    assert_eq!(
        router
            .clone()
            .oneshot(request(
                "POST",
                "/v1/sessions/session/messages",
                json!({"message_id":"first","text":"first","mode":"queue"}),
                None
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::ACCEPTED
    );
    runtime.emit(
        &session,
        RuntimeEvent::StateSnapshot {
            state: AgentState {
                format: "test-state".into(),
                data: "checkpoint".into(),
            },
        },
    );
    wait_event(&daemon, &session, |kind| {
        matches!(kind, EventKind::TurnStarted {})
    })
    .await;
    runtime.crash(&session);
    wait_event(&daemon, &session, |kind| matches!(kind, EventKind::MessageCompleted { reason: Some(reason), .. } if reason == "interrupted")).await;
    assert_eq!(runtime.0.lock().opened.len(), 1);
    assert_eq!(
        router
            .oneshot(request(
                "POST",
                "/v1/sessions/session/messages",
                json!({"message_id":"second","text":"second","mode":"queue"}),
                None
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::ACCEPTED
    );
    {
        let state = runtime.0.lock();
        assert_eq!(state.opened.len(), 2);
        assert_eq!(
            state.opened[1].resume_state.as_ref().unwrap().data,
            "checkpoint"
        );
        assert_eq!(state.sent.len(), 2);
    }
    daemon.shutdown().await;
}

#[tokio::test]
async fn authentication_failure_is_fail_closed_until_a_send_finds_credentials_again() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    runtime.0.lock().auth_failure = true;
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), true);
    let send = |id: &'static str| {
        let router = router.clone();
        async move {
            let response = router
                .oneshot(request(
                    "POST",
                    "/v1/sessions/session/messages",
                    json!({"message_id":id,"text":"task","mode":"queue"}),
                    None,
                ))
                .await
                .unwrap();
            (response.status(), json_body(response).await)
        }
    };
    // Credentials unavailable: the send says so instead of a generic conflict, repeatedly.
    for id in ["first", "second"] {
        let (status, body) = send(id).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["code"], "auth_unavailable");
    }
    assert!(daemon.shared.auth_failed(&session).await.unwrap());
    assert_eq!(
        daemon.shared.db.state(&session).await.unwrap().status,
        SessionStatus::Stopped
    );
    assert!(runtime.0.lock().opened.is_empty());
    // Credentials are back: the next send restarts the runtime and clears the latch.
    runtime.0.lock().auth_failure = false;
    assert_eq!(send("third").await.0, StatusCode::ACCEPTED);
    assert!(!daemon.shared.auth_failed(&session).await.unwrap());
    assert_eq!(runtime.0.lock().opened.len(), 1);
    // A failure reported by a running sidecar latches again and a later send recovers too.
    runtime.emit(
        &session,
        RuntimeEvent::Error {
            error: RuntimeError::AuthUnavailable("pool exhausted".into()),
        },
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while !daemon.shared.auth_failed(&session).await.unwrap() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(send("fourth").await.0, StatusCode::ACCEPTED);
    assert!(!daemon.shared.auth_failed(&session).await.unwrap());
    assert_eq!(runtime.0.lock().opened.len(), 2);
    daemon.shutdown().await;
}

#[tokio::test]
async fn sse_last_event_id_replays_only_newer_durable_events() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let initial = daemon
        .shared
        .db
        .events(&session, 0)
        .await
        .unwrap()
        .last()
        .unwrap()
        .seq;
    let request = Request::builder()
        .uri("/v1/sessions/session/events")
        .header("last-event-id", "0")
        .body(Body::empty())
        .unwrap();
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    let greeting = body.frame().await.unwrap().unwrap().into_data().unwrap();
    assert_eq!(std::str::from_utf8(&greeting).unwrap(), ": ok\n\n");
    let frame = body.frame().await.unwrap().unwrap().into_data().unwrap();
    assert!(std::str::from_utf8(&frame)
        .unwrap()
        .contains(&format!("id: {initial}")));
    let request = Request::builder()
        .uri("/v1/sessions/session/events")
        .header("last-event-id", initial.to_string())
        .body(Body::empty())
        .unwrap();
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request)
        .await
        .unwrap();
    let mut body = response.into_body();
    let greeting = body.frame().await.unwrap().unwrap().into_data().unwrap();
    assert_eq!(std::str::from_utf8(&greeting).unwrap(), ": ok\n\n");
    daemon
        .shared
        .emit(
            &SessionId::new("session"),
            None,
            EventKind::SessionSettled {},
        )
        .await
        .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(1), body.frame())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_data()
        .unwrap();
    let text = std::str::from_utf8(&frame).unwrap();
    assert!(text.contains(&format!("id: {}", initial + 1)));
    assert!(!text.contains("session.created"));
    daemon.shutdown().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn sandbox_setup_failure_is_denied_but_nonzero_command_exit_is_a_result() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let program = root.path().join("test-bwrap");
    std::fs::write(
        &program,
        "#!/bin/sh\nwhile [ \"$1\" != '--' ]; do shift; done\nshift\nexec \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    let runtime = TestRuntime::default();
    let mut config = resolved_config(root.path());
    config.bwrap = Some(program.clone());
    let daemon = Daemon::open_runtime(config, runtime).await.unwrap();
    let session = create_session(&daemon, "session", PolicyMode::Full, Some(root.path())).await;
    let host = host(&daemon, &session).await;
    let result = host
        .call(call(
            "bash",
            json!({"command":"printf 'ordinary-output'; exit 7"}),
            "nonzero",
        ))
        .await;
    let ToolOutcome::Done { for_model_json } = result else {
        panic!("command exit is not sandbox setup failure");
    };
    let result: Value = serde_json::from_str(&for_model_json).unwrap();
    assert_eq!(result["exit_code"], 7);
    assert_eq!(result["stdout"], "ordinary-output");
    std::fs::write(&program, "#!/bin/sh\nexit 1\n").unwrap();
    assert!(matches!(
        host.call(call(
            "bash",
            json!({"command":"touch no-fallback"}),
            "setup-failure"
        ))
        .await,
        ToolOutcome::Denied { .. }
    ));
    assert!(!root.path().join("no-fallback").exists());
    daemon.shutdown().await;
}

#[tokio::test]
async fn nonzero_unsandboxed_command_exit_is_still_a_result() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Full, Some(root.path())).await;
    let result = host(&daemon, &session)
        .await
        .call(call(
            "bash",
            json!({"command":"printf 'ordinary-output'; exit 7"}),
            "nonzero",
        ))
        .await;
    let ToolOutcome::Done { for_model_json } = result else {
        panic!("a nonzero command exit is still a shell result: {result:?}");
    };
    let result: Value = serde_json::from_str(&for_model_json).unwrap();
    assert_eq!(result["exit_code"], 7);
    assert_eq!(result["stdout"], "ordinary-output");
    daemon.shutdown().await;
}

#[tokio::test]
async fn pipeline_redacts_vault_patterns_before_limiting_model_and_event_output() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, Some(root.path())).await;
    let secret = openside_vault::Secret::new("vault-output-canary-593".into());
    daemon
        .shared
        .registry
        .redactors
        .write()
        .push(openside_vault::Redactor::new([&secret]));
    std::fs::write(
        root.path().join("large"),
        format!("vault-output-canary-593\n{}", "x".repeat(100_000)),
    )
    .unwrap();
    let ToolOutcome::Done { for_model_json } = host(&daemon, &session)
        .await
        .call(call(
            "read",
            json!({"path":"large","limit":2000}),
            "large-read",
        ))
        .await
    else {
        panic!("expected read");
    };
    assert!(for_model_json.len() <= crate::host::OUTPUT_LIMIT);
    assert!(!for_model_json.contains("vault-output-canary-593"));
    assert!(for_model_json.contains("[REDACTED]"));
    let events = daemon.shared.db.events(&session, 0).await.unwrap();
    assert!(!serde_json::to_string(&events)
        .unwrap()
        .contains("vault-output-canary-593"));
    daemon.shutdown().await;
}

/// Every live secret is masked as a substring whatever its length, so a one-character or
/// year-shaped secret is scrubbed from page and model text, timestamp-shaped text included;
/// only schema locations the daemon generates (ids, timestamps, enum tags, statuses) are kept,
/// so tickets and events still open, persist and parse.
#[tokio::test]
async fn short_secrets_are_masked_everywhere_but_schema_fields_survive() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, Some(root.path())).await;
    let year = chrono::Utc::now().format("%Y").to_string();
    for value in [
        year.clone(),
        "482915".into(),
        "7".into(),
        "e".into(),
        "pw".into(),
    ] {
        daemon
            .shared
            .registry
            .redactors
            .write()
            .push(openside_vault::Redactor::new([
                &openside_vault::Secret::new(value),
            ]));
    }
    let stamp = format!("{year}-10-12T04:41:26.482915+00:00");
    let redacted = daemon
        .shared
        .registry
        .redact_except(
            json!({
                "created_at": stamp,
                "id": "9ee24f7c482915a0b9c4d7e5f6071829",
                "text": format!("PIN {year}, code 482915, step 7, at {stamp}"),
            }),
            &[&["created_at"], &["id"]],
        )
        .unwrap();
    assert_eq!(redacted["created_at"], json!(stamp));
    assert_eq!(redacted["id"], json!("9ee24f7c482915a0b9c4d7e5f6071829"));
    let text = redacted["text"].as_str().unwrap();
    for leaked in [year.as_str(), "482915", "7", "e"] {
        assert!(!text.contains(leaked), "{leaked} in {text}");
    }
    // Without a schema path nothing is exempt, even text shaped like a timestamp.
    let plain = daemon.shared.registry.redact(json!(stamp)).unwrap();
    assert!(!plain.as_str().unwrap().contains(&year));
    let ToolOutcome::Deferred { ticket, .. } = host(&daemon, &session)
        .await
        .call(call(
            "request_human_step",
            json!({"kind":"info","summary":format!("Which PIN, {year} or 482915? Sent {stamp}")}),
            "short-secret-info",
        ))
        .await
    else {
        panic!("the ticket must open although its ids and timestamps contain secrets");
    };
    let tickets = daemon.shared.tickets().await.unwrap();
    let opened = tickets.iter().find(|item| item.id == ticket).unwrap();
    assert!(!opened.summary.contains("482915") && !opened.summary.contains(&year));
    let events =
        serde_json::to_string(&daemon.shared.db.events(&session, 0).await.unwrap()).unwrap();
    assert!(events.contains(ticket.as_str()));
    assert!(!events.contains("482915"));
    assert!(!events.contains(&format!("Which PIN, {year}")));
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request(
            "POST",
            &format!("/v1/tickets/{ticket}/resolve"),
            json!({"value":"the one ending in 7"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let events =
        serde_json::to_string(&daemon.shared.db.events(&session, 0).await.unwrap()).unwrap();
    assert!(events.contains("ticket.resolved"), "{events}");
    assert!(!events.contains("ending in 7"));
    daemon.shutdown().await;
}

#[tokio::test]
async fn malformed_tool_arguments_do_not_leak_unknown_secret_fields() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, Some(root.path())).await;
    let result = host(&daemon, &session).await.call(call("request_human_step", json!({"kind":"secret_input","summary":"Fill browser field","value":"argument-canary-671"}), "invalid")).await;
    assert!(matches!(result, ToolOutcome::Denied { .. }));
    let events = daemon.shared.db.events(&session, 0).await.unwrap();
    assert!(!serde_json::to_string(&events)
        .unwrap()
        .contains("argument-canary-671"));
    assert!(daemon.shared.tickets().await.unwrap().is_empty());
    daemon.shutdown().await;
}
