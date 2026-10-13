//! WS-3 policy surface: scoped tokens (PP-20), live policy API (PP-12, D-1, D-2), approval
//! objects (D-4) and the sandbox escape suite (D-3).

use super::*;

const ALL_SCOPES: [CapabilityScope; 8] = [
    CapabilityScope::Read,
    CapabilityScope::Steer,
    CapabilityScope::Approve,
    CapabilityScope::HumanStep,
    CapabilityScope::Admin,
    CapabilityScope::Browse,
    CapabilityScope::VaultUse,
    CapabilityScope::OtpIngest,
];

async fn send(
    router: &axum::Router,
    method: &str,
    path: &str,
    body: Value,
    token: Option<&str>,
) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(request(method, path, body, token))
        .await
        .unwrap();
    let status = response.status();
    // SSE bodies never end; only JSON bodies are read.
    let json = response
        .headers()
        .get("content-type")
        .is_some_and(|value| value.as_bytes().starts_with(b"application/json"));
    let body = if json {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    (status, body)
}

fn concrete(path: &str) -> String {
    let out = path
        .replace("{mid}", "message")
        .replace("{sid}", "child")
        .replace("{name}", "skill")
        .replace("{path}", "transcript.jsonl")
        .replace("{thumbnail_id}", "thumbnail")
        .replace("{provider}", "anthropic")
        .replace("{source}", "claude-code")
        .replace("{kind}", "telegram")
        .replace("{account_id}", "work")
        .replace("{key}", "proposal")
        .replace("{key_id}", "key")
        .replace("{ticket}", "ticket")
        .replace("{origin}", "invalid-origin")
        .replace("{space_id}", "space")
        .replace("{id}", "session");
    assert!(!out.contains('{'), "unsubstituted path parameter in {out}");
    out
}

/// Every route in the public table refuses a token that lacks its scope (403), refuses a
/// missing or unknown token (401), and lets a token holding exactly its scope through the
/// capability layer.
#[tokio::test]
async fn every_route_rejects_tokens_lacking_its_scope() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), false);
    let mut tokens = HashMap::new();
    for scope in ALL_SCOPES {
        tokens.insert(scope, daemon.shared.db.pair(vec![scope]).await.unwrap().1);
    }
    for route in ROUTES {
        let path = concrete(route.path);
        let method = route.method.as_str();
        // Public routes carry their own credential (code or signed URL); core_gaps covers them.
        if openside_proto::PUBLIC_ROUTES.contains(&(method, route.path)) {
            continue;
        }
        // Ticket resolve/reject check the ticket kind later; either ticket scope passes the
        // route layer.
        let kind_specific = matches!(
            route.path,
            "/v1/tickets/{id}/resolve" | "/v1/tickets/{id}/reject"
        );
        let admitted = |scope: CapabilityScope| {
            if kind_specific {
                [CapabilityScope::Approve, CapabilityScope::HumanStep]
                    .iter()
                    .any(|required| scope.grants(*required))
            } else {
                scope.grants(route.scope)
            }
        };
        let (status, _) = send(&router, method, &path, json!({}), None).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {path} without token"
        );
        let (status, _) = send(&router, method, &path, json!({}), Some("not-a-token")).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {path} unknown token"
        );
        let lacking: Vec<_> = ALL_SCOPES.into_iter().filter(|s| !admitted(*s)).collect();
        assert!(!lacking.contains(&CapabilityScope::Admin));
        if !lacking.is_empty() {
            let (_, token) = daemon.shared.db.pair(lacking.clone()).await.unwrap();
            let (status, _) = send(&router, method, &path, json!({}), Some(&token)).await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{method} {path} accepted {lacking:?}"
            );
        }
        let (status, body) = send(
            &router,
            method,
            &path,
            if route.path == "/v1/memory/model/download" {
                json!({"unknown":true})
            } else {
                json!({})
            },
            Some(&tokens[&route.scope]),
        )
        .await;
        if api::SOCKET_ONLY.contains(&(method, route.path)) {
            assert_eq!(
                (status, body["code"].as_str()),
                (StatusCode::FORBIDDEN, Some("local_socket_required")),
                "{method} {path} must refuse every TCP token"
            );
            continue;
        }
        if api::LOCAL_REQUIRED.contains(&(method, route.path))
            || route.path == "/v1/pairing"
            || route.path == "/v1/access-requests/{id}/approve"
            || route.path.starts_with("/v1/import")
            || method == "PUT" && route.path.starts_with("/v1/remote/")
        {
            assert_eq!(
                (status, body["code"].as_str()),
                (StatusCode::FORBIDDEN, Some("local_access_required")),
                "{method} {path}"
            );
            continue;
        }
        if route.path == "/v1/extension/import/{ticket}" {
            assert_eq!(
                (status, body["code"].as_str()),
                (StatusCode::FORBIDDEN, Some("invalid_import_ticket"))
            );
            continue;
        }
        assert!(
            status != StatusCode::UNAUTHORIZED && status != StatusCode::FORBIDDEN,
            "{method} {path} refused its own scope {:?}: {status}",
            route.scope
        );
    }
    daemon.shutdown().await;
}

