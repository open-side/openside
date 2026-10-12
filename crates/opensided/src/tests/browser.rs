use super::*;
use axum::{response::Html, routing::get, Router};
use openside_vault::{AccessPolicy, AllowedOrigin, Entry, PassphraseLock, Secret, Totp, Vault};

const PAGE: &str = r#"<!doctype html><html><body>
<a href='/complete'>Continue</a><button onclick='window.paid = true'>Pay now</button>
<label>Username<input id='username'></label><label>Password<input id='password' type='password'></label>
<label>Code<input id='code'></label><div id='echo'></div><button>Sign in</button>
<script>window.paid = false; username.oninput = () => echo.textContent = username.value;</script>
</body></html>"#;

async fn fixture() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let router = Router::new()
        .route("/", get(|| async { Html(PAGE) }))
        .route("/verify", get(|| async { Html(PAGE) }))
        .route("/complete", get(|| async { Html("<h1>Complete</h1>") }));
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (origin, task)
}
pub(super) fn browser_config(root: &Path) -> Option<Resolved> {
    let mut config = resolved_config(root);
    match test_browser() {
        Ok(executable) => {
            config.loaded.config.browser.executable = Some(executable);
            config.loaded.config.tickets.confirm_ttl_secs = 2;
            Some(config)
        }
        Err(error) => {
            eprintln!("SKIP daemon browser fixture tests: {error}");
            None
        }
    }
}
pub(super) async fn tool(
    daemon: &Daemon,
    session: &SessionId,
    name: &str,
    args: Value,
    id: &str,
) -> ToolOutcome {
    host(daemon, session).await.call(call(name, args, id)).await
}
pub(super) fn model(outcome: ToolOutcome) -> Value {
    let (ToolOutcome::Done { for_model_json } | ToolOutcome::DoneWithParts { for_model_json, .. }) =
        outcome
    else {
        panic!("expected completed tool: {outcome:?}");
    };
    serde_json::from_str(&for_model_json).unwrap()
}
pub(super) async fn open_tab(daemon: &Daemon, session: &SessionId, url: &str, id: &str) -> String {
    model(tool(daemon, session, "browser_open_tab", json!({"url":url}), id).await)["tab_id"]
        .as_str()
        .unwrap()
        .into()
}
pub(super) async fn reference(
    daemon: &Daemon,
    session: &SessionId,
    tab: &str,
    label: &str,
    id: &str,
) -> String {
    model(
        tool(
            daemon,
            session,
            "browser_snapshot",
            json!({"tab_id":tab}),
            id,
        )
        .await,
    );
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap();
    browser
        .snapshot(tab)
        .await
        .unwrap()
        .nodes
        .into_iter()
        .find(|node| {
            node.name == label
                && matches!(node.role.as_str(), "textbox" | "button" | "link")
                && node.reference.is_some()
        })
        .unwrap_or_else(|| panic!("no interactive ref for {label}"))
        .reference
        .unwrap()
}
async fn human(daemon: &Daemon, session: &SessionId, args: Value, id: &str) -> TicketId {
    let ToolOutcome::Deferred {
        ticket,
        kind: DeferKind::External,
    } = tool(daemon, session, "request_human_step", args, id).await
    else {
        panic!("expected human ticket");
    };
    ticket
}
pub(super) async fn resolve_value(daemon: &Daemon, ticket: &TicketId, value: Value) -> Response {
    person_resolve(daemon, ticket, value).await
}
async fn finish(daemon: &Daemon, ticket: &TicketId) {
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request(
            "POST",
            &format!("/v1/tickets/{ticket}/done"),
            json!({}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
pub(super) async fn assert_private(
    daemon: &Daemon,
    runtime: &TestRuntime,
    session: &SessionId,
    canary: &str,
) {
    assert!(
        !serde_json::to_string(&daemon.shared.db.events(session, 0).await.unwrap())
            .unwrap()
            .contains(canary)
    );
    assert!(!format!("{:?}", runtime.0.lock().opened).contains(canary));
    assert!(!format!("{:?}", runtime.0.lock().resolved).contains(canary));
    for entry in std::fs::read_dir(&daemon.shared.config.data_dir).unwrap() {
        let entry = entry.unwrap();
        if entry.path().is_file() {
            assert!(!std::fs::read(entry.path())
                .unwrap()
                .windows(canary.len())
                .any(|window| window == canary.as_bytes()));
        }
    }
}
fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap()),
            None,
        )
        .is_ok()
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

pub(super) struct BrowserCleanup(pub(super) Option<u32>);
impl Drop for BrowserCleanup {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.0 {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap()),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
    }
}

