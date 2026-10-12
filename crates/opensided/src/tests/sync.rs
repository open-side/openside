//! B-9 through the daemon API: two daemons and one loopback fake WebDAV share exchange the
//! live policy, a pinned tab, a vault item and a tagged memory, converge under the conflict
//! rules, the share holds no plaintext, and a tampered blob or a wrong passphrase is refused.

use super::*;
use crate::engine::Shared;
use openside_sync::{AccountId, Backend, PairingCode, Pushed, SyncKey, WebDavBackend};
use openside_vault::{AccessPolicy, AllowedOrigin, Entry, PassphraseLock, Secret, Vault};

const PASSWORD: &str = "sync-daemon-password-canary-5521";
const USERNAME: &str = "sync-daemon-user-canary";
const URL: &str = "https://pinned-canary.example.test/inbox";
const TITLE: &str = "PinnedTitleCanary";
const SYNC_PASSPHRASE: &str = "sync passphrase for both devices";
const MEMORY: &str = "MemoryCanary remembers the quarterly plan";
const POLICY_ORIGIN: &str = "https://policy-canary.example.test";
const DAV_USER: &str = "dav-user";
const DAV_PASSWORD: &str = "dav-password-canary";

async fn sync_daemon(root: &Path, webdav: &str, passphrase: &str, item: bool) -> Daemon {
    std::fs::create_dir_all(root).unwrap();
    let mut config = resolved_config(root);
    let path = root.join("fixture.vault");
    let mut vault = Vault::create(
        &path,
        root.join("setup.audit.jsonl"),
        &Secret::new("vault-passphrase".into()),
        PassphraseLock::default(),
    )
    .unwrap();
    if item {
        vault
            .put(
                Entry::new(
                    "bank".into(),
                    vec![AllowedOrigin::parse("https://bank.example.test").unwrap()],
                    Secret::new(USERNAME.into()),
                    Secret::new(PASSWORD.into()),
                    None,
                    AccessPolicy::Always,
                )
                .unwrap(),
            )
            .unwrap();
    }
    drop(vault);
    std::fs::write(root.join("vault-pass"), "vault-passphrase\n").unwrap();
    std::fs::write(root.join("sync-pass"), format!("{passphrase}\n")).unwrap();
    std::fs::write(root.join("dav-pass"), format!("{DAV_PASSWORD}\n")).unwrap();
    let cat = |file: &str| {
        Some(vec![
            "cat".into(),
            root.join(file).to_string_lossy().into_owned(),
        ])
    };
    config.loaded.config.vault.path = Some(path);
    config.loaded.config.vault.passphrase_cmd = cat("vault-pass");
    config.loaded.config.sync.webdav_url = Some(webdav.to_owned());
    config.loaded.config.sync.settings = true;
    config.loaded.config.sync.webdav_username = Some(DAV_USER.into());
    config.loaded.config.sync.webdav_password_cmd = cat("dav-pass");
    config.loaded.config.sync.passphrase_cmd = cat("sync-pass");
    Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap()
}

async fn api(daemon: &Daemon, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    api_shared(daemon.shared.clone(), method, path, body).await
}

