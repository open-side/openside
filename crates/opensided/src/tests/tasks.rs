use super::*;
use futures_util::FutureExt;

async fn front_daemon(root: &Path, runtime: &TestRuntime) -> Daemon {
    let mut config = resolved_config(root);
    config.loaded.config.agent.orchestration = openside_config::Orchestration::Front;
    Daemon::open_runtime(config, runtime.clone()).await.unwrap()
}

#[tokio::test]
async fn front_denies_actions_but_allows_reads_and_session_model_fallback() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = front_daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "front", PolicyMode::Full, None).await;
    let parent = host(&daemon, &session).await;
    assert!(
        matches!(parent.call(call("browser_navigate", json!({"tab_id":"task-tab","url":"https://example.test"}), "front-act")).await,
        ToolOutcome::Denied { reason, .. } if reason == "front_agent_cannot_act")
    );
    assert!(crate::tasks::front_denial(
        &daemon.shared,
        &call(
            "browser_snapshot",
            json!({"tab_id":"task-tab"}),
            "front-read"
        ),
        None
    )
    .is_none());
    assert!(matches!(
        parent
            .call(call(
                "browser_import_lookup",
                json!({"kind":"bookmarks"}),
                "front-import-read",
            ))
            .await,
        ToolOutcome::Deferred { .. }
    ));
    daemon
        .shared
        .command(&session, crate::engine::Action::Recover)
        .await
        .unwrap();
    let opened = runtime.0.lock().opened.clone();
    let agent: Value = serde_json::from_str(&opened[0].agent_json).unwrap();
    assert_eq!(agent["orchestration"], "front");
    assert!(agent["front_tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool == "browser_import_lookup"));
    assert!(agent.get("front_model").is_none());
    assert_eq!(opened[0].model, std::any::type_name::<TestRuntime>());
    daemon.shutdown().await;
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
        crate::tasks::report_ended(&daemon.shared, &session)
            .await
            .unwrap();
    }
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
                crate::tasks::report_ended(&daemon.shared, &session).await.unwrap();
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
                assert!(daemon.shared.db.events(&session, 0).await.unwrap().iter().any(|event|
                    matches!(&event.kind, EventKind::BrowserState(state) if state.task_id.as_deref() == Some("task") && state.phase == BrowserPhase::Done)));
            }
        }
    }).catch_unwind().await;
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
        crate::tasks::report_ended(&daemon.shared, &session).await.unwrap();
        let result = parent.call(ToolCall {
            subagent: Some("sibling".into()),
            ..call("browser_repl", json!({"tab_id":tab,"code":"await page.getByRole('textbox').focus(); await page.keyboard.press('KeyA'); await page.getByRole('textbox').inputValue()"}), "sibling-input")
        }).await;
        let ToolOutcome::Done { for_model_json } = result else { panic!("sibling REPL failed"); };
        let result: Value = serde_json::from_str(&for_model_json).unwrap();
        assert_eq!(result["return_value"], "a", "cancelled Task left Shift held: {result}");
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
            crate::tasks::report_ended(&daemon.shared, &session).await.unwrap();
            let result = parent.call(ToolCall {
                subagent: Some("sibling".into()),
                ..call("browser_repl", json!({"tab_id":tab,"code":"await page.waitForTimeout(200); await page.title()"}), "sibling-read")
            }).await;
            let ToolOutcome::Done { for_model_json } = result else { panic!("sibling read failed"); };
            let result: Value = serde_json::from_str(&for_model_json).unwrap();
            assert_eq!(result["return_value"], "Original", "{result}");
            let browser = daemon.shared.registry.browser.existing().await.unwrap_or_else(|error| panic!("{}", error.0));
            assert!(!browser.tabs().await.iter().any(|info| info.url.ends_with("/delayed")));
        }).catch_unwind().await;
        daemon.shutdown().await;
        if let Err(error) = checked {
            std::panic::resume_unwind(error);
        }
    }
}
