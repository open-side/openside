//! The person's own browser through a fake openside extension: live tab lists without
//! incognito tabs, person-only routes, tab commands, and the attach → snapshot → act path of
//! the agent tools under each permission mode.

use super::*;
use crate::person_browser::Hub;

pub(super) type Calls = Arc<Mutex<Vec<(String, Value)>>>;

/// Connect a fake extension to `hub` that answers each request with `answer`.
pub(super) fn fake_extension(
    hub: Arc<Hub>,
    run: &str,
    answer: impl Fn(&str, &Value) -> std::result::Result<Value, &'static str> + Send + 'static,
) -> Calls {
    let (conn, mut out) = hub.connect(run.into(), "fake-extension".into()).unwrap();
    let calls = Calls::default();
    let log = calls.clone();
    tokio::spawn(async move {
        while let Some(frame) = out.recv().await {
            let request: Value = serde_json::from_str(&frame).unwrap();
            let method = request["method"].as_str().unwrap_or_default().to_owned();
            let params = request["params"].clone();
            log.lock().push((method.clone(), params.clone()));
            let reply = match answer(&method, &params) {
                Ok(result) => json!({"id": request["id"], "result": result}),
                Err(code) => json!({"id": request["id"], "error": {"code": code}}),
            };
            hub.deliver(conn, &reply.to_string());
        }
    });
    calls
}

fn tab(id: i64, url: &str, title: &str, active: bool) -> Value {
    json!({"tab_id": id, "window_id": 1, "index": id, "title": title, "url": url,
        "favicon_url": null, "active": active, "pinned": false, "audible": false,
        "muted": false, "discarded": false, "group_id": null})
}

/// A browser with one ordinary page (tab 7, selected), a browser page (tab 9) and an
/// incognito tab (tab 8), whose page has an ordinary link and a payment button.
pub(super) fn standard_browser(
    page: String,
    title: String,
) -> impl Fn(&str, &Value) -> std::result::Result<Value, &'static str> + Send + 'static {
    move |method, params| match method {
        "tabs.list" => {
            let mut incognito = tab(8, "https://private.example/", "Private", true);
            incognito["incognito"] = json!(true);
            incognito["window_id"] = json!(2);
            let mut icon = tab(9, "chrome://newtab/", "New Tab", false);
            icon["favicon_url"] = json!("javascript:alert(1)");
            Ok(json!({
                "focused_window_id": 1,
                // Tab 10 is an ordinary page no chat has attached yet.
                "tabs": [tab(7, &page, &title, true), incognito, icon, tab(10, &format!("{page}/next"), &title, false)],
                "groups": [],
            }))
        }
        "tab.snapshot" if params["tab_id"] == 7 => Ok(json!({
            "url": page,
            "title": title,
            "text": format!("heading \"{title}\"\nlink \"Open\" [ref=u1]\nbutton \"Pay now\" [ref=u2]"),
            "refs": [{"ref": "u1", "role": "link", "name": "Open"},
                {"ref": "u2", "role": "button", "name": "Pay now"}],
            "truncated": false,
        })),
        "tab.act" => Ok(json!({"url": page})),
        // A faulty extension: duplicate reports an incognito tab, pinning an unbounded title.
        "tabs.command" if params["command"] == "duplicate" => {
            let mut private = tab(8, "https://private.example/", "Private", false);
            private["incognito"] = json!(true);
            Ok(json!({"tab": private, "group_id": null}))
        }
        "tabs.command" if params["command"] == "set_pinned" => {
            Ok(json!({"tab": tab(7, &page, &"t".repeat(5000), true), "group_id": null}))
        }
        "tabs.command" => Ok(json!({"tab": null, "group_id": null})),
        "browsing_data.clear" => Ok(json!({})),
        _ => Err("tab_not_found"),
    }
}

