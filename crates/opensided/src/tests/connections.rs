use super::*;
use crate::connections::{self, Service};
use openside_browser::CookieKey;

fn key(domain: &str, name: &str, expires: f64) -> CookieKey {
    CookieKey {
        domain: domain.into(),
        name: name.into(),
        path: "/".into(),
        expires,
        http_only: true,
        secure: true,
    }
}

fn record(service: &Service, keys: &[CookieKey], identity: Option<&str>) -> Connection {
    service
        .record(connections::Record {
            site: "example.com",
            keys,
            source: ConnectionSource::Login,
            status: ConnectionStatus::Active,
            identity,
            identity_source: ConnectionIdentitySource::LoginTicket,
            home: Some("https://app.example.com/home?token=URL_CANARY#secret"),
            session: None,
            agent: "chief",
            vault_item: None,
            account_space: None,
        })
        .unwrap()
}

#[tokio::test]
async fn connections_admin_agent_cannot_mutate_or_self_grant() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let row = record(&daemon.shared.connections, &[], None);
    let (_, agent) = daemon
        .shared
        .db
        .pair_as(
            "agent".into(),
            vec![CapabilityScope::Admin],
            DeviceKind::AgentClient,
        )
        .await
        .unwrap();
    let (_, person) = daemon
        .shared
        .db
        .pair_as(
            "person".into(),
            vec![CapabilityScope::Admin],
            DeviceKind::PersonDevice,
        )
        .await
        .unwrap();
    let router = api::router(daemon.shared.clone(), false);
    let grant = json!({"grants":[{"agent_id":"self","access":"allowed"}]});
    for (method, path, body) in [
        (
            "PATCH",
            format!("/v1/connections/{}", row.id),
            grant.clone(),
        ),
        (
            "POST",
            format!("/v1/connections/{}/check", row.id),
            json!({}),
        ),
        (
            "POST",
            format!("/v1/connections/{}/relogin", row.id),
            json!({}),
        ),
        (
            "POST",
            format!("/v1/connections/{}/revoke", row.id),
            json!({}),
        ),
        (
            "POST",
            format!("/v1/connections/candidates/{}/dismiss", row.id),
            json!({}),
        ),
        (
            "POST",
            "/v1/connections/spaces".into(),
            json!({"site":"example.com"}),
        ),
        ("DELETE", "/v1/connections/spaces/alpha".into(), json!({})),
    ] {
        let response = router
            .clone()
            .oneshot(request(method, &path, body, Some(&agent)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {path}");
        assert_eq!(json_body(response).await["code"], "person_device_required");
    }
    assert!(!daemon
        .shared
        .connections
        .get(&row.id)
        .unwrap()
        .unwrap()
        .grants
        .iter()
        .any(|grant| grant.agent_id == "self"));
    for path in [
        "/v1/connections".to_owned(),
        format!("/v1/connections/{}", row.id),
        "/v1/connections/audit".into(),
    ] {
        let response = router
            .clone()
            .oneshot(request("GET", &path, json!({}), Some(&agent)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
    }
    let response = router
        .oneshot(request(
            "PATCH",
            &format!("/v1/connections/{}", row.id),
            grant,
            Some(&person),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(daemon
        .shared
        .connections
        .get(&row.id)
        .unwrap()
        .unwrap()
        .grants
        .iter()
        .any(|grant| grant.agent_id == "self" && grant.access == ConnectionAccess::Allowed));
    daemon.shutdown().await;
}

#[test]
fn connections_migration_identity_and_canary_storage() {
    const CANARY: &str = "COOKIE_VALUE_CANARY_82adc";
    let root = tempfile::tempdir().unwrap();
    let cookie: CookieKey = serde_json::from_value(json!({"domain":".example.com","name":"session","path":"/","expires":4102444800.0,"httpOnly":true,"secure":true,"value":CANARY})).unwrap();
    let service = Service::open(root.path()).unwrap();
    let row = record(&service, &[cookie], Some("jane.private@example.net"));
    assert_eq!(row.identity.as_deref(), Some("j•••@example.net"));
    assert_eq!(
        row.home_url.as_deref(),
        Some("https://app.example.com/home")
    );
    let id = row.id.clone();
    drop(service);
    let reopened = Service::open(root.path()).unwrap();
    assert_eq!(reopened.list().unwrap()[0].id, id);
    let db = rusqlite::Connection::open(root.path().join("connections.db")).unwrap();
    let version: i64 = db
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2);
    let account_key: String = db
        .query_row("SELECT account_key FROM connections", [], |row| row.get(0))
        .unwrap();
    assert_eq!(account_key.len(), 64);
    assert!(!account_key.contains("jane"));
    let second = record(&reopened, &[], Some("  JANE.PRIVATE@EXAMPLE.NET  "));
    assert_eq!(
        second.id, id,
        "normalized identity deduplicates after restart"
    );
    let wire = serde_json::to_string(&reopened.list().unwrap()).unwrap();
    for secret in [CANARY, "jane.private@example.net", "URL_CANARY"] {
        assert!(!wire.contains(secret));
    }
    drop(db);
    drop(reopened);
    for entry in std::fs::read_dir(root.path()).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().unwrap() == "connections.key" {
            continue;
        }
        let bytes = std::fs::read(path).unwrap();
        for secret in [CANARY, "jane.private@example.net", "URL_CANARY"] {
            assert!(!bytes
                .windows(secret.len())
                .any(|window| window == secret.as_bytes()));
        }
    }
}

#[test]
fn connections_future_migration_is_refused_without_reset() {
    let root = tempfile::tempdir().unwrap();
    drop(Service::open(root.path()).unwrap());
    let db = rusqlite::Connection::open(root.path().join("connections.db")).unwrap();
    db.pragma_update(None, "user_version", 99).unwrap();
    assert!(Service::open(root.path()).is_err());
    assert_eq!(
        db.pragma_query_value::<i64, _>(None, "user_version", |row| row.get(0))
            .unwrap(),
        99
    );
}

#[test]
fn connections_v1_migration_preserves_rows_and_grants() {
    let root = tempfile::tempdir().unwrap();
    let service = Service::open(root.path()).unwrap();
    let row = record(&service, &[], None);
    drop(service);
    let db = rusqlite::Connection::open(root.path().join("connections.db")).unwrap();
    db.execute_batch("DROP TABLE connection_origins; PRAGMA user_version=1;")
        .unwrap();
    drop(db);
    let service = Service::open(root.path()).unwrap();
    assert_eq!(service.get(&row.id).unwrap().unwrap(), row);
    drop(service);
    let db = rusqlite::Connection::open(root.path().join("connections.db")).unwrap();
    assert_eq!(
        db.pragma_query_value::<i64, _>(None, "user_version", |row| row.get(0))
            .unwrap(),
        2
    );
}

#[test]
fn connections_psl_groups_subdomains_and_isolates_private_tenants() {
    assert_eq!(
        connections::site(".accounts.example.co.uk"),
        Some("example.co.uk".into())
    );
    assert_eq!(connections::site("a.github.io"), Some("a.github.io".into()));
    assert_eq!(connections::site("b.github.io"), Some("b.github.io".into()));
    assert_eq!(connections::site("co.uk"), None);
    assert_eq!(connections::site("localhost"), Some("localhost".into()));
}

#[test]
fn connections_expiry_requires_all_fingerprint_keys_gone() {
    let root = tempfile::tempdir().unwrap();
    let service = Service::open(root.path()).unwrap();
    let keys = [
        key(".example.com", "a", 4102444800.0),
        key("app.example.com", "b", -1.0),
    ];
    let row = record(&service, &keys, None);
    service.sweep(&keys[1..], &[]).unwrap();
    assert_eq!(
        service.get(&row.id).unwrap().unwrap().status,
        ConnectionStatus::Active
    );
    assert_eq!(service.get(&row.id).unwrap().unwrap().expires_at, None);
    service.sweep(&[], &[]).unwrap();
    assert_eq!(
        service.get(&row.id).unwrap().unwrap().status,
        ConnectionStatus::Expired
    );
}

#[test]
fn connections_account_space_identity_and_sweep_are_independent() {
    let root = tempfile::tempdir().unwrap();
    let service = Service::open(root.path()).unwrap();
    let keys = [key(".example.com", "session", 4102444800.0)];
    let shared = record(&service, &keys, Some("same@example.net"));
    let space = service
        .record(connections::Record {
            site: "example.com",
            keys: &keys,
            source: ConnectionSource::Login,
            status: ConnectionStatus::Active,
            identity: Some("same@example.net"),
            identity_source: ConnectionIdentitySource::LoginTicket,
            home: None,
            session: None,
            agent: "chief",
            vault_item: None,
            account_space: Some("alpha"),
        })
        .unwrap();
    assert_ne!(shared.id, space.id);
    assert_eq!(space.account_space.as_deref(), Some("alpha"));
    service
        .identify(
            &space,
            &["changed@example.net".into(), "other@example.net".into()],
        )
        .unwrap();
    let repeated = service
        .record(connections::Record {
            site: "example.com",
            keys: &keys,
            source: ConnectionSource::Login,
            status: ConnectionStatus::Active,
            identity: Some("changed@example.net"),
            identity_source: ConnectionIdentitySource::LoginTicket,
            home: None,
            session: None,
            agent: "chief",
            vault_item: None,
            account_space: Some("alpha"),
        })
        .unwrap();
    assert_eq!(repeated.id, space.id);
    assert_eq!(service.list().unwrap().len(), 2);
    service.sweep(&[], &[]).unwrap();
    assert_eq!(
        service.get(&shared.id).unwrap().unwrap().status,
        ConnectionStatus::Expired
    );
    assert_eq!(
        service.get(&space.id).unwrap().unwrap().status,
        ConnectionStatus::Active
    );
}

#[test]
fn connections_weak_sweep_requires_history_and_dismissal_is_sticky() {
    let root = tempfile::tempdir().unwrap();
    let service = Service::open(root.path()).unwrap();
    let keys = [key("app.example.com", "session", 4102444800.0)];
    service.sweep(&keys, &[]).unwrap();
    assert!(service.list().unwrap().is_empty());
    service.sweep(&keys, &["example.com".into()]).unwrap();
    let row = service.list().unwrap().remove(0);
    assert_eq!(row.status, ConnectionStatus::Candidate);
    service.remove(&row, false, true).unwrap();
    service.sweep(&keys, &["example.com".into()]).unwrap();
    assert!(service.list().unwrap().is_empty());
}

#[tokio::test]
async fn connections_scope_matrix_and_api_metadata_canary() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let key_path = root.path().join("data/connections.key");
    assert!(daemon.shared.credential_paths.is_protected(&key_path));
    let alias = root.path().join("key-alias");
    std::fs::hard_link(&key_path, &alias).unwrap();
    assert!(daemon.shared.credential_paths.is_protected(&alias));
    let row = record(
        &daemon.shared.connections,
        &[key("example.com", "session", 4102444800.0)],
        Some("jane.private@example.net"),
    );
    let router = api::router(daemon.shared.clone(), false);
    let routes = [
        ("GET", "/v1/connections".into()),
        ("GET", format!("/v1/connections/{}", row.id)),
        ("PATCH", format!("/v1/connections/{}", row.id)),
        ("POST", format!("/v1/connections/{}/check", row.id)),
        ("POST", format!("/v1/connections/{}/relogin", row.id)),
        ("POST", format!("/v1/connections/{}/revoke", row.id)),
        (
            "POST",
            format!("/v1/connections/candidates/{}/dismiss", row.id),
        ),
        ("GET", "/v1/connections/audit".into()),
        ("POST", "/v1/connections/spaces".into()),
        ("DELETE", "/v1/connections/spaces/alpha".into()),
    ];
    for scope in [
        CapabilityScope::Read,
        CapabilityScope::Steer,
        CapabilityScope::Browse,
        CapabilityScope::Approve,
        CapabilityScope::HumanStep,
        CapabilityScope::VaultUse,
        CapabilityScope::OtpIngest,
    ] {
        let token = daemon.shared.db.pair(vec![scope]).await.unwrap().1;
        for (method, path) in &routes {
            let response = router
                .clone()
                .oneshot(request(method, path, json!({}), Some(&token)))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "{scope:?} {method} {path}"
            );
        }
    }
    let token = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Admin])
        .await
        .unwrap()
        .1;
    let response = router
        .oneshot(request("GET", "/v1/connections", json!({}), Some(&token)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["connections"][0]["identity"], "j•••@example.net");
    assert!(!body.to_string().contains("jane.private"));
    assert!(!body.to_string().contains("fingerprint"));
    assert!(!body.to_string().contains("account_key"));
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connections_headless_probe_unauthorized_and_site_revoke() {
    use axum::{response::Html, routing::get, Router};
    let root = tempfile::tempdir().unwrap();
    let mut config =
        super::browser::browser_config(root.path()).expect("headless Chromium required");
    config.loaded.config.policy.mode = openside_config::PolicyModeSetting::Full;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let app = Router::new()
        .route("/active", get(|| async {
            ([(axum::http::header::SET_COOKIE, "session=COOKIE_VALUE_CANARY_PROBE_aa12; HttpOnly; Secure; Max-Age=3600; Path=/"),
                (axum::http::header::WWW_AUTHENTICATE, "HEADER_VALUE_CANARY_PROBE_bb34")],
                Html("<header>jane.private@example.net</header><a href='/logout'>Sign out</a>"))
        }))
        .route("/login", get(|| async { Html("<label>Password<input type='password'></label><button>Sign in</button>") }))
        .route("/unauthorized", get(|| async { (StatusCode::UNAUTHORIZED, Html("<h1>Account</h1>")) }))
        .route("/empty", get(|| async { Html("<h1>Empty</h1>") }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let session = daemon
        .shared
        .agents
        .get("chief")
        .unwrap()
        .unwrap()
        .home_session_id;
    let browser = daemon
        .shared
        .registry
        .browser
        .get(&daemon.shared)
        .await
        .map_err(|error| error.0)
        .unwrap();
    let tab = browser
        .create_agent_tab(session.as_str(), "about:blank")
        .await
        .unwrap()
        .id;
    browser
        .navigate(session.as_str(), &tab, &format!("{origin}/active"))
        .await
        .unwrap();
    browser
        .evaluate(
            &tab,
            "localStorage.setItem('session','LOCAL_STORAGE_CANARY'); null",
        )
        .await
        .unwrap();
    let other_origin = origin.replace("localhost", "127.0.0.1");
    let other_tab = browser
        .create_agent_tab(session.as_str(), "about:blank")
        .await
        .unwrap()
        .id;
    browser
        .navigate(
            session.as_str(),
            &other_tab,
            &format!("{other_origin}/active"),
        )
        .await
        .unwrap();
    browser
        .evaluate(
            &other_tab,
            "localStorage.setItem('session','UNRELATED_STORAGE_CANARY'); null",
        )
        .await
        .unwrap();
    let keys = browser.site_cookie_keys().await.unwrap();
    let tab_count = browser.tabs().await.len();
    assert!(keys.iter().any(|key| key.name == "session"));
    let row = daemon
        .shared
        .connections
        .record(connections::Record {
            site: "localhost",
            keys: &keys,
            source: ConnectionSource::Import,
            status: ConnectionStatus::Unverified,
            identity: None,
            identity_source: ConnectionIdentitySource::None,
            home: Some(&format!("{origin}/active")),
            session: None,
            agent: "chief",
            vault_item: None,
            account_space: None,
        })
        .unwrap();
    let router = api::router(daemon.shared.clone(), true);
    let check_path = format!("/v1/connections/{}/check", row.id);
    let checked = router
        .clone()
        .oneshot(request("POST", &check_path, json!({}), None))
        .await
        .unwrap();
    assert_eq!(checked.status(), StatusCode::OK);
    let checked = json_body(checked).await;
    assert_eq!(checked["status"], "active");
    assert_eq!(checked["identity"], "j•••@example.net");
    for secret in [
        "COOKIE_VALUE_CANARY_PROBE_aa12",
        "HEADER_VALUE_CANARY_PROBE_bb34",
    ] {
        assert!(!checked.to_string().contains(secret));
    }
    assert_eq!(browser.tabs().await.len(), tab_count, "probe tab is closed");
    for path in ["/login", "/unauthorized"] {
        let updated = router
            .clone()
            .oneshot(request(
                "PATCH",
                &format!("/v1/connections/{}", row.id),
                json!({"home_url":format!("{origin}{path}")}),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(updated.status(), StatusCode::OK);
        let checked = router
            .clone()
            .oneshot(request("POST", &check_path, json!({}), None))
            .await
            .unwrap();
        assert_eq!(checked.status(), StatusCode::OK);
        assert_eq!(json_body(checked).await["status"], "needs_login");
    }
    let updated = router
        .clone()
        .oneshot(request(
            "PATCH",
            &format!("/v1/connections/{}", row.id),
            json!({"home_url":format!("{origin}/login")}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let relogin = router
        .clone()
        .oneshot(request(
            "POST",
            &format!("/v1/connections/{}/relogin", row.id),
            json!({}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(relogin.status(), StatusCode::OK);
    let relogin = json_body(relogin).await;
    assert_eq!(relogin["status"], "needs_you");
    assert_eq!(relogin["ticket"]["session_id"], json!(session));
    let revoked = router
        .clone()
        .oneshot(request(
            "POST",
            &format!("/v1/connections/{}/revoke", row.id),
            json!({"sign_out":true}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::NO_CONTENT);
    let remaining = browser.site_cookie_keys().await.unwrap();
    assert!(remaining
        .iter()
        .all(|key| key.domain != "localhost" || key.name != "session"));
    assert!(
        remaining
            .iter()
            .any(|key| key.domain == "127.0.0.1" && key.name == "session"),
        "unrelated site cookies survive"
    );
    assert!(browser
        .tabs()
        .await
        .iter()
        .all(|remaining| remaining.id != tab));
    let tab = browser
        .create_agent_tab(session.as_str(), "about:blank")
        .await
        .unwrap()
        .id;
    browser
        .navigate(session.as_str(), &tab, &format!("{origin}/empty"))
        .await
        .unwrap();
    assert_eq!(
        browser
            .evaluate(&tab, "localStorage.getItem('session')")
            .await
            .unwrap(),
        Value::Null
    );
    assert_eq!(
        browser
            .evaluate(&other_tab, "localStorage.getItem('session')")
            .await
            .unwrap(),
        json!("UNRELATED_STORAGE_CANARY")
    );
    let missing = router
        .oneshot(request(
            "GET",
            &format!("/v1/connections/{}", row.id),
            json!({}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let events =
        serde_json::to_string(&daemon.shared.db.events(&session, 0).await.unwrap()).unwrap();
    assert!(!events.contains("COOKIE_VALUE_CANARY_PROBE_aa12"));
    let audit = std::fs::read_to_string(root.path().join("data/connections-audit.jsonl")).unwrap();
    assert!(!audit.contains("COOKIE_VALUE_CANARY_PROBE_aa12"));
    assert!(!events.contains("HEADER_VALUE_CANARY_PROBE_bb34"));
    assert!(!audit.contains("HEADER_VALUE_CANARY_PROBE_bb34"));
    for entry in std::fs::read_dir(root.path().join("data")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy();
        if !name.starts_with("connections") || name == "connections.key" {
            continue;
        }
        let stored = std::fs::read(path).unwrap();
        for secret in [
            "COOKIE_VALUE_CANARY_PROBE_aa12",
            "HEADER_VALUE_CANARY_PROBE_bb34",
        ] {
            assert!(!stored
                .windows(secret.len())
                .any(|part| part == secret.as_bytes()));
        }
    }
    assert!(!audit.contains("jane.private"));
    daemon.shutdown().await;
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connections_account_space_routes_and_revoke_leave_shared_browser_untouched() {
    let root = tempfile::tempdir().unwrap();
    let mut config =
        super::browser::browser_config(root.path()).expect("headless Chromium required");
    config.loaded.config.policy.mode = openside_config::PolicyModeSetting::Full;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let rejected = crate::connections::spaces::create(
        axum::extract::State(daemon.shared.clone()),
        axum::Extension(api::Caller(Some("agent".into()), DeviceKind::AgentClient)),
        axum::Json(CreateAccountSpaceRequest {
            site: "example.com".into(),
        }),
    )
    .await;
    assert!(matches!(
        rejected,
        Err(api::HttpError(
            StatusCode::FORBIDDEN,
            "person_device_required"
        ))
    ));
    let shared = &daemon.shared;
    let session = shared.agents.get("chief").unwrap().unwrap().home_session_id;
    let main = shared
        .registry
        .browser
        .get(shared)
        .await
        .map_err(|error| error.0)
        .unwrap();
    let main_tab = main
        .create_agent_tab(session.as_str(), "about:blank")
        .await
        .unwrap();
    let space = shared
        .registry
        .browser
        .account_browser(shared, "example.com", "alpha")
        .await
        .map_err(|error| error.0)
        .unwrap();
    let tab = space
        .create_agent_tab(session.as_str(), "about:blank")
        .await
        .unwrap();
    let routed = shared
        .registry
        .browser
        .for_tab(shared, &session, &tab.id)
        .await
        .map_err(|error| error.0)
        .unwrap();
    assert_eq!(routed.process_id(), space.process_id());
    let routed = shared
        .registry
        .browser
        .for_tab(shared, &session, &main_tab.id)
        .await
        .map_err(|error| error.0)
        .unwrap();
    assert_eq!(routed.process_id(), main.process_id());
    let row = shared
        .connections
        .record(connections::Record {
            site: "example.com",
            keys: &[],
            source: ConnectionSource::Login,
            status: ConnectionStatus::NeedsLogin,
            identity: None,
            identity_source: ConnectionIdentitySource::None,
            home: None,
            session: None,
            agent: "chief",
            vault_item: None,
            account_space: Some("alpha"),
        })
        .unwrap();
    let router = api::router(shared.clone(), true);
    let response = router
        .oneshot(request(
            "POST",
            &format!("/v1/connections/{}/revoke", row.id),
            json!({"sign_out":false}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(!root
        .path()
        .join("data/browser-profiles/accounts/alpha")
        .exists());
    assert!(main.is_connected());
    assert!(main.tabs().await.iter().any(|tab| tab.id == main_tab.id));
    assert!(shared.connections.get(&row.id).unwrap().is_none());
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connections_revoke_clears_closed_cookie_less_subdomain_frame_storage() {
    use axum::{response::Html, routing::get, Router};
    let root = tempfile::tempdir().unwrap();
    let mut config =
        super::browser::browser_config(root.path()).expect("headless Chromium required");
    config.loaded.config.policy.mode = openside_config::PolicyModeSetting::Full;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let origin = format!("http://example.localhost:{port}");
    let child = format!("http://sub.example.localhost:{port}");
    let other = format!("http://unrelated.localhost:{port}");
    let app = Router::new().route("/", get(|| async { Html("<title>Storage fixture</title>") }))
        .route("/store", get(|| async { Html(r#"<script>
localStorage.setItem('token','FRAME_STORAGE_TOKEN');
const request=indexedDB.open('auth',1);
request.onupgradeneeded=()=>request.result.createObjectStore('tokens');
request.onsuccess=()=>{const db=request.result;const tx=db.transaction('tokens','readwrite');tx.objectStore('tokens').put('IDB_TOKEN','token');tx.oncomplete=()=>{db.close();parent.postMessage('stored','*');};};
</script>"#) }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let session = daemon
        .shared
        .agents
        .get("chief")
        .unwrap()
        .unwrap()
        .home_session_id;
    let browser = daemon
        .shared
        .registry
        .browser
        .get(&daemon.shared)
        .await
        .map_err(|error| error.0)
        .unwrap();
    let row = daemon
        .shared
        .connections
        .record(connections::Record {
            site: "example.localhost",
            keys: &[],
            source: ConnectionSource::Login,
            status: ConnectionStatus::Active,
            identity: None,
            identity_source: ConnectionIdentitySource::None,
            home: Some(&origin),
            session: Some(&session),
            agent: "chief",
            vault_item: None,
            account_space: None,
        })
        .unwrap();
    let tab = browser
        .create_agent_tab(session.as_str(), &origin)
        .await
        .unwrap()
        .id;
    browser
        .navigate(session.as_str(), &tab, &origin)
        .await
        .unwrap();
    browser.evaluate(&tab, &format!("new Promise(resolve=>{{addEventListener('message',e=>{{if(e.origin==={}&&e.data==='stored')resolve(true);}});const f=document.createElement('iframe');f.src={};document.body.append(f);}})", json!(child), json!(format!("{child}/store")))).await.unwrap();
    for _ in 0..100 {
        if daemon
            .shared
            .connections
            .origins("example.localhost")
            .unwrap()
            .contains(&child)
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        daemon
            .shared
            .connections
            .origins("example.localhost")
            .unwrap()
            .contains(&child),
        "committed iframe origin must be recorded"
    );
    browser
        .close_agent_tab(session.as_str(), &tab)
        .await
        .unwrap();
    let unrelated = browser
        .create_agent_tab(session.as_str(), &other)
        .await
        .unwrap()
        .id;
    browser
        .navigate(session.as_str(), &unrelated, &other)
        .await
        .unwrap();
    browser
        .evaluate(&unrelated, "localStorage.setItem('token','KEEP');null")
        .await
        .unwrap();
    assert!(browser.site_cookie_keys().await.unwrap().is_empty());
    let disk_origins =
        openside_import::profile_storage_origins(&browser.endpoint().profile_dir.join("Default"))
            .expect("live storage inventory must remain readable");
    assert!(disk_origins.iter().all(|origin| !origin.contains("TOKEN")));
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request(
            "POST",
            &format!("/v1/connections/{}/revoke", row.id),
            json!({"sign_out":true}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let reopened = browser
        .create_agent_tab(session.as_str(), &child)
        .await
        .unwrap()
        .id;
    browser
        .navigate(session.as_str(), &reopened, &child)
        .await
        .unwrap();
    assert_eq!(
        browser
            .evaluate(&reopened, "localStorage.getItem('token')")
            .await
            .unwrap(),
        Value::Null
    );
    assert_eq!(
        browser
            .evaluate(&reopened, "indexedDB.databases().then(d=>d.map(x=>x.name))")
            .await
            .unwrap(),
        json!([])
    );
    assert_eq!(
        browser
            .evaluate(&unrelated, "localStorage.getItem('token')")
            .await
            .unwrap(),
        json!("KEEP")
    );
    daemon.shutdown().await;
    server.abort();
}
