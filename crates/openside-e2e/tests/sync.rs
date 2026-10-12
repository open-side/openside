//! B-9 end to end: two real daemons, the real CLI (`openside sync setup|pair|now|status`)
//! and one loopback fake WebDAV share with credentials. The live policy, a pinned tab, a
//! vault item and a tagged memory reach the second device, concurrent edits converge, and
//! neither the share nor either device's files hold the item's secret in plaintext.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use openside_e2e::Harness;
use openside_vault::{AccessPolicy, AllowedOrigin, Entry, PassphraseLock, Secret, Vault};
use serde_json::{json, Value};

const DAEMON: &str = env!("CARGO_BIN_EXE_e2e-opensided");
const CLI: &str = env!("CARGO_BIN_EXE_e2e-openside");
const PASSWORD: &str = "E2E-SYNC-PASSWORD-CANARY-9d41";
const USERNAME: &str = "e2e-sync-user-canary";
const URL: &str = "https://e2e-pinned-canary.example.test/board";
const TITLE: &str = "E2ESyncTitleCanary";
const MEMORY: &str = "E2EMemoryCanary prefers window seats";
const POLICY_ORIGIN: &str = "https://e2e-policy-canary.example.test";
const DAV_USER: &str = "e2e-dav";
const DAV_PASSWORD: &str = "e2e-dav-password-canary";

