use super::*;
use crate::engine::Action as SessionAction;
use openside_core::{
    CredentialAction, CredentialErrorCode, CredentialOutcome, CredentialReport, CredentialRequest,
    CredentialScope, CredentialSnapshot, CredentialUsageWindow, HostToolHandler, ModelAccountRef,
};
use openside_proto::DeviceKind;
use openside_vault::{PassphraseLock, PlatformLock, Secret, Vault};

const KEY: &str = "model-vault-key-canary-1209abcd";
const SECOND: &str = "model-vault-second-canary-7654dcba";

async fn call(
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
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        },
    )
}
async fn fixture(root: &Path) -> (Daemon, TestRuntime) {
    fixture_sources(root, None).await
}
async fn fixture_sources(root: &Path, credentials: Option<Value>) -> (Daemon, TestRuntime) {
    let mut config = resolved_config(root);
    config.loaded.config.vault.path = Some(root.join("model-vault"));
    config.loaded.config.models.default = Some("anthropic:test-ready".into());
    config.loaded.config.credentials = json!({"order":{"anthropic":["vault"],"openai":["vault"],"google":["vault"]},"env":{"anthropic":[],"openai":[],"google":[],"openai-codex":[],"github-copilot":[]}});
    if let Some(credentials) = credentials {
        config.loaded.config.credentials = credentials;
    }
    #[cfg(unix)]
    {
        let index = json!({"providers":[{"provider":"anthropic","models":[{"id":"anthropic:test-ready","label":"test-ready"},{"id":"anthropic:test-second","label":"test-second"}]},{"provider":"openai","models":[{"id":"openai:test-unavailable","label":"test-unavailable"}]},{"provider":"google","models":[]} ]});
        config.argv = vec!["sh".into(), "-c".into(), format!("printf '%s' '{}'", index)];
    }
    let runtime = TestRuntime::default();
    let daemon = Daemon::open_runtime(config, runtime.clone()).await.unwrap();
    let vault = Vault::create(
        root.join("model-vault"),
        root.join("model-vault-audit.jsonl"),
        &Secret::new("fixture-provider-passphrase".into()),
        Box::new(PassphraseLock::default()) as Box<dyn PlatformLock + Send>,
    )
    .unwrap();
    *daemon.shared.registry.vault.vault.lock().await = Some(vault);
    (daemon, runtime)
}
async fn add(router: &axum::Router, key: &str, label: &str) -> String {
    let (status, value) = call(
        router,
        "POST",
        "/v1/settings/keys/anthropic",
        json!({"key":key,"label":label}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "key creation failed");
    value["key_id"].as_str().unwrap().into()
}
fn account(id: &str) -> ModelAccountRef {
    ModelAccountRef {
        source: "vault".into(),
        account_id: id.into(),
    }
}
async fn pool(router: &axum::Router, ids: &[&str]) {
    let references: Vec<_> = ids.iter().map(|id| account(id)).collect();
    assert_eq!(
        call(
            router,
            "PUT",
            "/v1/settings/pools/anthropic",
            json!({"accounts":references}),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
}
async fn model_session(daemon: &Daemon, name: &str) -> SessionId {
    let id = SessionId::new(name);
    let stored = id.clone();
    daemon
        .shared
        .db
        .store(move |store| {
            store.create_session(
                stored,
                "Test",
                "anthropic:test-ready",
                PolicyMode::Guard,
                None,
            )?;
            Ok(())
        })
        .await
        .unwrap();
    id
}
fn acquire(id: &str, attempt: u32) -> CredentialRequest {
    CredentialRequest {
        request_id: id.into(),
        provider: "anthropic".into(),
        model: "anthropic:test-ready".into(),
        attempt,
        subagent: None,
    }
}
fn report(
    lease: &CredentialSnapshot,
    outcome: CredentialOutcome,
    stream_started: bool,
) -> CredentialReport {
    CredentialReport {
        request_id: lease.request_id.clone(),
        lease_id: lease.lease_id.clone(),
        outcome,
        stream_started,
        retry_after_ms: Some(60_000),
        scope: CredentialScope::Account,
        usage: None,
        usage_windows: vec![],
    }
}

#[tokio::test]
async fn model_keys_require_admin_and_person_and_never_echo_or_log_values() {
    let root = tempfile::tempdir().unwrap();
    let (daemon, _) = fixture(root.path()).await;
    let tcp = api::router(daemon.shared.clone(), false);
    let local = api::router(daemon.shared.clone(), true);
    let (_, read) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Read])
        .await
        .unwrap();
    let (_, admin) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Admin])
        .await
        .unwrap();
    let (_, agent) = daemon
        .shared
        .db
        .pair_as(
            "Agent".into(),
            vec![CapabilityScope::Admin],
            DeviceKind::AgentClient,
        )
        .await
        .unwrap();
    for (method, path, body) in [
        ("GET", "/v1/settings/keys", json!({})),
        (
            "POST",
            "/v1/settings/keys/anthropic",
            json!({"key":KEY,"label":"Work"}),
        ),
        (
            "PUT",
            "/v1/settings/keys/anthropic/missing",
            json!({"key":KEY}),
        ),
        (
            "PATCH",
            "/v1/settings/keys/anthropic/missing",
            json!({"enabled":false}),
        ),
        ("DELETE", "/v1/settings/keys/anthropic/missing", json!({})),
        (
            "PUT",
            "/v1/settings/models",
            json!({"default_model":"anthropic:test-ready","fallback_models":[]}),
        ),
        ("GET", "/v1/settings/subscriptions", json!({})),
        (
            "POST",
            "/v1/settings/subscriptions/claude-code",
            json!({"label":"Work"}),
        ),
        (
            "PATCH",
            "/v1/settings/subscriptions/claude-code/default",
            json!({"enabled":true}),
        ),
        (
            "PUT",
            "/v1/settings/pools/anthropic",
            json!({"accounts":[]}),
        ),
    ] {
        let (status, value) = call(&tcp, method, path, body, Some(&read)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(!value.to_string().contains(KEY));
    }
    for (method, path, body) in [
        (
            "POST",
            "/v1/settings/keys/anthropic",
            json!({"key":KEY,"label":"Work"}),
        ),
        (
            "PUT",
            "/v1/settings/keys/anthropic/missing",
            json!({"key":KEY}),
        ),
        (
            "PATCH",
            "/v1/settings/keys/anthropic/missing",
            json!({"enabled":false}),
        ),
        ("DELETE", "/v1/settings/keys/anthropic/missing", json!({})),
        (
            "POST",
            "/v1/settings/subscriptions/codex-cli",
            json!({"label":"Work"}),
        ),
        (
            "PUT",
            "/v1/settings/pools/anthropic",
            json!({"accounts":[]}),
        ),
    ] {
        let (status, value) = call(&tcp, method, path, body, Some(&agent)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(value["code"], "person_device_required");
    }
    let (status, value) = call(
        &tcp,
        "POST",
        "/v1/settings/keys/anthropic",
        json!({"key":KEY,"label":"Work"}),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(value["last_four"], "abcd");
    assert!(value["created_at"].as_str().is_some());
    assert!(!value.to_string().contains(KEY));
    let id = value["key_id"].as_str().unwrap();
    let second = add(&local, SECOND, "Personal").await;
    let (_, keys) = call(&tcp, "GET", "/v1/settings/keys", json!({}), Some(&admin)).await;
    assert_eq!(keys["keys"].as_array().unwrap().len(), 2);
    assert!(!keys.to_string().contains(KEY));
    assert!(!keys.to_string().contains(SECOND));
    for method in ["DELETE", "PATCH"] {
        assert_eq!(
            call(
                &local,
                method,
                &format!("/v1/vault/items/provider:anthropic:{id}"),
                json!({"policy":"always"}),
                None
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        call(
            &tcp,
            "DELETE",
            &format!("/v1/settings/keys/anthropic/{id}"),
            json!({}),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let (_, keys) = call(&local, "GET", "/v1/settings/keys", json!({}), None).await;
    assert_eq!(keys["keys"][0]["key_id"], second);
    for file in [
        root.path().join("model-vault"),
        root.path().join("model-vault-audit.jsonl"),
        root.path().join("data/audit.jsonl"),
    ] {
        let bytes = std::fs::read(file).unwrap_or_default();
        assert!(!String::from_utf8_lossy(&bytes).contains(KEY));
        assert!(!String::from_utf8_lossy(&bytes).contains(SECOND));
    }
    daemon.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn model_catalog_groups_accounts_and_model_selection_validates_availability() {
    let root = tempfile::tempdir().unwrap();
    let (daemon, runtime) = fixture(root.path()).await;
    let local = api::router(daemon.shared.clone(), true);
    let id = add(&local, KEY, "Work").await;
    let (status, catalog) = call(&local, "GET", "/v1/settings/models", json!({}), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(catalog["providers"][0]["provider"], "anthropic");
    assert_eq!(
        catalog["providers"][0]["pool"]["accounts"][0]["account_id"],
        id
    );
    assert_eq!(catalog["providers"][0]["models"][0]["available"], true);
    assert_eq!(catalog["providers"][2]["models"][0]["available"], false);
    assert!(!catalog.to_string().contains(KEY));
    assert!(catalog.get("subscriptions").is_none());
    for model in ["anthropic:unknown", "openai:test-unavailable"] {
        let (status, body) = call(
            &local,
            "PUT",
            "/v1/settings/models",
            json!({"default_model":model,"fallback_models":[]}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "model_unavailable");
    }
    assert_eq!(call(&local,"PUT","/v1/settings/models",json!({"default_model":"anthropic:test-ready","fallback_models":["anthropic:test-ready"]}),None).await.0,StatusCode::BAD_REQUEST);
    let (status, value) = call(
        &local,
        "PUT",
        "/v1/settings/models",
        json!({"default_model":"anthropic:test-second","fallback_models":["anthropic:test-ready"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["restart_required"], false);
    assert_eq!(value["effective"], "new_sessions_and_home_next_turn");
    let (_, session) = call(&local, "POST", "/v1/sessions", json!({}), None).await;
    assert_eq!(session["model"], "anthropic:test-second");
    assert!(runtime
        .0
        .lock()
        .opened
        .iter()
        .all(|spec| !spec.credentials_json.contains(KEY)));
    let config = std::fs::read_to_string(root.path().join("config.toml")).unwrap();
    assert!(config.contains("anthropic:test-second"));
    assert!(!config.contains(KEY));
    assert_eq!(
        call(&local, "POST", "/v1/vault/lock", json!({}), None)
            .await
            .0,
        StatusCode::OK
    );
    let (_, locked) = call(&local, "GET", "/v1/settings/models", json!({}), None).await;
    assert_eq!(locked["providers"][0]["models"][0]["available"], false);
    daemon.shutdown().await;
}

#[tokio::test]
async fn per_request_vault_boundary_rotates_sticks_and_observes_replace_delete_disable() {
    let root = tempfile::tempdir().unwrap();
    let (daemon, _) = fixture(root.path()).await;
    let local = api::router(daemon.shared.clone(), true);
    let first = add(&local, KEY, "Work").await;
    let second = add(&local, SECOND, "Personal").await;
    pool(&local, &[&first, &second]).await;
    let session = model_session(&daemon, "pool").await;
    let h = host(&daemon, &session).await;
    let one = h.credentials(acquire("logical-1", 0)).await.unwrap();
    assert!(one.auth.value == KEY);
    assert_eq!(one.account.account_id, first);
    assert!(!format!("{one:?}").contains(KEY));
    let blocked = report(&one, CredentialOutcome::RateLimit, false);
    let repeated = report(&one, CredentialOutcome::RateLimit, false);
    assert_eq!(
        h.credential_report(blocked).await.unwrap().action,
        CredentialAction::Rotate
    );
    assert_eq!(
        h.credential_report(repeated).await.unwrap().action,
        CredentialAction::Rotate
    );
    let two = h.credentials(acquire("logical-1", 1)).await.unwrap();
    assert!(two.auth.value == SECOND);
    assert_eq!(two.account.account_id, second);
    assert_eq!(
        h.credential_report(report(&two, CredentialOutcome::Success, false))
            .await
            .unwrap()
            .action,
        CredentialAction::Complete
    );
    let sticky = h.credentials(acquire("logical-2", 0)).await.unwrap();
    assert_eq!(sticky.account.account_id, second);
    h.credential_report(report(&sticky, CredentialOutcome::Success, false))
        .await
        .unwrap();
    assert_eq!(
        call(
            &local,
            "PUT",
            &format!("/v1/settings/keys/anthropic/{second}"),
            json!({"key":"replacement-vault-canary-1234"}),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    let fresh = h.credentials(acquire("logical-3", 0)).await.unwrap();
    assert!(fresh.auth.value == "replacement-vault-canary-1234");
    h.credential_report(report(&fresh, CredentialOutcome::Success, false))
        .await
        .unwrap();
    assert_eq!(
        call(
            &local,
            "PATCH",
            &format!("/v1/settings/keys/anthropic/{second}"),
            json!({"enabled":false}),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(h.credentials(acquire("logical-4", 0)).await.is_err());
    assert_eq!(
        call(
            &local,
            "DELETE",
            &format!("/v1/settings/keys/anthropic/{first}"),
            json!({}),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert!(h.credentials(acquire("logical-5", 0)).await.is_err());
    daemon.shutdown().await;
}

#[tokio::test]
async fn account_pool_restriction_stop_after_stream_and_report_authority() {
    let root = tempfile::tempdir().unwrap();
    let (daemon, _) = fixture(root.path()).await;
    let local = api::router(daemon.shared.clone(), true);
    let first = add(&local, KEY, "Work").await;
    let second = add(&local, SECOND, "Personal").await;
    pool(&local, &[&first, &second]).await;
    let mut chief = daemon.shared.agents.get("chief").unwrap().unwrap();
    chief.account_pool = Some(vec![account(&first)]);
    daemon
        .shared
        .agents
        .put(&chief, "test-restriction")
        .unwrap();
    let (_, created) = call(
        &local,
        "POST",
        "/v1/sessions",
        json!({"agent_id":"chief"}),
        None,
    )
    .await;
    let session = SessionId::new(created["id"].as_str().unwrap());
    let h = host(&daemon, &session).await;
    let lease = h.credentials(acquire("restricted", 0)).await.unwrap();
    assert_eq!(
        h.credential_report(report(&lease, CredentialOutcome::RateLimit, false))
            .await
            .unwrap()
            .action,
        CredentialAction::Stop
    );
    assert_eq!(
        call(
            &local,
            "PUT",
            &format!("/v1/sessions/{session}/account"),
            json!({"account":account(&second)}),
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let other = model_session(&daemon, "other").await;
    let other_host = host(&daemon, &other).await;
    assert_eq!(
        other_host
            .credential_report(report(&lease, CredentialOutcome::RateLimit, false))
            .await
            .unwrap_err()
            .code,
        CredentialErrorCode::InvalidCredentialRequest
    );
    let streamed = other_host
        .credentials(acquire("streamed", 0))
        .await
        .unwrap();
    assert_eq!(
        other_host
            .credential_report(report(&streamed, CredentialOutcome::RateLimit, true))
            .await
            .unwrap()
            .action,
        CredentialAction::Stop
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn stale_report_cannot_block_replacement_and_usage_reserves_are_attributed() {
    let root = tempfile::tempdir().unwrap();
    let (daemon, _) = fixture(root.path()).await;
    let local = api::router(daemon.shared.clone(), true);
    let first = add(&local, KEY, "Work").await;
    let second = add(&local, SECOND, "Personal").await;
    pool(&local, &[&first, &second]).await;
    let session = model_session(&daemon, "usage").await;
    let h = host(&daemon, &session).await;
    let old = h.credentials(acquire("old", 0)).await.unwrap();
    call(
        &local,
        "PUT",
        &format!("/v1/settings/keys/anthropic/{first}"),
        json!({"key":"new-vault-key-canary-3333"}),
        None,
    )
    .await;
    let ack = h
        .credential_report(report(&old, CredentialOutcome::Auth, false))
        .await
        .unwrap();
    assert_eq!(ack.reason.as_deref(), Some("Stale lease effects ignored"));
    let new = h.credentials(acquire("usage", 0)).await.unwrap();
    assert_eq!(new.account.account_id, first);
    assert!(new.auth.value == "new-vault-key-canary-3333");
    let mut measured = report(&new, CredentialOutcome::Success, false);
    measured.usage_windows = vec![CredentialUsageWindow {
        name: "5h".into(),
        used_percent: Some(95.0),
        reset_at_ms: Some((chrono::Utc::now().timestamp_millis() + 60_000) as f64),
    }];
    h.credential_report(measured).await.unwrap();
    let reserve = h.credentials(acquire("reserve", 0)).await.unwrap();
    assert_eq!(reserve.account.account_id, second);
    h.credential_report(report(&reserve, CredentialOutcome::Success, false))
        .await
        .unwrap();
    daemon.shutdown().await;
}

#[tokio::test]
async fn credential_changes_refresh_a_deterministically_paused_startup() {
    for replace in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (daemon, runtime) = fixture(root.path()).await;
        let local = api::router(daemon.shared.clone(), true);
        let id = add(&local, KEY, "Work").await;
        pool(&local, &[&id]).await;
        let session = model_session(&daemon, "paused").await;
        let h = Arc::new(host(&daemon, &session).await);
        let shared = daemon.shared.clone();
        let test_runtime = runtime.clone();
        let (captured, ready) = tokio::sync::oneshot::channel();
        let (resume, resumed) = tokio::sync::oneshot::channel();
        let startup = tokio::spawn(async move {
            let generation = shared.models.credentials_generation();
            let mut spec = SessionSpec {
                session: session.clone(),
                model: "anthropic:test-ready".into(),
                fallback_models: vec![],
                system_prompt_parts: vec![],
                tools: h.specs(),
                resume_state: None,
                credentials_json: shared.models.sidecar_options().to_string(),
                agent_json: "{}".into(),
                team: None,
            };
            captured.send(generation).unwrap();
            resumed.await.unwrap();
            let fence =
                crate::models_api::fence_startup(&shared, generation, &mut spec.credentials_json)
                    .await;
            let mut runtime = test_runtime.open_session(spec, h.clone()).await.unwrap();
            drop(fence);
            let result = h.credentials(acquire("after-startup", 0)).await;
            runtime.shutdown().await.unwrap();
            result
        });
        let generation = ready.await.unwrap();
        let (method, body) = if replace {
            ("PUT", json!({"key":"replacement-key-canary-5678"}))
        } else {
            ("DELETE", json!({}))
        };
        assert!(call(
            &local,
            method,
            &format!("/v1/settings/keys/anthropic/{id}"),
            body,
            None
        )
        .await
        .0
        .is_success());
        assert_ne!(generation, daemon.shared.models.credentials_generation());
        resume.send(()).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(3), startup)
            .await
            .unwrap()
            .unwrap();
        if replace {
            assert!(result.unwrap().auth.value == "replacement-key-canary-5678");
        } else {
            assert!(result.is_err());
        }
        assert!(runtime
            .0
            .lock()
            .opened
            .iter()
            .all(|spec| !spec.credentials_json.contains(KEY)));
        daemon.shutdown().await;
    }
}

#[tokio::test]
async fn credential_mutations_cannot_commit_between_the_final_check_and_spawn() {
    let root = tempfile::tempdir().unwrap();
    let (daemon, _) = fixture(root.path()).await;
    let local = api::router(daemon.shared.clone(), true);
    let id = add(&local, KEY, "Work").await;
    let generation = daemon.shared.models.credentials_generation();
    let mut snapshot = daemon.shared.models.sidecar_options().to_string();
    let fence = crate::models_api::fence_startup(&daemon.shared, generation, &mut snapshot).await;
    let path = format!("/v1/settings/keys/anthropic/{id}");
    let deletion = call(&local, "DELETE", &path, json!({}), None);
    tokio::pin!(deletion);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut deletion)
            .await
            .is_err()
    );
    assert_eq!(generation, daemon.shared.models.credentials_generation());
    drop(fence);
    assert_eq!(deletion.await.0, StatusCode::NO_CONTENT);
    assert_ne!(generation, daemon.shared.models.credentials_generation());
    daemon.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn configured_broker_outage_never_migrates_into_unlisted_vault_source() {
    let root = tempfile::tempdir().unwrap();
    let token = root.path().join("broker.token");
    std::fs::write(&token, "fake-broker-token-canary").unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let credentials = json!({"order":{"anthropic":["omp-broker","vault"]},"omp_broker":{"url":format!("http://{address}"),"token_file":token,"timeout_seconds":0.1},"env":{"anthropic":[],"openai":[],"google":[],"openai-codex":[],"github-copilot":[]}});
    let (daemon, _) = fixture_sources(root.path(), Some(credentials)).await;
    let local = api::router(daemon.shared.clone(), true);
    let id = add(&local, KEY, "Work").await;
    let (_, catalog) = call(&local, "GET", "/v1/settings/models", json!({}), None).await;
    assert_eq!(catalog["providers"][0]["models"][0]["available"], false);
    assert!(catalog["providers"][0]["pool"]["accounts"]
        .as_array()
        .unwrap()
        .is_empty());
    let session = model_session(&daemon, "fail-closed").await;
    let h = host(&daemon, &session).await;
    assert_eq!(
        h.credentials(acquire("unlisted", 0))
            .await
            .unwrap_err()
            .code,
        CredentialErrorCode::ProviderUnavailable
    );
    pool(&local, &[&id]).await;
    let lease = h.credentials(acquire("explicit", 0)).await.unwrap();
    assert!(lease.auth.value == KEY);
    h.credential_report(report(&lease, CredentialOutcome::Success, false))
        .await
        .unwrap();
    daemon.shutdown().await;
}

#[tokio::test]
async fn null_and_excessive_report_windows_expire_and_out_of_order_reports_do_not_reduce_usage() {
    let root = tempfile::tempdir().unwrap();
    let (daemon, _) = fixture(root.path()).await;
    let local = api::router(daemon.shared.clone(), true);
    let first = add(&local, KEY, "Work").await;
    let second = add(&local, SECOND, "Personal").await;
    pool(&local, &[&first, &second]).await;
    let session = model_session(&daemon, "window-order").await;
    let h = host(&daemon, &session).await;
    let old = h.credentials(acquire("older-window", 0)).await.unwrap();
    let newer = h.credentials(acquire("newer-window", 0)).await.unwrap();
    let before = chrono::Utc::now().timestamp_millis() as f64;
    let mut measured = report(&newer, CredentialOutcome::Success, false);
    measured.usage_windows = vec![CredentialUsageWindow {
        name: "unsafe-private-window".into(),
        used_percent: Some(95.0),
        reset_at_ms: Some(f64::MAX),
    }];
    h.credential_report(measured).await.unwrap();
    let mut delayed = report(&old, CredentialOutcome::Success, false);
    delayed.usage_windows = vec![CredentialUsageWindow {
        name: "older".into(),
        used_percent: Some(10.0),
        reset_at_ms: None,
    }];
    h.credential_report(delayed).await.unwrap();
    let response = crate::model_accounts::diagnostics(&daemon.shared)
        .await
        .unwrap();
    let row = response
        .sources
        .iter()
        .find(|source| source.name == "vault")
        .unwrap()
        .accounts
        .iter()
        .find(|account| account.id == first)
        .unwrap();
    assert_eq!(row.usage_windows[0].used_percent, Some(95.0));
    assert!(
        row.usage_windows[0].reset_at_ms.unwrap() <= before + 7.0 * 24.0 * 3600.0 * 1000.0 + 1000.0
    );
    let fallback = h.credentials(acquire("protected-window", 0)).await.unwrap();
    assert_eq!(fallback.account.account_id, second);
    let mut unknown = report(&fallback, CredentialOutcome::Auth, false);
    unknown.retry_after_ms = None;
    let ack = h.credential_report(unknown).await.unwrap();
    assert!(ack
        .retry_at_ms
        .is_some_and(|until| until < chrono::Utc::now().timestamp_millis() as u64 + 61_000));
    let response = crate::model_accounts::diagnostics(&daemon.shared)
        .await
        .unwrap();
    let row = response
        .sources
        .iter()
        .find(|source| source.name == "vault")
        .unwrap()
        .accounts
        .iter()
        .find(|account| account.id == second)
        .unwrap();
    assert_eq!(
        row.reason.as_deref(),
        Some("Account needs credential check")
    );
    let (_, keys) = call(&local, "GET", "/v1/settings/keys", json!({}), None).await;
    assert!(keys["keys"]
        .as_array()
        .unwrap()
        .iter()
        .all(|key| key["enabled"] == true));
    daemon.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn narrowing_agent_pool_during_slow_credential_resolution_never_leases_removed_account() {
    let root = tempfile::tempdir().unwrap();
    let started = root.path().join("command-started");
    let resume = root.path().join("command-resume");
    let secret = root.path().join("command-key");
    std::fs::write(&secret, SECOND).unwrap();
    let script = format!(
        "touch '{}'; while [ ! -f '{}' ]; do sleep 0.01; done; cat '{}'",
        started.display(),
        resume.display(),
        secret.display()
    );
    let credentials = json!({"order":{"anthropic":["command","vault"]},"command":{"anthropic":["sh","-c",script]},"env":{"anthropic":[],"openai":[],"google":[],"openai-codex":[],"github-copilot":[]}});
    let (daemon, _) = fixture_sources(root.path(), Some(credentials)).await;
    let local = api::router(daemon.shared.clone(), true);
    let key = add(&local, KEY, "Work").await;
    let command = ModelAccountRef {
        source: "command".into(),
        account_id: "anthropic".into(),
    };
    call(
        &local,
        "PUT",
        "/v1/settings/pools/anthropic",
        json!({"accounts":[command,account(&key)]}),
        None,
    )
    .await;
    let (_, created) = call(
        &local,
        "POST",
        "/v1/sessions",
        json!({"agent_id":"chief"}),
        None,
    )
    .await;
    let session = SessionId::new(created["id"].as_str().unwrap());
    let h = Arc::new(host(&daemon, &session).await);
    let resolving = {
        let h = h.clone();
        tokio::spawn(async move { h.credentials(acquire("narrowed", 0)).await })
    };
    tokio::time::timeout(Duration::from_secs(3), async {
        while !started.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let generation = daemon.shared.models.credentials_generation();
    assert_eq!(
        call(
            &local,
            "PATCH",
            "/v1/agents/chief",
            json!({"account_pool":[account(&key)]}),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_ne!(generation, daemon.shared.models.credentials_generation());
    std::fs::write(&resume, "ready").unwrap();
    let lease = tokio::time::timeout(Duration::from_secs(3), resolving)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(lease.account, account(&key));
    assert!(lease.auth.value == KEY);
    h.credential_report(report(&lease, CredentialOutcome::Success, false))
        .await
        .unwrap();
    daemon.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn creating_a_vault_key_does_not_replace_the_migrated_existing_source() {
    let root = tempfile::tempdir().unwrap();
    let secret = root.path().join("existing-command-key");
    std::fs::write(&secret, SECOND).unwrap();
    let credentials = json!({"order":{"anthropic":["vault","command"]},"command":{"anthropic":["sh","-c",format!("cat '{}'",secret.display())]},"env":{"anthropic":[],"openai":[],"google":[],"openai-codex":[],"github-copilot":[]}});
    let (daemon, _) = fixture_sources(root.path(), Some(credentials)).await;
    let local = api::router(daemon.shared.clone(), true);
    let key = add(&local, KEY, "New Vault account").await;
    let (_, pools) = call(&local, "GET", "/v1/settings/pools", json!({}), None).await;
    assert_eq!(
        pools["pools"][0]["accounts"],
        json!([{"source":"command","account_id":"anthropic"}])
    );
    let session = model_session(&daemon, "prior-source").await;
    let h = host(&daemon, &session).await;
    let existing = h.credentials(acquire("existing-source", 0)).await.unwrap();
    assert_eq!(existing.account.source, "command");
    assert!(existing.auth.value == SECOND);
    h.credential_report(report(&existing, CredentialOutcome::Success, false))
        .await
        .unwrap();
    pool(&local, &[&key]).await;
    let explicit = h.credentials(acquire("person-switched", 0)).await.unwrap();
    assert_eq!(explicit.account, account(&key));
    assert!(explicit.auth.value == KEY);
    h.credential_report(report(&explicit, CredentialOutcome::Success, false))
        .await
        .unwrap();
    daemon.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn default_selection_reopens_team_home_next_turn_but_preserves_history_and_active_model_binding(
) {
    let root = tempfile::tempdir().unwrap();
    let (daemon, runtime) = fixture(root.path()).await;
    let local = api::router(daemon.shared.clone(), true);
    add(&local, KEY, "Work").await;
    let home = daemon
        .shared
        .agents
        .get("chief")
        .unwrap()
        .unwrap()
        .home_session_id;
    let (_, created) = call(
        &local,
        "POST",
        "/v1/sessions",
        json!({"agent_id":"chief","model":"anthropic:test-ready"}),
        None,
    )
    .await;
    let history = SessionId::new(created["id"].as_str().unwrap());
    daemon
        .shared
        .command(&home, SessionAction::Recover)
        .await
        .unwrap();
    daemon
        .shared
        .command(&history, SessionAction::Recover)
        .await
        .unwrap();
    let home_host = host(&daemon, &home).await;
    let generation = daemon.shared.models.default_generation();
    assert_eq!(
        call(
            &local,
            "PUT",
            "/v1/settings/models",
            json!({"default_model":"anthropic:test-second","fallback_models":[]}),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_ne!(generation, daemon.shared.models.default_generation());
    let captured = home_host
        .credentials(acquire("captured-old", 0))
        .await
        .unwrap();
    home_host
        .credential_report(report(&captured, CredentialOutcome::Success, false))
        .await
        .unwrap();
    daemon
        .shared
        .command(&home, SessionAction::Recover)
        .await
        .unwrap();
    let opened = runtime
        .0
        .lock()
        .opened
        .iter()
        .filter(|spec| spec.session == home)
        .map(|spec| spec.model.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        opened,
        vec!["anthropic:test-ready", "anthropic:test-second"]
    );
    let mut next = acquire("home-next", 0);
    next.model = "anthropic:test-second".into();
    let fresh = home_host.credentials(next).await.unwrap();
    assert!(fresh.auth.value == KEY);
    home_host
        .credential_report(report(&fresh, CredentialOutcome::Success, false))
        .await
        .unwrap();
    daemon
        .shared
        .command(&history, SessionAction::Recover)
        .await
        .unwrap();
    let history_host = host(&daemon, &history).await;
    let stored = history_host
        .credentials(acquire("history-model", 0))
        .await
        .unwrap();
    history_host
        .credential_report(report(&stored, CredentialOutcome::Success, false))
        .await
        .unwrap();
    assert!(runtime
        .0
        .lock()
        .opened
        .iter()
        .filter(|spec| spec.session == history)
        .all(|spec| spec.model == "anthropic:test-ready"));
    let generation = daemon.shared.models.default_generation();
    assert_eq!(
        call(
            &local,
            "PUT",
            "/v1/settings/models",
            json!({"default_model":"anthropic:test-second","fallback_models":[]}),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(generation, daemon.shared.models.default_generation());
    daemon.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn daemon_doctor_uses_owned_source_metadata_without_returning_keys() {
    let root = tempfile::tempdir().unwrap();
    let (daemon, _) = fixture(root.path()).await;
    let local = api::router(daemon.shared.clone(), true);
    add(&local, KEY, "Work").await;
    let (status, value) = call(&local, "GET", "/v1/doctor", json!({}), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!value.to_string().contains(KEY));
    let check = value["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|group| group["checks"].as_array().unwrap())
        .find(|check| check["id"] == "credentials.vault")
        .unwrap();
    assert_eq!(check["status"], "ok");
    daemon.shutdown().await;
}

#[tokio::test]
async fn broker_credentials_never_reach_diagnostics_events_or_sidecar_options() {
    use crate::model_broker::tests::{snapshot, FakeBroker, ACCESS, REFRESH, TOKEN};
    let root = tempfile::tempdir().unwrap();
    let mut data = snapshot();
    data["credentials"][0]["provider"] = json!("anthropic");
    data["credentials"][0]["models"] = json!(["anthropic:test-ready"]);
    let server = FakeBroker::start(data, json!({"reports":[]}), json!({"ok":true})).await;
    let token = root.path().join("broker.token");
    std::fs::write(&token, TOKEN).unwrap();
    let credentials = json!({"omp_broker":{"url":server.url,"token_file":token},
        "order":{"anthropic":["omp-broker"]},
        "env":{"anthropic":[],"openai":[],"google":[],"openai-codex":[],"github-copilot":[]}});
    let (daemon, _) = fixture_sources(root.path(), Some(credentials)).await;
    let local = api::router(daemon.shared.clone(), true);
    let session = model_session(&daemon, "broker-redaction").await;
    let handler = host(&daemon, &session).await;
    let lease = handler
        .credentials(acquire("broker-redaction", 0))
        .await
        .unwrap();
    assert!(lease.auth.value == ACCESS);
    handler
        .credential_report(report(&lease, CredentialOutcome::Success, false))
        .await
        .unwrap();
    let mut surfaces = vec![
        format!("{lease:?}"),
        daemon.shared.models.sidecar_options().to_string(),
    ];
    for path in ["/v1/settings/models", "/v1/doctor", "/v1/accounts"] {
        let (status, body) = call(&local, "GET", path, json!({}), None).await;
        assert_eq!(status, StatusCode::OK);
        surfaces.push(body.to_string());
    }
    surfaces
        .push(serde_json::to_string(&daemon.shared.db.events(&session, 0).await.unwrap()).unwrap());
    for path in ["data/audit.jsonl", "model-vault-audit.jsonl"] {
        surfaces.push(
            String::from_utf8_lossy(&std::fs::read(root.path().join(path)).unwrap_or_default())
                .into_owned(),
        );
    }
    for surface in surfaces {
        for secret in [ACCESS, REFRESH, TOKEN] {
            assert!(!surface.contains(secret), "broker secret leaked");
        }
    }
    daemon.shutdown().await;
}