/// Attach the standard browser's tab 7 to `session` as its last snapshot saw it, without a
/// browser round trip (for callers outside an async context).
pub(super) fn attach_fixture(hub: &Hub, session: &SessionId, page: &str, title: &str) {
    let tab: PersonTab = serde_json::from_value(tab(7, page, title, true)).unwrap();
    hub.attach(session, &tab, AttachedBy::Person);
    let refs = [("u1", "link", "Open"), ("u2", "button", "Pay now")]
        .into_iter()
        .map(|(element, role, name)| {
            (
                element.to_owned(),
                crate::person_browser::RefInfo {
                    role: role.into(),
                    name: name.into(),
                },
            )
        })
        .collect();
    hub.set_refs(session, 7, refs);
}

async fn send(router: &axum::Router, method: &str, path: &str, body: Value) -> Response {
    router
        .clone()
        .oneshot(request(method, path, body, None))
        .await
        .unwrap()
}

#[tokio::test]
async fn person_tabs_are_live_without_incognito_and_report_offline() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let local = api::router(daemon.shared.clone(), true);

    let offline = send(&local, "GET", "/v1/browser/person/tabs", json!({})).await;
    assert_eq!(offline.status(), StatusCode::OK);
    assert_eq!(
        json_body(offline).await,
        json!({"connected": false, "focused_window_id": null, "tabs": [], "groups": []})
    );
    for path in [
        "/v1/browser/person/bookmarks",
        "/v1/browser/person/history?q=x",
        "/v1/browser/person/recently-closed",
        "/v1/browser/person/downloads",
        "/v1/browser/person/extensions",
    ] {
        let response = send(&local, "GET", path, json!({})).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(json_body(response).await["connected"], false, "{path}");
    }
    let refused = send(
        &local,
        "POST",
        "/v1/browser/person/tabs/command",
        json!({"command": "reload", "tab_id": 7}),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body(refused).await["code"],
        "person_browser_not_connected"
    );

    fake_extension(
        daemon.shared.registry.person_browser.clone(),
        "run-1",
        standard_browser("https://docs.example/guide".into(), "Guide".into()),
    );
    let list = json_body(send(&local, "GET", "/v1/browser/person/tabs", json!({})).await).await;
    assert_eq!(list["connected"], true);
    let ids: Vec<_> = list["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tab| tab["tab_id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, [7, 9, 10], "incognito tab 8 must never be listed");
    assert!(list["tabs"][1]["favicon_url"].is_null());
    assert!(!list.to_string().contains("incognito"));
}

