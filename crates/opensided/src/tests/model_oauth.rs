#![cfg(target_os = "linux")]

use super::*;
use openside_core::{CredentialAuthKind, CredentialRequest};
use openside_proto::DeviceKind;
use openside_vault::{AuditContext, OAuthInfo, PassphraseLock, PlatformLock, Secret, Vault};

const ACCESS: &str = "oauth-access-canary-5e1f0a";
const REFRESH: &str = "oauth-refresh-canary-77c2b9";
const NEWER: &str = "oauth-newer-login-canary-3d4e";
const REFRESHED: &str = "oauth-refreshed-canary-91ab";
const CODE: &str = "pasted-code-canary-4410";
const NEWER_REFRESH: &str = "oauth-newer-refresh-canary-a8d0";
const ROTATED: &str = "oauth-rotated-refresh-canary-6c13";
const CANARIES: [&str; 7] = [
    ACCESS,
    REFRESH,
    NEWER,
    REFRESHED,
    CODE,
    NEWER_REFRESH,
    ROTATED,
];
/// Provider-side identifiers fake helpers emit; OAUTH-5 keeps them out of every reply.
const UPSTREAM_IDS: [&str; 2] = ["acct-1", "project-1"];
const FUTURE: u64 = 4_102_444_800_000;

fn credentials(access: &str, expires: u64, extra: &str) -> String {
    format!(
        r#"{{"type":"credentials","credentials":{{"access":"{access}","refresh":"{REFRESH}","expires":{expires}{extra}}}}}"#
    )
}

/// Fake agent argv: `sh -c <script> <mode>` sees the mode as `$0`; the scenario and
/// replies are files under `root` so each test scripts the helper without real providers.
fn script(root: &Path) -> String {
    let r = root.display();
    format!(
        r#"R='{r}'
case "$0" in
models) printf '%s' '{{"providers":[]}}' ;;
auth-login)
  echo $$ > "$R/pid"
  read -r first
  printf '%s\n' "$first" >> "$R/login-requests"
  s=$(cat "$R/scenario")
  case "$s" in
  browser)
    printf '%s\n' '{{"type":"auth","url":"https://auth.example.test/authorize?state=1","launch_url":"http://127.0.0.1:1455/start"}}'
    printf '%s\n' '{{"type":"manual_input_wanted"}}'
    read -r input
    printf '%s\n' "$input" > "$R/input"
    printf '%s\n' '{{"type":"progress","stage":"exchanging"}}'
    cat "$R/credentials" ;;
  callback)
    printf '%s\n' '{{"type":"auth","url":"https://auth.example.test/authorize?state=2"}}'
    while [ ! -e "$R/release" ]; do sleep 0.05; done
    cat "$R/credentials" ;;
  device)
    printf '%s\n' '{{"type":"auth","url":"https://device.example.test/login/device","instructions":"Enter code: ABCD-1234","user_code":"ABCD-1234"}}'
    while [ ! -e "$R/release" ]; do sleep 0.05; done
    cat "$R/credentials" ;;
  error:*)
    printf '{{"type":"error","code":"%s"}}\n' "${{s#error:}}"
    exit 1 ;;
  repaste)
    printf '%s\n' '{{"type":"auth","url":"https://auth.example.test/authorize?state=4"}}'
    printf '%s\n' '{{"type":"manual_input_wanted"}}'
    read -r input
    printf '%s\n' '{{"type":"manual_input_wanted"}}'
    read -r input
    printf '%s\n' "$input" > "$R/input"
    printf '%s\n' '{{"type":"progress","stage":"exchanging"}}'
    cat "$R/credentials" ;;
  oversize)
    head -c 70000 /dev/zero | tr '\0' a
    echo
    while :; do sleep 1; done ;;
  unterminated)
    printf '%s' "$(cat "$R/credentials")" ;;
  hang)
    printf '%s\n' '{{"type":"auth","url":"https://auth.example.test/authorize?state=3"}}'
    while :; do sleep 1; done ;;
  esac ;;
auth-refresh)
  read -r request
  echo x >> "$R/refresh-count"
  while [ -e "$R/hold" ] && [ ! -e "$R/release-refresh" ]; do sleep 0.05; done
  cat "$R/refresh-reply" ;;
esac"#
    )
}

async fn fixture(root: &Path) -> Daemon {
    let mut config = resolved_config(root);
    config.loaded.config.vault.path = Some(root.join("model-vault"));
    config.loaded.config.models.default = Some("google-gemini-cli:gemini-test".into());
    config.loaded.config.credentials = json!({"order":{},"env":{"anthropic":[],"openai":[],"google":[],"openai-codex":[],"github-copilot":[]}});
    config.argv = vec!["sh".into(), "-c".into(), script(root)];
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let vault = Vault::create(
        root.join("model-vault"),
        root.join("model-vault-audit.jsonl"),
        &Secret::new("fixture-oauth-passphrase".into()),
        Box::new(PassphraseLock::default()) as Box<dyn PlatformLock + Send>,
    )
    .unwrap();
    *daemon.shared.registry.vault.vault.lock().await = Some(vault);
    daemon
}

