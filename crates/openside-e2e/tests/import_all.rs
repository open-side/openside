//! Local-person onboarding import against copied real-shaped fixtures and a private daemon.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{path::Path, sync::Arc, time::Duration};

use parking_lot::Mutex;

use axum::{extract::State, http::HeaderMap, response::Html, routing::get, Router};
use openside_e2e::Harness;
use openside_import::testing::{
    set_cookie_times, write_saved_passwords, BookmarkEntry, ChromeProfile, CookieSpec, Encryption,
    FirefoxProfile, HistoryEntry,
};
use serde_json::{json, Value};

const DAEMON: &str = env!("CARGO_BIN_EXE_e2e-opensided");
const PASSWORD: &str = "import-csv-password-canary-836";
const COOKIE: &str = "import-session-cookie-canary-836";
const OLDER_COOKIE: &str = "older-import-cookie-canary-836";

fn copy_tree(source: &Path, target: &Path) {
    std::fs::create_dir_all(target).unwrap();
    for entry in std::fs::read_dir(source).unwrap().map(Result::unwrap) {
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target.join(entry.file_name()));
        } else {
            std::fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
    }
}

fn source_bytes(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn visit(root: &Path, dir: &Path, rows: &mut Vec<(String, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).unwrap().map(Result::unwrap) {
            if entry.file_type().unwrap().is_dir() {
                visit(root, &entry.path(), rows);
            } else {
                rows.push((
                    entry
                        .path()
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    std::fs::read(entry.path()).unwrap(),
                ));
            }
        }
    }
    let mut rows = Vec::new();
    visit(root, root, &mut rows);
    rows.sort();
    rows
}