#[tokio::test]
async fn browser_tools_sensitive_expiry_vault_secret_device_and_readonly() {
    let root = tempfile::tempdir().unwrap();
    let Some(mut config) = browser_config(root.path()) else {
        return;
    };
    let (origin, fixture) = fixture().await;
    let passphrase = Secret::new("fixture-passphrase".into());
    let path = root.path().join("vault.db");
    let mut vault = Vault::create(
        &path,
        root.path().join("setup-audit.jsonl"),
        &passphrase,
        PassphraseLock::default(),
    )
    .unwrap();
    const PASSWORD: &str = "password-canary-private-4891";
    vault
        .put(
            Entry::new(
                "fixture".into(),
                vec![AllowedOrigin::parse(&origin).unwrap()],
                Secret::new("username-canary-private-4891".into()),
                Secret::new(PASSWORD.into()),
                Some(Totp::parse(Secret::new("JBSWY3DPEHPK3PXP".into())).unwrap()),
                AccessPolicy::Always,
            )
            .unwrap(),
        )
        .unwrap();
    drop(vault);
    // The passphrase exists only in the synthetic command fixture, not product settings.
    let pass_file = root.path().join("passphrase.txt");
    std::fs::write(&pass_file, "fixture-passphrase\n").unwrap();
    config.loaded.config.vault.path = Some(path);
    config.loaded.config.vault.passphrase_cmd =
        Some(vec!["cat".into(), pass_file.to_string_lossy().into_owned()]);
    let runtime = TestRuntime::default();
    let daemon = Daemon::open_runtime(config, runtime.clone()).await.unwrap();
    let session = create_session(&daemon, "browser-session", PolicyMode::Full, None).await;
    let readonly = create_session(&daemon, "readonly", PolicyMode::ReadOnly, None).await;
    let tab = open_tab(&daemon, &session, &origin, "open").await;
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap();
    let pid = browser.process_id();
    drop(browser);
    let mut cleanup = BrowserCleanup(Some(pid));
    let specs = host(&daemon, &readonly).await.specs();
    for name in [
        "browser_act",
        "vault_login",
        "vault_otp",
        "write",
        "edit",
        "bash",
    ] {
        assert!(!specs.iter().any(|tool| tool.name == name));
    }
    assert!(matches!(
        tool(
            &daemon,
            &readonly,
            "browser_act",
            json!({"tab_id":tab,"target":"e1","action":"click"}),
            "readonly-act"
        )
        .await,
        ToolOutcome::Denied { .. }
    ));
    assert!(matches!(
        tool(
            &daemon,
            &readonly,
            "browser_snapshot",
            json!({"tab_id":tab}),
            "foreign-tab"
        )
        .await,
        ToolOutcome::Denied { .. }
    ));
    assert_eq!(
        model(
            tool(
                &daemon,
                &readonly,
                "browser_tabs",
                json!({}),
                "foreign-tabs"
            )
            .await
        ),
        json!([])
    );
    let screenshot = model(
        tool(
            &daemon,
            &session,
            "browser_screenshot",
            json!({"tab_id":tab}),
            "shot",
        )
        .await,
    );
    assert_eq!(screenshot["saved"], true);
    assert!(screenshot["width"].as_u64().unwrap() > 0);
    assert!(screenshot.get("path").is_none());
    let path = daemon
        .shared
        .config
        .data_dir
        .join("artifacts/browser-session/shot.png");
    assert!(path.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let continued = reference(&daemon, &session, &tab, "Continue", "snapshot-link").await;
    assert_eq!(
        model(
            tool(
                &daemon,
                &session,
                "browser_act",
                json!({"tab_id":tab,"target":continued,"action":"click"}),
                "click-link"
            )
            .await
        )["navigated"],
        true
    );
    let back = model(
        tool(
            &daemon,
            &session,
            "browser_navigate",
            json!({"tab_id":tab,"url":origin}),
            "back",
        )
        .await,
    );
    assert_eq!(
        back,
        json!({"status":"navigated","url":format!("{origin}/")}),
        "navigation reports the loaded page as a structured url"
    );
    let pay = reference(&daemon, &session, &tab, "Pay now", "snapshot-pay").await;
    let ToolOutcome::Deferred {
        ticket: payment,
        kind: DeferKind::Approval,
    } = tool(
        &daemon,
        &session,
        "browser_act",
        json!({"tab_id":tab,"target":pay,"action":"click"}),
        "pay",
    )
    .await
    else {
        panic!("Sensitive must ask in Full");
    };
    wait_event(
        &daemon,
        &session,
        |kind| matches!(kind, EventKind::TicketExpired { ticket_id } if ticket_id == &payment),
    )
    .await;
    assert_eq!(
        crate::tickets::resolution(
            &daemon.shared.db.state(&session).await.unwrap().tickets[&payment]
        ),
        TicketResolution::Denied {
            reason: "expired".into()
        }
    );
    let mut expired_call = call(
        "browser_act",
        json!({"tab_id":tab,"target":pay,"action":"click"}),
        "pay",
    );
    expired_call.approval_ticket = Some(payment);
    assert!(matches!(
        host(&daemon, &session).await.call(expired_call).await,
        ToolOutcome::Denied { .. }
    ));
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap();
    assert_eq!(browser.evaluate(&tab, "window.paid").await.unwrap(), false);
    drop(browser);
    let username = reference(&daemon, &session, &tab, "Username", "username-ref").await;
    let password = reference(&daemon, &session, &tab, "Password", "password-ref").await;
    assert_eq!(model(tool(&daemon, &session, "vault_login", json!({"tab_id":tab,"site":"fixture","username_ref":username,"password_ref":password}), "vault").await), json!({"status":"filled"}));
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap();
    assert!(
        browser
            .evaluate(&tab, &format!("password.value === {}", json!(PASSWORD)))
            .await
            .unwrap()
            == true
    );
    drop(browser);
    assert!(!model(
        tool(
            &daemon,
            &session,
            "browser_snapshot",
            json!({"tab_id":tab}),
            "redacted-snapshot"
        )
        .await
    )
    .to_string()
    .contains(PASSWORD));
    assert!(
        matches!(tool(&daemon, &session, "browser_screenshot", json!({"tab_id":tab}), "secret-shot").await, ToolOutcome::Denied { reason, .. } if reason == "screenshot_unavailable_after_secret_fill")
    );
    // A page-driven document replacement (no tool navigation) must not reopen captures:
    // the replacement could be a BFCache restore or carry the filled value.
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap();
    let before = browser.document_id(&tab).await.unwrap();
    browser
        .evaluate(&tab, "setTimeout(() => location.reload(), 0); true")
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while browser.document_id(&tab).await.ok().as_ref() == Some(&before) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the page never reloaded"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    drop(browser);
    assert!(
        matches!(tool(&daemon, &session, "browser_screenshot", json!({"tab_id":tab}), "reloaded-shot").await, ToolOutcome::Denied { reason, .. } if reason == "screenshot_unavailable_after_secret_fill")
    );
    let code = reference(&daemon, &session, &tab, "Code", "code-ref").await;
    assert_eq!(
        model(
            tool(
                &daemon,
                &session,
                "vault_otp",
                json!({"tab_id":tab,"site":"fixture","field_ref":code}),
                "otp"
            )
            .await
        ),
        json!({"status":"filled"})
    );
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap();
    assert_eq!(
        browser
            .evaluate(&tab, "/^[0-9]{6}$/.test(code.value)")
            .await
            .unwrap(),
        true
    );
    drop(browser);
    assert_private(&daemon, &runtime, &session, PASSWORD).await;
    model(
        tool(
            &daemon,
            &session,
            "browser_navigate",
            json!({"tab_id":tab,"url":origin}),
            "fresh",
        )
        .await,
    );
    // Navigating away does not reopen captures in a tab that has held a secret fill.
    assert!(
        matches!(tool(&daemon, &session, "browser_screenshot", json!({"tab_id":tab}), "fresh-shot").await, ToolOutcome::Denied { reason, .. } if reason == "screenshot_unavailable_after_secret_fill")
    );
    let field = reference(&daemon, &session, &tab, "Code", "secret-code-ref").await;
    let secret = human(&daemon, &session, json!({"kind":"secret_input","summary":"Enter verification code","tab_id":tab,"origin":origin,"field_ref":field}), "secret-code").await;
    let other_origin = origin.replace("localhost", "127.0.0.1");
    model(
        tool(
            &daemon,
            &session,
            "browser_navigate",
            json!({"tab_id":tab,"url":other_origin}),
            "other-origin",
        )
        .await,
    );
    let response = resolve_value(&daemon, &secret, json!("secret-canary-private-9203")).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(response).await["code"], "origin_mismatch");
    assert_eq!(
        daemon.shared.db.state(&session).await.unwrap().tickets[&secret].status,
        TicketStatus::Pending
    );
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request(
            "POST",
            &format!("/v1/tickets/{secret}/reject"),
            json!({}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    model(
        tool(
            &daemon,
            &session,
            "browser_navigate",
            json!({"tab_id":tab,"url":origin}),
            "secret-origin",
        )
        .await,
    );
    let field = reference(&daemon, &session, &tab, "Code", "secret-fresh-ref").await;
    let secret = human(&daemon, &session, json!({"kind":"secret_input","summary":"Enter verification code","tab_id":tab,"origin":origin,"field_ref":field}), "secret-fresh").await;
    const SECRET: &str = "secret-canary-private-9203";
    let response = resolve_value(&daemon, &secret, json!(SECRET)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await["resolution"],
        json!({"status":"filled"})
    );
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap();
    assert_eq!(
        browser
            .evaluate(&tab, &format!("code.value === {}", json!(SECRET)))
            .await
            .unwrap(),
        true
    );
    drop(browser);
    assert!(!model(
        tool(
            &daemon,
            &session,
            "browser_snapshot",
            json!({"tab_id":tab}),
            "secret-output"
        )
        .await
    )
    .to_string()
    .contains(SECRET));
    assert_private(&daemon, &runtime, &session, SECRET).await;
    model(
        tool(
            &daemon,
            &session,
            "browser_navigate",
            json!({"tab_id":tab,"url":format!("{origin}/verify")}),
            "verify",
        )
        .await,
    );
    let device = human(&daemon, &session, json!({"kind":"device_approval","summary":"Approve on your device","tab_id":tab,"resume_when":{"url_not_contains":"/verify"}}), "device").await;
    let selector = human(&daemon, &session, json!({"kind":"device_approval","summary":"Wait for completion element","tab_id":tab,"resume_when":{"selector_visible":"h1"}}), "selector").await;
    let text = human(&daemon, &session, json!({"kind":"device_approval","summary":"Wait for completion text","tab_id":tab,"resume_when":{"text_visible":"Complete"}}), "text").await;
    model(
        tool(
            &daemon,
            &session,
            "browser_navigate",
            json!({"tab_id":tab,"url":format!("{origin}/complete")}),
            "verified",
        )
        .await,
    );
    wait_event(&daemon, &session, |kind| matches!(kind, EventKind::TicketResolved { ticket_id, resolution } if ticket_id == &device && resolution["status"] == "done")).await;
    for ticket in [&selector, &text] {
        wait_event(&daemon, &session, |kind| matches!(kind, EventKind::TicketResolved { ticket_id, resolution } if ticket_id == ticket && resolution["status"] == "done")).await;
    }
    let manual = human(&daemon, &session, json!({"kind":"device_approval","summary":"Manual approval","tab_id":tab,"resume_when":{"manual":true}}), "manual").await;
    daemon
        .shared
        .registry
        .browser
        .poll(&daemon.shared)
        .await
        .unwrap();
    assert_eq!(
        daemon.shared.db.state(&session).await.unwrap().tickets[&manual].status,
        TicketStatus::Pending
    );
    finish(&daemon, &manual).await;
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap();
    browser.mark_asleep(&tab, true, false).await.unwrap();
    drop(browser);
    daemon
        .shared
        .registry
        .browser
        .poll(&daemon.shared)
        .await
        .unwrap();
    wait_event(
        &daemon,
        &session,
        |kind| matches!(kind, EventKind::TabAsleep { tab_id } if tab_id == &tab),
    )
    .await;
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap();
    browser.mark_asleep(&tab, false, false).await.unwrap();
    drop(browser);
    model(
        tool(
            &daemon,
            &session,
            "browser_close_tab",
            json!({"tab_id":tab}),
            "close",
        )
        .await,
    );
    wait_event(
        &daemon,
        &session,
        |kind| matches!(kind, EventKind::TabClosed { tab_id } if tab_id == &tab),
    )
    .await;
    daemon.shutdown().await;
    assert!(!process_alive(pid));
    cleanup.0 = None;
    fixture.abort();
}

#[tokio::test]
async fn pending_secret_survives_daemon_restart_and_dead_endpoint_relaunches() {
    let root = tempfile::tempdir().unwrap();
    let Some(config) = browser_config(root.path()) else {
        return;
    };
    let (origin, fixture) = fixture().await;
    let runtime = TestRuntime::default();
    let daemon = Daemon::open_runtime(config.clone(), runtime.clone())
        .await
        .unwrap();
    let session = create_session(&daemon, "restart-session", PolicyMode::Full, None).await;
    let tab = open_tab(&daemon, &session, &origin, "restart-open").await;
    let field = reference(&daemon, &session, &tab, "Code", "restart-ref").await;
    let secret = human(&daemon, &session, json!({"kind":"secret_input","summary":"Enter code","tab_id":tab,"origin":origin,"field_ref":field}), "restart-secret").await;
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap();
    let pid = browser.process_id();
    drop(browser);
    let mut cleanup = BrowserCleanup(Some(pid));
    daemon.shutdown().await;
    assert!(process_alive(pid));
    drop(daemon);
    let daemon = Daemon::open_runtime(config.clone(), runtime.clone())
        .await
        .unwrap();
    const CANARY: &str = "restart-canary-private-8472";
    let response = resolve_value(&daemon, &secret, json!(CANARY)).await;
    let status = response.status();
    let body = json_body(response).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap();
    assert_eq!(browser.process_id(), pid);
    assert_eq!(
        browser
            .evaluate(&tab, &format!("code.value === {}", json!(CANARY)))
            .await
            .unwrap(),
        true
    );
    drop(browser);
    assert_private(&daemon, &runtime, &session, CANARY).await;
    let field = reference(&daemon, &session, &tab, "Code", "stale-ref").await;
    let stale = human(&daemon, &session, json!({"kind":"secret_input","summary":"Enter another code","tab_id":tab,"origin":origin,"field_ref":field}), "stale-secret").await;
    model(
        tool(
            &daemon,
            &session,
            "browser_navigate",
            json!({"tab_id":tab,"url":origin}),
            "same-origin-reload",
        )
        .await,
    );
    let response = resolve_value(&daemon, &stale, json!(CANARY)).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(response).await["code"], "secret_input_failed");
    assert_eq!(
        daemon.shared.db.state(&session).await.unwrap().tickets[&stale].status,
        TicketStatus::Pending
    );
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request(
            "POST",
            &format!("/v1/tickets/{stale}/reject"),
            json!({}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let saved = std::fs::read(daemon.shared.config.data_dir.join("browser-state.json")).unwrap();
    daemon.shutdown().await;
    assert!(!process_alive(pid));
    drop(daemon);
    cleanup.0 = None;
    std::fs::write(config.data_dir.join("browser-state.json"), saved).unwrap();
    let daemon = Daemon::open_runtime(config, runtime).await.unwrap();
    let tab = open_tab(&daemon, &session, &origin, "relaunch-open").await;
    assert!(!tab.is_empty());
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap();
    let new_pid = browser.process_id();
    assert_ne!(pid, new_pid);
    drop(browser);
    cleanup.0 = Some(new_pid);
    daemon.shutdown().await;
    assert!(!process_alive(new_pid));
    cleanup.0 = None;
    fixture.abort();
}

struct ScriptedSink {
    codes: Mutex<Vec<&'static str>>,
    calls: Arc<Mutex<usize>>,
}
impl SecretSink for ScriptedSink {
    fn fill<'a>(
        &'a self,
        _: &'a TicketId,
        _: &'a SessionId,
        _: &'a TicketTarget,
        _: Zeroizing<String>,
    ) -> BoxFuture<'a, std::result::Result<(), SecretSinkError>> {
        Box::pin(async move {
            *self.calls.lock() += 1;
            match self.codes.lock().remove(0) {
                "ok" => Ok(()),
                code => Err(SecretSinkError { code }),
            }
        })
    }
}

#[tokio::test]
async fn indeterminate_secret_fill_refuses_blind_retry_until_rejected() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let deadline = Utc::now() + chrono::Duration::minutes(5);
    opened(
        &daemon,
        &session,
        TicketKind::SecretInput,
        "flaky",
        deadline,
    )
    .await;
    opened(
        &daemon,
        &session,
        TicketKind::SecretInput,
        "crashed",
        deadline,
    )
    .await;
    let calls = Arc::new(Mutex::new(0));
    *daemon.shared.registry.secret_sink.write() = Some(Arc::new(ScriptedSink {
        codes: Mutex::new(vec!["target_unavailable", "fill_failed"]),
        calls: calls.clone(),
    }));
    let flaky = TicketId::new("flaky");
    for expected in [
        "secret_input_failed",
        "secret_input_failed",
        "secret_input_indeterminate",
    ] {
        let response = resolve_value(&daemon, &flaky, json!("indeterminate-canary")).await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(json_body(response).await["code"], expected);
    }
    // A definite refusal allowed the second attempt; the unknown outcome blocks the third.
    assert_eq!(*calls.lock(), 2);
    assert_eq!(
        daemon.shared.db.state(&session).await.unwrap().tickets[&flaky].status,
        TicketStatus::Pending
    );

    // An intent recorded before a crash survives the restart and blocks the fill.
    let crashed = TicketId::new("crashed");
    daemon
        .shared
        .registry
        .browser
        .record_fill_intent(&daemon.shared, &crashed, None)
        .await
        .unwrap();
    daemon.shutdown().await;
    drop(daemon);
    let daemon = super::daemon(root.path(), &runtime).await;
    let calls = Arc::new(Mutex::new(0));
    *daemon.shared.registry.secret_sink.write() = Some(Arc::new(ScriptedSink {
        codes: Mutex::new(vec!["ok"]),
        calls: calls.clone(),
    }));
    let response = resolve_value(&daemon, &crashed, json!("indeterminate-canary")).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body(response).await["code"],
        "secret_input_indeterminate"
    );
    assert_eq!(*calls.lock(), 0);
    assert!(runtime.0.lock().resolved.is_empty());
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request(
            "POST",
            &format!("/v1/tickets/{crashed}/reject"),
            json!({}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!daemon
        .shared
        .registry
        .browser
        .fill_intent_pending(&daemon.shared, &crashed)
        .await
        .unwrap());
    assert_private(&daemon, &runtime, &session, "indeterminate-canary").await;
    daemon.shutdown().await;
}