#[tokio::test]
async fn only_the_person_reaches_their_browser() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "person", PolicyMode::Guard, None).await;
    fake_extension(
        daemon.shared.registry.person_browser.clone(),
        "run-1",
        standard_browser("https://docs.example/".into(), "Docs".into()),
    );
    let local = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let (_, agent) = daemon
        .shared
        .db
        .pair_as(
            "agent".into(),
            vec![CapabilityScope::Read, CapabilityScope::Steer],
            DeviceKind::AgentClient,
        )
        .await
        .unwrap();
    for (method, path) in [
        ("GET", "/v1/browser/person/tabs".to_owned()),
        ("GET", "/v1/browser/person/bookmarks".to_owned()),
        ("GET", format!("/v1/sessions/{session}/attached-tabs")),
        ("POST", format!("/v1/sessions/{session}/attached-tabs")),
    ] {
        let response = tcp
            .clone()
            .oneshot(request(method, &path, json!({}), Some(&agent)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
        assert_eq!(json_body(response).await["code"], "person_device_required");
        let mut marked = request(method, &path, json!({}), None);
        marked
            .headers_mut()
            .insert("x-openside-client", "mcp".parse().unwrap());
        let response = local.clone().oneshot(marked).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
    }
    // Clearing the person's browsing data is the person's alone.
    let clear = json!({"types": ["history"], "since_ms": 0});
    let response = tcp
        .clone()
        .oneshot(request(
            "POST",
            "/v1/browser/person/clear-data",
            clear,
            Some(&agent),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(daemon
        .shared
        .registry
        .person_browser
        .attached(&session)
        .is_empty());
}

#[tokio::test]
async fn tab_commands_are_checked_before_they_reach_the_browser() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let calls = fake_extension(
        daemon.shared.registry.person_browser.clone(),
        "run-1",
        standard_browser("https://docs.example/".into(), "Docs".into()),
    );
    let local = api::router(daemon.shared.clone(), true);
    let command = |body: Value| send(&local, "POST", "/v1/browser/person/tabs/command", body);
    for refused in [
        json!({"command": "create", "url": "file:///etc/passwd"}),
        json!({"command": "create", "url": "https://user:secret@example.com/"}),
        json!({"command": "create", "url": "chrome://flags"}),
        json!({"command": "close", "tab_ids": []}),
        json!({"command": "group", "tab_ids": [7], "title": "x".repeat(201)}),
    ] {
        let response = command(refused.clone()).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{refused}");
    }
    assert!(
        calls.lock().is_empty(),
        "refused commands reached the browser"
    );
    for accepted in [
        json!({"command": "create", "url": "chrome://settings/"}),
        json!({"command": "create", "url": "chrome://settings/clearBrowserData"}),
        json!({"command": "create", "url": "chrome://extensions/shortcuts"}),
        json!({"command": "create", "url": "https://example.com/a", "after_tab_id": 7}),
        json!({"command": "set_pinned", "tab_id": 7, "pinned": true}),
        json!({"command": "group", "tab_ids": [7, 9], "color": "blue"}),
        json!({"command": "new_incognito_window"}),
    ] {
        let response = command(accepted.clone()).await;
        assert_eq!(response.status(), StatusCode::OK, "{accepted}");
        if accepted["command"] == "set_pinned" {
            let body = json_body(response).await;
            assert_eq!(body["tab"]["title"].as_str().unwrap().chars().count(), 1000);
        }
    }
    let duplicated = json_body(command(json!({"command": "duplicate", "tab_id": 7})).await).await;
    assert_eq!(
        duplicated["tab"],
        Value::Null,
        "an incognito tab left the daemon"
    );
    let forwarded: Vec<_> = calls
        .lock()
        .iter()
        .map(|(_, params)| params["command"].clone())
        .collect();
    assert_eq!(
        forwarded,
        [
            "create",
            "create",
            "create",
            "create",
            "set_pinned",
            "group",
            "new_incognito_window",
            "duplicate"
        ]
    );

    calls.lock().clear();
    let clear = |body: Value| send(&local, "POST", "/v1/browser/person/clear-data", body);
    for refused in [
        json!({"types": [], "since_ms": 0}),
        json!({"types": ["history", "history"], "since_ms": 0}),
        json!({"types": ["history"], "since_ms": -1}),
    ] {
        assert_eq!(
            clear(refused.clone()).await.status(),
            StatusCode::BAD_REQUEST,
            "{refused}"
        );
    }
    for unknown in [
        json!({"types": ["bookmarks"], "since_ms": 0}),
        json!({"types": ["cache"]}),
    ] {
        assert!(
            clear(unknown.clone()).await.status().is_client_error(),
            "{unknown}"
        );
    }
    assert!(
        calls.lock().is_empty(),
        "refused clears reached the browser"
    );
    let cleared =
        clear(json!({"types": ["cookies", "form_data"], "since_ms": 1_700_000_000_000_i64})).await;
    assert_eq!(cleared.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        *calls.lock(),
        [(
            "browsing_data.clear".to_owned(),
            json!({"types": ["cookies", "form_data"], "since_ms": 1_700_000_000_000_i64})
        )]
    );
}

#[tokio::test]
async fn attached_tabs_follow_the_permission_mode() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let guard = create_session(&daemon, "guard", PolicyMode::Guard, None).await;
    let full = create_session(&daemon, "full", PolicyMode::Full, None).await;
    let read_only = create_session(&daemon, "read-only", PolicyMode::ReadOnly, None).await;
    let hub = daemon.shared.registry.person_browser.clone();
    let calls = fake_extension(
        hub.clone(),
        "run-1",
        standard_browser("https://docs.example/guide".into(), "Guide".into()),
    );
    let host_guard = host(&daemon, &guard).await;

    // Listing is a private read the session's policy allows; nothing is attached yet.
    let ToolOutcome::Done { for_model_json } = host_guard
        .call(call("browser_user_tabs", json!({}), "list"))
        .await
    else {
        panic!("listing failed");
    };
    let listed: Value = serde_json::from_str(&for_model_json).unwrap();
    assert_eq!(listed["tabs"].as_array().unwrap().len(), 3);
    assert_eq!(listed["tabs"][0]["attached"], false);

    let outcome = host_guard
        .call(call(
            "browser_user_tab_snapshot",
            json!({"tab_id": 7}),
            "early",
        ))
        .await;
    assert!(
        matches!(
            &outcome,
            ToolOutcome::Denied { .. } | ToolOutcome::Failed { .. }
        ),
        "an unattached tab was read: {outcome:?}"
    );
    assert!(!calls
        .lock()
        .iter()
        .any(|(method, _)| method == "tab.snapshot"));

    // The person attaches the tab they are looking at.
    let local = api::router(daemon.shared.clone(), true);
    let attached = send(
        &local,
        "POST",
        &format!("/v1/sessions/{guard}/attached-tabs"),
        json!({}),
    )
    .await;
    assert_eq!(attached.status(), StatusCode::CREATED);
    let attached = json_body(attached).await;
    assert_eq!(attached["tab_id"], 7);
    assert_eq!(attached["attached_by"], "person");
    let browser_page = send(
        &local,
        "POST",
        &format!("/v1/sessions/{guard}/attached-tabs"),
        json!({"tab_id": 9}),
    )
    .await;
    assert_eq!(browser_page.status(), StatusCode::BAD_REQUEST);

    let ToolOutcome::Done { for_model_json } = host_guard
        .call(call(
            "browser_user_tab_snapshot",
            json!({"tab_id": 7}),
            "read",
        ))
        .await
    else {
        panic!("snapshot failed");
    };
    let snapshot: Value = serde_json::from_str(&for_model_json).unwrap();
    assert_eq!(snapshot["untrusted"], true);
    assert!(snapshot["content"].as_str().unwrap().contains("[ref=u1]"));

    // Acting asks in Guard and never reaches the browser before approval.
    let click = json!({"tab_id": 7, "action": "click", "ref": "u1"});
    let outcome = host_guard
        .call(call("browser_user_tab_act", click.clone(), "act"))
        .await;
    assert!(
        matches!(
            outcome,
            ToolOutcome::Deferred {
                kind: DeferKind::Approval,
                ..
            }
        ),
        "{outcome:?}"
    );
    assert!(!calls.lock().iter().any(|(method, _)| method == "tab.act"));

    // Full access acts on an agent-attached tab, but a payment button still asks.
    let host_full = host(&daemon, &full).await;
    let outcome = host_full
        .call(call(
            "browser_user_tab_attach",
            json!({"tab_id": 7}),
            "attach",
        ))
        .await;
    assert!(matches!(outcome, ToolOutcome::Done { .. }), "{outcome:?}");
    host_full
        .call(call(
            "browser_user_tab_snapshot",
            json!({"tab_id": 7}),
            "read",
        ))
        .await;
    let ToolOutcome::Done { for_model_json } = host_full
        .call(call("browser_user_tab_act", click.clone(), "act"))
        .await
    else {
        panic!("act failed");
    };
    let acted_output: Value = serde_json::from_str(&for_model_json).unwrap();
    assert_eq!(
        acted_output["untrusted"], true,
        "the page's URL comes back with the result"
    );
    let acted = calls
        .lock()
        .iter()
        .find(|(method, _)| method == "tab.act")
        .map(|(_, params)| params.clone())
        .unwrap();
    assert_eq!(acted["origin"], "https://docs.example");
    // The extension refuses the click if the element was relabelled since the snapshot.
    assert_eq!(acted["expect_name"], "Open");
    let pay = json!({"tab_id": 7, "action": "click", "ref": "u2"});
    let outcome = host_full
        .call(call("browser_user_tab_act", pay, "pay"))
        .await;
    assert!(
        matches!(
            outcome,
            ToolOutcome::Deferred {
                kind: DeferKind::Approval,
                ..
            }
        ),
        "{outcome:?}"
    );
    // Enter, or a key sent to whatever has focus, may submit an unclassified form: it asks too.
    for (id, args) in [
        (
            "enter",
            json!({"tab_id": 7, "action": "press", "ref": "u1", "key": "Enter"}),
        ),
        (
            "focus",
            json!({"tab_id": 7, "action": "press", "key": "Escape"}),
        ),
    ] {
        let outcome = host_full.call(call("browser_user_tab_act", args, id)).await;
        assert!(
            matches!(
                outcome,
                ToolOutcome::Deferred {
                    kind: DeferKind::Approval,
                    ..
                }
            ),
            "{id}: {outcome:?}"
        );
    }

    // Read-only sessions never get the acting tool.
    let host_read_only = host(&daemon, &read_only).await;
    hub.attach(
        &read_only,
        &hub.find_tab(Some(7)).await.unwrap(),
        AttachedBy::Person,
    );
    let outcome = host_read_only
        .call(call("browser_user_tab_act", click, "act"))
        .await;
    assert!(matches!(outcome, ToolOutcome::Denied { .. }), "{outcome:?}");
    assert_eq!(
        calls
            .lock()
            .iter()
            .filter(|(method, _)| method == "tab.act")
            .count(),
        1
    );
}

#[tokio::test]
async fn a_browser_restart_drops_attachments() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "restart", PolicyMode::Guard, None).await;
    let hub = daemon.shared.registry.person_browser.clone();
    let page = || standard_browser("https://docs.example/".into(), "Docs".into());
    fake_extension(hub.clone(), "run-1", page());
    let tab = hub.find_tab(None).await.unwrap();
    assert_eq!(tab.tab_id, 7);
    hub.attach(&session, &tab, AttachedBy::Person);

    // The same browser run reconnecting (service worker restart) keeps them.
    fake_extension(hub.clone(), "run-1", page());
    assert_eq!(hub.attached(&session).len(), 1);
    // A new run reuses tab ids for other pages.
    fake_extension(hub.clone(), "run-2", page());
    assert!(hub.attached(&session).is_empty());

    hub.attach(&session, &tab, AttachedBy::Person);
    let local = api::router(daemon.shared.clone(), true);
    let path = format!("/v1/sessions/{session}/attached-tabs/7");
    assert_eq!(
        send(&local, "DELETE", &path, json!({})).await.status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(&local, "DELETE", &path, json!({})).await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_silent_browser_times_out_and_a_dropped_one_disconnects() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let hub = daemon.shared.registry.person_browser.clone();
    let (conn, out) = hub.connect("run-1".into(), "device-a".into()).unwrap();
    // Another paired device cannot take over a live link.
    assert!(hub.connect("run-1".into(), "device-b".into()).is_none());
    tokio::time::pause();
    let pending = tokio::spawn({
        let hub = hub.clone();
        async move { hub.call("tabs.list", json!({})).await }
    });
    tokio::time::sleep(Duration::from_secs(11)).await;
    assert_eq!(
        pending.await.unwrap(),
        Err(crate::person_browser::BridgeError::Timeout)
    );
    tokio::time::resume();
    let pending = tokio::spawn({
        let hub = hub.clone();
        async move { hub.call("tabs.list", json!({})).await }
    });
    tokio::task::yield_now().await;
    hub.disconnect(conn);
    drop(out);
    assert_eq!(
        pending.await.unwrap(),
        Err(crate::person_browser::BridgeError::NotConnected)
    );
}

