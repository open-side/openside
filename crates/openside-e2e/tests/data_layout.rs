//! D-5: audit files created by real daemon/sidecar/browser operations, not source text.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
use openside_config::data_layout::{self, StorageRoot};
use openside_e2e::Harness;
use openside_proto::EventKind;
use serde_json::{json, Value};
use std::path::Path;

/// NSS's shared certificate store, created by the browser in the inherited XDG data home.
const NSS_STORE: &[&str] = &[
    "nssdb",
    "nssdb/cert9.db",
    "nssdb/key4.db",
    "nssdb/pkcs11.txt",
];
/// Caches the browser's GPU and font libraries keep in the inherited XDG cache home, as for
/// any app. Driver caches differ per machine; a new name here needs the same reasoning.
const USER_CACHES: &[&str] = &[
    "fontconfig",
    "mesa_shader_cache",
    "mesa_shader_cache_db",
    "radv_builtin_shaders",
];
/// Runtime entries the browser's libraries keep in the inherited XDG runtime dir (GSettings).
const USER_RUNTIME: &[&str] = &["dconf"];

fn audit(root: &Path, registry: StorageRoot) -> usize {
    audit_except(root, registry, &[])
}

/// Like `audit`, but `foreign` top-level entries belong to the user's environment, not to
/// openside; their contents are checked separately.
fn audit_except(root: &Path, registry: StorageRoot, foreign: &[&str]) -> usize {
    let mut pending = vec![root.to_path_buf()];
    let mut count = 0;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).unwrap() {
            let entry = entry
                .unwrap_or_else(|error| panic!("cannot audit {}: {error}", directory.display()));
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap();
            if foreign.iter().any(|name| relative == Path::new(name)) {
                continue;
            }
            assert!(
                registry.documents(relative),
                "undocumented {registry:?} path: {}",
                relative.display()
            );
            count += 1;
            if entry.file_type().unwrap().is_dir() {
                pending.push(path);
            }
        }
    }
    count
}

/// Every path below `root`, relative to it, sorted. A missing directory is empty; one that
/// cannot be read fails the audit rather than hiding its contents.
fn tree(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => panic!("cannot audit {}: {error}", directory.display()),
        };
        for entry in entries {
            let entry = entry
                .unwrap_or_else(|error| panic!("cannot audit {}: {error}", directory.display()));
            let path = entry.path();
            found.push(path.strip_prefix(root).unwrap().display().to_string());
            if entry.file_type().unwrap().is_dir() {
                pending.push(path);
            }
        }
    }
    found.sort();
    found
}

