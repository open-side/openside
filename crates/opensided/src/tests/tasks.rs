use super::*;
use futures_util::FutureExt;

async fn front_daemon(root: &Path, runtime: &TestRuntime) -> Daemon {
    let mut config = resolved_config(root);
    config.loaded.config.agent.orchestration = openside_config::Orchestration::Front;
    Daemon::open_runtime(config, runtime.clone()).await.unwrap()
}

#[tokio::test]
async fn front_denies_work_tools_and_uses_session_model_fallback() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = front_daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "front", PolicyMode::Full, None).await;
    let parent = host(&daemon, &session).await;
    assert!(
        matches!(parent.call(call("browser_navigate", json!({"tab_id":"task-tab","url":"https://example.test"}), "front-act")).await,
        ToolOutcome::Denied { reason, .. } if reason == "front_agent_cannot_act")
    );
    for name in [
        "browser_snapshot",
        "browser_screenshot",
        "browser_import_lookup",
        "memory_search",
        "present",
    ] {
        assert!(
            matches!(
                parent.call(call(name, json!({}), name)).await,
                ToolOutcome::Denied { reason, .. } if reason == "front_agent_cannot_act"
            ),
            "{name}"
        );
    }
    daemon
        .shared
        .command(&session, crate::engine::Action::Recover)
        .await
        .unwrap();
    let opened = runtime.0.lock().opened.clone();
    let agent: Value = serde_json::from_str(&opened[0].agent_json).unwrap();
    assert_eq!(agent["orchestration"], "front");
    let tools = agent["front_tools"].as_array().unwrap();
    for name in [
        "send_message",
        "create_task",
        "cancel_task",
        "get_tasks",
        "steer_task",
    ] {
        assert!(tools.iter().any(|tool| tool == name), "{name}");
    }
    assert!(tools
        .iter()
        .all(|tool| crate::tasks::is_front_tool(tool.as_str().unwrap())));
    assert!(agent.get("front_model").is_none());
    assert_eq!(opened[0].model, std::any::type_name::<TestRuntime>());
    daemon.shutdown().await;
}

#[tokio::test]
async fn send_message_records_one_complete_message_without_activity_or_deltas() {
    let root = tempfile::tempdir().unwrap();
    let daemon = front_daemon(root.path(), &TestRuntime::default()).await;
    let session = create_session(&daemon, "message", PolicyMode::Full, None).await;
    let parent = host(&daemon, &session).await;
    assert!(matches!(
        parent
            .call(call(
                "send_message",
                json!({"text":"Working on it."}),
                "say"
            ))
            .await,
        ToolOutcome::Done { .. }
    ));
    let events = daemon.shared.db.events(&session, 0).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(&event.kind,
        EventKind::AssistantFinal { text, .. } if text == "Working on it."))
            .count(),
        1
    );
    assert!(!events.iter().any(|event| matches!(
        &event.kind,
        EventKind::AssistantDelta { .. }
            | EventKind::ToolStarted { .. }
            | EventKind::ToolEnded { .. }
    )));
    daemon.shutdown().await;
}

#[tokio::test]
async fn front_model_override_does_not_change_task_admission_model() {
    for fast in [Some("openai:gpt-6-luna"), None] {
        let root = tempfile::tempdir().unwrap();
        let runtime = TestRuntime::default();
        let mut config = resolved_config(root.path());
        config.loaded.config.agent.orchestration = openside_config::Orchestration::Front;
        config.loaded.config.models.front = fast.map(str::to_owned);
        config.loaded.config.models.default = Some("openai:gpt-6.1-sol".into());
        let daemon = Daemon::open_runtime(config, runtime.clone()).await.unwrap();
        let session = daemon
            .shared
            .agents
            .get("chief")
            .unwrap()
            .unwrap()
            .home_session_id;
        daemon
            .shared
            .command(&session, crate::engine::Action::Recover)
            .await
            .unwrap();
        let opened = runtime.0.lock().opened.clone();
        let agent: Value = serde_json::from_str(&opened[0].agent_json).unwrap();
        assert_eq!(agent.get("front_model").and_then(Value::as_str), fast);
        let reasoning = opened[0].model.clone();
        assert_eq!(reasoning, "openai:gpt-6.1-sol");
        let parent = host(&daemon, &session).await;
        let result = parent
            .call(call(
                "create_task",
                json!({"goal":"reason about this"}),
                "start-role",
            ))
            .await;
        let ToolOutcome::Done { for_model_json, .. } = result else {
            panic!("Task admission failed: {result:?}");
        };
        let admitted: Value = serde_json::from_str(&for_model_json).unwrap();
        assert_eq!(admitted["model"], reasoning);
        let events = daemon.shared.db.events(&session, 0).await.unwrap();
        assert!(events.iter().any(|event| matches!(&event.kind,
            EventKind::SubagentStarted { model, background: true, .. } if model == &reasoning)));
        daemon.shutdown().await;
    }
}

