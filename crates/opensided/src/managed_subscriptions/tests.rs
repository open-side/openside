use super::*;
use crate::managed_subscriptions::{Error as ManagedError, Service};
use openside_vault::{AuditContext, PassphraseLock, PlatformLock, Secret, Vault};
use std::sync::atomic::{AtomicUsize, Ordering};

async fn fixture(root: &Path) -> Daemon {
    let config = resolved_config(root);
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let vault = Vault::create(
        root.join("subscription-vault"),
        root.join("subscription-audit"),
        &Secret::new("fake-subscription-test-passphrase".into()),
        Box::new(PassphraseLock::default()) as Box<dyn PlatformLock + Send>,
    )
    .unwrap();
    *daemon.shared.registry.vault.vault.lock().await = Some(vault);
    daemon
}
fn ctx() -> AuditContext<'static> {
    AuditContext {
        who: "test",
        session_id: None,
        tool_call_id: None,
    }
}
fn codex(access: &str, refresh: &str, expiry: u64) -> String {
    json!({"tokens":{"access_token":access,"refresh_token":refresh,"account_id":"fake-account","expires_at_ms":expiry}}).to_string()
}
fn home(root: &Path, source: &str, id: &str) -> std::path::PathBuf {
    root.join("managed-subscriptions").join(source).join(id)
}
fn login(root: &Path, id: &str, raw: &str) {
    std::fs::write(home(root, "codex-cli", id).join("auth.json"), raw).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn managed_subscription_fake_cli_signin_isolated_adopted_and_wiped() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let daemon = fixture(root.path()).await;
    let service = Service::new(root.path());
    let first = service
        .create(&daemon.shared, "codex-cli", "First")
        .await
        .unwrap();
    let second = service
        .create(&daemon.shared, "codex-cli", "Second")
        .await
        .unwrap();
    assert_ne!(first.account_id, second.account_id);
    let bin = root.path().join("fake-bin");
    std::fs::create_dir(&bin).unwrap();
    let cli = bin.join("codex");
    std::fs::write(&cli, "#!/bin/sh\n[ \"$1\" = '-c' ] && [ \"$2\" = 'cli_auth_credentials_store=\"file\"' ] && [ \"$3\" = login ] || exit 9\nprintf '%s' \"$FAKE_CLI_DOCUMENT\" > \"$CODEX_HOME/auth.json\"\n").unwrap();
    std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
    let raw = codex("fake-access-first", "fake-refresh-private-canary", u64::MAX);
    let status = tokio::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(&first.login_command)
        .env_clear()
        .env("PATH", &bin)
        .env("FAKE_CLI_DOCUMENT", &raw)
        .status()
        .await
        .unwrap();
    assert!(status.success());
    let adopted = service
        .get(&daemon.shared, "codex-cli", &first.account_id)
        .await
        .unwrap();
    assert!(adopted.logged_in);
    assert!(matches!(
        service
            .update(
                &daemon.shared,
                "codex-cli",
                &first.account_id,
                Some("fake-refresh-private-canary"),
                None,
                None
            )
            .await,
        Err(ManagedError::InvalidValue)
    ));
    assert!(
        !service
            .get(&daemon.shared, "codex-cli", &second.account_id)
            .await
            .unwrap()
            .logged_in
    );
    let auth = service
        .resolve(&daemon.shared, "codex-cli", &first.account_id, false)
        .await
        .unwrap();
    let wire = serde_json::to_string(&auth.auth).unwrap();
    assert!(wire.contains("fake-access-first"));
    assert!(!wire.contains("fake-refresh-private-canary"));
    assert!(!serde_json::to_string(&adopted)
        .unwrap()
        .contains("fake-refresh-private-canary"));
    assert!(!format!("{auth:?}").contains("fake-access-first"));
    let first_home = home(root.path(), "codex-cli", &first.account_id);
    assert_eq!(
        std::fs::metadata(&first_home).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(first_home.join("auth.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    service
        .remove(&daemon.shared, "codex-cli", &first.account_id)
        .await
        .unwrap();
    assert!(!first_home.exists());
    assert!(home(root.path(), "codex-cli", &second.account_id).is_dir());
    assert!(matches!(
        service
            .resolve(&daemon.shared, "codex-cli", &first.account_id, false)
            .await,
        Err(ManagedError::NotFound)
    ));
}

async fn endpoint(
    counter: Arc<AtomicUsize>,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
) -> (String, tokio::task::JoinHandle<()>) {
    let router = axum::Router::new().route("/token", axum::routing::post(move |body: String| {
        let counter = counter.clone(); let entered = entered.clone(); let release = release.clone();
        async move {
            assert!(body.contains("grant_type=refresh_token"));
            counter.fetch_add(1, Ordering::SeqCst);
            entered.notify_one(); release.notified().await;
            axum::Json(json!({"access_token":"fake-refreshed-access","refresh_token":"fake-rotated-refresh","expires_in":3600}))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{address}/token"), task)
}

#[tokio::test]
async fn managed_subscription_concurrent_refresh_is_single_flight() {
    let root = tempfile::tempdir().unwrap();
    let daemon = fixture(root.path()).await;
    let count = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let (url, server) = endpoint(count.clone(), entered.clone(), release.clone()).await;
    let mut service = Service::new(root.path());
    service.token_endpoint = Some(url);
    let service = Arc::new(service);
    let account = service
        .create(&daemon.shared, "codex-cli", "Concurrent")
        .await
        .unwrap();
    login(
        root.path(),
        &account.account_id,
        &codex("fake-old-access", "fake-old-refresh", 1),
    );
    service
        .get(&daemon.shared, "codex-cli", &account.account_id)
        .await
        .unwrap();
    let mut tasks = Vec::new();
    for _ in 0..12 {
        let service = service.clone();
        let shared = daemon.shared.clone();
        let id = account.account_id.clone();
        tasks.push(tokio::spawn(async move {
            service.resolve(&shared, "codex-cli", &id, true).await
        }));
    }
    entered.notified().await;
    release.notify_one();
    for task in tasks {
        assert!(task.await.unwrap().unwrap().auth.value == "fake-refreshed-access");
    }
    assert_eq!(count.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn managed_subscription_new_login_wins_over_inflight_refresh() {
    let root = tempfile::tempdir().unwrap();
    let daemon = fixture(root.path()).await;
    let count = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let (url, server) = endpoint(count, entered.clone(), release.clone()).await;
    let mut service = Service::new(root.path());
    service.token_endpoint = Some(url);
    let service = Arc::new(service);
    let account = service
        .create(&daemon.shared, "codex-cli", "Race")
        .await
        .unwrap();
    login(
        root.path(),
        &account.account_id,
        &codex("fake-expired", "fake-refresh-before-login", 1),
    );
    service
        .get(&daemon.shared, "codex-cli", &account.account_id)
        .await
        .unwrap();
    let original = daemon
        .shared
        .registry
        .vault
        .vault
        .lock()
        .await
        .as_ref()
        .unwrap()
        .subscriptions(&ctx())
        .unwrap()
        .into_iter()
        .find(|i| i.account_id == account.account_id)
        .unwrap();
    let task = {
        let shared = daemon.shared.clone();
        let service = service.clone();
        let id = account.account_id.clone();
        tokio::spawn(async move { service.resolve(&shared, "codex-cli", &id, false).await })
    };
    entered.notified().await;
    login(
        root.path(),
        &account.account_id,
        &codex("fake-new-login", "fake-new-login-refresh", u64::MAX),
    );
    release.notify_one();
    let resolved = task.await.unwrap().unwrap();
    assert!(resolved.auth.value == "fake-new-login");
    assert!(resolved.revision > original.revision);
    let mut vault = daemon.shared.registry.vault.vault.lock().await;
    assert!(vault
        .as_mut()
        .unwrap()
        .set_subscription(
            &ctx(),
            original.clone(),
            Secret::new("stale".into()),
            Some(original.revision)
        )
        .is_err());
    server.abort();
}

#[tokio::test]
async fn managed_subscription_crash_after_vault_commit_recovers_canonical_state() {
    use sha2::{Digest, Sha256};
    let root = tempfile::tempdir().unwrap();
    let daemon = fixture(root.path()).await;
    let service = Service::new(root.path());
    let account = service
        .create(&daemon.shared, "codex-cli", "Crash")
        .await
        .unwrap();
    let old = codex("fake-old", "fake-old-refresh", u64::MAX);
    login(root.path(), &account.account_id, &old);
    service
        .get(&daemon.shared, "codex-cli", &account.account_id)
        .await
        .unwrap();
    let mut vault = daemon.shared.registry.vault.vault.lock().await;
    let mut info = vault
        .as_ref()
        .unwrap()
        .subscriptions(&ctx())
        .unwrap()
        .into_iter()
        .find(|i| i.account_id == account.account_id)
        .unwrap();
    let revision = info.revision;
    info.external_digest = Some(format!("{:x}", Sha256::digest(old.as_bytes())));
    let newer = codex("fake-crash-canonical", "fake-private-refresh", u64::MAX);
    vault
        .as_mut()
        .unwrap()
        .set_subscription(&ctx(), info, Secret::new(newer.clone()), Some(revision))
        .unwrap();
    drop(vault);
    drop(service);
    let reopened = Service::new(root.path());
    let resolved = reopened
        .resolve(&daemon.shared, "codex-cli", &account.account_id, false)
        .await
        .unwrap();
    assert!(resolved.auth.value == "fake-crash-canonical");
    assert!(
        std::fs::read_to_string(
            home(root.path(), "codex-cli", &account.account_id).join("auth.json")
        )
        .unwrap()
            == newer
    );
}

#[cfg(unix)]
#[tokio::test]
async fn managed_subscription_wipe_does_not_follow_external_symlink() {
    let root = tempfile::tempdir().unwrap();
    let daemon = fixture(root.path()).await;
    let service = Service::new(root.path());
    let account = service
        .create(&daemon.shared, "codex-cli", "Safe wipe")
        .await
        .unwrap();
    let external = root.path().join("external");
    std::fs::create_dir(&external).unwrap();
    std::fs::write(external.join("keep"), "fake-external-document").unwrap();
    std::os::unix::fs::symlink(
        &external,
        home(root.path(), "codex-cli", &account.account_id).join("link"),
    )
    .unwrap();
    service
        .remove(&daemon.shared, "codex-cli", &account.account_id)
        .await
        .unwrap();
    assert!(external.join("keep").exists());
}

#[tokio::test]
async fn managed_subscription_default_home_is_read_only_and_external_owned() {
    let root = tempfile::tempdir().unwrap();
    let daemon = fixture(root.path()).await;
    let service = Service::new(root.path());
    let default = root
        .path()
        .join("managed-subscriptions/external-default/codex-cli");
    std::fs::create_dir_all(&default).unwrap();
    let path = default.join("auth.json");
    let raw = codex("fake-default-access", "fake-default-refresh", u64::MAX);
    std::fs::write(&path, &raw).unwrap();
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();
    let rows = service.list(&daemon.shared).await.unwrap();
    assert!(rows.iter().any(|row| row.source == "codex-cli"
        && row.account_id == "default"
        && row.logged_in
        && !row.managed));
    assert!(
        service
            .resolve(&daemon.shared, "codex-cli", "default", false)
            .await
            .unwrap()
            .auth
            .value
            == "fake-default-access"
    );
    assert!(matches!(
        service
            .resolve(&daemon.shared, "codex-cli", "default", true)
            .await,
        Err(ManagedError::AuthUnavailable)
    ));
    assert!(matches!(
        service
            .update(
                &daemon.shared,
                "codex-cli",
                "default",
                Some("fake-default-refresh"),
                None,
                None
            )
            .await,
        Err(ManagedError::InvalidValue)
    ));
    service
        .update(
            &daemon.shared,
            "codex-cli",
            "default",
            Some("Personal"),
            Some(false),
            Some(20),
        )
        .await
        .unwrap();
    service
        .remove(&daemon.shared, "codex-cli", "default")
        .await
        .unwrap();
    assert!(!service
        .list(&daemon.shared)
        .await
        .unwrap()
        .iter()
        .any(|a| a.source == "codex-cli" && a.account_id == "default"));
    assert!(matches!(
        Service::new(root.path())
            .get(&daemon.shared, "codex-cli", "default")
            .await,
        Err(ManagedError::NotFound)
    ));
    assert!(std::fs::read_to_string(&path).unwrap() == raw);
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        before
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn managed_subscription_claude_fake_cli_command_imports_only_its_home() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let daemon = fixture(root.path()).await;
    let service = Service::new(root.path());
    let account = service
        .create(&daemon.shared, "claude-code", "Claude")
        .await
        .unwrap();
    let bin = root.path().join("fake-bin");
    std::fs::create_dir(&bin).unwrap();
    let cli = bin.join("claude");
    std::fs::write(&cli, "#!/bin/sh\n[ \"$1\" = auth ] && [ \"$2\" = login ] || exit 9\n[ -z \"${CLAUDE_SECURESTORAGE_CONFIG_DIR+x}\" ] || exit 8\nprintf '%s' \"$FAKE_CLI_DOCUMENT\" > \"$CLAUDE_CONFIG_DIR/.credentials.json\"\n").unwrap();
    std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
    let raw = json!({"claudeAiOauth":{"accessToken":"fake-claude-access","refreshToken":"fake-claude-refresh","expiresAt":u64::MAX}}).to_string();
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let output = tokio::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(&account.login_command)
        .env_clear()
        .env("PATH", path)
        .env("FAKE_CLI_DOCUMENT", raw)
        .env("CLAUDE_SECURESTORAGE_CONFIG_DIR", "/not-used")
        .output()
        .await
        .unwrap();
    assert!(output.status.success());
    assert!(
        service
            .get(&daemon.shared, "claude-code", &account.account_id)
            .await
            .unwrap()
            .logged_in
    );
    let credential = service
        .resolve(&daemon.shared, "claude-code", &account.account_id, false)
        .await
        .unwrap();
    assert!(credential.auth.value == "fake-claude-access");
    assert!(!serde_json::to_string(&credential.auth)
        .unwrap()
        .contains("fake-claude-refresh"));
    service
        .remove(&daemon.shared, "claude-code", &account.account_id)
        .await
        .unwrap();
    assert!(!home(root.path(), "claude-code", &account.account_id).exists());
}

#[tokio::test]
async fn managed_subscription_older_file_timestamp_cannot_replace_adopted_revision() {
    let root = tempfile::tempdir().unwrap();
    let daemon = fixture(root.path()).await;
    let service = Service::new(root.path());
    let account = service
        .create(&daemon.shared, "codex-cli", "Timestamp")
        .await
        .unwrap();
    login(
        root.path(),
        &account.account_id,
        &codex("fake-newer", "fake-newer-refresh", u64::MAX),
    );
    service
        .get(&daemon.shared, "codex-cli", &account.account_id)
        .await
        .unwrap();
    let path = home(root.path(), "codex-cli", &account.account_id).join("auth.json");
    login(
        root.path(),
        &account.account_id,
        &codex("fake-stale", "fake-stale-refresh", u64::MAX),
    );
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1)),
        )
        .unwrap();
    assert!(matches!(
        service
            .resolve(&daemon.shared, "codex-cli", &account.account_id, false)
            .await,
        Err(ManagedError::Stale)
    ));
    let vault = daemon.shared.registry.vault.vault.lock().await;
    vault
        .as_ref()
        .unwrap()
        .with_subscription(&ctx(), "codex-cli", &account.account_id, |_, raw| {
            assert!(raw.contains("fake-newer"));
            assert!(!raw.contains("fake-stale"));
        })
        .unwrap();
}