#[tokio::test]
async fn abandoned_calls_free_their_slot_and_attachments_are_capped() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let hub = daemon.shared.registry.person_browser.clone();
    // A browser that reads every request but never answers.
    let (_, mut out) = hub.connect("run-1".into(), "device-a".into()).unwrap();
    tokio::spawn(async move { while out.recv().await.is_some() {} });
    let abandoned: Vec<_> = (0..64)
        .map(|_| {
            let hub = hub.clone();
            tokio::spawn(async move { hub.call("tabs.list", json!({})).await })
        })
        .collect();
    tokio::task::yield_now().await;
    for call in &abandoned {
        call.abort();
    }
    for call in abandoned {
        let _ = call.await;
    }
    // Every slot is free again: the next call waits for an answer instead of failing as busy.
    let next =
        tokio::time::timeout(Duration::from_millis(200), hub.call("tabs.list", json!({}))).await;
    assert!(next.is_err(), "abandoned calls kept their slots: {next:?}");

    let session = create_session(&daemon, "cap", PolicyMode::Guard, None).await;
    for id in 1..=21 {
        let tab: openside_proto::PersonTab =
            serde_json::from_value(tab(id, &format!("https://site{id}.example/"), "Tab", false))
                .unwrap();
        hub.attach(&session, &tab, AttachedBy::Person);
    }
    let kept: Vec<i64> = hub
        .attached(&session)
        .iter()
        .map(|tab| tab.tab_id)
        .collect();
    assert_eq!(
        kept,
        (2..=21).collect::<Vec<_>>(),
        "a chat keeps its 20 most recent tabs"
    );
}

