use super::*;
use openside_core::{CredentialAuthKind, CredentialRequest, CredentialRuleOutcome};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A fake relay whose `/health` answers `status` (with `location`, if given) and requires the
/// identity header; counts hits.
async fn relay_with(status: StatusCode, location: Option<String>) -> (String, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let app = axum::Router::new().route(
        "/health",
        axum::routing::get(move |headers: axum::http::HeaderMap| {
            let counter = counter.clone();
            let location = location.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                assert_eq!(headers["x-api-key"], "alice");
                let mut response = axum::response::Response::new(axum::body::Body::empty());
                *response.status_mut() = status;
                if let Some(location) = location {
                    response
                        .headers_mut()
                        .insert(axum::http::header::LOCATION, location.parse().unwrap());
                }
                response
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, hits)
}

async fn relay(status: StatusCode) -> (String, Arc<AtomicUsize>) {
    relay_with(status, None).await
}

fn manifest(url: &str) -> String {
    format!(
        r#"
id = "example-relay"
url = "{url}"
identity = "alice"

[providers.anthropic]
auth = "claude_code"

[providers.openai-codex]
path = "/backend"
auth = "codex_cli"

[[session_headers]]
name = "x-example-task"
template = "openside:{{session}}"

[probe]
path = "/health"
identity_header = "x-api-key"
timeout_ms = 500

[errors]
hint_header = "x-example-hint"
[[errors.rules]]
status = 429
hint = "unsupported-client"
outcome = "unsupported_client"
"#
    )
}

async fn open(root: &Path, url: &str) -> Daemon {
    open_with_mode(root, url, 0o600).await
}

async fn open_with_mode(root: &Path, url: &str, mode: u32) -> Daemon {
    let mut config = resolved_config(root);
    config.loaded.config.models.default = Some("anthropic:test-ready".into());
    config.loaded.config.credentials = json!({"env":{"anthropic":[],"openai-codex":[]}});
    #[cfg(unix)]
    {
        let index = json!({"providers":[{"provider":"anthropic","models":[{"id":"anthropic:test-ready","label":"test-ready","recommended_for":"reasoning","in_catalog":true,"tier":"reasoning","strengths":"coding"}]}]});
        config.argv = vec!["sh".into(), "-c".into(), format!("printf '%s' '{}'", index)];
    }
    let dir = openside_config::plugin_dir(&config.loaded.paths.config_file);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("example.toml");
    std::fs::write(&file, manifest(url)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap()
}

async fn session(daemon: &Daemon, name: &str, model: &'static str) -> SessionId {
    let id = SessionId::new(name);
    let stored = id.clone();
    daemon
        .shared
        .db
        .store(move |store| {
            store.create_session(stored, "Test", model, PolicyMode::Guard, None)?;
            Ok(())
        })
        .await
        .unwrap();
    id
}

fn acquire(provider: &str, model: &str) -> CredentialRequest {
    CredentialRequest {
        request_id: "plugin-1".into(),
        provider: provider.into(),
        model: model.into(),
        attempt: 0,
        subagent: None,
    }
}

async fn get(router: &axum::Router, path: &str) -> String {
    let response = router
        .clone()
        .oneshot(request("GET", path, Value::Null, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{path}");
    String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

/// The catalog entry for `anthropic:test-ready`, wherever the settings reply nests it.
async fn find_model(router: &axum::Router) -> Value {
    fn walk(value: &Value) -> Option<Value> {
        match value {
            Value::Object(map)
                if map.get("id") == Some(&json!("anthropic:test-ready"))
                    && map.contains_key("available") =>
            {
                Some(value.clone())
            }
            Value::Object(map) => map.values().find_map(walk),
            Value::Array(items) => items.iter().find_map(walk),
            _ => None,
        }
    }
    walk(&serde_json::from_str(&get(router, "/v1/settings/models").await).unwrap()).unwrap()
}

#[tokio::test]
async fn reachable_plugin_source_joins_pools_invisibly_with_private_lease_fields() {
    let root = tempfile::tempdir().unwrap();
    let (url, hits) = relay(StatusCode::OK).await;
    let daemon = open(root.path(), &url).await;
    let local = api::router(daemon.shared.clone(), true);
    let id = session(&daemon, "plugin-session", "anthropic:test-ready").await;
    let h = host(&daemon, &id).await;

    let lease = h
        .credentials(acquire("anthropic", "anthropic:test-ready"))
        .await
        .unwrap();
    assert_eq!(lease.account.source, "plugin");
    assert_eq!(lease.account.account_id, "example-relay");
    assert_eq!(lease.auth.kind, CredentialAuthKind::ClaudeCode);
    assert_eq!(lease.auth.value, "alice");
    assert_eq!(lease.auth.upstream_account_id, None);
    assert_eq!(lease.auth.base_url.as_deref(), Some(url.as_str()));
    assert_eq!(
        lease.auth.headers.as_ref().unwrap()["x-example-task"],
        "openside:plugin-session"
    );
    let rules = lease.auth.error_rules.as_ref().unwrap();
    assert_eq!(rules.hint_header.as_deref(), Some("x-example-hint"));
    assert_eq!(
        rules.rules[0].outcome,
        CredentialRuleOutcome::UnsupportedClient
    );

    let codex_session = session(&daemon, "plugin-codex", "openai-codex:test-codex").await;
    let codex = host(&daemon, &codex_session)
        .await
        .credentials(acquire("openai-codex", "openai-codex:test-codex"))
        .await
        .unwrap();
    assert_eq!(codex.auth.kind, CredentialAuthKind::CodexCli);
    assert_eq!(
        codex.auth.base_url.as_deref(),
        Some(format!("{url}/backend").as_str())
    );

    // Invisible: no pool, catalog row, diagnostics row or config entry names the source.
    for path in ["/v1/settings/pools", "/v1/settings/models", "/v1/accounts"] {
        let body = get(&local, path).await;
        assert!(!body.contains("example-relay"), "{path}: {body}");
        assert!(!body.contains(&url), "{path}");
        assert!(!body.contains("alice"), "{path}");
    }
    let model = find_model(&local).await;
    assert_eq!(model["available"], true);
    assert_eq!(model["reason"], "Ready");
    assert!(model["source"].is_null() && model["account_id"].is_null());
    let stored =
        std::fs::read_to_string(&daemon.shared.config.loaded.paths.config_file).unwrap_or_default();
    assert!(!stored.contains("example-relay"));

    // A person cannot pin the hidden account.
    let response = local
        .clone()
        .oneshot(request(
            "PUT",
            "/v1/settings/pools/anthropic",
            json!({"accounts":[{"source":"plugin","account_id":"example-relay"}]}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    // Probes are cached between requests.
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    daemon.shutdown().await;
}

#[tokio::test]
async fn unreachable_plugin_source_never_leases_or_appears() {
    let root = tempfile::tempdir().unwrap();
    let (url, hits) = relay(StatusCode::NOT_FOUND).await;
    let daemon = open(root.path(), &url).await;
    let local = api::router(daemon.shared.clone(), true);
    let id = session(&daemon, "plugin-offline", "anthropic:test-ready").await;
    let h = host(&daemon, &id).await;
    assert!(h
        .credentials(acquire("anthropic", "anthropic:test-ready"))
        .await
        .is_err());
    assert_eq!(find_model(&local).await["available"], false);
    assert!(hits.load(Ordering::SeqCst) >= 1);
    daemon.shutdown().await;
}

#[tokio::test]
async fn probe_never_follows_a_redirect_with_the_identity() {
    let root = tempfile::tempdir().unwrap();
    let (elsewhere, elsewhere_hits) = relay(StatusCode::OK).await;
    let (url, hits) = relay_with(StatusCode::FOUND, Some(format!("{elsewhere}/health"))).await;
    let daemon = open(root.path(), &url).await;
    let id = session(&daemon, "plugin-redirect", "anthropic:test-ready").await;
    assert!(host(&daemon, &id)
        .await
        .credentials(acquire("anthropic", "anthropic:test-ready"))
        .await
        .is_err());
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(elsewhere_hits.load(Ordering::SeqCst), 0);
    daemon.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn group_writable_manifest_is_ignored() {
    let root = tempfile::tempdir().unwrap();
    let (url, hits) = relay(StatusCode::OK).await;
    let daemon = open_with_mode(root.path(), &url, 0o660).await;
    let id = session(&daemon, "plugin-untrusted", "anthropic:test-ready").await;
    assert!(host(&daemon, &id)
        .await
        .credentials(acquire("anthropic", "anthropic:test-ready"))
        .await
        .is_err());
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    daemon.shutdown().await;
}
