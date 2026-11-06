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

    // A restart, and any other device, see the same answer; the recorded outcome is final.
    let daemon = daemon_with(root.path(), |_| {}).await;
    let tcp = api::router(daemon.shared.clone(), false);
    let reader = token(&daemon, vec![CapabilityScope::Read]).await;
    let (_, body) = call(&tcp, "GET", "/v1/onboarding", Value::Null, Some(&reader)).await;
    assert_eq!(body["state"], "skipped");
    let unix = api::router(daemon.shared.clone(), true);
    let (status, body) = call(
        &unix,
        "PUT",
        "/v1/onboarding",
        json!({"state": "completed"}),
        None,
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str(), body["state"].as_str()),
        (
            StatusCode::CONFLICT,
            Some("onboarding_already_set"),
            Some("skipped")
        )
    );

    // A damaged file never hides the guide for good, and the next outcome can be recorded.
    std::fs::write(
        daemon.shared.config.data_dir.join("onboarding.json"),
        b"{not json",
    )
    .unwrap();
    let (_, body) = call(&unix, "GET", "/v1/onboarding", Value::Null, None).await;
    assert_eq!(body["state"], "pending");
    let (status, body) = call(
        &unix,
        "PUT",
        "/v1/onboarding",
        json!({"state": "completed"}),
        None,
    )
    .await;
    assert_eq!(
        (status, body["state"].as_str()),
        (StatusCode::OK, Some("completed"))
    );
}