#[tokio::test]
async fn every_created_data_config_runtime_and_cache_path_is_documented() {
    let executable = match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("SKIP storage browser audit: {error}. Set OPENSIDE_BROWSER and rerun.");
            return;
        }
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://127.0.0.1:{}/",
        listener.local_addr().unwrap().port()
    );
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route(
                "/",
                axum::routing::get(|| async { axum::response::Html("<h1>Storage audit</h1>") }),
            ),
        )
        .await
        .unwrap();
    });
    let first = json!({"parts":[{"type":"tool_call","name":"browser_open_tab","args":{"url":url},"id":"open"}]});
    let done = json!({"parts":[{"type":"text","text":"audit completed"}]});
    let mut script = json!({"responses":[first, done]});
    let Some(mut h) = Harness::prepare(env!("CARGO_BIN_EXE_e2e-opensided"), &script) else {
        eprintln!("SKIP storage sidecar audit: run uv sync --project agent and rerun.");
        server.abort();
        return;
    };
    h.append_config(&format!(
        "[daemon]\ndata_dir = {}\n[browser]\nexecutable = {}\n",
        json!(h.data_dir),
        json!(executable)
    ));
    h.launch().await;
    let created = h
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"mode":"full","workspace":h.workspace()})),
        )
        .await;
    assert_eq!(created.status, 201);
    let session = created.body["id"].as_str().unwrap();
    let mut stream = h.events(session, 0).await;
    assert_eq!(
        h.request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            Some(json!({"message_id":"first","text":"open page","mode":"now"}))
        )
        .await
        .status,
        202
    );
    let events = stream.settled().await;
    let tab = events
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::ToolEnded {
                tool_call_id,
                result,
                is_error: false,
                ..
            } if tool_call_id == "open" => result["tab_id"].as_str(),
            _ => None,
        })
        .unwrap();
    script["responses"].as_array_mut().unwrap().extend([
        json!({"parts":[{"type":"tool_call","name":"browser_screenshot","args":{"tab_id":tab,"output":true},"id":"capture"}]}), done,
    ]);
    h.replace_script(&script);
    h.kill_sidecar();
    let _ = stream.settled().await;
    assert_eq!(
        h.request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            Some(json!({"message_id":"second","text":"capture page","mode":"now"}))
        )
        .await
        .status,
        202
    );
    let events = stream.settled().await;
    assert!(events.iter().any(|e| matches!(&e.kind, EventKind::ToolEnded { tool_call_id, is_error: false, .. } if tool_call_id == "capture")), "{events:?}");
    let policy = h.request("GET", "/v1/policy", None).await;
    assert_eq!(
        h.request("PUT", "/v1/policy", Some(policy.body))
            .await
            .status,
        200
    );
    assert_eq!(h.request("GET", "/v1/push/vapid", None).await.status, 200);
    let config = h.request("GET", "/v1/settings/config", None).await;
    assert_eq!(
        h.request(
            "PUT",
            "/v1/settings/config",
            Some(json!({"config":config.body["config"]}))
        )
        .await
        .status,
        200
    );
    assert!(h.data_dir.join("artifacts").is_dir());
    openside_config::crash_reports::write_report(&h.data_dir, Some("fixture.rs"), Some(1)).unwrap();
    let docs = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/specs/data-layout.md"),
    )
    .unwrap();
    assert_eq!(docs, data_layout::markdown());
    // The browser inherits the user's XDG environment on purpose (native-messaging hosts
    // need it), so libraries inside it write where they would for any app: NSS keeps the
    // user's shared certificate store in `$XDG_DATA_HOME/pki/nssdb`. The harness points
    // XDG_DATA_HOME at the data dir itself; in a real install that store sits beside
    // `openside/`. Everything else under each root is openside's and documented, and the
    // inherited roots hold nothing but the entries listed here.
    let nss = tree(&h.data_dir.join("pki"));
    assert!(
        nss.iter().all(|path| NSS_STORE.contains(&path.as_str())),
        "unexpected NSS store contents: {nss:?}"
    );
    assert!(audit_except(&h.data_dir, StorageRoot::Data, &["pki"]) > 10);
    assert!(audit(&h.root().join("config/openside"), StorageRoot::Config) > 0);
    assert!(audit(&h.root().join("run/openside"), StorageRoot::Runtime) > 0);
    assert!(StorageRoot::Runtime.documents(Path::new(h.socket.file_name().unwrap())));
    let socket = h.socket.file_name().unwrap().to_string_lossy().into_owned();
    for (name, owned, user) in [
        ("config", vec!["openside"], &[][..]),
        ("run", vec!["openside", socket.as_str()], USER_RUNTIME),
        ("cache", vec![], USER_CACHES),
    ] {
        let root = h.root().join(name);
        let foreign: Vec<String> = match std::fs::read_dir(&root) {
            Ok(entries) => entries
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|entry| !owned.contains(&entry.as_str()) && !user.contains(&entry.as_str()))
                .collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => panic!("cannot audit {}: {error}", root.display()),
        };
        assert!(
            foreign.is_empty(),
            "unexpected entries in the inherited {name} root: {foreign:?}; tree {:?}",
            tree(&root)
        );
    }
    // The browser's own disk cache is pinned in its profile.
    assert!(h.data_dir.join("browser-profile/cache").is_dir());
    let doctor = h
        .cli(
            env!("CARGO_BIN_EXE_e2e-openside"),
            &["doctor", "--json"],
            None,
        )
        .await;
    let doctor = serde_json::Deserializer::from_slice(&doctor.stdout)
        .into_iter::<Value>()
        .map(Result::unwrap)
        .find(|value| value["checks"].is_array())
        .unwrap();
    assert!(doctor["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check["id"] == "crash_reports"
            && check["summary"]
                .as_str()
                .unwrap()
                .contains("1 local redacted")));
    h.kill().await;
    server.abort();
}