#[tokio::test]
async fn recovered_secret_document_denies_snapshot_until_navigation() {
    const CANARY: &str = "recovered-taint-canary-5531";
    let root = tempfile::tempdir().unwrap();
    let Some(config) = browser_config(root.path()) else {
        return;
    };
    let (origin, fixture) = fixture().await;
    let runtime = TestRuntime::default();
    let daemon = Daemon::open_runtime(config.clone(), runtime.clone())
        .await
        .unwrap();
    let session = create_session(&daemon, "taint-session", PolicyMode::Full, None).await;
    let tab = open_tab(&daemon, &session, &origin, "taint-open").await;
    let username = reference(&daemon, &session, &tab, "Username", "taint-ref").await;
    let code = reference(&daemon, &session, &tab, "Code", "taint-code-ref").await;
    let filled = human(&daemon, &session, json!({"kind":"secret_input","summary":"Enter name","tab_id":tab,"origin":origin,"field_ref":username}), "taint-secret").await;
    // Keeps the browser and tab alive across the daemon restart.
    human(&daemon, &session, json!({"kind":"secret_input","summary":"Enter code","tab_id":tab,"origin":origin,"field_ref":code}), "taint-pending").await;
    let response = resolve_value(&daemon, &filled, json!(CANARY)).await;
    assert_eq!(response.status(), StatusCode::OK);
    // The page echoes the username as text; live filters still redact it before restart.
    let text = model(
        tool(
            &daemon,
            &session,
            "browser_snapshot",
            json!({"tab_id":tab}),
            "taint-live-snapshot",
        )
        .await,
    );
    assert!(!text.to_string().contains(CANARY));
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap();
    // The pending ticket keeps the browser detached at shutdown, so the guard must kill it.
    let _cleanup = BrowserCleanup(Some(browser.process_id()));
    drop(browser);
    daemon.shutdown().await;
    drop(daemon);

    let daemon = Daemon::open_runtime(config, runtime.clone()).await.unwrap();
    for id in ["taint-recovered-diff", "taint-recovered-full"] {
        let outcome = tool(
            &daemon,
            &session,
            "browser_snapshot",
            json!({"tab_id":tab,"since":if id.ends_with("diff") { "unknown-base" } else { "" }}),
            id,
        )
        .await;
        assert!(
            matches!(&outcome, ToolOutcome::Denied { reason, .. } if reason == "snapshot_unavailable_after_secret_fill"),
            "{outcome:?}"
        );
    }
    model(
        tool(
            &daemon,
            &session,
            "browser_navigate",
            json!({"tab_id":tab,"url":origin}),
            "taint-navigate",
        )
        .await,
    );
    let text = model(
        tool(
            &daemon,
            &session,
            "browser_snapshot",
            json!({"tab_id":tab}),
            "taint-fresh-snapshot",
        )
        .await,
    );
    assert!(!text.to_string().contains(CANARY));
    assert_private(&daemon, &runtime, &session, CANARY).await;
    daemon.shutdown().await;
    fixture.abort();
}