#[tokio::test]
async fn restart_repairs_a_task_end_without_a_report_and_never_queues_it_twice() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = front_daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "repair", PolicyMode::Full, None).await;
    for kind in [
        EventKind::SubagentStarted {
            subagent_id: "task-1".into(),
            parent_tool_call_id: "start-1".into(),
            task: "durable work".into(),
            model: "test/model".into(),
            tools: vec!["read".into()],
            background: true,
        },
        EventKind::SubagentEnded {
            subagent_id: "task-1".into(),
            status: SubagentStatus::Completed,
            reason: None,
            output: Some("durable result".into()),
        },
    ] {
        daemon.shared.emit(&session, None, kind).await.unwrap();
    }
    // This durable state is exactly the crash boundary between the terminal event and queue append.
    daemon.shutdown().await;
    drop(daemon);
    let daemon = front_daemon(root.path(), &runtime).await;
    for _ in 0..3 {
        crate::tasks::report_updates(&daemon.shared, &session)
            .await
            .unwrap();
    }
    until(|| {
        runtime
            .0
            .lock()
            .sent
            .iter()
            .any(|(_, id, _, _)| id.as_str() == "task-report-task-1")
    })
    .await;
    let events = daemon.shared.db.events(&session, 0).await.unwrap();
    assert_eq!(events.iter().filter(|event| matches!(&event.kind, EventKind::MessageQueued { message_id, text, .. } if message_id.as_str() == "task-report-task-1" && text.contains("durable result"))).count(), 1);
    assert!(events
        .iter()
        .any(|event| matches!(&event.kind, EventKind::MessageQueued {
        message_id, author_kind: MessageAuthorKind::Internal, from_agent: None, ..
    } if message_id.as_str() == "task-report-task-1")));
    assert_eq!(
        runtime
            .0
            .lock()
            .sent
            .iter()
            .filter(|(_, id, _, _)| id.as_str() == "task-report-task-1")
            .count(),
        1
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn reads_never_steal_tabs_and_login_tools_obey_task_ownership() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = front_daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "ownership", PolicyMode::Full, None).await;
    for task in ["first", "second"] {
        daemon
            .shared
            .emit(
                &session,
                None,
                EventKind::SubagentStarted {
                    subagent_id: task.into(),
                    parent_tool_call_id: format!("start-{task}"),
                    task: task.into(),
                    model: "test/model".into(),
                    tools: vec!["browser_navigate".into()],
                    background: true,
                },
            )
            .await
            .unwrap();
    }
    let child_call = |name: &str, task: &str| ToolCall {
        subagent: Some(task.into()),
        ..call(name, json!({"tab_id":"tab"}), name)
    };
    daemon
        .shared
        .registry
        .browser
        .task_tabs
        .claim(&session, "tab", "first");
    assert!(crate::browser_state::tab_denial(
        &daemon.shared,
        &session,
        &child_call("browser_snapshot", "second")
    )
    .await
    .unwrap()
    .is_none());
    for tool in [
        "browser_navigate",
        "browser_repl",
        "request_login",
        "request_choice",
        "vault_login",
        "request_otp",
    ] {
        assert_eq!(
            crate::browser_state::tab_denial(&daemon.shared, &session, &child_call(tool, "second"))
                .await
                .unwrap()
                .map(|denial| denial.0),
            Some("tab_owned_by_task"),
            "{tool}"
        );
    }
    assert!(crate::browser_state::tab_denial(
        &daemon.shared,
        &session,
        &child_call("browser_navigate", "first")
    )
    .await
    .unwrap()
    .is_none());
    let claim = |task: &str| ToolCall {
        subagent: Some(task.into()),
        ..call("browser_navigate", json!({"tab_id":"idle-tab"}), task)
    };
    let first = claim("first");
    let second = claim("second");
    let (a, b) = tokio::join!(
        crate::browser_state::tab_denial(&daemon.shared, &session, &first),
        crate::browser_state::tab_denial(&daemon.shared, &session, &second)
    );
    assert_ne!(
        a.unwrap().is_none(),
        b.unwrap().is_none(),
        "only one concurrent Task may claim a tab"
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn unused_prewarm_tab_expires_after_ten_minutes() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let mut config = resolved_config(root.path());
    config.loaded.config.agent.orchestration = openside_config::Orchestration::Front;
    config.loaded.config.browser.executable = Some(test_browser().unwrap());
    let daemon = Daemon::open_runtime(config, runtime).await.unwrap();
    let session = create_session(&daemon, "prewarm-expiry", PolicyMode::Full, None).await;
    let checked = std::panic::AssertUnwindSafe(async {
        crate::browser_state::prewarm_session(&daemon.shared, &session).await;
        let browser = daemon
            .shared
            .registry
            .browser
            .existing()
            .await
            .unwrap_or_else(|error| panic!("browser: {}", error.0));
        let tab = browser
            .tabs()
            .await
            .into_iter()
            .find(|tab| tab.owner_session_id.as_deref() == Some(session.as_str()))
            .unwrap();
        tokio::task::yield_now().await;
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(601)).await;
        tokio::time::resume();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while browser
            .tabs()
            .await
            .iter()
            .any(|current| current.id == tab.id)
        {
            assert!(
                std::time::Instant::now() < deadline,
                "unused prewarm tab survived TTL"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .catch_unwind()
    .await;
    daemon.shutdown().await;
    if let Err(error) = checked {
        std::panic::resume_unwind(error);
    }
}

#[tokio::test]
async fn browser_state_action_labels_come_from_the_page_not_the_task_goal_or_field_values() {
    use axum::{response::Html, routing::get, Router};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/login", listener.local_addr().unwrap());
    let app = Router::new().route("/login",get(||async {Html("<title>Account</title><h1>Sign in</h1><label>Email<input value='FIELD-VALUE-CANARY'></label><label>Password<input type=password></label><button>Continue</button>")}));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let mut config = resolved_config(root.path());
    config.loaded.config.agent.orchestration = openside_config::Orchestration::Front;
    config.loaded.config.browser.executable = Some(test_browser().unwrap());
    let daemon = Daemon::open_runtime(config, runtime).await.unwrap();
    let session = create_session(&daemon, "labels", PolicyMode::Full, None).await;
    daemon
        .shared
        .emit(
            &session,
            None,
            EventKind::SubagentStarted {
                subagent_id: "task".into(),
                parent_tool_call_id: "start".into(),
                task: "MODEL-INVENTED-PROGRESS".into(),
                model: "test/model".into(),
                tools: vec![
                    "browser_open_tab".into(),
                    "browser_snapshot".into(),
                    "browser_act".into(),
                ],
                background: true,
            },
        )
        .await
        .unwrap();
    let checked = std::panic::AssertUnwindSafe(async {
        let invoke = |name: &str, args: Value| ToolCall {
            subagent: Some("task".into()),
            ..call(name, args, name)
        };
        let parent = host(&daemon, &session).await;
        let ToolOutcome::Done { for_model_json } = parent
            .call(invoke("browser_open_tab", json!({"url":url})))
            .await
        else {
            panic!("open failed");
        };
        let opened: Value = serde_json::from_str(&for_model_json).unwrap();
        let tab = opened["tab_id"].as_str().unwrap();
        assert!(matches!(
            parent
                .call(invoke("browser_snapshot", json!({"tab_id":tab})))
                .await,
            ToolOutcome::Done { .. }
        ));
        let browser = daemon
            .shared
            .registry
            .browser
            .existing()
            .await
            .unwrap_or_else(|error| panic!("browser: {}", error.0));
        let snapshot = browser.snapshot(tab).await.unwrap();
        let target = snapshot
            .nodes
            .iter()
            .find(|node| node.role == "button" && node.name == "Continue")
            .unwrap()
            .reference
            .as_ref()
            .unwrap();
        let call = invoke(
            "browser_act",
            json!({"tab_id":tab,"target":target,"action":"click"}),
        );
        assert!(matches!(parent.call(call).await, ToolOutcome::Done { .. }));
        let events = daemon.shared.db.events(&session, 0).await.unwrap();
        let state = events
            .iter()
            .find_map(|event| match &event.kind {
                EventKind::BrowserState(state)
                    if state
                        .action
                        .as_ref()
                        .is_some_and(|action| action.target.as_deref() == Some("Continue")) =>
                {
                    Some(state)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(state.task_id.as_deref(), Some("task"));
        assert_eq!(
            state.action.as_ref().unwrap().phrase,
            "Working on “Continue”"
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let events = daemon.shared.db.events(&session, 0).await.unwrap();
            let observed = events.iter().find_map(|event| match &event.kind {
                EventKind::BrowserState(state) if state.page_kind == PageKind::Login => Some(state),
                _ => None,
            });
            if let Some(state) = observed {
                let encoded = serde_json::to_string(state).unwrap();
                assert!(!encoded.contains("FIELD-VALUE-CANARY"));
                assert!(!encoded.contains("MODEL-INVENTED-PROGRESS"));
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "classified page state was not emitted"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .catch_unwind()
    .await;
    daemon.shutdown().await;
    server.abort();
    if let Err(error) = checked {
        std::panic::resume_unwind(error);
    }
}

async fn browser_front_daemon(root: &Path) -> Daemon {
    let mut config = resolved_config(root);
    config.loaded.config.agent.orchestration = openside_config::Orchestration::Front;
    config.loaded.config.browser.executable = Some(test_browser().unwrap());
    Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap()
}

struct TaskSite {
    url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for TaskSite {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn task_site() -> TaskSite {
    use axum::{response::Html, routing::get, Router};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let app = Router::new().route(
        "/",
        get(|| async { Html("<title>Task fixture</title><h1>Task fixture</h1>") }),
    );
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    TaskSite { url, task }
}

async fn start_browser_task(daemon: &Daemon, session: &SessionId, task: &str) {
    daemon
        .shared
        .emit(
            session,
            None,
            EventKind::SubagentStarted {
                subagent_id: task.into(),
                parent_tool_call_id: format!("start-{task}"),
                task: task.into(),
                model: "test/model".into(),
                tools: vec![
                    "browser_repl".into(),
                    "browser_open_tab".into(),
                    "browser_navigate".into(),
                    "browser_snapshot".into(),
                ],
                background: true,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn browser_notices_steer_only_the_owning_task_and_ignore_terminal_tasks() {
    use openside_browser::{BrowserEvent, DownloadStatus, OpenedBy, TabInfo};
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = front_daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "task-notices", PolicyMode::Full, None).await;
    crate::engine::prewarm_runtime(&daemon.shared, &session).await;
    start_browser_task(&daemon, &session, "task").await;
    let ledger = &daemon.shared.registry.browser.task_tabs;
    ledger.claim(&session, "tab", "task");
    let tab = TabInfo {
        id: "tab".into(),
        url: "https://example.test/".into(),
        title: String::new(),
        opened_by: OpenedBy::Agent,
        owner_session_id: Some(session.to_string()),
        discarded: false,
        unloaded: false,
        browser_context_id: None,
    };
    let events = [
        BrowserEvent::DownloadStarted {
            guid: "download".into(),
            url: tab.url.clone(),
            suggested_filename: "file".into(),
            tab_id: Some(tab.id.clone()),
        },
        BrowserEvent::TabClosed { tab: tab.clone() },
        BrowserEvent::DownloadFinished {
            guid: "download".into(),
            status: DownloadStatus::Completed,
        },
    ];
    for event in &events {
        crate::browser::steer_browser_event(&daemon.shared, &session, event).await;
    }
    {
        let state = runtime.0.lock();
        assert_eq!(state.controls.len(), 3);
        assert!(state.controls.iter().all(|(task, control)| task == "task"
            && matches!(control, openside_core::SubagentControl::Steer { text } if text.starts_with("[browser event]"))));
        assert!(
            state.sent.is_empty(),
            "browser notices must not reach the chief"
        );
    }
    ledger.claim(&session, "tab", "task");
    crate::subagents::end(
        &daemon.shared,
        &session,
        "task",
        SubagentStatus::Completed,
        "completed",
    )
    .await
    .unwrap();
    crate::tasks::report_updates(&daemon.shared, &session)
        .await
        .unwrap();
    crate::browser::steer_browser_event(&daemon.shared, &session, &BrowserEvent::TabClosed { tab })
        .await;
    assert_eq!(runtime.0.lock().controls.len(), 3);
    assert!(!daemon.shared.db.events(&session, 0).await.unwrap().iter().any(|event|
        matches!(&event.kind, EventKind::MessageQueued { text, .. } if text.starts_with("[browser event]"))));
    daemon.shutdown().await;
}

#[tokio::test]
async fn repl_claim_retires_warm_lease_and_neither_open_nor_ttl_can_steal_the_tab() {
    let site = task_site().await;
    let root = tempfile::tempdir().unwrap();
    let daemon = browser_front_daemon(root.path()).await;
    let session = create_session(&daemon, "warm-claim", PolicyMode::Full, None).await;
    let checked = std::panic::AssertUnwindSafe(async {
        daemon
            .shared
            .registry
            .browser
            .prewarm_tab(&daemon.shared, &session)
            .await;
        for task in ["first", "second"] {
            start_browser_task(&daemon, &session, task).await;
        }
        let parent = host(&daemon, &session).await;
        let first = ToolCall {
            subagent: Some("first".into()),
            ..call(
                "browser_repl",
                json!({"code":"await page.title()"}),
                "claim",
            )
        };
        let ToolOutcome::Done { for_model_json } = parent.call(first).await else {
            panic!("REPL failed");
        };
        let result: Value = serde_json::from_str(&for_model_json).unwrap();
        let tab = result["tab_id"].as_str().unwrap();
        let path = openside_config::data_layout::DataPath::BrowserState
            .path(&daemon.shared.config.data_dir);
        let saved: Value = serde_json::from_slice(&tokio::fs::read(path).await.unwrap()).unwrap();
        assert!(saved["warm_tabs"].get(session.as_str()).is_none());
        assert_eq!(
            saved["tabs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["id"] == tab)
                .unwrap()["task"],
            "first"
        );
        let second = ToolCall {
            subagent: Some("second".into()),
            ..call("browser_open_tab", json!({"url":site.url}), "open-second")
        };
        let ToolOutcome::Done { for_model_json } = parent.call(second).await else {
            panic!("Open failed");
        };
        let second: Value = serde_json::from_str(&for_model_json).unwrap();
        assert_ne!(second["tab_id"], tab);
        daemon
            .shared
            .registry
            .browser
            .cleanup_warm(&daemon.shared, &session, Some(tab))
            .await;
        let browser = daemon
            .shared
            .registry
            .browser
            .existing()
            .await
            .unwrap_or_else(|error| panic!("{}", error.0));
        assert!(browser.tabs().await.iter().any(|item| item.id == tab));
        assert_eq!(
            daemon
                .shared
                .registry
                .browser
                .task_tabs
                .holder(&session, tab)
                .as_deref(),
            Some("first")
        );
    })
    .catch_unwind()
    .await;
    daemon.shutdown().await;
    if let Err(error) = checked {
        std::panic::resume_unwind(error);
    }
}

#[tokio::test]
async fn delayed_browser_probes_cannot_overwrite_newer_or_terminal_task_state() {
    let site = task_site().await;
    let root = tempfile::tempdir().unwrap();
    let daemon = browser_front_daemon(root.path()).await;
    let session = create_session(&daemon, "probe-fence", PolicyMode::Full, None).await;
    let checked = std::panic::AssertUnwindSafe(async {
        start_browser_task(&daemon, &session, "task").await;
        let parent = host(&daemon, &session).await;
        let ledger = &daemon.shared.registry.browser.task_tabs;
        let ToolOutcome::Done { for_model_json } = parent.call(ToolCall {
            subagent: Some("task".into()), ..call("browser_open_tab", json!({"url":site.url}), "open")
        }).await else { panic!("Open failed"); };
        let result: Value = serde_json::from_str(&for_model_json).unwrap();
        let tab = result["tab_id"].as_str().unwrap();
        let browser = daemon.shared.registry.browser.existing().await.unwrap_or_else(|error| panic!("{}", error.0));
        let person_tabs: Vec<_> = browser.tabs().await.into_iter()
            .filter(|info| info.owner_session_id.is_none()).map(|info| info.id).collect();
        // Drain the Open probe before deliberately parking the next observation.
        tokio::time::timeout(Duration::from_secs(10), ledger.probe_finished.notified()).await.unwrap();
        for terminal in [false, true] {
            let blocker = ledger.probe_gate.lock().await;
            let old = ToolCall { subagent: Some("task".into()), ..call("browser_snapshot", json!({"tab_id":tab}), if terminal { "old-terminal" } else { "old-action" }) };
            if terminal {
                assert!(matches!(parent.call(call("browser_repl",
                    json!({"code":"await page.title()"}), "api-old-terminal")).await,
                    ToolOutcome::Done { .. }));
                assert!(daemon.shared.db.events(&session, 0).await.unwrap().iter().any(|event|
                    matches!(&event.kind, EventKind::BrowserState(state) if state.task_id.as_deref() == Some("task")
                        && state.action.as_ref().is_some_and(|action| action.tool_call_id == "api-old-terminal"))));
            } else {
                let stamp = crate::browser_state::before(&daemon.shared, &session, &old).await;
                let outcome = ToolOutcome::Done { for_model_json: json!({"tab_id":tab}).to_string() };
                crate::browser_state::after(&daemon.shared, &session, &old, &outcome, stamp);
            }
            tokio::time::timeout(Duration::from_secs(10), ledger.probe_started.notified()).await.unwrap();
            if terminal {
                crate::subagents::end(&daemon.shared, &session, "task", SubagentStatus::Completed, "completed").await.unwrap();
                crate::tasks::report_updates(&daemon.shared, &session).await.unwrap();
            } else {
                let current = ToolCall { tool_call_id: "new-action".into(), ..old };
                let _ = crate::browser_state::before(&daemon.shared, &session, &current).await;
            }
            let boundary = daemon.shared.db.events(&session, 0).await.unwrap().last().unwrap().seq;
            drop(blocker);
            tokio::time::timeout(Duration::from_secs(10), ledger.probe_finished.notified()).await.unwrap();
            let later = daemon.shared.db.events(&session, boundary).await.unwrap();
            assert!(!later.iter().any(|event| matches!(&event.kind, EventKind::BrowserState(state)
                if state.action.as_ref().is_some_and(|action| action.tool_call_id.contains("old-")))));
            if terminal {
                tokio::time::timeout(Duration::from_secs(10), async {
                    while browser.tabs().await.iter().any(|info| info.id == tab) {
                        tokio::task::yield_now().await;
                    }
                }).await.expect("terminal Task must close its unreferenced agent tab");
                let remaining = browser.tabs().await;
                assert!(person_tabs.iter().all(|id| remaining.iter().any(|info| &info.id == id)),
                    "Task cleanup must preserve person tabs");
                assert!(ledger.holder(&session, tab).is_none());
                assert!(!daemon.shared.db.events(&session, 0).await.unwrap().iter().any(|event|
                    matches!(&event.kind, EventKind::TabClosed { tab_id } if tab_id == tab)),
                    "daemon cleanup must not generate a browser close event");
            }
        }
    }).catch_unwind().await;
    daemon.shutdown().await;
    if let Err(error) = checked {
        std::panic::resume_unwind(error);
    }
}

#[tokio::test]
async fn tab_less_approvals_do_not_fence_an_in_flight_browser_probe() {
    let site = task_site().await;
    let root = tempfile::tempdir().unwrap();
    let daemon = browser_front_daemon(root.path()).await;
    let session = create_session(&daemon, "probe-confirm", PolicyMode::Full, None).await;
    let checked = std::panic::AssertUnwindSafe(async {
        start_browser_task(&daemon, &session, "task").await;
        let parent = host(&daemon, &session).await;
        let ledger = &daemon.shared.registry.browser.task_tabs;
        let ToolOutcome::Done { for_model_json } = parent.call(ToolCall {
            subagent: Some("task".into()), ..call("browser_open_tab", json!({"url":site.url}), "open")
        }).await else { panic!("Open failed"); };
        let result: Value = serde_json::from_str(&for_model_json).unwrap();
        let tab = result["tab_id"].as_str().unwrap().to_owned();
        tokio::time::timeout(Duration::from_secs(10), ledger.probe_finished.notified()).await.unwrap();
        // Park a snapshot probe behind the probe gate, then let a confirm approval —
        // a call with no tab and nothing to publish — run its `before` first.
        let blocker = ledger.probe_gate.lock().await;
        let read = ToolCall {
            subagent: Some("task".into()),
            ..call("browser_snapshot", json!({"tab_id":tab}), "read-before-approval")
        };
        let stamp = crate::browser_state::before(&daemon.shared, &session, &read).await;
        crate::browser_state::after(
            &daemon.shared,
            &session,
            &read,
            &ToolOutcome::Done { for_model_json: json!({"tab_id":tab}).to_string() },
            stamp,
        );
        tokio::time::timeout(Duration::from_secs(10), ledger.probe_started.notified()).await.unwrap();
        let confirm = ToolCall {
            subagent: Some("task".into()),
            ..call("request_human_step",
                json!({"kind":"confirm","summary":"Approve this remote phone task"}), "confirm")
        };
        assert!(crate::browser_state::before(&daemon.shared, &session, &confirm).await.is_none());
        let boundary = daemon.shared.db.events(&session, 0).await.unwrap().last().unwrap().seq;
        drop(blocker);
        tokio::time::timeout(Duration::from_secs(10), ledger.probe_finished.notified()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if daemon.shared.db.events(&session, boundary).await.unwrap().iter().any(|event|
                    matches!(&event.kind, EventKind::BrowserState(state)
                        if state.action.as_ref().is_some_and(|action| action.tool_call_id == "read-before-approval")))
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        }).await.expect("the approval call fenced out the page probe's emission");
    })
    .catch_unwind()
    .await;
    daemon.shutdown().await;
    if let Err(error) = checked {
        std::panic::resume_unwind(error);
    }
}

#[tokio::test]
async fn task_cancellation_waits_for_repl_cleanup_and_releases_held_input_for_the_next_task() {
    let site = task_site().await;
    let root = tempfile::tempdir().unwrap();
    let daemon = browser_front_daemon(root.path()).await;
    let session = create_session(&daemon, "cancel-repl", PolicyMode::Full, None).await;
    let checked = std::panic::AssertUnwindSafe(async {
        for task in ["first", "sibling"] { start_browser_task(&daemon, &session, task).await; }
        let parent = Arc::new(host(&daemon, &session).await);
        let ToolOutcome::Done { for_model_json } = parent.call(ToolCall {
            subagent: Some("first".into()), ..call("browser_open_tab", json!({"url":site.url}), "open-first")
        }).await else { panic!("Open failed"); };
        let opened: Value = serde_json::from_str(&for_model_json).unwrap();
        let tab = opened["tab_id"].as_str().unwrap().to_owned();
        let running = {
            let parent = parent.clone();
            let tab = tab.clone();
            tokio::spawn(async move {
                parent.call(ToolCall {
                    subagent: Some("first".into()),
                    ..call("browser_repl", json!({"tab_id":tab,"code":
                        "await page.evaluate(() => { document.body.innerHTML = '<input aria-label=\"Query\">'; }); await page.getByRole('textbox').focus(); await page.keyboard.down('Shift'); await page.evaluate(() => { document.title = 'READY'; }); await page.waitForTimeout(25000); await page.evaluate(() => { document.title = 'FORBIDDEN'; });"
                    }), "in-flight")
                }).await
            })
        };
        let browser = daemon.shared.registry.browser.existing().await.unwrap_or_else(|error| panic!("{}", error.0));
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !browser.tabs().await.iter().any(|info| info.id == tab && info.title == "READY") {
            assert!(std::time::Instant::now() < deadline, "REPL never reached its cancellable wait");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::timeout(Duration::from_secs(5), crate::subagents::cancel(&daemon.shared, &session, "first")).await.unwrap().unwrap();
        let result = running.await.unwrap();
        assert!(matches!(result, ToolOutcome::Failed { message } if message.contains("cancelled")));
        assert!(browser.tabs().await.iter().any(|info| info.id == tab && info.title == "READY"));
        let result = parent.call(ToolCall {
            subagent: Some("sibling".into()),
            ..call("browser_repl", json!({"tab_id":tab,"code":"await page.getByRole('textbox').focus(); await page.keyboard.press('KeyA'); await page.getByRole('textbox').inputValue()"}), "sibling-input")
        }).await;
        let ToolOutcome::Done { for_model_json } = result else { panic!("sibling REPL failed"); };
        let result: Value = serde_json::from_str(&for_model_json).unwrap();
        assert_eq!(result["return_value"], "a", "cancelled Task left Shift held: {result}");
        crate::tasks::report_updates(&daemon.shared, &session).await.unwrap();
    }).catch_unwind().await;
    daemon.shutdown().await;
    if let Err(error) = checked {
        std::panic::resume_unwind(error);
    }
}

#[tokio::test]
async fn cancelled_navigation_and_open_cannot_commit_after_terminal_publication() {
    use axum::{response::Html, routing::get, Router};
    struct RequestFinished(Arc<tokio::sync::Notify>);
    impl Drop for RequestFinished {
        fn drop(&mut self) {
            self.0.notify_one();
        }
    }
    for open in [false, true] {
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let responded = Arc::new(tokio::sync::Notify::new());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route(
                "/",
                get(|| async { Html("<title>Original</title><h1>Original</h1>") }),
            )
            .route(
                "/delayed",
                get({
                    let started = started.clone();
                    let release = release.clone();
                    let responded = responded.clone();
                    move || {
                        let started = started.clone();
                        let release = release.clone();
                        let responded = responded.clone();
                        async move {
                            // Stopping a load can drop this handler before it produces a response.
                            let _finished = RequestFinished(responded);
                            started.notify_one();
                            release.notified().await;
                            Html("<title>FORBIDDEN</title><h1>Late document</h1>")
                        }
                    }
                }),
            );
        let site = TaskSite {
            url: origin.clone(),
            task: tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            }),
        };
        let root = tempfile::tempdir().unwrap();
        let daemon = browser_front_daemon(root.path()).await;
        let session = create_session(&daemon, "cancel-load", PolicyMode::Full, None).await;
        let checked = std::panic::AssertUnwindSafe(async {
            start_browser_task(&daemon, &session, "first").await;
            start_browser_task(&daemon, &session, "sibling").await;
            let parent = Arc::new(host(&daemon, &session).await);
            let ToolOutcome::Done { for_model_json } = parent.call(ToolCall {
                subagent: Some("first".into()), ..call("browser_open_tab", json!({"url":site.url}), "original")
            }).await else { panic!("Open failed"); };
            let original: Value = serde_json::from_str(&for_model_json).unwrap();
            let tab = original["tab_id"].as_str().unwrap().to_owned();
            let operation = {
                let parent = parent.clone();
                let tab = tab.clone();
                let url = format!("{origin}/delayed");
                tokio::spawn(async move {
                    let (name, args) = if open {
                        ("browser_open_tab", json!({"url":url}))
                    } else {
                        ("browser_navigate", json!({"url":url,"tab_id":tab}))
                    };
                    parent.call(ToolCall { subagent: Some("first".into()), ..call(name, args, "pending-load") }).await
                })
            };
            tokio::time::timeout(Duration::from_secs(10), started.notified()).await.unwrap();
            tokio::time::timeout(Duration::from_secs(5), crate::subagents::cancel(&daemon.shared, &session, "first")).await.unwrap().unwrap();
            assert!(matches!(operation.await.unwrap(), ToolOutcome::Failed { message } if message.contains("cancelled")));
            release.notify_one();
            tokio::time::timeout(Duration::from_secs(5), responded.notified()).await.unwrap();
            let result = parent.call(ToolCall {
                subagent: Some("sibling".into()),
                ..call("browser_repl", json!({"tab_id":tab,"code":"await page.waitForTimeout(200); await page.title()"}), "sibling-read")
            }).await;
            let ToolOutcome::Done { for_model_json } = result else { panic!("sibling read failed"); };
            let result: Value = serde_json::from_str(&for_model_json).unwrap();
            assert_eq!(result["return_value"], "Original", "{result}");
            crate::tasks::report_updates(&daemon.shared, &session).await.unwrap();
            let browser = daemon.shared.registry.browser.existing().await.unwrap_or_else(|error| panic!("{}", error.0));
            assert!(!browser.tabs().await.iter().any(|info| info.url.ends_with("/delayed")));
        }).catch_unwind().await;
        daemon.shutdown().await;
        if let Err(error) = checked {
            std::panic::resume_unwind(error);
        }
    }
}

#[tokio::test]
async fn message_documents_validate_and_freeze_admitted_bytes() {
    use crate::tools::{DaemonTool, ToolEnv, ToolExec};
    use openside_proto::DocumentDisplay;

    let root = tempfile::tempdir().unwrap();
    let daemon = front_daemon(root.path(), &TestRuntime::default()).await;
    let session = create_session(&daemon, "documents", PolicyMode::Full, None).await;
    let state = daemon.shared.db.state(&session).await.unwrap();
    let mut env = ToolEnv::new(daemon.shared.clone(), &state);
    env.tool_call_id = "document-write".into();
    env.subagent = Some("task-document".into());
    let secret_args = json!({"title":"sk-1234567890abcdefghijklmnop","markdown":"Safe body"});
    assert_eq!(
        crate::tasks::document::WriteDocument.redact_args(&secret_args),
        json!({"title":"[REDACTED]","markdown":"[REDACTED]"}),
    );
    assert!(matches!(
        crate::tasks::document::WriteDocument.call(secret_args, &env).await,
        ToolExec::Denied { reason, .. } if reason == "document_secret_detected"
    ));
    let document = crate::tasks::document::WriteDocument
        .call(
            json!({"title":"Report","markdown":"# Report\n\nA safe result."}),
            &env,
        )
        .await;
    let ToolExec::Done { for_model, .. } = document else {
        panic!("document write failed")
    };
    let path = for_model["artifact"].as_str().unwrap().to_owned();
    assert_eq!(
        crate::artifacts::task_documents(&daemon.shared, &session, "task-document")
            .await
            .unwrap(),
        std::slice::from_ref(&path),
    );
    let request = |path: &str| {
        vec![(
            path.to_owned(),
            Some("Report".into()),
            DocumentDisplay::Panel,
        )]
    };
    let admitted = crate::artifacts::admit_message_documents(&env, request(&path))
        .await
        .unwrap_or_else(|_| panic!("document admission failed"));
    assert_eq!(admitted.len(), 1);
    assert_eq!(admitted[0].bytes, 24);
    assert_eq!(admitted[0].content_type, "text/markdown");
    assert_ne!(admitted[0].path, path);
    crate::artifacts::save_tool_output(&env, "write", &path, b"Changed after admission".to_vec())
        .await
        .unwrap();
    daemon
        .shared
        .emit(
            &session,
            None,
            EventKind::AssistantFinal {
                text: "Here is the report.".into(),
                attachments: admitted.clone(),
            },
        )
        .await
        .unwrap();
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(super::request(
            "GET",
            &format!(
                "/v1/sessions/{session}/artifacts/{}",
                admitted[0].path.replace('/', "%2F")
            ),
            json!({}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        "# Report\n\nA safe result."
    );

    for (path, content, expected) in [
        ("missing.md", None, "artifact_not_found"),
        (
            "files/report.txt",
            Some("plain text"),
            "artifact_not_markdown",
        ),
        (
            "files/secret.md",
            Some("sk-1234567890abcdefghijklmnop"),
            "attachment_secret_detected",
        ),
        (
            "files/card.md",
            Some("4111 1111 1111 1111"),
            "attachment_secret_detected",
        ),
    ] {
        if let Some(content) = content {
            crate::artifacts::save_tool_output(&env, "write", path, content.as_bytes().to_vec())
                .await
                .unwrap();
        }
        assert!(
            matches!(crate::artifacts::admit_message_documents(&env, request(path)).await,
            Err(ToolExec::Denied { reason, hint: Some(_) }) if reason == format!("attachment[0]: {expected}"))
        );
    }
    let secret = openside_vault::Secret::new("private-vault-canary".into());
    daemon
        .shared
        .registry
        .redactors
        .write()
        .push(openside_vault::Redactor::new([&secret]));
    daemon
        .shared
        .registry
        .redactors
        .write()
        .push(openside_vault::Redactor::for_cookie("private-cookie-canary").unwrap());
    for (n, secret) in ["private-vault-canary", "private-cookie-canary"]
        .iter()
        .enumerate()
    {
        let path = format!("files/private-{n}.md");
        crate::artifacts::save_tool_output(&env, "write", &path, secret.as_bytes().to_vec())
            .await
            .unwrap();
        assert!(
            matches!(crate::artifacts::admit_message_documents(&env, request(&path)).await,
            Err(ToolExec::Denied { reason, .. }) if reason == "attachment[0]: attachment_secret_detected")
        );
    }
    crate::artifacts::save_tool_output(
        &env,
        "write",
        "files/large.md",
        vec![b'x'; openside_proto::MAX_DOCUMENT_BYTES + 1],
    )
    .await
    .unwrap();
    assert!(
        matches!(crate::artifacts::admit_message_documents(&env, request("files/large.md")).await,
        Err(ToolExec::Denied { reason, .. }) if reason == "attachment[0]: artifact_too_large")
    );
    assert!(
        matches!(crate::artifacts::admit_message_documents(&env, vec![
        (path.clone(), None, DocumentDisplay::Inline); 3
    ]).await, Err(ToolExec::Denied { reason, hint: Some(_) }) if reason == "attachment[2]: too_many_attachments")
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn terminal_task_preserves_tab_referenced_by_pending_person_step() {
    let site = task_site().await;
    let root = tempfile::tempdir().unwrap();
    let daemon = browser_front_daemon(root.path()).await;
    let session = create_session(&daemon, "retained-tab", PolicyMode::Full, None).await;
    let checked = std::panic::AssertUnwindSafe(async {
        start_browser_task(&daemon, &session, "task").await;
        let parent = host(&daemon, &session).await;
        let ToolOutcome::Done { for_model_json } = parent
            .call(ToolCall {
                subagent: Some("task".into()),
                ..call("browser_open_tab", json!({"url":site.url}), "open")
            })
            .await
        else {
            panic!("Open failed");
        };
        let opened: Value = serde_json::from_str(&for_model_json).unwrap();
        let tab = opened["tab_id"].as_str().unwrap();
        crate::tickets::open(
            &daemon.shared,
            &session,
            &call("ask_user", json!({}), "person"),
            openside_proto::TicketDeferral::External,
            crate::tools::TicketRequest {
                hire: None,
                kind: openside_proto::TicketKind::Info,
                summary: "Inspect this tab".into(),
                instructions: None,
                options: Vec::new(),
                target: Some(openside_proto::TicketTarget {
                    tab_id: Some(tab.into()),
                    origin: None,
                    field_ref: None,
                }),
                resume_when: None,
                otp: None,
                otp_register: None,
                login: None,
                choice: None,
                approval: None,
                form: None,
                pins: Vec::new(),
            },
            None,
            None,
            None,
        )
        .await
        .unwrap();
        crate::subagents::end(
            &daemon.shared,
            &session,
            "task",
            SubagentStatus::Completed,
            "completed",
        )
        .await
        .unwrap();
        crate::tasks::report_updates(&daemon.shared, &session)
            .await
            .unwrap();
        let browser = daemon
            .shared
            .registry
            .browser
            .existing()
            .await
            .unwrap_or_else(|error| panic!("{}", error.0));
        assert!(browser.tabs().await.iter().any(|info| info.id == tab));
        assert!(daemon
            .shared
            .registry
            .browser
            .task_tabs
            .holder(&session, tab)
            .is_none());
    })
    .catch_unwind()
    .await;
    daemon.shutdown().await;
    if let Err(error) = checked {
        std::panic::resume_unwind(error);
    }
}