async fn completed(harness: &Harness, id: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let job = harness
                .request("GET", &format!("/v1/import/jobs/{id}"), None)
                .await;
            assert_eq!(job.status, 200, "{}", job.body);
            if job.body["state"] != "running" {
                assert_eq!(job.body["state"], "completed", "{}", job.body);
                return job.body;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("import job completes")
}

#[tokio::test]
async fn all_profiles_preview_run_progress_vault_and_agent_login() {
    let executable = match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(path) => path,
        Err(reason) => {
            eprintln!("SKIP: import-all smoke needs Chromium: {reason}");
            return;
        }
    };
    let Some(mut harness) = Harness::prepare(DAEMON, &json!({"responses":[]})) else {
        return;
    };
    harness.append_config(&format!(
        "[browser]\nexecutable = {:?}\n[vault]\nunlock = \"passphrase\"\n",
        executable.to_str().unwrap()
    ));
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    async fn page(
        State(seen): State<Arc<Mutex<Vec<String>>>>,
        headers: HeaderMap,
    ) -> Html<&'static str> {
        seen.lock().push(
            headers
                .get("cookie")
                .and_then(|header| header.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
        );
        Html("<!doctype html><title>Import login</title><p>Local fixture</p>")
    }
    let server = tokio::spawn({
        let router = Router::new().route("/", get(page)).with_state(seen.clone());
        async move {
            axum::serve(listener, router).await.unwrap();
        }
    });
    let visit = HistoryEntry {
        url: url.clone(),
        title: Some("Imported fixture".to_owned()),
        visits_unix_ms: vec![1_700_000_000_000],
    };
    let shaped = harness.root().join("fixture-originals");
    ChromeProfile {
        history: vec![visit.clone()],
        bookmarks: vec![BookmarkEntry {
            folder: vec!["Work".to_owned()],
            title: "Local fixture".to_owned(),
            url: url.clone(),
        }],
        cookies: vec![CookieSpec::new("127.0.0.1", "import_session", COOKIE)],
        encryption: Encryption::Plain,
        ..ChromeProfile::default()
    }
    .write(&shaped.join("chromium"));
    write_saved_passwords(
        &shaped.join("chromium"),
        &[("https://example.test/login", "person", PASSWORD)],
    );
    FirefoxProfile {
        history: vec![visit],
        bookmarks: Vec::new(),
        cookies: vec![CookieSpec::new("127.0.0.1", "import_session", OLDER_COOKIE)],
    }
    .write(&shaped.join("firefox"));
    set_cookie_times(
        &shaped.join("chromium"),
        openside_import::BrowserKind::Chrome,
        "import_session",
        1_700_000_020_000,
        1_700_000_000_000,
    );
    set_cookie_times(
        &shaped.join("firefox"),
        openside_import::BrowserKind::Firefox,
        "import_session",
        1_700_000_010_000,
        1_700_000_001_000,
    );
    let chrome_root = harness.root().join("config/google-chrome");
    copy_tree(&shaped.join("chromium"), &chrome_root.join("Default"));
    std::fs::write(chrome_root.join("Local State"), r#"{"profile":{"info_cache":{"Default":{"name":"Personal","user_name":"person@example.test","avatar_icon":"chrome://theme/IDR_PROFILE_AVATAR_1"}}}}"#).unwrap();
    let firefox_root = harness.root().join("home/.mozilla/firefox");
    copy_tree(
        &shaped.join("firefox"),
        &firefox_root.join("fixture.default-release"),
    );
    std::fs::write(
        firefox_root.join("profiles.ini"),
        "[Profile0]\nName=Firefox personal\nIsRelative=1\nPath=fixture.default-release\n",
    )
    .unwrap();
    let chrome_before = source_bytes(&chrome_root);
    let firefox_before = source_bytes(&firefox_root);
    harness.launch().await;

    let detected = harness.request("GET", "/v1/import/profiles", None).await;
    assert_eq!(detected.status, 200);
    assert_eq!(detected.body["profiles"].as_array().unwrap().len(), 2);
    let chrome_id = detected.body["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|profile| profile["brand"] == "chrome")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    assert_eq!(detected.body["profiles"][0]["name"], "Personal");
    let bad = harness
        .request(
            "POST",
            "/v1/import/preview",
            Some(json!({"profiles":["/etc"],"what":null})),
        )
        .await;
    assert_eq!(bad.status, 400);
    assert_eq!(bad.body["code"], "unknown_profile");
    let preview = harness
        .request("POST", "/v1/import/preview", Some(json!({})))
        .await;
    assert_eq!(preview.status, 200, "{}", preview.body);
    assert_eq!(preview.body["totals"]["history"], 2);
    assert_eq!(preview.body["totals"]["bookmarks"], 1);
    assert_eq!(preview.body["totals"]["cookies"], 3);
    assert!(preview.body["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning.as_str().is_some_and(
            |warning| warning.contains("during import") && warning.contains("ask for access")
        )));
    assert!(!harness.data_dir.join("browser-profile").exists());
    assert_eq!(preview.body["totals"]["passwords"], 1);
    assert_eq!(preview.body["vault_needed"], true);
    let requires_vault = harness.request("POST", "/v1/import", Some(json!({}))).await;
    assert_eq!(requires_vault.status, 409);
    assert_eq!(requires_vault.body["code"], "vault_setup_required");
    assert!(!preview.body.to_string().contains(COOKIE));

    let csv = format!(
        "name,url,username,password\nExample,https://example.test/login,person,{PASSWORD}\n"
    );
    let missing = harness
        .request("POST", "/v1/import/passwords", Some(json!({"csv":csv})))
        .await;
    assert_eq!(missing.status, 409);
    assert_eq!(missing.body["code"], "vault_setup_required");
    let setup = harness
        .request(
            "POST",
            "/v1/vault/setup",
            Some(json!({"passphrase":"import-all-test-passphrase-836"})),
        )
        .await;
    assert!(setup.status == 200 || setup.status == 201, "{}", setup.body);
    for _ in 0..2 {
        let imported = harness
            .request("POST", "/v1/import/passwords", Some(json!({"csv":csv})))
            .await;
        assert_eq!(imported.status, 200, "{}", imported.body);
        assert_eq!(imported.body["imported"], 1);
        assert!(!imported.body.to_string().contains(PASSWORD));
    }
    let items = harness.request("GET", "/v1/vault/items", None).await;
    assert_eq!(items.body["items"].as_array().unwrap().len(), 1);
    harness.request("POST", "/v1/vault/lock", None).await;
    let locked = harness
        .request("POST", "/v1/import/passwords", Some(json!({"csv":csv})))
        .await;
    assert_eq!(locked.status, 409);
    assert_eq!(locked.body["code"], "vault_locked");
    harness
        .request(
            "POST",
            "/v1/vault/unlock",
            Some(json!({"passphrase":"import-all-test-passphrase-836"})),
        )
        .await;

    let mut ids = Vec::new();
    for _ in 0..2 {
        let started = harness.request("POST", "/v1/import", Some(json!({}))).await;
        assert_eq!(started.status, 202, "{}", started.body);
        let id = started.body["id"].as_str().unwrap();
        let job = completed(&harness, id).await;
        assert_eq!(job["completed_profiles"], 2);
        assert_eq!(job["summary"]["totals"]["history"], 2);
        assert_eq!(job["summary"]["totals"]["bookmarks"], 1);
        assert_eq!(job["summary"]["totals"]["cookies"], 1);
        assert_eq!(job["summary"]["totals"]["passwords"], 1);
        assert!(job["summary"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning
                .as_str()
                .is_some_and(|warning| warning.contains("127.0.0.1")
                    && warning.contains("most recently used"))));
        assert!(!job.to_string().contains(COOKIE));
        assert!(!job.to_string().contains(PASSWORD));
        ids.push(id.to_owned());
    }
    let items = harness.request("GET", "/v1/vault/items", None).await;
    assert_eq!(items.body["items"].as_array().unwrap().len(), 1);
    let (status, progress) = harness
        .get_bytes(&format!("/v1/import/jobs/{}/events", ids[0]))
        .await;
    assert_eq!(status, 200);
    let progress = String::from_utf8(progress).unwrap();
    assert!(progress.contains("event: import_progress"));
    assert!(progress.contains("\"state\":\"completed\""));
    assert!(!progress.contains(COOKIE));
    assert_eq!(source_bytes(&chrome_root), chrome_before);
    assert_eq!(source_bytes(&firefox_root), firefox_before);
    let sources = harness.data_dir.join("browser-imports/default/sources");
    let imported_sources = std::fs::read_dir(sources)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(imported_sources.len(), 2);
    for entry in imported_sources {
        let value: Value = serde_json::from_slice(&std::fs::read(entry.path()).unwrap()).unwrap();
        assert_eq!(value["history"].as_array().unwrap().len(), 1);
        assert!(value.get("passwords").is_none());
        assert!(value.get("cookies").is_none());
    }

    let session = harness
        .request("POST", "/v1/sessions", Some(json!({})))
        .await;
    assert_eq!(session.status, 201, "{}", session.body);
    let session = session.body["id"].as_str().unwrap();
    let opened = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/tools/browser_open_tab"),
            Some(json!({"args":{"url":url}})),
        )
        .await;
    assert_eq!(opened.status, 200, "{}", opened.body);
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if seen
                .lock()
                .iter()
                .any(|cookie| cookie.contains(&format!("import_session={COOKIE}")))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap();
    assert!(seen
        .lock()
        .iter()
        .all(|cookie| !cookie.contains(OLDER_COOKIE)));
    let audit = std::fs::read_to_string(harness.data_dir.join("audit.jsonl")).unwrap();
    assert!(audit.contains("previewed"));
    assert!(audit.contains("imported"));
    assert!(!audit.contains(COOKIE));
    assert!(!audit.contains(PASSWORD));
    let invalid_selected = harness
        .request(
            "POST",
            "/v1/import/preview",
            Some(json!({"profiles":[chrome_id],"what":[]})),
        )
        .await;
    assert_eq!(invalid_selected.status, 400);
    harness.assert_no_canaries(&[PASSWORD, COOKIE, OLDER_COOKIE]);
    server.abort();
    harness.kill().await;
}