fn stored_flag(daemon: &Daemon) -> Value {
    serde_json::from_slice(
        &std::fs::read(daemon.shared.config.data_dir.join("onboarding.json")).unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn a_recorded_outcome_is_never_overwritten() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon_with(root.path(), |_| {}).await;
    let unix = api::router(daemon.shared.clone(), true);
    let (status, first) = call(
        &unix,
        "PUT",
        "/v1/onboarding",
        json!({"state": "completed"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let before = std::fs::read(daemon.shared.config.data_dir.join("onboarding.json")).unwrap();

    for state in ["skipped", "completed"] {
        let (status, body) = call(
            &unix,
            "PUT",
            "/v1/onboarding",
            json!({ "state": state }),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(
            body,
            json!({
                "code": "onboarding_already_set",
                "message": "onboarding already set",
                "state": "completed",
                "updated_at": first["updated_at"],
            })
        );
    }
    assert_eq!(
        std::fs::read(daemon.shared.config.data_dir.join("onboarding.json")).unwrap(),
        before
    );
    let (_, body) = call(&unix, "GET", "/v1/onboarding", Value::Null, None).await;
    assert_eq!(body, first);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_devices_finishing_at_once_record_exactly_one_outcome() {
    for _ in 0..8 {
        let root = tempfile::tempdir().unwrap();
        let daemon = daemon_with(root.path(), |_| {}).await;
        let unix = api::router(daemon.shared.clone(), true);
        let put = |state: &'static str| {
            let unix = unix.clone();
            tokio::spawn(async move {
                call(
                    &unix,
                    "PUT",
                    "/v1/onboarding",
                    json!({ "state": state }),
                    None,
                )
                .await
            })
        };
        let (completed, skipped) = tokio::join!(put("completed"), put("skipped"));
        let (completed, skipped) = (completed.unwrap(), skipped.unwrap());
        let (winner, loser) = match (completed.0, skipped.0) {
            (StatusCode::OK, StatusCode::CONFLICT) => (completed.1, skipped.1),
            (StatusCode::CONFLICT, StatusCode::OK) => (skipped.1, completed.1),
            statuses => panic!("expected one 200 and one 409, got {statuses:?}"),
        };
        assert_eq!(loser["code"], "onboarding_already_set");
        assert_eq!(loser["state"], winner["state"]);
        assert_eq!(loser["updated_at"], winner["updated_at"]);
        assert_eq!(stored_flag(&daemon), winner);
    }
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
    crate::onboarding::set_test_env(&daemon.shared.config.data_dir, &[("HOME", &home)]);
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

    // Writing into an agent's skills folder is for this computer only: a paired admin device
    // elsewhere is refused before anything is written.
    let (status, body) = call(
        &tcp,
        "POST",
        "/v1/external-agents/claude/browser-skill",
        json!({}),
        Some(&admin),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("local_access_required"))
    );
    assert!(!claude.exists());

    let (status, installed) = call(
        &unix,
        "POST",
        "/v1/external-agents/claude/browser-skill",
        json!({}),
        None,
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

/// The skills folder an agent trusts is written only for the person on this computer, and only
/// inside their home, never through a link and never into openside's own data, whatever the
/// agent's environment variables say.
#[tokio::test]
async fn the_browser_skill_is_installed_only_by_the_person_into_a_checked_folder() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let real = home.join("real/claude-config");
    let outside = root.path().join("outside");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let daemon = daemon_with(root.path(), |_| {}).await;
    let data = daemon.shared.config.data_dir.clone();
    let unix = api::router(daemon.shared.clone(), true);
    let install = |agent: &str| {
        let unix = unix.clone();
        let path = format!("/v1/external-agents/{agent}/browser-skill");
        async move { call(&unix, "POST", &path, json!({}), None).await }
    };
    let refused = (
        StatusCode::BAD_REQUEST,
        Some("invalid_skills_dir".to_owned()),
    );
    let code =
        |(status, body): (StatusCode, Value)| (status, body["code"].as_str().map(str::to_owned));

    #[cfg(unix)]
    {
        // A preset folder below a linked parent inside home: the link is not followed.
        std::os::unix::fs::symlink(home.join("real"), home.join("linked")).unwrap();
        crate::onboarding::set_test_env(
            &data,
            &[
                ("HOME", &home),
                ("CLAUDE_CONFIG_DIR", &home.join("linked/claude-config")),
            ],
        );
        assert_eq!(code(install("claude").await), refused);
        assert!(!real.join("skills").exists());
    }
    // Outside home, and inside openside's data folder (itself under this home here).
    crate::onboarding::set_test_env(
        &data,
        &[
            ("HOME", root.path()),
            ("CODEX_HOME", &outside),
            ("OMP_CODING_AGENT_DIR", &data),
        ],
    );
    assert_eq!(code(install("omp").await), refused);
    assert!(!data.join("skills").exists());
    crate::onboarding::set_test_env(&data, &[("HOME", &home), ("CODEX_HOME", &outside)]);
    assert_eq!(code(install("codex").await), refused);
    assert!(!outside.join("skills").exists());

    // An MCP client on the socket has local access but is not the person.
    crate::onboarding::set_test_env(&data, &[("HOME", &home), ("CLAUDE_CONFIG_DIR", &real)]);
    let mut marked = request(
        "POST",
        "/v1/external-agents/claude/browser-skill",
        json!({}),
        None,
    );
    marked
        .headers_mut()
        .insert("x-openside-client", "mcp".parse().unwrap());
    let response = unix.clone().oneshot(marked).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("person_access_required"))
    );
    assert!(!real.join("skills").exists());
    // An agent-client token has no local authority over TCP.
    let (_, agent) = daemon
        .shared
        .db
        .pair_local(
            "agent".into(),
            vec![CapabilityScope::Admin],
            openside_proto::DeviceKind::AgentClient,
        )
        .await
        .unwrap();
    let tcp = api::router(daemon.shared.clone(), false);
    let (status, body) = call(
        &tcp,
        "POST",
        "/v1/external-agents/claude/browser-skill",
        json!({}),
        Some(&agent),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("local_access_required"))
    );
    assert!(!real.join("skills").exists());

    // Positive control: the same preset folder, reached without the link, takes the skill.
    let (status, installed) = install("claude").await;
    assert_eq!(status, StatusCode::CREATED, "{installed}");
    assert!(real.join("skills/openside-browser/SKILL.md").is_file());
}

#[tokio::test]
async fn import_endpoints_need_admin_and_local_person_authority() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon_with(root.path(), |_| {}).await;
    let unix = api::router(daemon.shared.clone(), true);
    let tcp = api::router(daemon.shared.clone(), false);
    let reader = token(&daemon, vec![CapabilityScope::Read]).await;
    let admin = token(&daemon, vec![CapabilityScope::Admin]).await;
    for (method, path, body) in [
        ("GET", "/v1/import/profiles", Value::Null),
        ("POST", "/v1/import", json!({})),
        ("POST", "/v1/import/preview", json!({})),
        (
            "POST",
            "/v1/import/passwords",
            json!({"csv":"url,username,password\n"}),
        ),
        (
            "GET",
            "/v1/import/jobs/00000000000000000000000000000000",
            Value::Null,
        ),
        (
            "GET",
            "/v1/import/jobs/00000000000000000000000000000000/events",
            Value::Null,
        ),
        ("GET", "/v1/import/delivery", Value::Null),
        (
            "POST",
            "/v1/import/delivery/00000000000000000000000000000000/ack",
            json!({"target":"user"}),
        ),
    ] {
        let (status, _) = call(&tcp, method, path, body.clone(), Some(&reader)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        let (status, body) = call(&tcp, method, path, body, Some(&admin)).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("local_access_required")),
            "{path}"
        );
    }
    // These bodies are rejected before discovery, so this test never reads real browsers.
    for body in [
        json!({"profile":"/etc","dry_run":true}),
        json!({"profiles":["chrome:Default"],"keyring_password_cmd":"id"}),
    ] {
        let (status, _) = call(&unix, "POST", "/v1/import", body, None).await;
        assert!(status.is_client_error());
    }
    let (status, _) = call(&unix, "GET", "/v1/import/jobs/not-a-job", Value::Null, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn password_csv_import_requires_vault_dedupes_and_never_returns_values() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon_with(root.path(), |config| {
        config.vault.unlock = openside_config::VaultUnlockSetting::Passphrase;
    })
    .await;
    let unix = api::router(daemon.shared.clone(), true);
    let csv = "name,url,username,password\nExample,https://example.test:443/login,person,import-password-canary-729\nExample,https://example.test/other,person,duplicate-canary\n";
    let (status, body) = call(
        &unix,
        "POST",
        "/v1/import/passwords",
        json!({"csv":csv}),
        None,
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("vault_setup_required"))
    );
    let (status, _) = call(
        &unix,
        "POST",
        "/v1/vault/setup",
        json!({"passphrase":"import-test-passphrase-729"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for _ in 0..2 {
        let (status, report) = call(
            &unix,
            "POST",
            "/v1/import/passwords",
            json!({"csv":csv}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{report}");
        assert_eq!(report["read"], 2);
        assert_eq!(report["imported"], 1);
        assert_eq!(report["skipped"], 1);
        assert!(!report.to_string().contains("import-password-canary"));
    }
    let (status, items) = call(&unix, "GET", "/v1/vault/items", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(items["items"].as_array().unwrap().len(), 1);
    call(&unix, "POST", "/v1/vault/lock", Value::Null, None).await;
    let (status, body) = call(
        &unix,
        "POST",
        "/v1/import/passwords",
        json!({"csv":csv}),
        None,
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("vault_locked"))
    );
    let bytes = std::fs::read(crate::tools::vault::Service::file(&daemon.shared)).unwrap();
    assert!(!bytes
        .windows(b"import-password-canary-729".len())
        .any(|window| window == b"import-password-canary-729"));
}
