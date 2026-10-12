//! First-run guide (F-5): the persisted flag, external-agent detection and skill install,
//! and the scope rules of the import endpoints. Import itself runs against fake browser
//! profiles in `crates/openside-e2e/tests/onboarding.rs`, because discovery reads HOME.

use axum::{http::StatusCode, Router};
use http_body_util::BodyExt;
use openside_proto::CapabilityScope;
use serde_json::{json, Value};
use tower::ServiceExt;

use super::*;

async fn call(
    router: &Router,
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
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn daemon_with(root: &Path, edit: impl FnOnce(&mut openside_config::Config)) -> Daemon {
    let mut config = resolved_config(root);
    edit(&mut config.loaded.config);
    Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap()
}

async fn token(daemon: &Daemon, scopes: Vec<CapabilityScope>) -> String {
    daemon
        .shared
        .db
        .pair_named("device".into(), scopes)
        .await
        .unwrap()
        .1
}

#[tokio::test]
async fn the_guide_flag_is_kept_on_the_daemon_and_only_admin_changes_it() {
    let root = tempfile::tempdir().unwrap();
    {
        let daemon = daemon_with(root.path(), |_| {}).await;
        let unix = api::router(daemon.shared.clone(), true);
        let tcp = api::router(daemon.shared.clone(), false);
        let reader = token(&daemon, vec![CapabilityScope::Read]).await;
        let admin = token(&daemon, vec![CapabilityScope::Admin]).await;

        let (status, body) = call(&unix, "GET", "/v1/onboarding", Value::Null, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"state": "pending", "updated_at": null}));

        let (status, _) = call(&tcp, "GET", "/v1/onboarding", Value::Null, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, body) = call(&tcp, "GET", "/v1/onboarding", Value::Null, Some(&reader)).await;
        assert_eq!(
            (status, body["state"].as_str()),
            (StatusCode::OK, Some("pending"))
        );
        let (status, body) = call(
            &tcp,
            "PUT",
            "/v1/onboarding",
            json!({"state": "skipped"}),
            Some(&reader),
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("forbidden"))
        );

        // `pending` is not something a client can write: reopening the guide leaves the flag.
        let (status, body) = call(
            &unix,
            "PUT",
            "/v1/onboarding",
            json!({"state": "pending"}),
            None,
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::BAD_REQUEST, Some("invalid_value"))
        );
        let (status, _) = call(
            &unix,
            "PUT",
            "/v1/onboarding",
            json!({"state": "bogus"}),
            None,
        )
        .await;
        assert!(status.is_client_error());

        let (status, body) = call(
            &tcp,
            "PUT",
            "/v1/onboarding",
            json!({"state": "skipped"}),
            Some(&admin),
        )
        .await;
        assert_eq!(
            (status, body["state"].as_str()),
            (StatusCode::OK, Some("skipped"))
        );
        assert!(body["updated_at"].is_string());
    }

    // A restart, and any other device, see the same answer.
    let daemon = daemon_with(root.path(), |_| {}).await;
    let tcp = api::router(daemon.shared.clone(), false);
    let reader = token(&daemon, vec![CapabilityScope::Read]).await;
    let (_, body) = call(&tcp, "GET", "/v1/onboarding", Value::Null, Some(&reader)).await;
    assert_eq!(body["state"], "skipped");
    let unix = api::router(daemon.shared.clone(), true);
    let (_, body) = call(
        &unix,
        "PUT",
        "/v1/onboarding",
        json!({"state": "completed"}),
        None,
    )
    .await;
    assert_eq!(body["state"], "completed");

    // A damaged file never hides the guide for good.
    std::fs::write(
        daemon.shared.config.data_dir.join("onboarding.json"),
        b"{not json",
    )
    .unwrap();
    let (_, body) = call(&unix, "GET", "/v1/onboarding", Value::Null, None).await;
    assert_eq!(body["state"], "pending");
}