fn script(harness: &Harness, name: &str, line: &str) -> String {
    let path = harness.root().join(name);
    std::fs::write(&path, format!("#!/bin/sh\nprintf '%s\\n' '{line}'\n")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path.to_string_lossy().into_owned()
}

async fn device(webdav: &str, with_item: bool) -> Option<Harness> {
    let mut harness = Harness::prepare(DAEMON, &json!({"responses": []}))?;
    let vault_path = harness.root().join("device.vault");
    let mut vault = Vault::create(
        &vault_path,
        harness.root().join("setup.audit.jsonl"),
        &Secret::new("e2e-vault-passphrase".into()),
        PassphraseLock::default(),
    )
    .unwrap();
    if with_item {
        vault
            .put(
                Entry::new(
                    "mail".into(),
                    vec![AllowedOrigin::parse("https://mail.example.test").unwrap()],
                    Secret::new(USERNAME.into()),
                    Secret::new(PASSWORD.into()),
                    None,
                    AccessPolicy::WhileUnlocked,
                )
                .unwrap(),
            )
            .unwrap();
    }
    drop(vault);
    let vault_unlock = script(&harness, "vault-unlock.sh", "e2e-vault-passphrase");
    let sync_unlock = script(&harness, "sync-unlock.sh", "e2e shared sync passphrase");
    let dav_password = script(&harness, "dav-password.sh", DAV_PASSWORD);
    harness.append_config(&format!(
        "[vault]\npath = {}\npassphrase_cmd = [{}]\n[sync]\nwebdav_url = {}\n\
         webdav_username = {}\nwebdav_password_cmd = [{}]\npassphrase_cmd = [{}]\nsettings = true\n",
        json!(vault_path),
        json!(vault_unlock),
        json!(webdav),
        json!(DAV_USER),
        json!(dav_password),
        json!(sync_unlock),
    ));
    harness.launch().await;
    Some(harness)
}

async fn cli(harness: &Harness, args: &[&str]) -> String {
    let output = harness.cli(CLI, args, None).await;
    assert!(
        output.status.success(),
        "openside {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn field(output: &str, name: &str) -> String {
    output
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{name}\t")))
        .unwrap_or_else(|| panic!("no {name} in {output}"))
        .to_owned()
}

/// Canary occurrences in file contents and in file and directory names under `dir`. A
/// finite canary check, not a proof that nothing else leaks.
fn canary_hits(dir: &Path, canaries: &[&str]) -> usize {
    let mut hits = 0;
    let mut pending = vec![dir.to_path_buf()];
    while let Some(path) = pending.pop() {
        let mut bytes = path
            .strip_prefix(dir)
            .unwrap()
            .to_string_lossy()
            .into_owned()
            .into_bytes();
        if path.is_dir() {
            pending.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
        } else {
            bytes.extend(std::fs::read(&path).unwrap());
        }
        for canary in canaries {
            hits += bytes
                .windows(canary.len())
                .filter(|w| *w == canary.as_bytes())
                .count();
        }
    }
    hits
}

async fn vault_items(harness: &Harness) -> Value {
    let response = harness.request("GET", "/v1/vault/items", None).await;
    assert_eq!(response.status, 200, "{}", response.body);
    response.body["items"].clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn two_devices_sync_settings_pins_vault_and_memories_e2e_encrypted() {
    let dav_root = tempfile::tempdir().unwrap();
    let dav_dir = dav_root.path().join("dav");
    let (addr, dav) = openside_sync::fake_webdav::spawn(
        "127.0.0.1:0".parse().unwrap(),
        dav_dir.clone(),
        Some((DAV_USER, DAV_PASSWORD)),
    )
    .await
    .unwrap();
    let dav_url = format!("http://{addr}/webdav/openside");
    let Some(mut a) = device(&dav_url, true).await else {
        dav.abort();
        return;
    };
    let Some(mut b) = device(&dav_url, false).await else {
        dav.abort();
        return;
    };

    let pinned = a
        .request(
            "POST",
            "/v1/pinned-tabs",
            Some(json!({"url": URL, "title": TITLE})),
        )
        .await;
    assert_eq!(pinned.status, 201, "{}", pinned.body);
    let policy = json!({"default_mode":"guard","allow_origins":[POLICY_ORIGIN],"deny_origins":[],"rules":[]});
    let response = a.request("PUT", "/v1/policy", Some(policy.clone())).await;
    assert_eq!(response.status, 200, "{}", response.body);
    for (content, tags) in [
        (MEMORY, json!(["sync"])),
        ("E2EPrivateMemory stays local", json!([])),
    ] {
        let response = a
            .request(
                "POST",
                "/v1/memory",
                Some(json!({"content": content, "tags": tags})),
            )
            .await;
        assert_eq!(response.status, 201, "{}", response.body);
    }
    let setup = cli(&a, &["sync", "setup"]).await;
    let code = field(&setup, "pairing code");
    assert!(code.starts_with("osync1-"), "{setup}");
    let report = cli(&a, &["sync", "now"]).await;
    assert_eq!(
        (field(&report, "pushed"), field(&report, "vault")),
        ("4".into(), "synced".into())
    );

    let paired = cli(&b, &["sync", "pair", &code]).await;
    assert_eq!(field(&paired, "set_up"), "true");
    let report = cli(&b, &["sync", "now"]).await;
    assert_eq!(
        (field(&report, "applied"), field(&report, "rejected")),
        ("4".into(), "0".into())
    );
    assert_eq!(b.request("GET", "/v1/policy", None).await.body, policy);
    let memories = b
        .request("GET", "/v1/memory?q=E2EMemoryCanary", None)
        .await
        .body;
    assert_eq!(
        memories["hits"].as_array().map(Vec::len),
        Some(1),
        "{memories}"
    );
    assert_eq!(memories["hits"][0]["memory"]["content"], MEMORY);
    let private = b
        .request("GET", "/v1/memory?q=E2EPrivateMemory", None)
        .await
        .body;
    assert_eq!(
        private["hits"],
        json!([]),
        "untagged memories stay on their device"
    );
    let tabs = b.request("GET", "/v1/pinned-tabs", None).await.body;
    assert_eq!(tabs["tabs"], json!([pinned.body]));
    let items = vault_items(&b).await;
    assert_eq!(items[0]["id"], "mail");
    assert_eq!(
        items[0]["origins"],
        json!(["https://mail.example.test:443"])
    );

    // Concurrent edits of the same vault item (policy overrides) on both devices converge
    // to one deterministic winner everywhere.
    for (harness, policy) in [(&a, "never"), (&b, "always")] {
        let response = harness
            .request(
                "PATCH",
                "/v1/vault/items/mail",
                Some(json!({"policy": policy})),
            )
            .await;
        assert_eq!(response.status, 200, "{}", response.body);
    }
    cli(&a, &["sync", "now"]).await;
    cli(&b, &["sync", "now"]).await;
    cli(&a, &["sync", "now"]).await;
    let (on_a, on_b) = (vault_items(&a).await, vault_items(&b).await);
    assert_eq!(on_a, on_b);
    assert!(["never", "always"].contains(&on_a[0]["policy"].as_str().unwrap()));
    let status = cli(&b, &["sync", "status"]).await;
    assert_eq!(
        (field(&status, "unlocked"), field(&status, "pending")),
        ("true".into(), "0".into())
    );

    // The share never sees plaintext: no secret, username, URL, title, item id, memory,
    // policy entry or WebDAV password.
    let dav_canaries = [
        PASSWORD,
        USERNAME,
        "e2e-pinned-canary",
        TITLE,
        "mail.example",
        "\"mail\"",
        "E2EMemoryCanary",
        "e2e-policy-canary",
        DAV_PASSWORD,
    ];
    assert_eq!(canary_hits(&dav_dir, &dav_canaries), 0);
    // On the receiving device the secret lives only inside the encrypted vault file:
    // sync state, pinned tabs, audit log, daemon and sidecar logs never contain it, nor the
    // WebDAV password.
    b.assert_no_canaries(&[PASSWORD, USERNAME, DAV_PASSWORD]);
    a.assert_no_canaries(&[PASSWORD, USERNAME, DAV_PASSWORD]);

    a.kill().await;
    b.kill().await;
    dav.abort();
}