#[tokio::test]
async fn stale_secret_ticket_cannot_displace_newer_document_taint() {
    const CANARY: &str = "stale-taint-canary-7713";
    let root = tempfile::tempdir().unwrap();
    let Some(config) = browser_config(root.path()) else {
        return;
    };
    let (origin, fixture) = fixture().await;
    let runtime = TestRuntime::default();
    let daemon = Daemon::open_runtime(config.clone(), runtime.clone())
        .await
        .unwrap();
    let session = create_session(&daemon, "stale-taint", PolicyMode::Full, None).await;
    let tab = open_tab(&daemon, &session, &origin, "stale-open").await;
    let old_field = reference(&daemon, &session, &tab, "Username", "stale-old-ref").await;
    let stale = human(&daemon, &session, json!({"kind":"secret_input","summary":"Old","tab_id":tab,"origin":origin,"field_ref":old_field}), "stale-old").await;
    model(
        tool(
            &daemon,
            &session,
            "browser_navigate",
            json!({"tab_id":tab,"url":origin}),
            "stale-reload",
        )
        .await,
    );
    let new_field = reference(&daemon, &session, &tab, "Username", "stale-new-ref").await;
    let fresh = human(&daemon, &session, json!({"kind":"secret_input","summary":"New","tab_id":tab,"origin":origin,"field_ref":new_field}), "stale-new").await;
    let response = resolve_value(&daemon, &fresh, json!(CANARY)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = resolve_value(&daemon, &stale, json!(CANARY)).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(response).await["code"], "secret_input_failed");
    assert_eq!(
        daemon.shared.db.state(&session).await.unwrap().tickets[&stale].status,
        TicketStatus::Pending
    );
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap();
    // The stale ticket stays pending, so shutdown detaches and the guard must kill the browser.
    let _cleanup = BrowserCleanup(Some(browser.process_id()));
    drop(browser);
    daemon.shutdown().await;
    drop(daemon);

    let daemon = Daemon::open_runtime(config, runtime.clone()).await.unwrap();
    let outcome = tool(
        &daemon,
        &session,
        "browser_snapshot",
        json!({"tab_id":tab}),
        "stale-recovered-snapshot",
    )
    .await;
    assert!(
        matches!(&outcome, ToolOutcome::Denied { reason, .. } if reason == "snapshot_unavailable_after_secret_fill"),
        "{outcome:?}"
    );
    assert_private(&daemon, &runtime, &session, CANARY).await;
    daemon.shutdown().await;
    fixture.abort();
}

