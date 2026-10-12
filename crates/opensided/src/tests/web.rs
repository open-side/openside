//! Web UI support: session list metadata, settings, and static bundle serving.

use super::*;

async fn send(router: &axum::Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(request(method, path, body, None))
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    // Axum's own extractor rejections are plain text.
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, value)
}

fn titles(list: &Value) -> Vec<&str> {
    list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|session| session["title"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn session_metadata_is_durable_and_drives_list_search_pin_and_archive() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let first = daemon(root.path(), &runtime).await;
    for name in ["alpha", "bravo", "charlie"] {
        create_session(&first, name, PolicyMode::Guard, None).await;
    }
    first
        .shared
        .emit(
            &SessionId::new("charlie"),
            None,
            EventKind::MessageQueued {
                message_id: MessageId::new("m1"),
                text: "Find the Quarterly REPORT".into(),
                mode: QueueMode::Queue,
                attachments: Vec::new(),
            },
        )
        .await
        .unwrap();
    let router = api::router(first.shared.clone(), true);

    let (status, body) = send(
        &router,
        "PATCH",
        "/v1/sessions/alpha",
        json!({"title":"  Groceries  "}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["title"], "Groceries");
    let (status, body) = send(
        &router,
        "PATCH",
        "/v1/sessions/bravo",
        json!({"pinned":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["pinned"], true);
    let (status, body) = send(
        &router,
        "PATCH",
        "/v1/sessions/alpha",
        json!({"archived":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["archived"], true);

    // Default list hides archived sessions and puts pinned ones first.
    let (_, list) = send(&router, "GET", "/v1/sessions", json!({})).await;
    let ids: Vec<_> = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["bravo", "charlie"]);
    let (_, archived) = send(&router, "GET", "/v1/sessions?archived=true", json!({})).await;
    assert_eq!(titles(&archived), ["Groceries"]);
    // Search matches the first message text case-insensitively, and the title.
    let (_, found) = send(
        &router,
        "GET",
        "/v1/sessions?q=quarterly%20report",
        json!({}),
    )
    .await;
    assert_eq!(found["sessions"][0]["id"], "charlie");
    assert_eq!(found["sessions"].as_array().unwrap().len(), 1);
    let (_, found) = send(
        &router,
        "GET",
        "/v1/sessions?q=GROC&archived=true",
        json!({}),
    )
    .await;
    assert_eq!(titles(&found), ["Groceries"]);
    let (_, none) = send(&router, "GET", "/v1/sessions?q=groc", json!({})).await;
    assert!(none["sessions"].as_array().unwrap().is_empty());

    // Rejected updates leave no trace.
    for body in [
        json!({}),
        json!({"title":"   "}),
        json!({"title":"x".repeat(201)}),
    ] {
        let (status, _) = send(&router, "PATCH", "/v1/sessions/bravo", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let (status, _) = send(
        &router,
        "PATCH",
        "/v1/sessions/bravo",
        json!({"color":"red"}),
    )
    .await;
    assert!(status.is_client_error());
    let (status, body) = send(
        &router,
        "PATCH",
        "/v1/sessions/missing",
        json!({"pinned":true}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    // A no-op update does not append an event.
    let before = first
        .shared
        .db
        .events(&SessionId::new("bravo"), 0)
        .await
        .unwrap()
        .len();
    let (status, _) = send(
        &router,
        "PATCH",
        "/v1/sessions/bravo",
        json!({"pinned":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let events = first
        .shared
        .db
        .events(&SessionId::new("bravo"), 0)
        .await
        .unwrap();
    assert_eq!(events.len(), before);
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        EventKind::SessionUpdated {
            pinned: Some(true),
            title: None,
            archived: None
        }
    )));

    first.shutdown().await;
    drop(first);
    let second = daemon(root.path(), &TestRuntime::default()).await;
    let router = api::router(second.shared.clone(), true);
    let (_, list) = send(&router, "GET", "/v1/sessions", json!({})).await;
    assert_eq!(list["sessions"][0]["id"], "bravo");
    assert_eq!(list["sessions"][0]["pinned"], true);
    let (_, archived) = send(&router, "GET", "/v1/sessions?archived=true", json!({})).await;
    assert_eq!(titles(&archived), ["Groceries"]);
    second.shutdown().await;
}

#[tokio::test]
async fn session_metadata_update_requires_steer_capability() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let (_, read) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Read])
        .await
        .unwrap();
    let (_, steer) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Steer])
        .await
        .unwrap();
    let router = api::router(daemon.shared.clone(), false);
    for (token, expected) in [(&read, StatusCode::FORBIDDEN), (&steer, StatusCode::OK)] {
        let response = router
            .clone()
            .oneshot(request(
                "PATCH",
                "/v1/sessions/session",
                json!({"pinned":true}),
                Some(token),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    let response = router
        .oneshot(request("GET", "/v1/sessions", json!({}), Some(&read)))
        .await
        .unwrap();
    assert_eq!(json_body(response).await["sessions"][0]["pinned"], true);
    daemon.shutdown().await;
}

#[tokio::test]
async fn settings_expose_configured_models_without_credentials() {
    let root = tempfile::tempdir().unwrap();
    let mut config = resolved_config(root.path());
    config.loaded.config.models.default = Some("provider:configured".into());
    config.loaded.config.models.fallback = vec!["provider:backup".into()];
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let (status, body) = send(
        &api::router(daemon.shared.clone(), true),
        "GET",
        "/v1/settings",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"default_model":"provider:configured","fallback_models":["provider:backup"],"policy_mode":"guard"})
    );
    daemon.shutdown().await;
}

async fn raw(
    router: &axum::Router,
    method: &str,
    path: &str,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

async fn ui_daemon(root: &Path) -> (Daemon, axum::Router) {
    let bundle = root.join("bundle");
    std::fs::create_dir_all(bundle.join("assets")).unwrap();
    std::fs::write(
        bundle.join("index.html"),
        "<!doctype html><title>openside</title>",
    )
    .unwrap();
    std::fs::write(bundle.join("assets/app.js"), "console.log('app')").unwrap();
    std::fs::write(bundle.join("manifest.webmanifest"), "{}").unwrap();
    std::fs::write(root.join("secret.txt"), "outside the bundle").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let outside = root.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("index.html"), "outside the bundle").unwrap();
        symlink(root.join("secret.txt"), bundle.join("assets/leak.txt")).unwrap();
        // Extensionless escapes must not be masked by the SPA shell either.
        symlink(root.join("secret.txt"), bundle.join("leak")).unwrap();
        symlink(root.join("missing-target"), bundle.join("dangling")).unwrap();
        symlink(&outside, bundle.join("linked-dir")).unwrap();
        std::fs::create_dir_all(bundle.join("docs")).unwrap();
        symlink(root.join("secret.txt"), bundle.join("docs/index.html")).unwrap();
        std::fs::create_dir_all(bundle.join("guide")).unwrap();
        std::fs::write(bundle.join("guide/index.html"), "inside guide").unwrap();
        symlink(bundle.join("assets/app.js"), bundle.join("alias.js")).unwrap();
    }
    let mut config = resolved_config(root);
    config.loaded.config.ui.dir = Some(bundle);
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let router = api::router(daemon.shared.clone(), false);
    (daemon, router)
}

#[tokio::test]
async fn ui_bundle_is_served_with_spa_fallback_and_security_headers() {
    let root = tempfile::tempdir().unwrap();
    let (daemon, router) = ui_daemon(root.path()).await;
    for path in ["/", "/index.html", "/sessions/abc", "/settings"] {
        let (status, headers, body) = raw(&router, "GET", path).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert!(body.contains("<title>openside</title>"), "{path}");
        assert_eq!(headers["content-type"], "text/html; charset=utf-8");
        let csp = headers["content-security-policy"].to_str().unwrap();
        assert!(csp.contains("script-src 'self'") && csp.contains("connect-src 'self'"));
        assert!(!csp.contains("unsafe-inline") && !csp.contains("unsafe-eval"));
        assert_eq!(headers["x-content-type-options"], "nosniff");
        assert_eq!(headers["referrer-policy"], "no-referrer");
    }
    let (status, headers, body) = raw(&router, "GET", "/assets/app.js").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "console.log('app')");
    assert_eq!(headers["content-type"], "text/javascript; charset=utf-8");
    assert!(headers["cache-control"]
        .to_str()
        .unwrap()
        .contains("immutable"));
    let (_, headers, _) = raw(&router, "GET", "/manifest.webmanifest").await;
    assert_eq!(headers["content-type"], "application/manifest+json");
    // Missing assets are not masked by the SPA shell.
    let (status, headers, _) = raw(&router, "GET", "/assets/missing.js").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(headers.contains_key("content-security-policy"));
    daemon.shutdown().await;
}

#[tokio::test]
async fn ui_rejects_traversal_and_symlink_escape() {
    let root = tempfile::tempdir().unwrap();
    let (daemon, router) = ui_daemon(root.path()).await;
    for path in [
        "/../secret.txt",
        "/assets/../../secret.txt",
        "/%2e%2e/secret.txt",
        "/assets/%2E%2E/%2e%2e/secret.txt",
        "/..%2fsecret.txt",
        "/%5c..%5csecret.txt",
    ] {
        let (status, _, body) = raw(&router, "GET", path).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}");
        assert!(!body.contains("outside the bundle"), "{path}");
    }
    for path in [
        "/assets/leak.txt",
        "/leak",
        "/dangling",
        "/linked-dir",
        "/linked-dir/",
        "/linked-dir/index.html",
        "/docs",
        "/docs/",
        "/docs/index.html",
    ] {
        let (status, _, body) = raw(&router, "GET", path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert!(!body.contains("outside the bundle"), "{path}");
    }
    // Symlinks and directory indexes that stay inside the bundle still work.
    let (status, _, body) = raw(&router, "GET", "/alias.js").await;
    assert_eq!(
        (status, body.as_str()),
        (StatusCode::OK, "console.log('app')")
    );
    let (status, _, body) = raw(&router, "GET", "/guide").await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "inside guide"));
    let (status, _, _) = raw(&router, "POST", "/index.html").await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    daemon.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn ui_shell_index_symlinked_outside_is_never_served() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    // Replacing the shell after startup must not turn the SPA fallback into a leak.
    let (daemon, router) = ui_daemon(root.path()).await;
    let index = root.path().join("bundle/index.html");
    std::fs::remove_file(&index).unwrap();
    symlink(root.path().join("secret.txt"), &index).unwrap();
    for path in ["/", "/index.html", "/sessions/abc"] {
        let (status, _, body) = raw(&router, "GET", path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert!(!body.contains("outside the bundle"), "{path}");
    }
    daemon.shutdown().await;

    // A bundle whose shell already escapes at startup gets no UI routes at all.
    let restarted = tempfile::tempdir().unwrap();
    let mut config = resolved_config(restarted.path());
    config.loaded.config.ui.dir = Some(root.path().join("bundle"));
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let router = api::router(daemon.shared.clone(), false);
    let (status, headers, body) = raw(&router, "GET", "/").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!headers.contains_key("content-security-policy"));
    assert!(!body.contains("outside the bundle"));
    daemon.shutdown().await;
}

#[tokio::test]
async fn ui_never_shadows_versioned_api_routes() {
    let root = tempfile::tempdir().unwrap();
    let (daemon, router) = ui_daemon(root.path()).await;
    // Real API routes still require a capability token over TCP.
    let (status, _, body) = raw(&router, "GET", "/v1/health").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body.contains("unauthorized"));
    for path in ["/v1", "/v1/", "/v1/unknown", "/v1/sessions/x/unknown"] {
        let (status, headers, body) = raw(&router, "GET", path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(headers["content-type"], "application/json");
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap()["code"],
            "not_found"
        );
    }
    let (_, token) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Read])
        .await
        .unwrap();
    let response = router
        .clone()
        .oneshot(request("GET", "/v1/health", json!({}), Some(&token)))
        .await
        .unwrap();
    assert_eq!(json_body(response).await["api_version"], "v1");
    daemon.shutdown().await;
}

#[tokio::test]
async fn ui_routes_are_absent_without_configuration() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let router = api::router(daemon.shared.clone(), false);
    let (status, headers, _) = raw(&router, "GET", "/").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!headers.contains_key("content-security-policy"));
    daemon.shutdown().await;
}
