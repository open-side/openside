use super::*;
use axum::{response::Html, routing::get, Router};

const PAGE: &str = r#"<!doctype html><html lang="en"><head><title>Shop</title></head><body>
<h1>Shop</h1><a href="/next">Next page</a>
<button onclick="document.getElementById('bought').textContent = 'bought'">Buy now</button>
<button onclick="document.getElementById('deleted').textContent = 'deleted'">Delete item</button>
<span id="bought">not bought</span> <span id="deleted">kept</span>
<label>Note<input id="note"></label>
</body></html>"#;

async fn fixture() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let router = Router::new()
        .route("/", get(|| async { Html(PAGE) }))
        .route(
            "/next",
            get(|| async { Html("<!doctype html><title>Next</title><h1>Next</h1>") }),
        );
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (origin, task)
}

fn browser_config(root: &Path) -> Option<Resolved> {
    let mut config = resolved_config(root);
    match test_browser() {
        Ok(executable) => {
            config.loaded.config.browser.executable = Some(executable);
            Some(config)
        }
        Err(error) => {
            eprintln!("SKIP daemon browser_repl tests: {error}");
            None
        }
    }
}

struct BrowserCleanup(u32);
impl Drop for BrowserCleanup {
    fn drop(&mut self) {
        #[cfg(unix)]
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(i32::try_from(self.0).unwrap()),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
}

async fn tool(
    daemon: &Daemon,
    session: &SessionId,
    name: &str,
    args: Value,
    id: &str,
) -> ToolOutcome {
    host(daemon, session).await.call(call(name, args, id)).await
}

fn model(outcome: ToolOutcome) -> Value {
    let ToolOutcome::Done { for_model_json } = outcome else {
        panic!("expected completed tool: {outcome:?}");
    };
    serde_json::from_str(&for_model_json).unwrap()
}

async fn repl(daemon: &Daemon, session: &SessionId, tab: &str, code: &str, id: &str) -> Value {
    model(
        tool(
            daemon,
            session,
            "browser_repl",
            json!({"tab_id": tab, "code": code}),
            id,
        )
        .await,
    )
}

async fn approve(daemon: &Daemon, ticket: &TicketId) {
    let response = person_resolve(daemon, ticket, json!(true)).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browser_repl_shares_snapshot_refs_and_reruns_sensitive_steps_after_approval() {
    let root = tempfile::tempdir().unwrap();
    let Some(config) = browser_config(root.path()) else {
        return;
    };
    let (origin, server) = fixture().await;
    let runtime = TestRuntime::default();
    let daemon = Daemon::open_runtime(config, runtime.clone()).await.unwrap();
    let session = create_session(&daemon, "repl", PolicyMode::Guard, None).await;
    let readonly = create_session(&daemon, "readonly", PolicyMode::ReadOnly, None).await;
    assert!(host(&daemon, &session)
        .await
        .specs()
        .iter()
        .any(|tool| tool.name == "browser_repl"));
    assert!(!host(&daemon, &readonly)
        .await
        .specs()
        .iter()
        .any(|tool| tool.name == "browser_repl"));

    let tab = model(
        tool(
            &daemon,
            &session,
            "browser_open_tab",
            json!({"url": origin}),
            "open",
        )
        .await,
    )["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap();
    let _cleanup = BrowserCleanup(browser.process_id());
    browser.record_protocol(true);

    // E-3: the agent's snapshot text and refs are the REPL's snapshot and aria-ref targets.
    let snapshot = model(
        tool(
            &daemon,
            &session,
            "browser_snapshot",
            json!({"tab_id": tab}),
            "snap",
        )
        .await,
    );
    // The first line is the capture's `[snapshot_id …]`; the tree below it is the REPL's.
    let snapshot = snapshot
        .as_str()
        .unwrap()
        .split_once('\n')
        .unwrap()
        .1
        .to_owned();
    let line = snapshot
        .lines()
        .find(|line| line.contains("link \"Next page\""))
        .unwrap();
    let reference = line
        .trim()
        .trim_start_matches('[')
        .split(']')
        .next()
        .unwrap()
        .to_owned();
    let parity = repl(
        &daemon,
        &session,
        &tab,
        &format!(
            "const byRef = page.locator('aria-ref={reference}'); const byRole = page.getByRole('link', {{ name: 'Next page' }}); \
             [await page.snapshot(), await byRef.getAttribute('href'), JSON.stringify(await byRef.boundingBox()) === JSON.stringify(await byRole.boundingBox())]"
        ),
        "parity",
    )
    .await;
    assert_eq!(
        parity["return_value"],
        json!([snapshot, "/next", true]),
        "{parity}"
    );
    assert_eq!(parity["tab_id"], json!(tab));
    // Structured page URL for citation checks, never scraped from page text.
    assert_eq!(parity["url"], json!(format!("{origin}/")), "{parity}");
    let internal_reads = repl(
        &daemon,
        &session,
        &tab,
        r#"await page.locator('#note').fill('note'); await page.locator('xpath=//input[@id="note"]').getAttribute('id')"#,
        "internal-readers",
    )
    .await;
    assert_eq!(internal_reads["return_value"], "note");

    // A sensitive click stops before it happens and reports a descriptor bound to the
    // operation, target, origin and tab; sending it back asks for approval.
    let buy = "await page.getByRole('button', { name: 'Buy now' }).click(); await page.locator('#bought').textContent()";
    let buy_action = format!("click button \"Buy now\" on {origin} in tab {tab}");
    let first = tool(
        &daemon,
        &session,
        "browser_repl",
        json!({"tab_id": tab, "code": buy}),
        "buy-1",
    )
    .await;
    let approvals = approvals_of(&first, &buy_action);
    assert_eq!(
        approvals,
        json!([{"capability": "sensitive", "target": buy_action}])
    );
    let untouched = repl(
        &daemon,
        &session,
        &tab,
        "await page.locator('#bought').textContent()",
        "check-1",
    )
    .await;
    assert_eq!(untouched["return_value"], "not bought");
    let args = json!({"tab_id": tab, "code": buy, "approvals": approvals});
    let ToolOutcome::Deferred {
        ticket,
        kind: DeferKind::Approval,
    } = tool(&daemon, &session, "browser_repl", args.clone(), "buy-2").await
    else {
        panic!("a call carrying the descriptor must open an approval ticket");
    };
    let opened = daemon.shared.db.state(&session).await.unwrap().tickets[&ticket].clone();
    let shown = &opened.tool.as_ref().unwrap().args;
    assert_eq!(shown["code_length"], json!(buy.len()));
    assert_eq!(
        shown["approvals"], approvals,
        "the ticket names the operation"
    );
    assert!(shown.get("code").is_none());
    approve(&daemon, &ticket).await;
    let mut approved = call("browser_repl", args.clone(), "buy-2");
    approved.approval_ticket = Some(ticket.clone());
    let done = model(host(&daemon, &session).await.call(approved.clone()).await);
    assert_eq!(done["return_value"], "bought", "{done}");
    assert!(matches!(
        host(&daemon, &session).await.call(approved).await,
        ToolOutcome::Denied { reason, .. } if reason == "approval_already_used"
    ));

    // One approved descriptor permits one operation: a second identical click stops again.
    let twice = "const buy = page.getByRole('button', { name: 'Buy now' }); await buy.click(); await buy.click()";
    let mut again = call(
        "browser_repl",
        json!({"tab_id": tab, "code": twice, "approvals": approvals}),
        "twice",
    );
    let ToolOutcome::Deferred { ticket, .. } =
        host(&daemon, &session).await.call(again.clone()).await
    else {
        panic!("descriptor must be approved first");
    };
    // Other arguments cannot ride on that approval.
    let mut changed = call(
        "browser_repl",
        json!({"tab_id": tab, "code": format!("{twice}; 1"), "approvals": approvals}),
        "twice",
    );
    changed.approval_ticket = Some(ticket.clone());
    approve(&daemon, &ticket).await;
    assert!(matches!(
        host(&daemon, &session).await.call(changed).await,
        ToolOutcome::Denied { reason, .. } if reason == "approval_args_mismatch"
    ));
    again.approval_ticket = Some(ticket);
    let stopped = host(&daemon, &session).await.call(again).await;
    assert_eq!(
        approvals_of(&stopped, &buy_action),
        json!([
            {"capability": "sensitive", "target": buy_action},
            {"capability": "sensitive", "target": buy_action}
        ])
    );

    // An approval is bound to its tab. Opening, tainting and closing a tab are other tools,
    // so they stay out of the no-page-script audit.
    let mut log = browser.take_protocol_log();
    browser.record_protocol(false);
    let other_tab = model(
        tool(
            &daemon,
            &session,
            "browser_open_tab",
            json!({"url": origin}),
            "open-2",
        )
        .await,
    )["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut elsewhere = call(
        "browser_repl",
        json!({"tab_id": other_tab, "code": buy, "approvals": approvals}),
        "elsewhere",
    );
    let ToolOutcome::Deferred { ticket, .. } =
        host(&daemon, &session).await.call(elsewhere.clone()).await
    else {
        panic!("descriptor must be approved first");
    };
    approve(&daemon, &ticket).await;
    elsewhere.approval_ticket = Some(ticket);
    approvals_of(
        &host(&daemon, &session).await.call(elsewhere).await,
        &format!("in tab {other_tab}"),
    );
    // A tab that received a secret fill is refused outright: a script could transform the
    // secret past exact-match redaction.
    daemon
        .shared
        .registry
        .browser
        .mark_secret(&daemon.shared, &browser, &other_tab)
        .await
        .unwrap();
    assert!(matches!(
        tool(
            &daemon,
            &session,
            "browser_repl",
            json!({"tab_id": other_tab, "code": "await page.content()"}),
            "tainted",
        )
        .await,
        ToolOutcome::Denied { reason, .. } if reason == "repl_unavailable_after_secret_fill"
    ));
    model(
        tool(
            &daemon,
            &session,
            "browser_close_tab",
            json!({"tab_id": other_tab}),
            "close-2",
        )
        .await,
    );
    browser.record_protocol(true);

    // The same pipeline over HTTP, across two approval boundaries: an approved rerun that
    // reaches a new boundary comes back deferred on a ticket covering every boundary so far.
    let router = api::router(daemon.shared.clone(), true);
    let post = |body: Value| {
        let router = router.clone();
        let session = session.clone();
        async move {
            let response = router
                .oneshot(request(
                    "POST",
                    &format!("/v1/sessions/{session}/repl"),
                    body,
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            json_body(response).await
        }
    };
    let url =
        post(json!({"code": format!("await page.goto({origin:?}); page.url()"), "tab_id": tab}))
            .await;
    assert_eq!(url["status"], "done", "{url}");
    assert_eq!(url["result"]["return_value"], json!(format!("{origin}/")));
    let both = "await page.getByRole('button', { name: 'Buy now' }).click(); await page.getByRole('button', { name: 'Delete item' }).click(); [await page.locator('#bought').textContent(), await page.locator('#deleted').textContent()]";
    let deferred = post(json!({"code": both, "tab_id": tab})).await;
    assert_eq!(deferred["status"], "deferred", "{deferred}");
    let ticket = TicketId::new(deferred["ticket_id"].as_str().unwrap());
    approve(&daemon, &ticket).await;
    let second = post(json!({"code": both, "tab_id": tab, "approval_ticket": ticket})).await;
    assert_eq!(second["status"], "deferred", "{second}");
    let second_ticket = TicketId::new(second["ticket_id"].as_str().unwrap());
    let pending = daemon.shared.db.state(&session).await.unwrap().tickets[&second_ticket].clone();
    assert_eq!(
        pending.tool.as_ref().unwrap().args["approvals"],
        json!([
            {"capability": "sensitive", "target": buy_action},
            {"capability": "sensitive", "target": format!("click button \"Delete item\" on {origin} in tab {tab}")}
        ])
    );
    approve(&daemon, &second_ticket).await;
    let rerun = post(json!({"code": both, "tab_id": tab, "approval_ticket": second_ticket})).await;
    assert_eq!(rerun["status"], "done", "{rerun}");
    assert_eq!(
        rerun["result"]["return_value"],
        json!(["bought", "deleted"])
    );
    // The agent runtime never receives a resolution for an API-initiated call.
    assert!(!runtime
        .0
        .lock()
        .resolved
        .iter()
        .any(|(id, _, _)| id == &ticket || id == &second_ticket));

    // Policy denial is reported, not thrown past the tool.
    let denied =
        post(json!({"code": "await page.goto('file:///etc/hostname')", "tab_id": tab})).await;
    assert_eq!(denied["status"], "denied", "{denied}");
    assert!(denied["reason"]
        .as_str()
        .unwrap()
        .starts_with("policy_denied"));

    // A sensitive Deny on the origin wins over approvals while browsing it stays allowed,
    // including for a ticket approved before the rule appeared.
    let reset = format!("await page.goto({origin:?}); await page.locator('#bought').textContent()");
    let mut resumed = call(
        "browser_repl",
        json!({"tab_id": tab, "code": buy, "approvals": approvals}),
        "resume-denied",
    );
    let ToolOutcome::Deferred { ticket, .. } =
        host(&daemon, &session).await.call(resumed.clone()).await
    else {
        panic!("descriptor must be approved first");
    };
    approve(&daemon, &ticket).await;
    resumed.approval_ticket = Some(ticket);
    let response = router
        .clone()
        .oneshot(request(
            "PUT",
            "/v1/policy",
            json!({
                "default_mode": "guard",
                "allow_origins": [],
                "deny_origins": [],
                "rules": [{"effect": "deny", "capability": "sensitive", "pattern": origin}]
            }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let browsed = repl(&daemon, &session, &tab, &reset, "browse-allowed").await;
    assert_eq!(browsed["return_value"], "not bought", "{browsed}");
    assert!(matches!(
        tool(&daemon, &session, "browser_repl", json!({"tab_id": tab, "code": buy}), "deny-fresh").await,
        ToolOutcome::Denied { reason, .. } if reason.starts_with("policy_denied")
    ));
    assert!(matches!(
        tool(
            &daemon,
            &session,
            "browser_repl",
            json!({"tab_id": tab, "code": buy, "approvals": approvals}),
            "deny-carried",
        )
        .await,
        ToolOutcome::Denied { .. }
    ));
    assert!(matches!(
        host(&daemon, &session).await.call(resumed).await,
        ToolOutcome::Denied { .. }
    ));
    let untouched = repl(
        &daemon,
        &session,
        &tab,
        "await page.locator('#bought').textContent()",
        "deny-check",
    )
    .await;
    assert_eq!(untouched["return_value"], "not bought");

    // Internal reads are fixed isolated-world primitives, never the caller's REPL source.
    log.extend(browser.take_protocol_log());
    browser.record_protocol(false);
    let mut readers = std::collections::HashSet::new();
    for (index, (method, params)) in log.iter().enumerate() {
        if matches!(
            method.as_str(),
            "Runtime.evaluate" | "Runtime.callFunctionOn"
        ) {
            let expression = method == "Runtime.evaluate";
            let source = &params[if expression {
                "expression"
            } else {
                "functionDeclaration"
            }];
            let primitive = openside_repl::INTERNAL_PRIMITIVES
                .iter()
                .find(|primitive| primitive.method == method && source == primitive.source)
                .unwrap_or_else(|| panic!("unexpected page script {method}: {params}"));
            assert_eq!(
                params["returnByValue"].as_bool().unwrap_or(false),
                primitive.return_by_value
            );
            assert_eq!(
                params["awaitPromise"].as_bool().unwrap_or(false),
                primitive.await_promise
            );
            let world_index = if expression {
                assert!(params["contextId"].as_i64().is_some_and(|id| id > 0));
                index.checked_sub(1).unwrap()
            } else {
                assert!(params["objectId"].as_str().is_some_and(|id| !id.is_empty()));
                let (resolved, node) = &log[index.checked_sub(1).unwrap()];
                assert_eq!(resolved, "DOM.resolveNode");
                assert!(node["executionContextId"].as_i64().is_some_and(|id| id > 0));
                if let Some(group) = primitive.object_group {
                    assert_eq!(node["objectGroup"], group);
                    if !primitive.return_by_value {
                        assert!(log[index + 1..].iter().any(|(method, params)| {
                            method == "Runtime.releaseObjectGroup" && params["objectGroup"] == group
                        }));
                    }
                }
                index.checked_sub(2).unwrap()
            };
            let (created, world) = &log[world_index];
            assert_eq!(created, "Page.createIsolatedWorld");
            assert_eq!(world["worldName"], primitive.world);
            assert_ne!(world["grantUniveralAccess"], true);
            readers.insert(primitive.name);
        } else {
            assert!(
                !matches!(
                    method.as_str(),
                    "Page.addScriptToEvaluateOnNewDocument"
                        | "Page.addScriptToEvaluateOnLoad"
                        | "Runtime.runScript"
                        | "Runtime.compileScript"
                        | "Debugger.evaluateOnCallFrame"
                ),
                "browser_repl issued {method}: {params}"
            );
        }
    }
    for primitive in openside_repl::INTERNAL_PRIMITIVES {
        assert!(
            readers.contains(primitive.name),
            "{} was not audited",
            primitive.name
        );
    }
    drop(browser);
    assert!(daemon
        .shared
        .registry
        .browser
        .shutdown(&daemon.shared)
        .await
        .is_ok());
    server.abort();
}

/// The `approvals` value of an `approval_required` stop whose reason mentions `expected`.
fn approvals_of(outcome: &ToolOutcome, expected: &str) -> Value {
    let ToolOutcome::Denied {
        reason,
        hint: Some(hint),
    } = outcome
    else {
        panic!("expected an approval stop: {outcome:?}");
    };
    assert!(
        reason.starts_with("approval_required: ") && reason.contains(expected),
        "{reason}"
    );
    let marker = "\"approvals\": ";
    serde_json::from_str(&hint[hint.rfind(marker).unwrap() + marker.len()..]).unwrap()
}