const SINGLE_CODE_FIELD: &str = "<!doctype html><title>Code</title><label>Verification code<input id='code' type='text' inputmode='numeric' maxlength='6' autocomplete='one-time-code'></label>";

/// Six one-character boxes that advance on input and, like browser autofill and paste
/// handlers, spread a multi-character value written into one box over the following ones.
fn split_code_boxes() -> String {
    let boxes: String = (1..=6)
        .map(|n| format!("<input aria-label='Digit {n}' inputmode='numeric' maxlength='1' autocomplete='one-time-code'>"))
        .collect();
    format!(
        r#"<!doctype html><title>Split code</title><body>{boxes}<script>
const boxes = [...document.querySelectorAll('input')];
boxes.forEach((box, at) => box.addEventListener('input', () => {{
  if (box.value.length > 1) {{
    [...box.value].slice(0, boxes.length - at).forEach((c, k) => boxes[at + k].value = c);
  }}
  if (box.value && boxes[at + 1]) boxes[at + 1].focus();
}}));
window.boxValues = () => boxes.map(b => b.value).join('|');
</script></body>"#
    )
}

/// Six boxes that never spread a value themselves: each keeps what was written into it and
/// only moves focus on `input`, like hand-rolled React/Vue code inputs. `attributes` decides
/// how the run is recognised (`maxlength='1'`, or `autocomplete='one-time-code'` alone).
/// `eventLog()` lists the event types each box saw, never the characters.
fn naive_code_boxes(attributes: &str) -> String {
    odd_code_boxes(attributes, "")
}