fn scenario(root: &Path, name: &str, reply: &str) {
    std::fs::write(root.join("scenario"), name).unwrap();
    std::fs::write(root.join("credentials"), format!("{reply}\n")).unwrap();
    let _ = std::fs::remove_file(root.join("release"));
}

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
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    let text = value.to_string();
    for canary in CANARIES {
        assert!(!text.contains(canary), "{path} leaked a token");
    }
    for upstream in UPSTREAM_IDS {
        assert!(
            !text.contains(upstream),
            "{path} leaked a provider-side identifier"
        );
    }
    (status, value)
}

async fn until(
    router: &axum::Router,
    id: &str,
    token: Option<&str>,
    done: impl Fn(&Value) -> bool,
) -> Value {
    for _ in 0..200 {
        let (status, value) = call(
            router,
            "GET",
            &format!("/v1/settings/oauth/transactions/{id}"),
            json!({}),
            token,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{value}");
        if done(&value) {
            return value;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("transaction {id} never reached the expected state");
}

fn status(value: &Value, expected: &str) -> bool {
    value["status"] == expected
}

async fn create(
    router: &axum::Router,
    provider: &str,
    body: Value,
    token: Option<&str>,
) -> (StatusCode, Value) {
    call(
        router,
        "POST",
        &format!("/v1/settings/oauth/accounts/{provider}"),
        body,
        token,
    )
    .await
}

async fn signed_in(
    router: &axum::Router,
    root: &Path,
    provider: &str,
    pool: bool,
    extra: &str,
) -> String {
    scenario(root, "callback", &credentials(ACCESS, FUTURE, extra));
    let (code, value) = create(
        router,
        provider,
        json!({"label":"Work","join_pool":pool}),
        None,
    )
    .await;
    assert_eq!(code, StatusCode::ACCEPTED, "{value}");
    let id = value["transaction_id"].as_str().unwrap().to_owned();
    until(router, &id, None, |v| status(v, "awaiting_browser")).await;
    std::fs::write(root.join("release"), "").unwrap();
    let done = until(router, &id, None, |v| status(v, "ready")).await;
    let account = done["account_id"].as_str().unwrap().to_owned();
    // The reply handle is openside's random local id, never the provider's account id.
    assert!(!UPSTREAM_IDS.contains(&account.as_str()));
    let (_, accounts) = call(
        router,
        "GET",
        "/v1/settings/oauth/accounts",
        json!({}),
        None,
    )
    .await;
    assert!(accounts["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| !row.to_string().contains("email")));
    account
}

async fn pools(router: &axum::Router) -> Value {
    call(router, "GET", "/v1/settings/pools", json!({}), None)
        .await
        .1
}

fn in_pool(pools: &Value, provider: &str, id: &str) -> bool {
    pools["pools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|pool| pool["provider"] == provider)
        .flat_map(|pool| pool["accounts"].as_array().unwrap())
        .any(|account| account["source"] == "pi-oauth" && account["account_id"] == id)
}

fn no_tokens_on_disk(root: &Path) {
    for file in [
        root.join("model-vault"),
        root.join("model-vault-audit.jsonl"),
        root.join("data/audit.jsonl"),
        root.join("data/settings.json"),
    ] {
        let bytes = std::fs::read(file).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        for canary in CANARIES {
            assert!(!text.contains(canary));
        }
    }
}

fn context() -> AuditContext<'static> {
    AuditContext {
        who: "test",
        session_id: None,
        tool_call_id: None,
    }
}

async fn info(daemon: &Daemon, provider: &str, id: &str) -> Option<OAuthInfo> {
    daemon
        .shared
        .registry
        .vault
        .vault
        .lock()
        .await
        .as_ref()
        .unwrap()
        .oauth_accounts(&context())
        .unwrap()
        .into_iter()
        .find(|info| info.provider == provider && info.account_id == id)
}

async fn stored_access(daemon: &Daemon, provider: &str, id: &str) -> String {
    stored_field(daemon, provider, id, "access").await
}

async fn stored_field(daemon: &Daemon, provider: &str, id: &str, field: &str) -> String {
    daemon
        .shared
        .registry
        .vault
        .vault
        .lock()
        .await
        .as_ref()
        .unwrap()
        .with_oauth(&context(), provider, id, |_, raw| {
            serde_json::from_str::<Value>(raw).unwrap()[field]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .unwrap()
}

/// Replace the stored payload as a new revision, as a sign-in elsewhere would.
async fn store(daemon: &Daemon, provider: &str, id: &str, payload: String) {
    let current = info(daemon, provider, id).await.unwrap();
    let revision = current.revision;
    daemon
        .shared
        .registry
        .vault
        .vault
        .lock()
        .await
        .as_mut()
        .unwrap()
        .set_oauth(&context(), current, Secret::new(payload), Some(revision))
        .unwrap();
}

fn refreshes(root: &Path) -> usize {
    std::fs::read_to_string(root.join("refresh-count"))
        .unwrap_or_default()
        .lines()
        .count()
}

async fn until_refreshes(root: &Path, count: usize) {
    for _ in 0..200 {
        if refreshes(root) >= count {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("refresh helper never started");
}

fn audited_reads(root: &Path) -> usize {
    std::fs::read_to_string(root.join("model-vault-audit.jsonl"))
        .unwrap_or_default()
        .matches("oauth_request")
        .count()
}

async fn session(daemon: &Daemon, name: &str, model: &str) -> SessionId {
    let id = SessionId::new(name);
    let stored = id.clone();
    let model = model.to_owned();
    daemon
        .shared
        .db
        .store(move |store| {
            store.create_session(stored, "Test", &model, PolicyMode::Guard, None)?;
            Ok(())
        })
        .await
        .unwrap();
    id
}

fn acquire(id: &str, provider: &str, model: &str) -> CredentialRequest {
    CredentialRequest {
        request_id: id.into(),
        provider: provider.into(),
        model: model.into(),
        attempt: 0,
        subagent: None,
    }
}

#[tokio::test]
async fn browser_sign_in_gates_manual_input_and_stores_without_leaking() {
    let root = tempfile::tempdir().unwrap();
    let daemon = fixture(root.path()).await;
    let local = api::router(daemon.shared.clone(), true);
    let (code, providers) = call(
        &local,
        "GET",
        "/v1/settings/oauth/providers",
        json!({}),
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    let names: Vec<_> = providers["providers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["provider"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "anthropic",
            "openai-codex",
            "google-gemini-cli",
            "google-antigravity",
            "github-copilot"
        ]
    );
    assert_eq!(
        create(&local, "unknown", json!({"label":"Work"}), None)
            .await
            .1["code"],
        "oauth_provider_unknown"
    );
    assert_eq!(
        create(
            &local,
            "anthropic",
            json!({"label":"Work","method":"device"}),
            None
        )
        .await
        .1["code"],
        "oauth_flow_unsupported"
    );
    assert_eq!(
        create(&local, "anthropic", json!({"label":" "}), None)
            .await
            .1["code"],
        "invalid_value"
    );

    scenario(root.path(), "browser", &credentials(ACCESS, FUTURE, ""));
    let (code, started) = create(&local, "anthropic", json!({"label":"Work"}), None).await;
    assert_eq!(code, StatusCode::ACCEPTED);
    assert_eq!(started["status"], "starting");
    assert_eq!(started["kind"], "create");
    assert_eq!(started["method"], "browser");
    let id = started["transaction_id"].as_str().unwrap().to_owned();
    let account_id = started["account_id"].as_str().unwrap().to_owned();
    assert!(info(&daemon, "anthropic", &account_id).await.is_none());
    let waiting = until(&local, &id, None, |v| v["manual_input"] == true).await;
    assert_eq!(waiting["status"], "awaiting_browser");
    assert_eq!(
        waiting["authorization"]["url"],
        "https://auth.example.test/authorize?state=1"
    );
    assert_eq!(
        waiting["authorization"]["launch_url"],
        "http://127.0.0.1:1455/start"
    );
    let requests = std::fs::read_to_string(root.path().join("login-requests")).unwrap();
    assert_eq!(
        requests.trim(),
        r#"{"type":"login","provider":"anthropic"}"#
    );
    let (code, accepted) = call(
        &local,
        "POST",
        &format!("/v1/settings/oauth/transactions/{id}/input"),
        json!({"code":CODE}),
        None,
    )
    .await;
    assert_eq!(code, StatusCode::ACCEPTED, "{accepted}");
    assert_eq!(accepted["status"], "exchanging");
    assert_eq!(accepted["manual_input"], false);
    let ready = until(&local, &id, None, |v| status(v, "ready")).await;
    assert!(ready["authorization"].is_null());
    assert_eq!(ready["account"]["source"], "pi-oauth");
    assert_eq!(ready["account"]["available"], true);
    assert_eq!(ready["account"]["reason"], "OAuth account ready");
    let input = std::fs::read_to_string(root.path().join("input")).unwrap();
    assert_eq!(
        input.trim(),
        format!(r#"{{"type":"manual_input","value":"{CODE}"}}"#)
    );
    assert_eq!(
        call(
            &local,
            "POST",
            &format!("/v1/settings/oauth/transactions/{id}/input"),
            json!({"code":CODE}),
            None
        )
        .await
        .1["code"],
        "oauth_transaction_finished"
    );
    let (_, list) = call(
        &local,
        "GET",
        "/v1/settings/oauth/accounts",
        json!({}),
        None,
    )
    .await;
    assert_eq!(list["accounts"][0]["account_id"], account_id);
    assert_eq!(list["accounts"][0]["label"], "Work");
    assert_eq!(list["accounts"][0]["reserve_percent"], 10);
    assert!(!in_pool(&pools(&local).await, "anthropic", &account_id));
    let (_, diagnostics) = call(&local, "GET", "/v1/accounts", json!({}), None).await;
    let source = diagnostics["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|source| source["name"] == "pi-oauth")
        .unwrap();
    assert_eq!(source["accounts"][0]["id"], account_id);
    assert_eq!(
        stored_access(&daemon, "anthropic", &account_id).await,
        ACCESS
    );
    no_tokens_on_disk(root.path());

    let joined = signed_in(&local, root.path(), "anthropic", true, "").await;
    let pools = pools(&local).await;
    assert!(in_pool(&pools, "anthropic", &joined));
    assert!(!in_pool(&pools, "anthropic", &account_id));
    daemon.shutdown().await;
}

#[tokio::test]
async fn device_flow_errors_cancel_and_expiry() {
    let root = tempfile::tempdir().unwrap();
    let daemon = fixture(root.path()).await;
    let local = api::router(daemon.shared.clone(), true);
    scenario(root.path(), "device", &credentials(ACCESS, FUTURE, ""));
    let (_, started) = create(&local, "github-copilot", json!({"label":"Copilot"}), None).await;
    assert_eq!(started["method"], "device");
    let id = started["transaction_id"].as_str().unwrap().to_owned();
    let waiting = until(&local, &id, None, |v| status(v, "awaiting_code")).await;
    assert_eq!(waiting["authorization"]["user_code"], "ABCD-1234");
    assert_eq!(waiting["manual_input"], false);
    assert_eq!(
        call(
            &local,
            "POST",
            &format!("/v1/settings/oauth/transactions/{id}/input"),
            json!({"code":CODE}),
            None
        )
        .await
        .1["code"],
        "oauth_manual_input_not_allowed"
    );
    std::fs::write(root.path().join("release"), "").unwrap();
    until(&local, &id, None, |v| status(v, "ready")).await;

    for (code, status_name, reason) in [
        ("callback_busy", "failed", "oauth_callback_busy"),
        ("invalid_grant", "failed", "oauth_invalid_grant"),
        ("unsupported", "failed", "oauth_flow_unsupported"),
        ("failed", "failed", "oauth_failed"),
        ("cancelled", "cancelled", "oauth_cancelled"),
    ] {
        scenario(root.path(), &format!("error:{code}"), "");
        let (_, started) = create(&local, "anthropic", json!({"label":"Work"}), None).await;
        let id = started["transaction_id"].as_str().unwrap().to_owned();
        let done = until(&local, &id, None, |v| v["status"] != "starting").await;
        assert_eq!(done["status"], status_name);
        assert_eq!(done["reason"], reason);
    }

    scenario(root.path(), "hang", "");
    let (_, started) = create(&local, "anthropic", json!({"label":"Work"}), None).await;
    let id = started["transaction_id"].as_str().unwrap().to_owned();
    until(&local, &id, None, |v| status(v, "awaiting_browser")).await;
    let pid: i32 = std::fs::read_to_string(root.path().join("pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(
        call(
            &local,
            "POST",
            &format!("/v1/settings/oauth/transactions/{id}/cancel"),
            json!({}),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let cancelled = until(&local, &id, None, |v| status(v, "cancelled")).await;
    assert_eq!(cancelled["reason"], "oauth_cancelled");
    assert!(cancelled["authorization"].is_null());
    let mut gone = false;
    for _ in 0..200 {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"));
        if stat.map_or(true, |stat| stat.contains(") Z ")) {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(gone, "cancelled helper still running");

    daemon
        .shared
        .model_oauth
        .ttl_ms
        .store(300, std::sync::atomic::Ordering::Relaxed);
    let (_, started) = create(&local, "anthropic", json!({"label":"Work"}), None).await;
    let id = started["transaction_id"].as_str().unwrap().to_owned();
    let expired = until(&local, &id, None, |v| status(v, "expired")).await;
    assert_eq!(expired["reason"], "oauth_expired");
    daemon.shutdown().await;
}

#[tokio::test]
async fn helper_protocol_edges_cache_and_commit_races() {
    let root = tempfile::tempdir().unwrap();
    let daemon = fixture(root.path()).await;
    let local = api::router(daemon.shared.clone(), true);
    let paste = |id: &str| format!("/v1/settings/oauth/transactions/{id}/input");

    // A provider that rejects the first paste asks again, and the person can paste once more.
    scenario(root.path(), "repaste", &credentials(ACCESS, FUTURE, ""));
    let (_, started) = create(&local, "anthropic", json!({"label":"Work"}), None).await;
    let id = started["transaction_id"].as_str().unwrap().to_owned();
    until(&local, &id, None, |v| v["manual_input"] == true).await;
    let (code, _) = call(
        &local,
        "POST",
        &paste(&id),
        json!({"code":"wrong-code"}),
        None,
    )
    .await;
    assert_eq!(code, StatusCode::ACCEPTED);
    let again = until(&local, &id, None, |v| v["manual_input"] == true).await;
    assert_eq!(again["status"], "awaiting_browser");
    let (code, _) = call(&local, "POST", &paste(&id), json!({"code":CODE}), None).await;
    assert_eq!(code, StatusCode::ACCEPTED);
    until(&local, &id, None, |v| status(v, "ready")).await;
    let input = std::fs::read_to_string(root.path().join("input")).unwrap();
    assert!(input.contains(CODE));

    // An oversize line and a final line without its newline are protocol errors.
    for name in ["oversize", "unterminated"] {
        scenario(root.path(), name, &credentials(ACCESS, FUTURE, ""));
        let (_, started) = create(&local, "anthropic", json!({"label":"Work"}), None).await;
        let id = started["transaction_id"].as_str().unwrap().to_owned();
        let account_id = started["account_id"].as_str().unwrap().to_owned();
        let done = until(&local, &id, None, |v| v["status"] != "starting").await;
        assert_eq!(done["status"], "failed", "{name}");
        assert_eq!(done["reason"], "oauth_failed", "{name}");
        assert!(
            info(&daemon, "anthropic", &account_id).await.is_none(),
            "{name}"
        );
    }

    // A cancel that lands while the commit waits for the Vault wins: nothing is stored.
    scenario(root.path(), "callback", &credentials(ACCESS, FUTURE, ""));
    let (_, started) = create(
        &local,
        "anthropic",
        json!({"label":"Work","join_pool":true}),
        None,
    )
    .await;
    let id = started["transaction_id"].as_str().unwrap().to_owned();
    let account_id = started["account_id"].as_str().unwrap().to_owned();
    until(&local, &id, None, |v| status(v, "awaiting_browser")).await;
    let pid: i32 = std::fs::read_to_string(root.path().join("pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let vault = daemon.shared.registry.vault.vault.lock().await;
    std::fs::write(root.path().join("release"), "").unwrap();
    for _ in 0..200 {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"));
        if stat.map_or(true, |stat| stat.contains(") Z ")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    daemon.shared.model_oauth.cancel(None, &id).unwrap();
    drop(vault);
    let cancelled = until(&local, &id, None, |v| status(v, "cancelled")).await;
    assert_eq!(cancelled["reason"], "oauth_cancelled");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(info(&daemon, "anthropic", &account_id).await.is_none());
    assert!(!in_pool(&pools(&local).await, "anthropic", &account_id));

    // Readiness of accounts with required fields is cached per revision, so listing them
    // does not read (and audit) their tokens every time.
    let google = signed_in(
        &local,
        root.path(),
        "google-gemini-cli",
        false,
        r#","project_id":"project-1""#,
    )
    .await;
    let before = audited_reads(root.path());
    stored_access(&daemon, "google-gemini-cli", &google).await;
    assert!(
        audited_reads(root.path()) > before,
        "token reads are audited"
    );
    let before = audited_reads(root.path());
    for _ in 0..3 {
        let (_, list) = call(
            &local,
            "GET",
            "/v1/settings/oauth/accounts",
            json!({}),
            None,
        )
        .await;
        assert_eq!(list["accounts"].as_array().unwrap().len(), 2);
        call(&local, "GET", "/v1/accounts", json!({}), None).await;
    }
    assert_eq!(audited_reads(root.path()), before);
    no_tokens_on_disk(root.path());
    daemon.shutdown().await;
}

#[tokio::test]
async fn callers_are_isolated_and_agents_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let daemon = fixture(root.path()).await;
    let tcp = api::router(daemon.shared.clone(), false);
    let local = api::router(daemon.shared.clone(), true);
    let (_, first) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Admin])
        .await
        .unwrap();
    let (_, second) = daemon
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
    scenario(root.path(), "hang", "");
    let (code, started) = create(&tcp, "anthropic", json!({"label":"Work"}), Some(&first)).await;
    assert_eq!(code, StatusCode::ACCEPTED);
    let id = started["transaction_id"].as_str().unwrap().to_owned();
    let path = format!("/v1/settings/oauth/transactions/{id}");
    assert_eq!(
        call(&tcp, "GET", &path, json!({}), Some(&first)).await.0,
        StatusCode::OK
    );
    let (code, value) = call(&tcp, "GET", &path, json!({}), Some(&second)).await;
    assert_eq!(code, StatusCode::NOT_FOUND);
    assert_eq!(value["code"], "oauth_transaction_unknown");
    assert_eq!(
        call(
            &tcp,
            "POST",
            &format!("{path}/cancel"),
            json!({}),
            Some(&second)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&local, "GET", &path, json!({}), None).await.0,
        StatusCode::OK
    );
    for (method, route, body) in [
        ("GET", "/v1/settings/oauth/providers".to_owned(), json!({})),
        ("GET", "/v1/settings/oauth/accounts".to_owned(), json!({})),
        (
            "POST",
            "/v1/settings/oauth/accounts/anthropic".to_owned(),
            json!({"label":"Work"}),
        ),
        ("GET", path.clone(), json!({})),
        ("POST", format!("{path}/cancel"), json!({})),
    ] {
        let (code, value) = call(&tcp, method, &route, body.clone(), Some(&agent)).await;
        assert_eq!(code, StatusCode::FORBIDDEN);
        assert_eq!(value["code"], "person_device_required");
        let mut mcp = request(method, &route, body, None);
        mcp.headers_mut()
            .insert("x-openside-client", "mcp".parse().unwrap());
        let response = local.clone().oneshot(mcp).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["code"], "person_device_required");
    }
    for _ in 0..3 {
        assert_eq!(
            create(&local, "anthropic", json!({"label":"Work"}), None)
                .await
                .0,
            StatusCode::ACCEPTED
        );
    }
    let (code, value) = create(&local, "anthropic", json!({"label":"Work"}), None).await;
    assert_eq!(code, StatusCode::CONFLICT);
    assert_eq!(value["code"], "oauth_busy");
    daemon.shutdown().await;
}

#[tokio::test]
async fn relogin_update_and_delete_keep_local_state_consistent() {
    let root = tempfile::tempdir().unwrap();
    let daemon = fixture(root.path()).await;
    let local = api::router(daemon.shared.clone(), true);
    let id = signed_in(&local, root.path(), "anthropic", true, "").await;
    let account = format!("/v1/settings/oauth/accounts/anthropic/{id}");
    let (code, updated) = call(
        &local,
        "PATCH",
        &account,
        json!({"label":"Renamed","reserve_percent":25}),
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(updated["label"], "Renamed");
    assert_eq!(updated["reserve_percent"], 25);
    {
        let mut info = info(&daemon, "anthropic", &id).await.unwrap();
        let revision = info.revision;
        info.auth_invalid = true;
        let mut guard = daemon.shared.registry.vault.vault.lock().await;
        guard
            .as_mut()
            .unwrap()
            .update_oauth_info(&context(), info, revision)
            .unwrap();
    }
    assert_eq!(
        call(&local, "GET", &account, json!({}), None).await.1["reason"],
        "Sign in again"
    );

    scenario(root.path(), "callback", &credentials(NEWER, FUTURE, ""));
    let (code, started) = call(
        &local,
        "POST",
        &format!("{account}/relogin"),
        json!({}),
        None,
    )
    .await;
    assert_eq!(code, StatusCode::ACCEPTED, "{started}");
    assert_eq!(started["kind"], "relogin");
    assert_eq!(started["account_id"], id);
    let transaction = started["transaction_id"].as_str().unwrap().to_owned();
    let (code, busy) = call(
        &local,
        "POST",
        &format!("{account}/relogin"),
        json!({}),
        None,
    )
    .await;
    assert_eq!(code, StatusCode::CONFLICT);
    assert_eq!(busy["code"], "oauth_busy");
    until(&local, &transaction, None, |v| {
        status(v, "awaiting_browser")
    })
    .await;
    std::fs::write(root.path().join("release"), "").unwrap();
    let ready = until(&local, &transaction, None, |v| status(v, "ready")).await;
    assert_eq!(ready["account"]["label"], "Renamed");
    assert_eq!(ready["account"]["reserve_percent"], 25);
    assert_eq!(ready["account"]["auth_invalid"], false);
    assert_eq!(stored_access(&daemon, "anthropic", &id).await, NEWER);
    assert!(in_pool(&pools(&local).await, "anthropic", &id));

    // Relogin whose target disappears before the provider answers never recreates it.
    scenario(root.path(), "callback", &credentials(ACCESS, FUTURE, ""));
    let (_, started) = call(
        &local,
        "POST",
        &format!("{account}/relogin"),
        json!({}),
        None,
    )
    .await;
    let transaction = started["transaction_id"].as_str().unwrap().to_owned();
    until(&local, &transaction, None, |v| {
        status(v, "awaiting_browser")
    })
    .await;
    {
        let revision = info(&daemon, "anthropic", &id).await.unwrap().revision;
        let mut guard = daemon.shared.registry.vault.vault.lock().await;
        guard
            .as_mut()
            .unwrap()
            .delete_oauth(&context(), "anthropic", &id, revision)
            .unwrap();
    }
    std::fs::write(root.path().join("release"), "").unwrap();
    let failed = until(&local, &transaction, None, |v| status(v, "failed")).await;
    assert_eq!(failed["reason"], "oauth_account_removed");
    assert!(info(&daemon, "anthropic", &id).await.is_none());

    // Delete cancels a live relogin, removes pool membership and is not repeatable.
    let id = signed_in(&local, root.path(), "anthropic", true, "").await;
    let account = format!("/v1/settings/oauth/accounts/anthropic/{id}");
    scenario(root.path(), "hang", "");
    let (_, started) = call(
        &local,
        "POST",
        &format!("{account}/relogin"),
        json!({}),
        None,
    )
    .await;
    let transaction = started["transaction_id"].as_str().unwrap().to_owned();
    until(&local, &transaction, None, |v| {
        status(v, "awaiting_browser")
    })
    .await;
    assert_eq!(
        call(&local, "DELETE", &account, json!({}), None).await.0,
        StatusCode::NO_CONTENT
    );
    let removed = until(&local, &transaction, None, |v| status(v, "failed")).await;
    assert_eq!(removed["reason"], "oauth_account_removed");
    assert!(!in_pool(&pools(&local).await, "anthropic", &id));
    let (code, value) = call(&local, "DELETE", &account, json!({}), None).await;
    assert_eq!(code, StatusCode::NOT_FOUND);
    assert_eq!(value["code"], "unknown_account");
    assert_eq!(
        call(
            &local,
            "POST",
            &format!("{account}/relogin"),
            json!({}),
            None
        )
        .await
        .1["code"],
        "unknown_account"
    );
    no_tokens_on_disk(root.path());
    daemon.shutdown().await;
}

#[tokio::test]
async fn leases_carry_provider_fields_and_refresh_with_cas() {
    let root = tempfile::tempdir().unwrap();
    let daemon = fixture(root.path()).await;
    let local = api::router(daemon.shared.clone(), true);
    let model = "google-gemini-cli:gemini-test";

    let missing = signed_in(&local, root.path(), "google-gemini-cli", false, "").await;
    let (_, account) = call(
        &local,
        "GET",
        &format!("/v1/settings/oauth/accounts/google-gemini-cli/{missing}"),
        json!({}),
        None,
    )
    .await;
    assert_eq!(account["available"], false);
    assert_eq!(
        account["reason"],
        "Account is missing required sign-in details"
    );

    let id = signed_in(
        &local,
        root.path(),
        "google-gemini-cli",
        true,
        r#","project_id":"project-1""#,
    )
    .await;
    let chat = session(&daemon, "oauth-lease", model).await;
    let h = host(&daemon, &chat).await;
    let lease = h
        .credentials(acquire("oauth-1", "google-gemini-cli", model))
        .await
        .unwrap();
    assert_eq!(lease.account.source, "pi-oauth");
    assert_eq!(lease.account.account_id, id);
    assert_eq!(lease.auth.kind, CredentialAuthKind::Bearer);
    assert!(lease.auth.value == ACCESS);
    assert_eq!(lease.auth.expires_at_ms, Some(FUTURE));
    assert!(lease.auth.upstream_account_id.is_none());
    assert_eq!(
        lease
            .auth
            .request_metadata
            .as_ref()
            .unwrap()
            .project_id
            .as_deref(),
        Some("project-1")
    );
    assert!(!format!("{lease:?}").contains(ACCESS));
    assert!(!root.path().join("refresh-count").exists());
    drop(lease);

    // Expired access refreshes once; the refreshed payload is stored as a new revision.
    let revision = {
        let current = info(&daemon, "google-gemini-cli", &id).await.unwrap();
        let mut guard = daemon.shared.registry.vault.vault.lock().await;
        let payload = format!(
            r#"{{"access":"{ACCESS}","refresh":"{REFRESH}","expires":1000,"project_id":"project-1"}}"#
        );
        let revision = current.revision;
        guard
            .as_mut()
            .unwrap()
            .set_oauth(&context(), current, Secret::new(payload), Some(revision))
            .unwrap()
            .revision
    };
    std::fs::write(
        root.path().join("refresh-reply"),
        credentials(REFRESHED, FUTURE, r#","project_id":"project-1""#) + "\n",
    )
    .unwrap();
    let lease = h
        .credentials(acquire("oauth-2", "google-gemini-cli", model))
        .await
        .unwrap();
    assert!(lease.auth.value == REFRESHED);
    assert_eq!(
        stored_access(&daemon, "google-gemini-cli", &id).await,
        REFRESHED
    );
    assert_eq!(
        info(&daemon, "google-gemini-cli", &id)
            .await
            .unwrap()
            .revision,
        revision + 1
    );
    drop(lease);

    // A newer sign-in that lands while the refresh helper runs is never overwritten.
    let current = info(&daemon, "google-gemini-cli", &id).await.unwrap();
    {
        let mut guard = daemon.shared.registry.vault.vault.lock().await;
        let payload = format!(
            r#"{{"access":"{ACCESS}","refresh":"{REFRESH}","expires":1000,"project_id":"project-1"}}"#
        );
        let revision = current.revision;
        guard
            .as_mut()
            .unwrap()
            .set_oauth(&context(), current, Secret::new(payload), Some(revision))
            .unwrap();
    }
    std::fs::write(root.path().join("hold"), "").unwrap();
    let pending = {
        let h = host(&daemon, &chat).await;
        tokio::spawn(async move {
            h.credentials(acquire("oauth-3", "google-gemini-cli", model))
                .await
        })
    };
    for _ in 0..200 {
        let count = std::fs::read_to_string(root.path().join("refresh-count")).unwrap_or_default();
        if count.lines().count() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    {
        let current = info(&daemon, "google-gemini-cli", &id).await.unwrap();
        let mut guard = daemon.shared.registry.vault.vault.lock().await;
        let payload = format!(
            r#"{{"access":"{NEWER}","refresh":"{NEWER_REFRESH}","expires":{FUTURE},"project_id":"project-1"}}"#
        );
        let revision = current.revision;
        guard
            .as_mut()
            .unwrap()
            .set_oauth(&context(), current, Secret::new(payload), Some(revision))
            .unwrap();
    }
    std::fs::write(root.path().join("release-refresh"), "").unwrap();
    let lease = pending.await.unwrap().unwrap();
    assert!(lease.auth.value == NEWER);
    assert_eq!(
        stored_access(&daemon, "google-gemini-cli", &id).await,
        NEWER
    );
    drop(lease);
    std::fs::remove_file(root.path().join("hold")).unwrap();
    std::fs::remove_file(root.path().join("release-refresh")).unwrap();

    // A rename while the refresh helper runs must not drop the rotated tokens.
    store(
        &daemon,
        "google-gemini-cli",
        &id,
        format!(r#"{{"access":"{ACCESS}","refresh":"{REFRESH}","expires":1000,"project_id":"project-1"}}"#),
    )
    .await;
    std::fs::write(
        root.path().join("refresh-reply"),
        format!(
            r#"{{"type":"credentials","credentials":{{"access":"{REFRESHED}","refresh":"{ROTATED}","expires":{FUTURE},"project_id":"project-1"}}}}"#
        ) + "\n",
    )
    .unwrap();
    std::fs::write(root.path().join("hold"), "").unwrap();
    let count = refreshes(root.path());
    let pending = {
        let shared = daemon.shared.clone();
        let id = id.clone();
        tokio::spawn(async move {
            shared
                .model_oauth
                .resolve(&shared, "google-gemini-cli", &id, false, 0)
                .await
        })
    };
    until_refreshes(root.path(), count + 1).await;
    let (code, renamed) = call(
        &local,
        "PATCH",
        &format!("/v1/settings/oauth/accounts/google-gemini-cli/{id}"),
        json!({"label":"Renamed"}),
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{renamed}");
    std::fs::write(root.path().join("release-refresh"), "").unwrap();
    let (lease, _) = pending.await.unwrap().unwrap();
    assert!(lease.value == REFRESHED);
    drop(lease);
    assert_eq!(
        stored_field(&daemon, "google-gemini-cli", &id, "refresh").await,
        ROTATED
    );
    assert_eq!(
        stored_access(&daemon, "google-gemini-cli", &id).await,
        REFRESHED
    );
    assert_eq!(
        info(&daemon, "google-gemini-cli", &id).await.unwrap().label,
        "Renamed"
    );
    std::fs::remove_file(root.path().join("hold")).unwrap();

    // Concurrent forced resolves against one leased revision share a single refresh.
    std::fs::write(
        root.path().join("refresh-reply"),
        credentials(REFRESHED, FUTURE, r#","project_id":"project-1""#) + "\n",
    )
    .unwrap();
    let leased = info(&daemon, "google-gemini-cli", &id)
        .await
        .unwrap()
        .revision;
    let count = refreshes(root.path());
    let oauth = &daemon.shared.model_oauth;
    let (first, second) = tokio::join!(
        oauth.resolve(&daemon.shared, "google-gemini-cli", &id, true, leased),
        oauth.resolve(&daemon.shared, "google-gemini-cli", &id, true, leased),
    );
    assert!(first.is_ok() && second.is_ok());
    assert_eq!(refreshes(root.path()), count + 1);

    // A transient refresh failure inside the expiry skew still leases the valid token, but a
    // forced refresh after an auth failure does not fall back to it.
    let soon = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 30_000;
    store(
        &daemon,
        "google-gemini-cli",
        &id,
        format!(r#"{{"access":"{ACCESS}","refresh":"{REFRESH}","expires":{soon},"project_id":"project-1"}}"#),
    )
    .await;
    std::fs::write(
        root.path().join("refresh-reply"),
        "{\"type\":\"error\",\"code\":\"failed\"}\n",
    )
    .unwrap();
    let count = refreshes(root.path());
    let (lease, revision) = oauth
        .resolve(&daemon.shared, "google-gemini-cli", &id, false, 0)
        .await
        .unwrap();
    assert!(lease.value == ACCESS);
    assert_eq!(refreshes(root.path()), count + 1);
    drop(lease);
    assert!(oauth
        .resolve(&daemon.shared, "google-gemini-cli", &id, true, revision)
        .await
        .is_err());

    // invalid_grant marks only the revision it refreshed.
    let current = info(&daemon, "google-gemini-cli", &id).await.unwrap();
    {
        let mut guard = daemon.shared.registry.vault.vault.lock().await;
        let payload = format!(
            r#"{{"access":"{ACCESS}","refresh":"{REFRESH}","expires":1000,"project_id":"project-1"}}"#
        );
        let revision = current.revision;
        guard
            .as_mut()
            .unwrap()
            .set_oauth(&context(), current, Secret::new(payload), Some(revision))
            .unwrap();
    }
    std::fs::write(
        root.path().join("refresh-reply"),
        "{\"type\":\"error\",\"code\":\"invalid_grant\"}\n",
    )
    .unwrap();
    assert!(h
        .credentials(acquire("oauth-4", "google-gemini-cli", model))
        .await
        .is_err());
    let (_, account) = call(
        &local,
        "GET",
        &format!("/v1/settings/oauth/accounts/google-gemini-cli/{id}"),
        json!({}),
        None,
    )
    .await;
    assert_eq!(account["auth_invalid"], true);
    assert_eq!(account["reason"], "Sign in again");
    no_tokens_on_disk(root.path());

    // Codex leases carry the upstream account id and fail closed without it.
    let codex = signed_in(
        &local,
        root.path(),
        "openai-codex",
        false,
        r#","account_id":"acct-1""#,
    )
    .await;
    let lease = daemon
        .shared
        .model_oauth
        .resolve(&daemon.shared, "openai-codex", &codex, false, 0)
        .await
        .unwrap();
    assert_eq!(lease.0.kind, CredentialAuthKind::CodexCli);
    assert_eq!(lease.0.upstream_account_id.as_deref(), Some("acct-1"));
    assert!(lease.0.request_metadata.is_none());
    let bare = signed_in(&local, root.path(), "openai-codex", false, "").await;
    assert!(daemon
        .shared
        .model_oauth
        .resolve(&daemon.shared, "openai-codex", &bare, false, 0)
        .await
        .is_err());
    let claude = signed_in(&local, root.path(), "anthropic", false, "").await;
    let lease = daemon
        .shared
        .model_oauth
        .resolve(&daemon.shared, "anthropic", &claude, false, 0)
        .await
        .unwrap();
    assert_eq!(lease.0.kind, CredentialAuthKind::ClaudeCode);
    daemon.shutdown().await;
}