fn audit_lines(daemon: &Daemon) -> Vec<Value> {
    std::fs::read_to_string(daemon.shared.config.data_dir.join("audit.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[tokio::test]
async fn external_agents_are_detected_and_get_the_bundled_browser_skill() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("agents");
    // The agent's configuration directory exists, so it counts as installed even when its
    // executable is not on PATH; `ghost` has neither.
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::create_dir_all(home.join("custom-config")).unwrap();
    let (claude, custom, ghost) = (
        home.join(".claude/skills"),
        home.join("custom-config/skills"),
        home.join("nowhere/skills"),
    );
    let daemon = daemon_with(root.path(), |config| {
        config.skills.agents.insert("claude".into(), claude.clone());
        config.skills.agents.insert("custom".into(), custom.clone());
        config.skills.agents.insert("ghost".into(), ghost.clone());
    })
    .await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let reader = token(&daemon, vec![CapabilityScope::Read]).await;
    let admin = token(&daemon, vec![CapabilityScope::Admin]).await;

    let find = |list: &Value, id: &str| -> Value {
        list["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|agent| agent["id"] == id)
            .unwrap_or_else(|| panic!("{id} missing from {list}"))
            .clone()
    };
    let (status, list) = call(&unix, "GET", "/v1/external-agents", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let ids: Vec<_> = list["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| agent["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["claude", "codex", "omp", "custom", "ghost"]);
    let claude_entry = find(&list, "claude");
    assert_eq!(claude_entry["label"], "Claude Code");
    assert_eq!(claude_entry["detected"], true);
    assert_eq!(claude_entry["skill_installed"], false);
    assert_eq!(claude_entry["skills_dir"], claude.to_str().unwrap());
    assert_eq!(find(&list, "custom")["detected"], true);
    assert_eq!(find(&list, "ghost")["detected"], false);

    let (status, _) = call(
        &tcp,
        "GET",
        "/v1/external-agents",
        Value::Null,
        Some(&reader),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, installed) = call(
        &tcp,
        "POST",
        "/v1/external-agents/claude/browser-skill",
        json!({}),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{installed}");
    assert_eq!(installed["agent"], "claude");
    assert_eq!(installed["skill"], "openside-browser");
    assert_eq!(
        installed["path"],
        claude.join("openside-browser").to_str().unwrap()
    );
    assert!(installed["files"].as_u64().unwrap() >= 1);
    let skill_md = std::fs::read_to_string(claude.join("openside-browser/SKILL.md")).unwrap();
    assert!(skill_md.contains("name: openside-browser"), "{skill_md}");

    let (_, list) = call(&unix, "GET", "/v1/external-agents", Value::Null, None).await;
    assert_eq!(find(&list, "claude")["skill_installed"], true);
    assert_eq!(find(&list, "custom")["skill_installed"], false);

    // The same request again keeps what is there until the person asks to replace it.
    std::fs::write(claude.join("openside-browser/SKILL.md"), "edited").unwrap();
    let (status, body) = call(
        &unix,
        "POST",
        "/v1/external-agents/claude/browser-skill",
        json!({}),
        None,
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("exists"))
    );
    assert_eq!(
        std::fs::read_to_string(claude.join("openside-browser/SKILL.md")).unwrap(),
        "edited"
    );
    let (status, _) = call(
        &unix,
        "POST",
        "/v1/external-agents/claude/browser-skill",
        json!({"force": true}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        std::fs::read_to_string(claude.join("openside-browser/SKILL.md")).unwrap(),
        skill_md
    );

    // Only agents in the list: an arbitrary name is not a way to pick a directory.
    for agent in ["unknown", "..", "CLAUDE"] {
        let (status, _) = call(
            &unix,
            "POST",
            &format!("/v1/external-agents/{agent}/browser-skill"),
            json!({}),
            None,
        )
        .await;
        assert!(status.is_client_error(), "{agent} -> {status}");
    }
    let (status, _) = call(
        &tcp,
        "POST",
        "/v1/external-agents/custom/browser-skill",
        json!({}),
        Some(&reader),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(!custom.exists());

    let installs: Vec<_> = audit_lines(&daemon)
        .into_iter()
        .filter(|line| line["tool"] == "external_agent")
        .collect();
    // The refused repeat (409) wrote nothing, so it leaves no line.
    assert_eq!(installs.len(), 2, "{installs:?}");
    assert_eq!(installs[1]["forced"], true);
    assert!(installs.iter().all(|line| line["agent"] == "claude"));
}

#[tokio::test]
async fn import_endpoints_need_admin_and_only_take_discovered_profile_ids() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon_with(root.path(), |_| {}).await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let reader = token(&daemon, vec![CapabilityScope::Read]).await;
    let admin = token(&daemon, vec![CapabilityScope::Admin]).await;

    let (status, _) = call(
        &tcp,
        "GET",
        "/v1/import/profiles",
        Value::Null,
        Some(&reader),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &tcp,
        "POST",
        "/v1/import",
        json!({"profile": "chrome:Default", "dry_run": true}),
        Some(&reader),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, list) = call(
        &tcp,
        "GET",
        "/v1/import/profiles",
        Value::Null,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert!(list["profiles"].is_array());
    // Whether a Chromium is installed is a property of the machine running the test.
    assert!(list["cookies_writable"].is_boolean(), "{list}");

    // A path, a traversal or an id nobody discovered is refused before anything is read.
    for profile in ["/etc", "chrome:../../etc", "chrome:Nope", ""] {
        let (status, body) = call(
            &unix,
            "POST",
            "/v1/import",
            json!({"profile": profile, "dry_run": true}),
            None,
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::BAD_REQUEST, Some("unknown_profile")),
            "{profile}"
        );
    }
    let (status, _) = call(
        &unix,
        "POST",
        "/v1/import",
        json!({"profile": "chrome:Default", "keyring_password_cmd": "id"}),
        None,
    )
    .await;
    assert!(
        status.is_client_error(),
        "no command line may come from the network"
    );
}