/// Like [`naive_code_boxes`], with `odd` added to the fourth box only.
fn odd_code_boxes(attributes: &str, odd: &str) -> String {
    let boxes: String = (1..=6)
        .map(|n| {
            let odd = if n == 4 { odd } else { "" };
            format!("<input aria-label='Digit {n}' inputmode='numeric' {attributes} {odd}>")
        })
        .collect();
    format!(
        r#"<!doctype html><title>Split code</title><body><form>{boxes}<button type='button'>Verify</button></form><script>
const boxes = [...document.querySelectorAll('input')];
const log = boxes.map(() => []);
boxes.forEach((box, at) => {{
  for (const type of ['keydown', 'beforeinput', 'input', 'keyup', 'change']) {{
    box.addEventListener(type, () => log[at].push(type));
  }}
  box.addEventListener('input', () => {{ if (box.value && boxes[at + 1]) boxes[at + 1].focus(); }});
}});
window.boxValues = () => boxes.map(b => b.value).join('|');
window.eventLog = () => log.map(types => types.join(',')).join('|');
</script></body>"#
    )
}

fn code_screens() -> Router {
    let boxes = split_code_boxes();
    let naive = naive_code_boxes("maxlength='1'");
    let one_time = naive_code_boxes("autocomplete='one-time-code'");
    let hidden = odd_code_boxes("maxlength='1'", "style='display:none'");
    let mixed = odd_code_boxes("maxlength='1'", "type='checkbox'");
    Router::new()
        .route("/single", get(|| async { Html(SINGLE_CODE_FIELD) }))
        .route("/boxes", get(move || async move { Html(boxes) }))
        .route("/naive", get(move || async move { Html(naive) }))
        .route("/one-time", get(move || async move { Html(one_time) }))
        .route("/hidden", get(move || async move { Html(hidden) }))
        .route("/mixed", get(move || async move { Html(mixed) }))
}