/// PP-20: a `browse` token reaches only browser routes; `steer` also grants `browse`.
#[tokio::test]
async fn browse_token_reaches_only_browse_routes() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), false);
    let (_, browse) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Browse])
        .await
        .unwrap();
    let (_, steer) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Steer])
        .await
        .unwrap();
    for route in ROUTES {
        let path = concrete(route.path);
        if openside_proto::PUBLIC_ROUTES.contains(&(route.method.as_str(), route.path)) {
            continue;
        }
        let (status, _) = send(
            &router,
            route.method.as_str(),
            &path,
            json!({}),
            Some(&browse),
        )
        .await;
        if route.scope == CapabilityScope::Browse {
            assert_ne!(status, StatusCode::FORBIDDEN, "{path}");
        } else {
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{} {path}",
                route.method.as_str()
            );
        }
    }
    let (status, _) = send(
        &router,
        "POST",
        "/v1/sessions/session/repl",
        json!({"code":"1"}),
        Some(&steer),
    )
    .await;
    assert_ne!(status, StatusCode::FORBIDDEN);
    daemon.shutdown().await;
}

/// PP-12 / D-1 / D-2: the live policy validates, persists 0600, survives restart and changes
/// decisions of an already-open session on its next tool call.
#[tokio::test]
async fn policy_api_validates_persists_and_hot_reloads() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(
        &daemon,
        "session",
        PolicyMode::Guard,
        Some(workspace.path()),
    )
    .await;
    let host = host(&daemon, &session).await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let (_, read) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Read])
        .await
        .unwrap();

    let (status, initial) = send(&tcp, "GET", "/v1/policy", json!({}), Some(&read)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        initial,
        json!({"default_mode":"guard","allow_origins":[],"deny_origins":[],"rules":[]})
    );
    assert!(matches!(
        host.call(call("write", json!({"path":"a.txt","content":"1"}), "w1"))
            .await,
        ToolOutcome::Done { .. }
    ));

    for invalid in [
        json!({"default_mode":"guard","rules":[{"effect":"allow","capability":"sensitive","pattern":"**"}]}),
        json!({"default_mode":"guard","deny_origins":["evil.example"]}),
        json!({"default_mode":"guard","rules":[{"effect":"deny","capability":"fs_write","pattern":""}]}),
        json!({"default_mode":"guard","unknown":true}),
    ] {
        let (status, _) = send(&unix, "PUT", "/v1/policy", invalid.clone(), None).await;
        assert!(status.is_client_error(), "{invalid}");
    }
    assert!(!root.path().join("data/policy.json").exists());

    let canonical = std::fs::canonicalize(workspace.path()).unwrap();
    let settings = json!({
        "default_mode": "read-only",
        "allow_origins": [],
        "deny_origins": ["http://*.evil.test"],
        "rules": [{"effect":"deny","capability":"fs_write","pattern": format!("{}/**", canonical.display())}]
    });
    let (status, _) = send(&tcp, "PUT", "/v1/policy", settings.clone(), Some(&read)).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "read scope cannot write policy"
    );
    let (status, body) = send(&unix, "PUT", "/v1/policy", settings.clone(), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, settings);

    // The session opened before the PUT now sees the agent-level deny on its next call.
    assert!(matches!(
        host.call(call("write", json!({"path":"b.txt","content":"2"}), "w2"))
            .await,
        ToolOutcome::Denied { .. }
    ));
    assert!(!workspace.path().join("b.txt").exists());

    for (body, effect, reason) in [
        (
            json!({"target":"http://cdn.evil.test/x","capability":"browse","mode":"full"}),
            "deny",
            "origin_denied",
        ),
        (
            json!({"target":"https://ok.test","capability":"browse","mode":"guard"}),
            "allow",
            "mode",
        ),
        (
            json!({"target":"https://ok.test","capability":"sensitive","mode":"full"}),
            "ask",
            "sensitive_floor",
        ),
        (
            json!({"target":"/etc/x","capability":"fs_write"}),
            "deny",
            "mode",
        ),
        (
            json!({"target":format!("{}/c", canonical.display()),"capability":"fs_write","mode":"full"}),
            "deny",
            "deny_rule",
        ),
    ] {
        let (status, decision) =
            send(&tcp, "POST", "/v1/policy/test", body.clone(), Some(&read)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(decision, json!({"effect":effect,"reason":reason}), "{body}");
    }

    let (status, created) = send(
        &unix,
        "POST",
        "/v1/sessions",
        json!({"model":"test/model"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        created["mode"], "read-only",
        "default_mode applies to new sessions"
    );
    let (_, settings_view) = send(&tcp, "GET", "/v1/settings", json!({}), Some(&read)).await;
    assert_eq!(settings_view["policy_mode"], "read-only");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(root.path().join("data/policy.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    daemon.shutdown().await;
    drop(daemon);
    let daemon = super::daemon(root.path(), &runtime).await;
    let unix = api::router(daemon.shared.clone(), true);
    let (_, persisted) = send(&unix, "GET", "/v1/policy", json!({}), None).await;
    assert_eq!(persisted, settings);
    daemon.shutdown().await;
    drop(daemon);

    // A corrupt policy file stops startup instead of silently falling back to defaults.
    std::fs::write(root.path().join("data/policy.json"), "{not json").unwrap();
    assert!(
        Daemon::open_runtime(resolved_config(root.path()), runtime.clone())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn approval_ticket_is_an_api_object_person_devices_can_answer() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(
        &daemon,
        "session",
        PolicyMode::Guard,
        Some(workspace.path()),
    )
    .await;
    let target = outside.path().join("x.txt");
    let outcome = host(&daemon, &session)
        .await
        .call(call(
            "write",
            json!({"path":target,"content":"x"}),
            "outside-write",
        ))
        .await;
    let ToolOutcome::Deferred { ticket, kind } = outcome else {
        panic!("outside write must ask: {outcome:?}");
    };
    assert_eq!(kind, DeferKind::Approval);
    assert!(!target.exists());
    let router = api::router(daemon.shared.clone(), false);
    let mut tokens = HashMap::new();
    for scope in [
        CapabilityScope::Read,
        CapabilityScope::Approve,
        CapabilityScope::HumanStep,
    ] {
        tokens.insert(scope, daemon.shared.db.pair(vec![scope]).await.unwrap().1);
    }
    let path = format!("/v1/tickets/{ticket}");
    let (status, object) = send(
        &router,
        "GET",
        &path,
        json!({}),
        Some(&tokens[&CapabilityScope::Read]),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(object["id"], ticket.as_str());
    assert_eq!(object["kind"], "confirm");
    assert_eq!(
        object["summary"],
        format!(
            "Allow openside to save {} outside the workspace?",
            target.file_name().unwrap().to_str().unwrap()
        )
    );
    assert_eq!(object["tool_call_id"], "outside-write");
    assert_eq!(object["tool"]["name"], "write");
    assert!(object["expires_at"].as_str().is_some());
    assert!(object["target"].is_null(), "file actions have no origin");
    for scope in [CapabilityScope::Read, CapabilityScope::HumanStep] {
        let (status, _) = send(
            &router,
            "POST",
            &format!("{path}/resolve"),
            json!({"value":false}),
            Some(&tokens[&scope]),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{scope:?}");
    }
    let (status, resolved) = send(
        &router,
        "POST",
        &format!("{path}/resolve"),
        json!({"value":false}),
        Some(&tokens[&CapabilityScope::Approve]),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(resolved["status"], "resolved");
    assert!(!target.exists());
    daemon.shutdown().await;
}

/// D-3: out-of-sandbox reads and writes never run silently. Guard asks before any path
/// outside the workspace (including through symlinks and `..`), dangling symlink writes are
/// denied, and bash under bubblewrap cannot see or write outside the workspace even in Full.
#[cfg(unix)]
#[tokio::test]
async fn sandbox_escape_suite_never_touches_outside_paths() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    const CANARY: &str = "outside-canary-5c1e";
    std::fs::write(outside.path().join("secret"), CANARY).unwrap();
    std::fs::create_dir(workspace.path().join("sub")).unwrap();
    let link = |name: &str, to: &std::path::Path| {
        std::os::unix::fs::symlink(to, workspace.path().join(name)).unwrap()
    };
    link("secret-link", &outside.path().join("secret"));
    link("dir-link", outside.path());
    link("dangling", &outside.path().join("absent"));
    let runtime = TestRuntime::default();
    let mut config = resolved_config(root.path());
    config.bwrap = openside_config::find_on_path("bwrap");
    let sandboxed = config.bwrap.is_some();
    let daemon = Daemon::open_runtime(config, runtime).await.unwrap();
    let guard = create_session(&daemon, "guard", PolicyMode::Guard, Some(workspace.path())).await;
    let host = host(&daemon, &guard).await;
    let escape = outside.path().join("secret");
    let relative = format!(
        "sub/../../{}/secret",
        outside.path().file_name().unwrap().to_string_lossy()
    );
    for (i, args) in [
        json!({"path": escape}),
        json!({"path": "secret-link"}),
        json!({"path": "dir-link/secret"}),
        json!({"path": relative}),
    ]
    .into_iter()
    .enumerate()
    {
        let outcome = host
            .call(call("read", args.clone(), &format!("r{i}")))
            .await;
        assert!(
            matches!(
                outcome,
                ToolOutcome::Deferred {
                    kind: DeferKind::Approval,
                    ..
                }
            ),
            "{args}: {outcome:?}"
        );
    }
    for (i, args) in [
        json!({"path": outside.path().join("new"), "content": "x"}),
        json!({"path": "dir-link/new", "content": "x"}),
    ]
    .into_iter()
    .enumerate()
    {
        let outcome = host
            .call(call("write", args.clone(), &format!("w{i}")))
            .await;
        assert!(
            matches!(
                outcome,
                ToolOutcome::Deferred {
                    kind: DeferKind::Approval,
                    ..
                }
            ),
            "{args}: {outcome:?}"
        );
    }
    assert!(matches!(
        host.call(call(
            "write",
            json!({"path":"dangling","content":"x"}),
            "w-dangling"
        ))
        .await,
        ToolOutcome::Denied { .. }
    ));
    assert!(!outside.path().join("new").exists());
    assert!(!outside.path().join("absent").exists());

    if !sandboxed {
        eprintln!("SKIP: bash escape cases need bubblewrap (bwrap) on PATH");
        daemon.shutdown().await;
        return;
    }
    let full = create_session(&daemon, "full", PolicyMode::Full, Some(workspace.path())).await;
    let host = super::host(&daemon, &full).await;
    let outside_dir = outside.path().display().to_string();
    let command = format!(
        "cat {outside_dir}/secret; cat secret-link; cat dir-link/secret; \
         echo pwned > {outside_dir}/bash-new; echo pwned > dir-link/bash-new; \
         echo ok > inside.txt; true",
    );
    let outcome = host
        .call(call("bash", json!({"command": command}), "escape"))
        .await;
    let ToolOutcome::Done { for_model_json } = outcome else {
        panic!("sandboxed bash should run: {outcome:?}");
    };
    assert!(!for_model_json.contains(CANARY), "{for_model_json}");
    assert!(!outside.path().join("bash-new").exists());
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("inside.txt")).unwrap(),
        "ok\n",
        "positive control: the workspace stays writable"
    );
    daemon.shutdown().await;
}

/// Concurrent PUTs serialize: the live policy, the persisted file and a restart all agree on
/// one submitted document, and no temporary file is left behind.
#[tokio::test]
async fn concurrent_policy_puts_leave_live_and_disk_identical() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let unix = api::router(daemon.shared.clone(), true);
    let documents: Vec<Value> = (0..16)
        .map(|i| {
            json!({"default_mode":"guard","allow_origins":[],
            "deny_origins":[format!("https://d{i}.example")],"rules":[]})
        })
        .collect();
    let barrier = Arc::new(tokio::sync::Barrier::new(documents.len()));
    let mut tasks = Vec::new();
    for document in documents.clone() {
        let (router, barrier) = (unix.clone(), barrier.clone());
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            send(&router, "PUT", "/v1/policy", document, None).await.0
        }));
    }
    for task in tasks {
        assert_eq!(task.await.unwrap(), StatusCode::OK);
    }
    let (_, live) = send(&unix, "GET", "/v1/policy", json!({}), None).await;
    let disk: Value =
        serde_json::from_slice(&std::fs::read(root.path().join("data/policy.json")).unwrap())
            .unwrap();
    assert_eq!(live, disk);
    assert!(documents.contains(&live));
    let leftovers: Vec<_> = std::fs::read_dir(root.path().join("data"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    daemon.shutdown().await;
    drop(daemon);
    let daemon = super::daemon(root.path(), &runtime).await;
    let (_, restarted) = send(
        &api::router(daemon.shared.clone(), true),
        "GET",
        "/v1/policy",
        json!({}),
        None,
    )
    .await;
    assert_eq!(restarted, live);
    daemon.shutdown().await;
}

/// A PUT whose client goes away while it waits for the writer lock is still committed in full:
/// the persist-and-publish transaction is not tied to the request future, so a later PUT can
/// never race a half-finished one and live, disk and restart cannot disagree.
#[tokio::test]
async fn aborted_policy_put_still_commits_live_and_disk_together() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let unix = api::router(daemon.shared.clone(), true);
    let first = json!({"default_mode":"guard","allow_origins":[],
        "deny_origins":["https://aborted.example"],"rules":[]});
    let second = json!({"default_mode":"guard","allow_origins":[],
        "deny_origins":["https://later.example"],"rules":[]});
    let writer = daemon.shared.policy.writer.lock().await;
    let holders = Arc::strong_count(&daemon.shared.policy);
    let request = {
        let (router, body) = (unix.clone(), first.clone());
        tokio::spawn(async move { send(&router, "PUT", "/v1/policy", body, None).await })
    };
    // The transaction task holds its own handle on the policy once it exists.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while Arc::strong_count(&daemon.shared.policy) == holders {
        assert!(
            std::time::Instant::now() < deadline,
            "PUT never reached the writer lock"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    drop(writer);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let (_, live) = send(&unix, "GET", "/v1/policy", json!({}), None).await;
        if live == first {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "aborted PUT was never published"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let on_disk = |root: &std::path::Path| -> Value {
        serde_json::from_slice(&std::fs::read(root.join("data/policy.json")).unwrap()).unwrap()
    };
    assert_eq!(on_disk(root.path()), first);
    let (status, _) = send(&unix, "PUT", "/v1/policy", second.clone(), None).await;
    assert_eq!(status, StatusCode::OK);
    let (_, live) = send(&unix, "GET", "/v1/policy", json!({}), None).await;
    assert_eq!((live, on_disk(root.path())), (second.clone(), second));
    daemon.shutdown().await;
}

/// Origin entries are compiled with the browser's URL canonicalization, so case, default
/// ports, IDNA and trailing dots cannot make a deny entry miss; malformed entries are refused
/// instead of being stored as restrictions that match nothing.
#[tokio::test]
async fn origin_entries_share_browser_canonicalization() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let unix = api::router(daemon.shared.clone(), true);
    for malformed in [
        "https://user@pay.example",
        "https://pay.example/path",
        "https://pay.example?x",
        "https:\\\\pay.example",
        "https://a*.pay.example",
        "https://pay.*.example",
        "ftp://pay.example",
        "https://pay.example:99999",
        "pay.example",
    ] {
        let (status, body) = send(
            &unix,
            "PUT",
            "/v1/policy",
            json!({"default_mode":"guard","deny_origins":[malformed]}),
            None,
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::BAD_REQUEST, Some("invalid_policy")),
            "{malformed}"
        );
    }
    let (status, _) = send(
        &unix,
        "PUT",
        "/v1/policy",
        json!({"default_mode":"guard","deny_origins":[
            "https://PAY.example:443", "https://bücher.example", "http://*.Evil.TEST."]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for (target, effect) in [
        ("https://pay.example/checkout", "deny"),
        ("HTTPS://PAY.EXAMPLE.:443/", "deny"),
        ("https://good@pay.example/", "deny"),
        ("https:\\\\pay.example\\x", "deny"),
        ("https://xn--bcher-kva.example/", "deny"),
        ("http://a.b.evil.test/", "deny"),
        ("https://pay.example:8443/", "allow"),
        ("http://evil.test/", "allow"),
        ("https://pay.example.other.test/", "allow"),
    ] {
        let (status, decision) = send(
            &unix,
            "POST",
            "/v1/policy/test",
            json!({"target":target,"capability":"browse"}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(decision["effect"], effect, "{target}");
    }
    daemon.shutdown().await;
}

/// Releasing a secret re-evaluates the live policy. A deny added after the step opened
/// refuses the fill with 403 `policy_denied`, nothing reaches the page and the ticket stays
/// pending; the same request succeeds once the deny is lifted.
#[tokio::test]
async fn secret_release_rechecks_live_policy() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let filled = Arc::new(parking_lot::Mutex::new(0));
    *daemon.shared.registry.secret_sink.write() = Some(Arc::new(CanarySink {
        filled: filled.clone(),
        expected: "canary",
        mismatch: false,
    }));
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    opened(
        &daemon,
        &session,
        TicketKind::SecretInput,
        "secret",
        Utc::now() + chrono::Duration::minutes(5),
    )
    .await;
    let unix = api::router(daemon.shared.clone(), true);
    for deny in [
        json!({"default_mode":"guard","deny_origins":["https://EXAMPLE.test"]}),
        json!({"default_mode":"guard","allow_origins":["https://other.test"]}),
        json!({"default_mode":"guard","rules":[{"effect":"deny","capability":"vault_use","pattern":"https://example.test"}]}),
    ] {
        assert_eq!(
            send(&unix, "PUT", "/v1/policy", deny.clone(), None).await.0,
            StatusCode::OK
        );
        let (status, body) = send(
            &unix,
            "POST",
            "/v1/tickets/secret/resolve",
            json!({"value":"canary"}),
            None,
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("policy_denied")),
            "{deny}"
        );
        assert_eq!(*filled.lock(), 0);
        assert_eq!(
            daemon.shared.db.state(&session).await.unwrap().tickets[&TicketId::new("secret")]
                .status,
            TicketStatus::Pending
        );
    }
    assert_eq!(
        send(
            &unix,
            "PUT",
            "/v1/policy",
            json!({"default_mode":"guard"}),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, _) = send(
        &unix,
        "POST",
        "/v1/tickets/secret/resolve",
        json!({"value":"canary"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(*filled.lock(), 1);
    daemon.shutdown().await;
}

/// Opening a browser secret step is itself a Browse + VaultUse action on its origin, so a
/// denied origin or a read-only session cannot even open one.
#[tokio::test]
async fn secret_input_step_is_decided_by_policy_when_opened() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let unix = api::router(daemon.shared.clone(), true);
    let args = json!({"kind":"secret_input","summary":"Password","tab_id":"tab",
        "origin":"https://example.test","field_ref":"e1"});
    let read_only = create_session(&daemon, "readonly", PolicyMode::ReadOnly, None).await;
    assert!(matches!(
        host(&daemon, &read_only)
            .await
            .call(call("request_human_step", args.clone(), "s1"))
            .await,
        ToolOutcome::Denied { .. }
    ));
    let guard = create_session(&daemon, "guard", PolicyMode::Guard, None).await;
    assert_eq!(
        send(
            &unix,
            "PUT",
            "/v1/policy",
            json!({"default_mode":"guard","deny_origins":["https://example.test"]}),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(matches!(
        host(&daemon, &guard)
            .await
            .call(call("request_human_step", args, "s2"))
            .await,
        ToolOutcome::Denied { .. }
    ));
    assert!(daemon
        .shared
        .db
        .state(&guard)
        .await
        .unwrap()
        .tickets
        .is_empty());
    daemon.shutdown().await;
}

/// Tokens are minted only through the local socket. TCP callers are refused even with
/// `admin`, and a narrower TCP token gets the ordinary scope refusal.
#[tokio::test]
async fn token_minting_is_socket_only() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let request = json!({"name":"phone","scopes":["admin"]});
    let (status, minted) = send(&unix, "POST", "/v1/devices/pair", request.clone(), None).await;
    assert_eq!(status, StatusCode::CREATED);
    let admin = minted["token"].as_str().unwrap().to_owned();
    let (status, body) = send(
        &tcp,
        "POST",
        "/v1/devices/pair",
        request.clone(),
        Some(&admin),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("local_socket_required"))
    );
    let (_, browse) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Browse])
        .await
        .unwrap();
    let (status, body) = send(&tcp, "POST", "/v1/devices/pair", request, Some(&browse)).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("forbidden"))
    );
    // The admin TCP token still works for everything else.
    let (status, _) = send(&tcp, "GET", "/v1/policy", json!({}), Some(&admin)).await;
    assert_eq!(status, StatusCode::OK);
    daemon.shutdown().await;
}

/// A loopback server counting every request; `page` is served at `/`, `/redirect` answers 302
/// to `redirect_to`.
async fn counting_server(
    page: String,
    redirect_to: Option<String>,
) -> (
    String,
    Arc<std::sync::atomic::AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    use axum::{
        response::{Html, IntoResponse, Redirect},
        routing::get,
        Router,
    };
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = hits.clone();
    let router = Router::new()
        .route("/favicon.ico", get(|| async { StatusCode::NOT_FOUND }))
        .fallback(get(move |uri: axum::http::Uri| {
            let (page, redirect_to, counter) = (page.clone(), redirect_to.clone(), counter.clone());
            async move {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                match (uri.path(), redirect_to) {
                    ("/redirect", Some(to)) => Redirect::temporary(&to).into_response(),
                    _ => Html(page).into_response(),
                }
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (origin, hits, task)
}

/// The live origin policy guards every navigation of an agent-owned tab at the browser
/// controller, before any request leaves: a redirect on open, a redirect on navigate, a link
/// click, a form submission, a `target=_blank` link and `window.open`. The denied server sees
/// zero requests; lifting the deny (positive control) lets the same tab reach it.
#[tokio::test]
async fn denied_origin_is_unreachable_through_any_agent_navigation() {
    let root = tempfile::tempdir().unwrap();
    let mut config = resolved_config(root.path());
    match test_browser() {
        Ok(executable) => config.loaded.config.browser.executable = Some(executable),
        Err(error) => {
            eprintln!("SKIP navigation guard test: {error}");
            return;
        }
    }
    let (denied, denied_hits, denied_task) =
        counting_server("<!doctype html><title>collector</title>".into(), None).await;
    let page = format!(
        r#"<!doctype html><title>Site</title>
<a href="{denied}/link">Next link</a>
<form action="{denied}/form"><input name="q" aria-label="Query"><button>Go</button></form>
<a href="{denied}/blank" target="_blank">New tab link</a>
<button onclick="window.open('{denied}/window')">Open window</button>"#
    );
    let (site, _, site_task) = counting_server(page, Some(format!("{denied}/redirected"))).await;
    let runtime = TestRuntime::default();
    let daemon = Daemon::open_runtime(config, runtime.clone()).await.unwrap();
    let unix = api::router(daemon.shared.clone(), true);
    let deny = json!({"default_mode":"guard","deny_origins":[denied.clone()]});
    assert_eq!(
        send(&unix, "PUT", "/v1/policy", deny, None).await.0,
        StatusCode::OK
    );
    let session = create_session(&daemon, "session", PolicyMode::Full, None).await;
    let tool = |name: &'static str, args: Value, id: &'static str| {
        let (daemon, session) = (&daemon, &session);
        async move { host(daemon, session).await.call(call(name, args, id)).await }
    };

    // Allowed URL whose first response redirects to the denied origin.
    let outcome = tool(
        "browser_open_tab",
        json!({"url":format!("{site}/redirect")}),
        "open-redirect",
    )
    .await;
    assert!(!matches!(outcome, ToolOutcome::Done { .. }), "{outcome:?}");
    let ToolOutcome::Done { for_model_json } = tool(
        "browser_open_tab",
        json!({"url":format!("{site}/")}),
        "open",
    )
    .await
    else {
        panic!("allowed page must open");
    };
    let tab = serde_json::from_str::<Value>(&for_model_json).unwrap()["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|e| e.0)
        .unwrap();
    let pid = browser.process_id();
    let _cleanup = scopeguard(pid);
    let outcome = tool(
        "browser_navigate",
        json!({"tab_id":tab,"url":format!("{site}/redirect")}),
        "navigate-redirect",
    )
    .await;
    assert!(!matches!(outcome, ToolOutcome::Done { .. }), "{outcome:?}");

    // Phase 1 (denied): no click reaches the collector. Phase 2 (positive control, deny lifted
    // live without a restart): each same click does reach it, so phase 1 proves the guard and
    // not a click that never happened.
    let mut expected = 0;
    for denied_phase in [true, false] {
        if !denied_phase {
            assert_eq!(
                send(
                    &unix,
                    "PUT",
                    "/v1/policy",
                    json!({"default_mode":"guard"}),
                    None
                )
                .await
                .0,
                StatusCode::OK
            );
        }
        for (label, id) in [
            ("Next link", "click-link"),
            ("Go", "submit-form"),
            ("New tab link", "click-blank"),
            ("Open window", "click-window-open"),
        ] {
            // Every click starts from the allowed page with fresh refs.
            assert!(matches!(
                tool(
                    "browser_navigate",
                    json!({"tab_id":tab,"url":format!("{site}/")}),
                    "reset"
                )
                .await,
                ToolOutcome::Done { .. }
            ));
            let snapshot = browser.snapshot(&tab).await.unwrap();
            let target = snapshot
                .nodes
                .iter()
                .find(|node| node.name == label && node.reference.is_some())
                .and_then(|node| node.reference.clone())
                .unwrap_or_else(|| panic!("no ref for {label}: {}", snapshot.text));
            let _ = tool(
                "browser_act",
                json!({"tab_id":tab,"target":target,"action":"click"}),
                id,
            )
            .await;
            if denied_phase {
                // Give a navigation that slipped through time to arrive.
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            } else {
                expected += 1;
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
                while denied_hits.load(std::sync::atomic::Ordering::SeqCst) < expected
                    && tokio::time::Instant::now() < deadline
                {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
            assert_eq!(
                denied_hits.load(std::sync::atomic::Ordering::SeqCst),
                expected,
                "{label} (denied: {denied_phase})"
            );
        }
        if denied_phase {
            // The popups exist but never loaded the denied document.
            assert!(browser
                .tabs()
                .await
                .iter()
                .all(|tab| !tab.url.starts_with(&denied)));
        }
    }
    drop(browser);
    daemon.shutdown().await;
    site_task.abort();
    denied_task.abort();
}

/// An approval admits the `Ask` origin only while the approved call runs: returning to an
/// allowed page and clicking into that origin needs a fresh approval, and an origin that was
/// merely Allow when a call was approved is not exempt from a later change to Ask.
#[tokio::test]
async fn approved_ask_navigation_does_not_outlive_the_approved_call() {
    use std::sync::atomic::Ordering::SeqCst;
    let root = tempfile::tempdir().unwrap();
    let mut config = resolved_config(root.path());
    match test_browser() {
        Ok(executable) => config.loaded.config.browser.executable = Some(executable),
        Err(error) => {
            eprintln!("SKIP approval lifetime navigation test: {error}");
            return;
        }
    }
    let (asked, asked_hits, asked_task) =
        counting_server("<!doctype html><title>asked</title>".into(), None).await;
    let page = format!(r#"<!doctype html><title>Site</title><a href="{asked}/link">Ask link</a>"#);
    let (site, _, site_task) = counting_server(page, None).await;
    let runtime = TestRuntime::default();
    let daemon = Daemon::open_runtime(config, runtime.clone()).await.unwrap();
    let unix = api::router(daemon.shared.clone(), true);
    let ask = json!({"default_mode":"guard","rules":[
        {"effect":"ask","capability":"browse","pattern":asked.clone()}]});
    let allow = json!({"default_mode":"guard"});
    assert_eq!(
        send(&unix, "PUT", "/v1/policy", ask.clone(), None).await.0,
        StatusCode::OK
    );
    let session = create_session(&daemon, "session", PolicyMode::Full, None).await;
    let tool = |name: &'static str, args: Value, id: &'static str| {
        let (daemon, session) = (&daemon, &session);
        async move { host(daemon, session).await.call(call(name, args, id)).await }
    };
    let ToolOutcome::Done { for_model_json } = tool(
        "browser_open_tab",
        json!({"url":format!("{site}/")}),
        "open",
    )
    .await
    else {
        panic!("allowed page must open");
    };
    let tab = serde_json::from_str::<Value>(&for_model_json).unwrap()["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|e| e.0)
        .unwrap();
    let _cleanup = scopeguard(browser.process_id());

    // Approve one navigation to the Ask origin: it loads, once.
    let navigate = json!({"tab_id":tab,"url":format!("{asked}/")});
    let ToolOutcome::Deferred { ticket, .. } =
        tool("browser_navigate", navigate.clone(), "nav-ask").await
    else {
        panic!("Ask origin must defer");
    };
    assert_eq!(asked_hits.load(SeqCst), 0);
    assert_eq!(
        person_resolve(&daemon, &ticket, json!(true)).await.status(),
        StatusCode::OK
    );
    let mut approved = call("browser_navigate", navigate, "nav-ask");
    approved.approval_ticket = Some(ticket);
    let outcome = host(&daemon, &session).await.call(approved).await;
    assert!(matches!(outcome, ToolOutcome::Done { .. }), "{outcome:?}");
    assert_eq!(asked_hits.load(SeqCst), 1);

    let click_link = |id: &'static str| {
        let (tab, browser, site, tool) = (tab.clone(), browser.clone(), site.clone(), &tool);
        async move {
            let reset = tool(
                "browser_navigate",
                json!({"tab_id":tab,"url":format!("{site}/")}),
                "reset",
            )
            .await;
            assert!(matches!(reset, ToolOutcome::Done { .. }), "{reset:?}");
            let snapshot = browser.snapshot(&tab).await.unwrap();
            let target = snapshot
                .nodes
                .iter()
                .find(|node| node.name == "Ask link" && node.reference.is_some())
                .and_then(|node| node.reference.clone())
                .unwrap_or_else(|| panic!("no ref: {}", snapshot.text));
            let _ = tool(
                "browser_act",
                json!({"tab_id":tab,"target":target,"action":"click"}),
                id,
            )
            .await;
            tokio::time::sleep(Duration::from_millis(1000)).await;
        }
    };
    // Back on the allowed page, a click into the Ask origin gets no leftover approval.
    click_link("click-after-approval").await;
    assert_eq!(
        asked_hits.load(SeqCst),
        1,
        "approval must not outlive the approved call"
    );

    // Origin is plain Allow: the same click reaches it (positive control)...
    assert_eq!(
        send(&unix, "PUT", "/v1/policy", allow, None).await.0,
        StatusCode::OK
    );
    click_link("click-allowed").await;
    assert_eq!(asked_hits.load(SeqCst), 2);
    // ...and once it turns Ask again, no earlier approval or visit exempts it.
    assert_eq!(
        send(&unix, "PUT", "/v1/policy", ask, None).await.0,
        StatusCode::OK
    );
    click_link("click-asked-again").await;
    assert_eq!(asked_hits.load(SeqCst), 2);
    drop(browser);
    daemon.shutdown().await;
    site_task.abort();
    asked_task.abort();
}

/// An Ask browser call opens its approval ticket under `ticket_gate`; it must give up the
/// browser gate first, or an answer holding `ticket_gate` and waiting for the browser gate
/// (the documented order) deadlocks with it.
#[tokio::test]
async fn an_ask_browser_call_waiting_for_the_ticket_gate_does_not_hold_the_browser_gate() {
    let root = tempfile::tempdir().unwrap();
    let daemon = Daemon::open_runtime(resolved_config(root.path()), TestRuntime::default())
        .await
        .unwrap();
    let unix = api::router(daemon.shared.clone(), true);
    let ask = json!({"default_mode":"guard","rules":[
        {"effect":"ask","capability":"browse","pattern":"https://asked.example"}]});
    assert_eq!(
        send(&unix, "PUT", "/v1/policy", ask, None).await.0,
        StatusCode::OK
    );
    let session = create_session(&daemon, "session", PolicyMode::Full, None).await;
    let ticket_gate = daemon.shared.ticket_gate.lock().await;
    let host = host(&daemon, &session).await;
    let asking = host.call(call(
        "browser_open_tab",
        json!({"url":"https://asked.example/"}),
        "ask-open",
    ));
    let shared = &daemon.shared;
    let answer = async move {
        // Let the call decide Ask and reach the ticket gate.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let browser_gate =
            tokio::time::timeout(Duration::from_secs(5), shared.registry.browser.gate.lock())
                .await
                .expect("the Ask call held the browser gate while waiting for ticket_gate");
        drop(browser_gate);
        drop(ticket_gate);
    };
    let (outcome, ()) = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(asking, answer)
    })
    .await
    .expect("deadlock");
    assert!(
        matches!(outcome, ToolOutcome::Deferred { .. }),
        "{outcome:?}"
    );
    daemon.shutdown().await;
}

/// A ticket's target strings are published verbatim only when the daemon checked them
/// against the live browser. With no browser (and for a non-secret step), a caller-supplied
/// tab id, origin and field ref are redacted like any other text.
#[tokio::test]
async fn unchecked_ticket_targets_are_redacted_like_any_caller_text() {
    const CANARY: &str = "s2-canary-7f3a9c";
    let root = tempfile::tempdir().unwrap();
    let mut config = resolved_config(root.path());
    config.loaded.config.vault.unlock = openside_config::VaultUnlockSetting::Passphrase;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let unix = api::router(daemon.shared.clone(), true);
    let (status, _) = send(
        &unix,
        "POST",
        "/v1/vault/setup",
        json!({"passphrase":"correct horse battery"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = send(
        &unix,
        "POST",
        "/v1/vault/items",
        json!({"id":"canary","origins":["https://canary.example"],"password":CANARY}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let session = create_session(&daemon, "session", PolicyMode::Full, None).await;
    let outcome = host(&daemon, &session)
        .await
        .call(call(
            "request_human_step",
            json!({"kind":"info","summary":"Check the page","tab_id":CANARY,
                "origin":format!("https://{CANARY}.example"),"field_ref":CANARY}),
            "unchecked",
        ))
        .await;
    let ToolOutcome::Deferred { ticket, .. } = outcome else {
        panic!("expected a ticket: {outcome:?}");
    };
    let (_, published) = send(
        &unix,
        "GET",
        &format!("/v1/tickets/{ticket}"),
        json!({}),
        None,
    )
    .await;
    let target = &published["target"];
    assert!(target.is_object(), "{published}");
    assert!(!target.to_string().contains(CANARY), "{target}");
    let state = daemon.shared.db.state(&session).await.unwrap();
    let state = serde_json::to_string(&state.tickets).unwrap();
    assert!(
        !state.contains(CANARY),
        "the event log kept an unchecked target"
    );
    daemon.shutdown().await;
}

/// Kills the browser process group if the test panics before shutdown.
fn scopeguard(pid: u32) -> impl Drop {
    struct Kill(u32);
    impl Drop for Kill {
        fn drop(&mut self) {
            #[cfg(unix)]
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(i32::try_from(self.0).unwrap()),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
    }
    Kill(pid)
}
