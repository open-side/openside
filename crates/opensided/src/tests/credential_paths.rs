#![cfg(unix)]
use super::*;
use crate::credential_paths::ProtectedPaths;
use std::os::unix::fs::symlink;

const CANARY: &str = "test-protected-credential-canary-d712a";

async fn fixture(root: &Path, sandbox: bool) -> (Daemon, std::path::PathBuf) {
    let workspace = root.join("work");
    std::fs::create_dir_all(workspace.join("sub")).unwrap();
    let source = workspace.join("source.txt");
    std::fs::write(&source, CANARY).unwrap();
    symlink(&source, workspace.join("alias.txt")).unwrap();
    let mut config = resolved_config(root);
    config.loaded.config.credentials = json!({"plugin":{"token_file":source}});
    if sandbox {
        config.bwrap = openside_config::find_on_path("bwrap");
    }
    let protected = ProtectedPaths::build(
        &config.loaded.config.credentials,
        Some(&root.join("home")),
        Some(&root.join("codex")),
    );
    let mut daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    // Default-source roots, like every fixture credential path, are disposable and never
    // resolve the test runner's real home directory.
    daemon.shared.credential_paths = Arc::new(protected);
    (daemon, workspace)
}

async fn audit(daemon: &Daemon, session: &SessionId) {
    let events = daemon.shared.db.events(session, 0).await.unwrap();
    assert!(!serde_json::to_string(&events).unwrap().contains(CANARY));
    let mut stack = vec![daemon.shared.config.data_dir.clone()];
    while let Some(path) = stack.pop() {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                stack.push(entry.path());
            } else {
                assert!(
                    !String::from_utf8_lossy(&std::fs::read(entry.path()).unwrap())
                        .contains(CANARY),
                    "{}",
                    entry.path().display()
                );
            }
        }
    }
}

#[tokio::test]
async fn credential_file_tools_refuse_every_mode_and_alias_and_hide_listing_entries() {
    let root = tempfile::tempdir().unwrap();
    let (daemon, workspace) = fixture(root.path(), false).await;
    std::fs::write(workspace.join("ordinary.txt"), "ordinary").unwrap();
    for (index, mode) in [PolicyMode::ReadOnly, PolicyMode::Guard, PolicyMode::Full]
        .into_iter()
        .enumerate()
    {
        let session =
            create_session(&daemon, &format!("mode-{index}"), mode, Some(&workspace)).await;
        let host = host(&daemon, &session).await;
        for path in ["source.txt", "alias.txt", "sub/../source.txt"] {
            for (tool, args) in [
                ("read", json!({"path":path})),
                ("write", json!({"path":path,"content":"replacement"})),
                (
                    "edit",
                    json!({"path":path,"old_text":"unknown","new_text":"replacement"}),
                ),
            ] {
                let outcome = host.call(call(tool, args, &format!("{tool}-{path}"))).await;
                assert!(
                    matches!(outcome, ToolOutcome::Denied { .. }),
                    "{mode:?} {tool} {path}: {outcome:?}"
                );
                assert!(!format!("{outcome:?}").contains(CANARY));
            }
        }
        let result = host
            .call(call("read", json!({"path":"."}), "listing"))
            .await;
        let ToolOutcome::Done { for_model_json } = result else {
            panic!("{result:?}");
        };
        let listing: Value = serde_json::from_str(&for_model_json).unwrap();
        assert_eq!(listing["entries"], json!(["ordinary.txt", "sub"]));
        audit(&daemon, &session).await;
    }
    assert_eq!(
        std::fs::read_to_string(workspace.join("source.txt")).unwrap(),
        CANARY
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn configured_credentials_with_innocuous_names_are_not_collected_as_artifacts() {
    let root = tempfile::tempdir().unwrap();
    let (daemon, workspace) = fixture(root.path(), false).await;
    std::fs::hard_link(workspace.join("source.txt"), workspace.join("hardlink.txt")).unwrap();
    let session = create_session(&daemon, "collection", PolicyMode::Full, Some(&workspace)).await;
    let host = host(&daemon, &session).await;
    let result = host
        .call(call(
            "bash",
            json!({"command":"sleep 0.01; touch source.txt hardlink.txt; printf public > ordinary.txt"}),
            "touch",
        ))
        .await;
    assert!(matches!(result, ToolOutcome::Done { .. }), "{result:?}");
    let saved: Vec<_> = daemon
        .shared
        .db
        .events(&session, 0)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::ArtifactSaved { path, .. } => Some(path),
            _ => None,
        })
        .collect();
    assert_eq!(saved, ["files/ordinary.txt"]);
    audit(&daemon, &session).await;
    daemon.shutdown().await;
}