pub(super) fn string_values<'a>(value: &'a Value, out: &mut Vec<&'a str>) {
    match value {
        Value::String(text) => out.push(text),
        Value::Array(items) => items.iter().for_each(|item| string_values(item, out)),
        Value::Object(map) => map.values().for_each(|item| string_values(item, out)),
        _ => {}
    }
}

/// The model's snapshot of a document that received a secret fill shows no field value, so
/// one-character boxes, too short for output redaction, cannot spell the code out.
pub(super) async fn assert_snapshot_hides_values(
    daemon: &Daemon,
    session: &SessionId,
    tab: &str,
    code: &str,
) {
    let snapshot = model(
        tool(
            daemon,
            session,
            "browser_snapshot",
            json!({"tab_id":tab}),
            &format!("hidden-{}", code.len()),
        )
        .await,
    );
    let mut texts = Vec::new();
    string_values(&snapshot, &mut texts);
    assert!(texts.iter().any(|text| text.contains("= \"<redacted>\"")));
    for value in code.chars().map(String::from).chain([code.to_owned()]) {
        let shown = format!("= {value:?}");
        assert!(
            !texts.iter().any(|text| text.contains(&shown)),
            "snapshot shows a filled value"
        );
    }
}

/// How the code reaches the page: a generic `secret_input` ticket (exactly one field), or a
/// `request_otp` ticket (split code boxes pinned when it opened).
#[derive(Clone, Copy, PartialEq)]
enum Via {
    SecretInput,
    RequestOtp,
}

/// A whole one-time code written into the field of a ticket: a human's answer and a
/// forwarded SMS take the same path. A single field sees one `input` event; a run of
/// one-character boxes of a `request_otp` ticket gets one character per box. Returns the
/// resolve status and `probe` evaluated on the page afterwards.
async fn fill_code_screen_via(
    via: Via,
    path: &str,
    label: &str,
    code: &str,
    probe: &str,
) -> Option<(StatusCode, Value)> {
    let root = tempfile::tempdir().unwrap();
    let config = browser_config(root.path())?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let router = code_screens();
    let fixture = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let runtime = TestRuntime::default();
    let daemon = Daemon::open_runtime(config, runtime.clone()).await.unwrap();
    let session = create_session(&daemon, "code-session", PolicyMode::Full, None).await;
    let tab = open_tab(&daemon, &session, &format!("{origin}{path}"), "code-open").await;
    let field = reference(&daemon, &session, &tab, label, "code-ref").await;
    let ticket = match via {
        Via::SecretInput => human(&daemon, &session, json!({"kind":"secret_input","summary":"Enter the verification code","tab_id":tab,"origin":origin,"field_ref":field}), "code-step").await,
        Via::RequestOtp => {
            let ToolOutcome::Deferred { ticket, .. } = tool(
                &daemon,
                &session,
                "request_otp",
                json!({"tab_id":tab,"field_ref":field,"channel":"sms"}),
                "code-otp",
            )
            .await
            else {
                panic!("expected a code ticket");
            };
            ticket
        }
    };
    let response = resolve_value(&daemon, &ticket, json!(code)).await;
    let status = response.status();
    if status == StatusCode::OK {
        assert_eq!(
            json_body(response).await["resolution"],
            json!({"status":"filled"})
        );
    }
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap();
    let page = browser.evaluate(&tab, probe).await.unwrap();
    drop(browser);
    if status == StatusCode::OK {
        assert_snapshot_hides_values(&daemon, &session, &tab, code).await;
    }
    assert_private(&daemon, &runtime, &session, code).await;
    daemon.shutdown().await;
    fixture.abort();
    Some((status, page))
}

async fn fill_code_screen(path: &str, label: &str, code: &str, probe: &str) -> Option<Value> {
    let (status, page) = fill_code_screen_via(Via::RequestOtp, path, label, code, probe).await?;
    assert_eq!(status, StatusCode::OK);
    Some(page)
}

#[tokio::test]
async fn one_time_code_field_with_maxlength_takes_the_whole_code() {
    const CODE: &str = "Q7K2XM";
    let Some(value) = fill_code_screen("/single", "Verification code", CODE, "code.value").await
    else {
        return;
    };
    assert_eq!(value, json!(CODE));
}

#[tokio::test]
async fn whole_code_in_the_first_split_box_reaches_every_box_on_a_distributing_page() {
    const CODE: &str = "Q7K2XM";
    let Some(value) = fill_code_screen("/boxes", "Digit 1", CODE, "boxValues()").await else {
        return;
    };
    assert_eq!(value, json!("Q|7|K|2|X|M"));
}

const EVERY_BOX_TYPED: &str = "keydown,beforeinput,input,keyup,change";

#[tokio::test]
async fn one_code_request_fills_six_maxlength_one_boxes_that_do_not_distribute() {
    const CODE: &str = "Q7K2XM";
    let Some(value) =
        fill_code_screen("/naive", "Digit 1", CODE, "boxValues() + '#' + eventLog()").await
    else {
        return;
    };
    let events = [EVERY_BOX_TYPED; 6].join("|");
    assert_eq!(value, json!(format!("Q|7|K|2|X|M#{events}")));
}