async fn api_shared(shared: Shared, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = api::router(shared, true)
        .oneshot(request(method, path, body, None))
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn run(daemon: &Daemon) -> Value {
    let (status, report) = api(daemon, "POST", "/v1/sync/run", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    report
}

async fn tabs(daemon: &Daemon) -> Vec<Value> {
    let (status, body) = api(daemon, "GET", "/v1/pinned-tabs", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    body["tabs"].as_array().unwrap().clone()
}

async fn synced_memories(daemon: &Daemon) -> Vec<String> {
    daemon
        .shared
        .memory
        .run(|store| store.tagged(&["sync".to_owned()]))
        .await
        .unwrap()
        .into_iter()
        .map(|(_, memory)| memory.content)
        .collect()
}

/// Canary occurrences in every file under `dir` (recursively), names included.
fn scan(dir: &Path, canaries: &[&str]) -> usize {
    let mut hits = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            hits += scan(&path, canaries);
            continue;
        }
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.extend_from_slice(path.to_string_lossy().as_bytes());
        for canary in canaries {
            hits += bytes
                .windows(canary.len())
                .filter(|w| *w == canary.as_bytes())
                .count();
        }
    }
    hits
}

#[tokio::test]
async fn two_daemons_sync_settings_pins_vault_and_memories_through_a_blind_webdav_share() {
    let root = tempfile::tempdir().unwrap();
    let dav_dir = root.path().join("dav");
    let (addr, dav) = openside_sync::fake_webdav::spawn(
        "127.0.0.1:0".parse().unwrap(),
        dav_dir.clone(),
        Some((DAV_USER, DAV_PASSWORD)),
    )
    .await
    .unwrap();
    let dav_url = format!("http://{addr}/dav");
    let a = sync_daemon(&root.path().join("a"), &dav_url, SYNC_PASSPHRASE, true).await;
    let b = sync_daemon(&root.path().join("b"), &dav_url, SYNC_PASSPHRASE, false).await;

    let (status, body) = api(&b, "GET", "/v1/sync", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (body["configured"].clone(), body["set_up"].clone()),
        (json!(true), json!(false))
    );
    let (status, body) = api(&b, "POST", "/v1/sync/run", json!({})).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("sync_not_set_up"))
    );

    let (status, tab) = api(
        &a,
        "POST",
        "/v1/pinned-tabs",
        json!({"url": URL, "title": TITLE}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{tab}");
    let policy = json!({"default_mode":"guard","allow_origins":[POLICY_ORIGIN],"deny_origins":[],"rules":[]});
    let (status, _) = api(&a, "PUT", "/v1/policy", policy.clone()).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = api(
        &a,
        "POST",
        "/v1/memory",
        json!({"content": MEMORY, "tags": ["sync"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = api(
        &a,
        "POST",
        "/v1/memory",
        json!({"content": "not selected for sync"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, setup) = api(&a, "POST", "/v1/sync/setup", json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{setup}");
    let code = setup["pairing_code"].as_str().unwrap().to_owned();
    let (status, again) = api(&a, "POST", "/v1/sync/setup", json!({})).await;
    assert_eq!(
        (status, again["code"].as_str()),
        (StatusCode::CONFLICT, Some("sync_already_set_up"))
    );
    let report = run(&a).await;
    // Policy, pinned tab, vault item and the one tagged memory.
    assert_eq!(
        (report["pushed"].clone(), report["vault_available"].clone()),
        (json!(4), json!(true))
    );

    // A device with another passphrase cannot join, even with the right pairing code.
    let c = sync_daemon(
        &root.path().join("c"),
        &dav_url,
        "a different passphrase",
        false,
    )
    .await;
    let (status, body) = api(&c, "POST", "/v1/sync/pair", json!({"pairing_code": code})).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("sync_wrong_passphrase"))
    );
    let (status, body) = api(
        &c,
        "POST",
        "/v1/sync/pair",
        json!({"pairing_code": "osync1-zzzz"}),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_pairing_code"))
    );

    let (status, paired) = api(&b, "POST", "/v1/sync/pair", json!({"pairing_code": code})).await;
    assert_eq!(status, StatusCode::OK, "{paired}");
    assert_eq!(paired["account_id"], setup["status"]["account_id"]);
    let report = run(&b).await;
    // B's own default policy loses to A's newer one; everything else is new to B.
    assert_eq!(
        (report["applied"].clone(), report["rejected"].clone()),
        (json!(4), json!(0))
    );
    assert_eq!(tabs(&b).await, vec![tab.clone()]);
    assert_eq!(api(&b, "GET", "/v1/policy", json!({})).await.1, policy);
    assert_eq!(synced_memories(&b).await, vec![MEMORY.to_owned()]);
    let (_, items) = api(&b, "GET", "/v1/vault/items", json!({})).await;
    assert_eq!(items["items"][0]["id"], "bank");
    assert_eq!(
        items["items"][0]["origins"],
        json!(["https://bank.example.test:443"])
    );
    // The imported secret is masked in B's event/tool redaction from now on.
    assert!(b
        .shared
        .registry
        .redactors
        .read()
        .iter()
        .any(|r| !r.redact(PASSWORD).contains(PASSWORD)));
    let idle = run(&b).await;
    assert_eq!(
        (idle["pushed"].clone(), idle["applied"].clone()),
        (json!(0), json!(0))
    );

    // Concurrent edits at the same Lamport counter: A unpins, B retitles. Tombstones win
    // ties, so both end without the tab.
    let id = tab["id"].as_str().unwrap();
    let (status, _) = api(&a, "DELETE", &format!("/v1/pinned-tabs/{id}"), json!({})).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = api(
        &b,
        "POST",
        "/v1/pinned-tabs",
        json!({"url": URL, "title": "renamed"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    run(&a).await;
    let report = run(&b).await;
    assert_eq!(report["conflicts"], 1, "{report}");
    run(&a).await;
    assert!(tabs(&a).await.is_empty());
    assert!(tabs(&b).await.is_empty());

    // The share stores sealed blobs only: no secret, URL, title or item id in plaintext.
    let canaries = [
        PASSWORD,
        USERNAME,
        "pinned-canary",
        TITLE,
        "bank.example",
        "\"bank\"",
        "MemoryCanary",
        "policy-canary",
        DAV_PASSWORD,
    ];
    assert_eq!(scan(&dav_dir, &canaries), 0);

    // Anyone with write access to the share (or its operator) replaces a record with a
    // modified blob: B authenticates it, counts it as rejected and applies nothing.
    let key = SyncKey::derive(
        SYNC_PASSPHRASE.as_bytes(),
        AccountId::parse(paired["account_id"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    assert!(key.matches(&PairingCode::parse(&code).unwrap()));
    let mallory = WebDavBackend::new(
        &dav_url,
        key.account(),
        Some((
            DAV_USER.into(),
            zeroize::Zeroizing::new(DAV_PASSWORD.to_owned()),
        )),
    )
    .unwrap();
    let target = key.record_key(openside_sync::Kind::VaultItem, "bank");
    let (mut blob, etag) = mallory.get(&target).await.unwrap().unwrap();
    let last = blob.len() - 1;
    blob[last] ^= 1;
    assert!(matches!(
        mallory.put(&target, Some(&etag), &blob).await.unwrap(),
        Pushed::Stored(_)
    ));
    let before = api(&b, "GET", "/v1/vault/items", json!({})).await.1;
    let report = run(&b).await;
    assert_eq!(
        (report["rejected"].clone(), report["applied"].clone()),
        (json!(1), json!(0))
    );
    assert_eq!(api(&b, "GET", "/v1/vault/items", json!({})).await.1, before);

    // Wrong share credentials surface as an authorization failure, not as data loss.
    std::fs::write(root.path().join("b").join("dav-pass"), "wrong\n").unwrap();
    let (status, body) = api(&b, "POST", "/v1/sync/run", json!({})).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::BAD_GATEWAY, Some("sync_unauthorized"))
    );

    dav.abort();
}

#[tokio::test]
async fn sync_routes_report_missing_configuration_and_validate_pins() {
    let root = tempfile::tempdir().unwrap();
    let daemon = Daemon::open_runtime(resolved_config(root.path()), TestRuntime::default())
        .await
        .unwrap();
    let (status, body) = api(&daemon, "POST", "/v1/sync/setup", json!({})).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("sync_not_configured"))
    );
    let (_, body) = api(&daemon, "GET", "/v1/sync", json!({})).await;
    assert_eq!(body["configured"], false);
    let (status, body) = api(
        &daemon,
        "POST",
        "/v1/pinned-tabs",
        json!({"url": "file:///etc/passwd"}),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_url"))
    );
    let (status, first) = api(
        &daemon,
        "POST",
        "/v1/pinned-tabs",
        json!({"url": "https://a.example.test"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, second) = api(
        &daemon,
        "POST",
        "/v1/pinned-tabs",
        json!({"url": "https://a.example.test/", "title": "A"}),
    )
    .await;
    assert_eq!(first["id"], second["id"], "same URL is one pinned tab");
    assert_eq!(tabs(&daemon).await.len(), 1);
    let (status, _) = api(&daemon, "DELETE", "/v1/pinned-tabs/unknown", json!({})).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "unpinning is idempotent");
    assert_eq!(tabs(&daemon).await.len(), 1);
}

/// Two daemons on one fake WebDAV share, paired and synced once; `a` starts with `setup`.
async fn paired(root: &Path) -> (Daemon, Daemon, tokio::task::JoinHandle<()>) {
    let (addr, dav) = openside_sync::fake_webdav::spawn(
        "127.0.0.1:0".parse().unwrap(),
        root.join("dav"),
        Some((DAV_USER, DAV_PASSWORD)),
    )
    .await
    .unwrap();
    let url = format!("http://{addr}/dav");
    let a = sync_daemon(&root.join("a"), &url, SYNC_PASSPHRASE, false).await;
    let b = sync_daemon(&root.join("b"), &url, SYNC_PASSPHRASE, false).await;
    let (status, setup) = api(&a, "POST", "/v1/sync/setup", json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{setup}");
    let (status, paired) = api(
        &b,
        "POST",
        "/v1/sync/pair",
        json!({"pairing_code": setup["pairing_code"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{paired}");
    run(&a).await;
    run(&b).await;
    (a, b, dav)
}

#[tokio::test]
async fn local_writes_wait_for_a_running_sync_and_are_published_by_the_next_run() {
    let root = tempfile::tempdir().unwrap();
    let (a, b, dav) = paired(root.path()).await;
    // Hold a's vault so its run stops after taking the policy and memory writer locks.
    let vault = a.shared.registry.vault.vault.lock().await;
    let running = tokio::spawn(api_shared(
        a.shared.clone(),
        "POST",
        "/v1/sync/run",
        json!({}),
    ));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while a.shared.memory.writers.try_read().is_ok() {
        assert!(
            std::time::Instant::now() < deadline,
            "sync run never took the writer lock"
        );
        tokio::task::yield_now().await;
    }
    let policy = json!({"default_mode":"guard","allow_origins":["https://during-sync.example.test"],"deny_origins":[],"rules":[]});
    let put = tokio::spawn(api_shared(
        a.shared.clone(),
        "PUT",
        "/v1/policy",
        policy.clone(),
    ));
    let note = tokio::spawn(api_shared(
        a.shared.clone(),
        "POST",
        "/v1/memory",
        json!({"content": "written during sync", "tags": ["sync"]}),
    ));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !put.is_finished() && !note.is_finished(),
        "local writers must wait for the run"
    );
    drop(vault);
    let (status, report) = running.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["pushed"], 0, "the run saw neither write");
    assert_eq!(put.await.unwrap().0, StatusCode::OK);
    assert_eq!(note.await.unwrap().0, StatusCode::CREATED);
    // Both writes are local edits for the next run, and reach the other device.
    let report = run(&a).await;
    assert_eq!(report["pushed"], 2, "{report}");
    let report = run(&b).await;
    assert_eq!(report["applied"], 2, "{report}");
    assert_eq!(api(&b, "GET", "/v1/policy", json!({})).await.1, policy);
    assert_eq!(
        synced_memories(&b).await,
        vec!["written during sync".to_owned()]
    );
    dav.abort();
}

#[tokio::test]
async fn memory_taken_out_of_the_selection_survives_a_concurrent_remote_delete() {
    let root = tempfile::tempdir().unwrap();
    let (a, b, dav) = paired(root.path()).await;
    let (status, memory) = api(
        &a,
        "POST",
        "/v1/memory",
        json!({"content": "shared then kept", "tags": ["sync"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, private) = api(&a, "POST", "/v1/memory", json!({"content": "never synced"})).await;
    assert_eq!(status, StatusCode::CREATED);
    run(&a).await;
    run(&b).await;
    let on_b = b
        .shared
        .memory
        .run(|store| store.tagged(&["sync".to_owned()]))
        .await
        .unwrap();
    assert_eq!(on_b.len(), 1);
    // a keeps the memory but takes it out of the selection; b deletes its copy meanwhile.
    let id = memory["id"].as_str().unwrap();
    let (status, _) = api(
        &a,
        "PATCH",
        &format!("/v1/memory/{id}"),
        json!({"tags": ["work"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = api(
        &b,
        "DELETE",
        &format!("/v1/memory/{}", on_b[0].1.id),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for daemon in [&a, &b, &a, &b] {
        run(daemon).await;
    }
    let (status, kept) = api(&a, "GET", &format!("/v1/memory/{id}"), json!({})).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the remote deletion must not reach a local-only memory"
    );
    assert_eq!(
        (kept["content"].clone(), kept["tags"].clone()),
        (json!("shared then kept"), json!(["work"]))
    );
    let private_id = private["id"].as_str().unwrap();
    assert_eq!(
        api(&a, "GET", &format!("/v1/memory/{private_id}"), json!({}))
            .await
            .0,
        StatusCode::OK
    );
    assert!(synced_memories(&b).await.is_empty());
    dav.abort();
}

#[tokio::test]
async fn a_cancelled_memory_write_keeps_sync_out_until_its_worker_finishes() {
    let root = tempfile::tempdir().unwrap();
    let (a, _b, dav) = paired(root.path()).await;
    let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let memory = a.shared.clone();
    let caller = tokio::spawn(async move {
        memory
            .memory
            .write(move |store| {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                store.create(
                    openside_memory::NewMemory::note("written by an aborted caller"),
                    "test",
                )
            })
            .await
    });
    tokio::task::spawn_blocking(move || started_rx.recv().unwrap())
        .await
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    // The worker is still writing: its guard outlives the aborted caller.
    assert!(a.shared.memory.writers.try_write().is_err());
    let running = tokio::spawn(api_shared(
        a.shared.clone(),
        "POST",
        "/v1/sync/run",
        json!({}),
    ));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !running.is_finished(),
        "sync must wait for the in-flight memory write"
    );
    release_tx.send(()).unwrap();
    let (status, report) = running.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{report}");
    dav.abort();
}

#[tokio::test]
async fn an_exhausted_counter_fails_runs_explicitly_and_keeps_the_edit_pending() {
    let root = tempfile::tempdir().unwrap();
    let (a, _b, dav) = paired(root.path()).await;
    let path = a.shared.config.data_dir.join("sync").join("state.json");
    let mut state = openside_sync::ClientState::load(&path).unwrap().unwrap();
    state.lamport = 1 << 62;
    state.save(&path).unwrap();
    let (status, _) = api(&a, "POST", "/v1/pinned-tabs", json!({"url": URL})).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = api(&a, "POST", "/v1/sync/run", json!({})).await;
    assert_eq!(
        (status, body["code"].clone()),
        (StatusCode::CONFLICT, json!("sync_counter_exhausted")),
        "{body}"
    );
    let (_, status) = api(&a, "GET", "/v1/sync", json!({})).await;
    assert_eq!(
        (status["exhausted"].clone(), status["pending"].clone()),
        (json!(true), json!(1)),
        "{status}"
    );
    dav.abort();
}