#[tokio::test]
async fn sandboxed_bash_cannot_read_configured_credentials_even_inside_workspace() {
    let root = tempfile::tempdir().unwrap();
    let (daemon, workspace) = fixture(root.path(), true).await;
    if daemon.shared.config.bwrap.is_none() {
        eprintln!("SKIP: credential sandbox regression requires bubblewrap on PATH");
        daemon.shutdown().await;
        return;
    }
    let session = create_session(&daemon, "sandbox", PolicyMode::Full, Some(&workspace)).await;
    let result = host(&daemon, &session)
        .await
        .call(call(
            "bash",
            json!({"command":"cat source.txt; cat alias.txt; ln source.txt new-link.txt; test ! -e new-link.txt; printf public > ordinary.txt"}),
            "masked",
        ))
        .await;
    let ToolOutcome::Done { for_model_json } = result else {
        panic!("sandbox must initialize and run its positive control: {result:?}");
    };
    assert!(!for_model_json.contains(CANARY));
    let output: Value = serde_json::from_str(&for_model_json).unwrap();
    assert_eq!(output["stdout"], "");
    assert_eq!(output["exit_code"], 0);
    assert!(!workspace.join("new-link.txt").exists());
    assert_eq!(
        std::fs::read_to_string(workspace.join("ordinary.txt")).unwrap(),
        "public"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("source.txt")).unwrap(),
        CANARY
    );
    audit(&daemon, &session).await;
    daemon.shutdown().await;
}

#[tokio::test]
async fn sandbox_refuses_existing_hardlink_aliases_before_running_commands() {
    let root = tempfile::tempdir().unwrap();
    let (daemon, workspace) = fixture(root.path(), true).await;
    std::fs::hard_link(workspace.join("source.txt"), workspace.join("hardlink.txt")).unwrap();
    if daemon.shared.config.bwrap.is_none() {
        eprintln!("SKIP: credential sandbox alias regression requires bubblewrap on PATH");
        daemon.shutdown().await;
        return;
    }
    let session = create_session(
        &daemon,
        "hardlink-sandbox",
        PolicyMode::Full,
        Some(&workspace),
    )
    .await;
    let result = host(&daemon, &session)
        .await
        .call(call(
            "bash",
            json!({"command":"cat hardlink.txt; touch ran.txt"}),
            "alias",
        ))
        .await;
    assert!(matches!(result, ToolOutcome::Denied { .. }), "{result:?}");
    assert!(!workspace.join("ran.txt").exists());
    audit(&daemon, &session).await;
    daemon.shutdown().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn bind_mount_aliases_are_denied_by_descriptor_and_sandbox_preflight() {
    const CHILD_ROOT: &str = "OPENSIDE_CREDENTIAL_BIND_TEST_ROOT";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = std::path::PathBuf::from(root);
        let (daemon, workspace) = fixture(&root, true).await;
        let session =
            create_session(&daemon, "bind-alias", PolicyMode::Full, Some(&workspace)).await;
        let host = host(&daemon, &session).await;
        let alias = root.join("bind-alias/source.txt");
        for (tool, args) in [
            ("read", json!({"path":alias})),
            ("write", json!({"path":alias,"content":"replacement"})),
            (
                "edit",
                json!({"path":alias,"old_text":"unknown","new_text":"replacement"}),
            ),
            (
                "read",
                json!({"path":format!("introspection/{}/environ", std::process::id())}),
            ),
        ] {
            let result = host.call(call(tool, args, tool)).await;
            assert!(!format!("{result:?}").contains(CANARY));
            assert!(matches!(result, ToolOutcome::Denied { .. }), "{result:?}");
        }
        let result = host
            .call(call(
                "bash",
                json!({"command":"cat source.txt; touch ran.txt"}),
                "bind-bash",
            ))
            .await;
        let ToolOutcome::Denied { reason, .. } = result else {
            panic!("sandbox alias was not refused");
        };
        assert!(reason.contains("bind-mount"), "{reason}");
        assert!(!workspace.join("ran.txt").exists());
        audit(&daemon, &session).await;
        daemon.shutdown().await;
        std::fs::write(root.join("host-checked"), b"checked").unwrap();
        return;
    }
    let Some(bwrap) = openside_config::find_on_path("bwrap") else {
        eprintln!("SKIP: bind-mount credential regression requires bubblewrap");
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("work");
    std::fs::create_dir_all(workspace.join("introspection")).unwrap();
    std::fs::create_dir(root.path().join("bind-alias")).unwrap();
    std::fs::create_dir(root.path().join("home")).unwrap();
    std::fs::write(workspace.join("source.txt"), CANARY).unwrap();
    let output = tokio::time::timeout(
        Duration::from_secs(45),
        tokio::process::Command::new(bwrap)
            .args([
                "--die-with-parent",
                "--unshare-user",
                "--bind",
                "/",
                "/",
                "--bind",
            ])
            .arg(&workspace)
            .arg(root.path().join("bind-alias"))
            .args(["--bind", "/proc"])
            .arg(workspace.join("introspection"))
            .arg("--")
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::credential_paths::bind_mount_aliases_are_denied_by_descriptor_and_sandbox_preflight",
                "--nocapture",
            ])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", root.path().join("home"))
            .env(CHILD_ROOT, root.path())
            .env("TEST_API_KEY", CANARY)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!String::from_utf8_lossy(&output.stdout).contains(CANARY));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(CANARY));
    assert!(
        output.status.success(),
        "isolated bind regression failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read(root.path().join("host-checked")).unwrap(),
        b"checked"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("source.txt")).unwrap(),
        CANARY
    );
}
