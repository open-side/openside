//! Profile snapshots (acceptance A-1/A-2, docs/specs/profiles.md). The committed files under
//! docs/specs/profiles are what tools/token-budget.py measures, so they must equal the `open`
//! message the sidecar actually receives. Regenerate after an intended change with
//! `OPENSIDE_UPDATE_PROFILE_SNAPSHOTS=1 cargo test -p opensided profile_`.

use super::*;
use openside_config::AgentProfile;

fn snapshot_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/specs/profiles")
}

async fn open_with(profile: AgentProfile) -> (tempfile::TempDir, TestRuntime, Daemon, SessionId) {
    let root = tempfile::tempdir().unwrap();
    let mut config = resolved_config(root.path());
    // Browser tools are listed whenever an executable is configured; nothing launches it here,
    // so the snapshot does not depend on whether this machine has Chromium.
    let browser = root.path().join("snapshot-browser");
    std::fs::write(&browser, "#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&browser, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    config.loaded.config.browser.executable = Some(browser);
    config.loaded.config.agent.profile = profile;
    let runtime = TestRuntime::default();
    let daemon = Daemon::open_runtime(config, runtime.clone()).await.unwrap();
    let session = create_session(&daemon, "profile", PolicyMode::Full, Some(root.path())).await;
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request(
            "POST",
            "/v1/sessions/profile/messages",
            json!({"message_id": "m1", "text": "hello", "mode": "queue"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    until(|| !runtime.0.lock().opened.is_empty()).await;
    (root, runtime, daemon, session)
}

/// Compare with the committed file, or rewrite it when explicitly requested.
fn check_snapshot(name: &str, actual: &str) {
    let path = snapshot_dir().join(name);
    if std::env::var_os("OPENSIDE_UPDATE_PROFILE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(snapshot_dir()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_default();
    if expected != actual {
        let mut diff = String::new();
        for (index, (old, new)) in expected
            .lines()
            .map(Some)
            .chain(std::iter::repeat(None))
            .zip(actual.lines().map(Some).chain(std::iter::repeat(None)))
            .take(expected.lines().count().max(actual.lines().count()))
            .enumerate()
        {
            if old != new {
                diff.push_str(&format!(
                    "line {}:\n- {}\n+ {}\n",
                    index + 1,
                    old.unwrap_or("<none>"),
                    new.unwrap_or("<none>")
                ));
            }
        }
        panic!(
            "docs/specs/profiles/{name} is stale; if the change is intended, rerun with \
             OPENSIDE_UPDATE_PROFILE_SNAPSHOTS=1 and re-check tools/token-budget.py\n{diff}"
        );
    }
}

async fn assert_profile_snapshot(profile: AgentProfile) {
    let (_root, runtime, daemon, session) = open_with(profile).await;
    let spec = runtime.0.lock().opened[0].clone();
    let open = openside_agent_sidecar::open_wire(spec).unwrap();
    assert_eq!(open["config"]["agent"]["profile"], profile.as_str());
    let names: Vec<&str> = open["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    let name = profile.as_str();
    check_snapshot(
        &format!("{name}.tools.json"),
        &format!("{}\n", serde_json::to_string_pretty(&names).unwrap()),
    );
    let payload = json!({
        "system_prompt_parts": open["system_prompt_parts"],
        "tools": open["tools"],
    });
    check_snapshot(
        &format!("{name}.open.json"),
        &format!("{}\n", serde_json::to_string_pretty(&payload).unwrap()),
    );

    // A hidden tool is refused at execution too, so naming it cannot bypass the profile.
    let host = host(&daemon, &session).await;
    // Each of these is visible under `full` in this configuration, so only the profile hides it.
    let other = if profile == AgentProfile::BrowserAgent {
        "browser_open_tab"
    } else {
        "bash"
    };
    let hidden = ["memory_write", "delegate", "skill", other];
    for tool in hidden {
        assert!(!names.contains(&tool), "{name} must not expose {tool}");
        assert!(
            matches!(
                host.call(call(tool, json!({}), "hidden")).await,
                ToolOutcome::Denied { .. }
            ),
            "{name} must deny {tool}"
        );
    }
    daemon.shutdown().await;
}

#[tokio::test]
async fn profile_browser_agent_tools_and_open_payload_match_snapshot() {
    assert_profile_snapshot(AgentProfile::BrowserAgent).await;
}

#[tokio::test]
async fn profile_ultrabrowse_tools_and_open_payload_match_snapshot() {
    assert_profile_snapshot(AgentProfile::Ultrabrowse).await;
}

#[test]
fn narrow_profiles_preserve_front_delegation_tools() {
    for profile in [AgentProfile::BrowserAgent, AgentProfile::Ultrabrowse] {
        for name in [
            "send_message",
            "create_task",
            "cancel_task",
            "get_tasks",
            "steer_task",
            "get_models",
            "team_delegate",
        ] {
            assert!(crate::profiles::allows(profile, name), "{name}");
        }
    }
}

#[tokio::test]
async fn profile_full_keeps_every_tool_and_the_full_prompt() {
    let (_root, runtime, daemon, _session) = open_with(AgentProfile::Full).await;
    let spec = runtime.0.lock().opened[0].clone();
    let names: Vec<&str> = spec.tools.iter().map(|tool| tool.name.as_str()).collect();
    for tool in [
        "memory_write",
        "delegate",
        "skill",
        "browser_open_tab",
        "browser_repl",
        "bash",
    ] {
        assert!(names.contains(&tool), "full must expose {tool}: {names:?}");
    }
    let full = crate::prompt::parts();
    assert_eq!(spec.system_prompt_parts[..full.len()], full[..]);
    daemon.shutdown().await;
}