#[tokio::test]
async fn browser_failures_map_to_statuses_and_closed_tabs_leave_every_chat() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let hub = daemon.shared.registry.person_browser.clone();
    let open = Arc::new(Mutex::new(vec![7_i64, 9]));
    let live = open.clone();
    fake_extension(hub.clone(), "run-1", move |method, params| match method {
        "tabs.list" => Ok(json!({
            "focused_window_id": 1,
            "tabs": live.lock().iter().map(|id| tab(*id, &format!("https://site{id}.example/"), "Tab", false)).collect::<Vec<_>>(),
            "groups": [],
        })),
        "tabs.command" => match params["tab_id"].as_i64() {
            Some(41) => Err("tab_not_found"),
            Some(42) => Err("site_access_required"),
            _ => Err("exploded"),
        },
        _ => Err("tab_not_found"),
    });
    let local = api::router(daemon.shared.clone(), true);
    for (tab_id, status, code) in [
        (41, StatusCode::NOT_FOUND, "tab_not_found"),
        (42, StatusCode::CONFLICT, "site_access_required"),
        (43, StatusCode::BAD_GATEWAY, "person_browser_failed"),
    ] {
        let response = send(
            &local,
            "POST",
            "/v1/browser/person/tabs/command",
            json!({"command": "set_pinned", "tab_id": tab_id, "pinned": true}),
        )
        .await;
        assert_eq!(response.status(), status, "{tab_id}");
        assert_eq!(json_body(response).await["code"], code);
    }

    // A tab the person closed leaves every chat: on the next tab list, or when the browser
    // says it is gone.
    let first = create_session(&daemon, "first", PolicyMode::Guard, None).await;
    let second = create_session(&daemon, "second", PolicyMode::Guard, None).await;
    for id in [7, 9] {
        let tab: openside_proto::PersonTab =
            serde_json::from_value(tab(id, &format!("https://site{id}.example/"), "Tab", false))
                .unwrap();
        hub.attach(&first, &tab, AttachedBy::Person);
        hub.attach(&second, &tab, AttachedBy::Person);
    }
    open.lock().retain(|id| *id != 7);
    assert_eq!(
        send(&local, "GET", "/v1/browser/person/tabs", json!({}))
            .await
            .status(),
        StatusCode::OK
    );
    for session in [&first, &second] {
        let ids: Vec<i64> = hub.attached(session).iter().map(|tab| tab.tab_id).collect();
        assert_eq!(ids, [9]);
    }
    let gone = hub
        .call(
            "tab.snapshot",
            json!({"tab_id": 9, "origin": "https://site9.example"}),
        )
        .await;
    assert_eq!(
        gone,
        Err(crate::person_browser::BridgeError::Remote(
            "tab_not_found".into()
        ))
    );
    assert!(hub.attached(&first).is_empty() && hub.attached(&second).is_empty());

    // A browser that never answers is a gateway timeout.
    let (_, out) = hub
        .connect("run-1".into(), "fake-extension".into())
        .unwrap();
    tokio::time::pause();
    let router = local.clone();
    let pending = tokio::spawn(async move {
        let body = json!({"command": "set_pinned", "tab_id": 7, "pinned": true});
        send(&router, "POST", "/v1/browser/person/tabs/command", body).await
    });
    tokio::time::sleep(Duration::from_secs(11)).await;
    let response = pending.await.unwrap();
    tokio::time::resume();
    drop(out);
    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(json_body(response).await["code"], "person_browser_timeout");
}