#[tokio::test]
async fn one_code_request_fills_a_run_of_one_time_code_boxes_without_maxlength() {
    const CODE: &str = "Q7K2XM";
    let Some(value) = fill_code_screen(
        "/one-time",
        "Digit 1",
        CODE,
        "boxValues() + '#' + eventLog()",
    )
    .await
    else {
        return;
    };
    let events = [EVERY_BOX_TYPED; 6].join("|");
    assert_eq!(value, json!(format!("Q|7|K|2|X|M#{events}")));
}

/// B4: spreading a value over neighbouring boxes is only for one-time codes. A generic
/// `secret_input` (a password, say) writes exactly its own field and never the inputs after it.
#[tokio::test]
async fn a_generic_secret_input_writes_only_its_own_field() {
    let Some((status, value)) = fill_code_screen_via(
        Via::SecretInput,
        "/one-time",
        "Digit 1",
        "Q7K2XM",
        "boxValues()",
    )
    .await
    else {
        return;
    };
    assert_eq!(status, StatusCode::OK);
    let boxes: Vec<String> = value
        .as_str()
        .unwrap()
        .split('|')
        .map(str::to_owned)
        .collect();
    assert!(boxes[1..].iter().all(String::is_empty), "{value}");
}

/// B4: a run broken by a hidden box or a box of another kind is not a code group, so the code
/// never reaches a box after the first.
#[tokio::test]
async fn split_codes_never_reach_hidden_or_different_boxes() {
    for path in ["/hidden", "/mixed"] {
        let Some((status, value)) =
            fill_code_screen_via(Via::RequestOtp, path, "Digit 1", "Q7K2XM", "boxValues()").await
        else {
            return;
        };
        // Either nothing is written or only the first box (a plain single-field fill); the
        // code never spreads past the break.
        let boxes: Vec<&str> = value.as_str().unwrap().split('|').collect();
        // ("on" is the checkbox's own default value, not a written character.)
        assert!(
            boxes[1..].iter().all(|b| b.is_empty() || *b == "on"),
            "{path} ({status}): {value}"
        );
    }
}

/// Regression: a one-character answer used to become a daemon-wide substring
/// redactor, rewrote every such character in later ticket ids, and the resolve returned 409
/// `invalid_transition` while the next step failed. One ticket per box must now complete, and
/// no digit may reach events, the runtime's resolutions or the model's snapshot.
#[tokio::test]
async fn one_ticket_per_split_box_with_one_character_values_completes_without_leaking_digits() {
    const CODE: &str = "482915";
    let root = tempfile::tempdir().unwrap();
    let Some(config) = browser_config(root.path()) else {
        return;
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let router = code_screens();
    let fixture = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let runtime = TestRuntime::default();
    let daemon = Daemon::open_runtime(config, runtime.clone()).await.unwrap();
    let session = create_session(&daemon, "digit-session", PolicyMode::Full, None).await;
    let tab = open_tab(&daemon, &session, &format!("{origin}/naive"), "digit-open").await;
    let mut tickets = Vec::new();
    for (index, digit) in CODE.chars().enumerate() {
        let label = format!("Digit {}", index + 1);
        let field = reference(
            &daemon,
            &session,
            &tab,
            &label,
            &format!("digit-ref-{index}"),
        )
        .await;
        let ticket = human(&daemon, &session, json!({"kind":"secret_input","summary":label,"tab_id":tab,"origin":origin,"field_ref":field}), &format!("digit-step-{index}")).await;
        let response = resolve_value(&daemon, &ticket, json!(digit.to_string())).await;
        assert_eq!(response.status(), StatusCode::OK, "digit {index}");
        let body = json_body(response).await;
        assert_eq!(body["id"], json!(ticket.as_str()));
        assert_eq!(body["resolution"], json!({"status":"filled"}));
        tickets.push(ticket);
    }
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap();
    assert_eq!(
        browser.evaluate(&tab, "boxValues()").await.unwrap(),
        json!("4|8|2|9|1|5")
    );
    drop(browser);
    assert_snapshot_hides_values(&daemon, &session, &tab, CODE).await;

    let digits: Vec<String> = CODE.chars().map(String::from).collect();
    let events = serde_json::to_value(daemon.shared.db.events(&session, 0).await.unwrap()).unwrap();
    let mut seen = Vec::new();
    string_values(&events, &mut seen);
    assert!(
        !seen
            .iter()
            .any(|text| digits.iter().any(|digit| text == digit)),
        "an event carries a filled digit as a value"
    );
    for ticket in &tickets {
        assert!(
            seen.contains(&ticket.as_str()),
            "ticket ids reach events intact"
        );
    }
    let resolved = runtime.0.lock().resolved.clone();
    for ticket in &tickets {
        let (_, _, resolution) = resolved
            .iter()
            .find(|(id, _, _)| id == ticket)
            .expect("every box ticket resumes the agent");
        let resolution = format!("{resolution:?}");
        assert!(resolution.contains("filled"), "{resolution}");
        for digit in &digits {
            assert!(!resolution.contains(&format!("{digit:?}")));
        }
    }
    assert_private(&daemon, &runtime, &session, CODE).await;
    daemon.shutdown().await;
    fixture.abort();
}