#[tokio::test]
async fn openside_itself_is_never_attached_under_a_loopback_alias() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    std::fs::write(
        openside_config::data_layout::DataPath::Port.path(&daemon.shared.config.data_dir),
        "4100",
    )
    .unwrap();
    let session = create_session(&daemon, "loopback", PolicyMode::Full, None).await;
    let local = api::router(daemon.shared.clone(), true);
    for page in [
        "http://127.0.0.1:4100/",
        "http://localhost:4100/chat",
        "http://localhost.:4100/",
        "http://127.0.0.2:4100/",
        "http://[::1]:4100/",
        "http://[::ffff:127.0.0.1]:4100/",
        "http://app.localhost:4100/",
    ] {
        fake_extension(
            daemon.shared.registry.person_browser.clone(),
            "run-1",
            standard_browser(page.into(), "openside".into()),
        );
        let attached = send(
            &local,
            "POST",
            &format!("/v1/sessions/{session}/attached-tabs"),
            json!({"tab_id": 7}),
        )
        .await;
        assert_eq!(attached.status(), StatusCode::BAD_REQUEST, "{page}");
    }
    // Another loopback service on a different port is an ordinary page.
    let calls = fake_extension(
        daemon.shared.registry.person_browser.clone(),
        "run-1",
        standard_browser("http://127.0.0.1:4101/".into(), "Dev server".into()),
    );
    let attached = send(
        &local,
        "POST",
        &format!("/v1/sessions/{session}/attached-tabs"),
        json!({"tab_id": 7}),
    )
    .await;
    assert_eq!(attached.status(), StatusCode::CREATED);

    // Even with full access, an attached tab is never navigated onto openside's own UI.
    let host_full = host(&daemon, &session).await;
    // Openside's own UI under loopback aliases, and a URL past the 8 KiB cap.
    let oversized = format!("https://docs.example/{}", "a".repeat(9000));
    for (index, destination) in [
        "http://127.0.0.1:4100/settings",
        "http://localhost.:4100/",
        "http://[::ffff:127.0.0.1]:4100/",
        oversized.as_str(),
    ]
    .into_iter()
    .enumerate()
    {
        let args = json!({"tab_id": 7, "action": "navigate", "url": destination});
        let outcome = host_full
            .call(call("browser_user_tab_act", args, &format!("nav-{index}")))
            .await;
        assert!(
            !matches!(
                outcome,
                ToolOutcome::Done { .. } | ToolOutcome::DoneWithParts { .. }
            ),
            "{destination}: {outcome:?}"
        );
    }
    assert!(
        !calls.lock().iter().any(|(method, _)| method == "tab.act"),
        "a navigation to openside reached the browser"
    );
    let outcome = host_full
        .call(call(
            "browser_user_tab_act",
            json!({"tab_id": 7, "action": "navigate", "url": "https://docs.example/"}),
            "nav-ok",
        ))
        .await;
    assert!(matches!(outcome, ToolOutcome::Done { .. }), "{outcome:?}");
}
