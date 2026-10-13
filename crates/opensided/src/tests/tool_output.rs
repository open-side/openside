#![cfg(unix)]

use super::*;
use crate::{tool_output, tools::ToolEnv};

async fn task_env(daemon: &Daemon, session: &SessionId) -> ToolEnv {
    let mut env = ToolEnv::new(
        daemon.shared.clone(),
        &daemon.shared.db.state(session).await.unwrap(),
    );
    env.subagent = Some("spill-task".into());
    env
}

fn done(outcome: ToolOutcome) -> Value {
    let ToolOutcome::Done { for_model_json } = outcome else {
        panic!("expected tool success: {outcome:?}");
    };
    assert!(for_model_json.len() <= crate::host::OUTPUT_LIMIT);
    serde_json::from_str(&for_model_json).unwrap()
}

#[tokio::test]
async fn threshold_ranges_scope_and_restart() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "spill", PolicyMode::Full, None).await;
    let env = task_env(&daemon, &session).await;
    let small = json!({"text":"x".repeat(tool_output::THRESHOLD - 11)});
    assert_eq!(
        serde_json::to_vec(&small).unwrap().len(),
        tool_output::THRESHOLD
    );
    assert_eq!(
        tool_output::prepare(&env, small.clone()).await.unwrap(),
        small
    );
    let original = json!({"text":"🙂한글".repeat(8000)});
    let expected = serde_json::to_string(&original).unwrap();
    let report = tool_output::prepare(&env, original).await.unwrap();
    let reference = report["output_ref"].as_str().unwrap().to_owned();
    assert_eq!(report["bytes"], expected.len());
    assert!(report["preview"].as_str().unwrap().len() <= 1024);
    let mut collected = String::new();
    loop {
        let page = tool_output::read(&env, &reference, collected.len(), 4096)
            .await
            .unwrap();
        collected.push_str(page["content"].as_str().unwrap());
        if page["more"] == false {
            break;
        }
    }
    assert_eq!(collected, expected);
    assert!(tool_output::read(&env, &reference, 10, 128).await.is_err());
    host(&daemon, &session)
        .await
        .context_compacted(Some("spill-task".into()));
    assert!(tool_output::read(&env, &reference, 0, 128).await.is_ok());
    assert!(tool_output::read(&env, &reference, 0, 0).await.is_err());
    assert!(tool_output::read(&env, &reference, 0, 4097).await.is_err());
    assert!(tool_output::read(&env, &reference, usize::MAX, 10)
        .await
        .is_err());
    assert!(tool_output::read(&env, "output:../../escape", 0, 10)
        .await
        .is_err());
    let other = create_session(&daemon, "other", PolicyMode::Full, None).await;
    assert!(
        tool_output::read(&task_env(&daemon, &other).await, &reference, 0, 10)
            .await
            .is_err()
    );
    let mut front = task_env(&daemon, &session).await;
    front.subagent = None;
    assert!(tool_output::read(&front, &reference, 0, 10).await.is_err());
    let large = json!({"text":"x".repeat(tool_output::THRESHOLD)});
    assert_eq!(
        tool_output::prepare(&front, large.clone()).await.unwrap(),
        large
    );
    drop(env);
    drop(front);
    daemon.shutdown().await;
    drop(daemon);
    let reopened = super::daemon(root.path(), &runtime).await;
    let env = task_env(&reopened, &session).await;
    // The saved reference, not an in-memory index or browser snapshot base, owns retrieval.
    let first = tool_output::read(&env, &reference, 0, 4096).await.unwrap();
    let next = first["next_offset"].as_u64().unwrap() as usize;
    let second = tool_output::read(&env, &reference, next, 128)
        .await
        .unwrap();
    assert!(expected[next..].starts_with(second["content"].as_str().unwrap()));
    reopened.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn private_mode_quota_public_route_and_deletion() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let session = create_session(&daemon, "private", PolicyMode::Full, None).await;
    let env = task_env(&daemon, &session).await;
    let report = tool_output::prepare(&env, json!({"text":"x".repeat(40000)}))
        .await
        .unwrap();
    let reference = report["output_ref"].as_str().unwrap();
    let dir = crate::artifacts::session_dir(&daemon.shared, &session).join(tool_output::DIRECTORY);
    let file = dir.join(&reference[7..]);
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let router = api::router(daemon.shared.clone(), true);
    let response = router
        .clone()
        .oneshot(request(
            "GET",
            &format!(
                "/v1/sessions/{session}/artifacts/{}%2F{}",
                tool_output::DIRECTORY,
                &reference[7..]
            ),
            json!({}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let filler = std::fs::File::create(dir.join("quota-fixture")).unwrap();
    filler.set_len(tool_output::QUOTA as u64).unwrap();
    assert!(
        tool_output::prepare(&env, json!({"text":"y".repeat(40000)}))
            .await
            .unwrap_err()
            .contains("quota")
    );
    assert!(tool_output::read(&env, reference, 0, 100).await.is_ok());
    std::fs::remove_file(&file).unwrap();
    symlink(root.path().join("outside"), &file).unwrap();
    std::fs::write(root.path().join("outside"), "private outside file").unwrap();
    assert!(tool_output::read(&env, reference, 0, 100).await.is_err());
    let response = router
        .oneshot(request(
            "DELETE",
            &format!("/v1/sessions/{session}"),
            json!({}),
            None,
        ))
        .await
        .unwrap();
    assert!(response.status().is_success());
    assert!(!dir.exists());
    assert!(
        tool_output::prepare(&env, json!({"text":"z".repeat(40000)}))
            .await
            .is_err()
    );
    assert!(!dir.exists());
    daemon.shutdown().await;
}

/// Scripted daemon E2E: a real shell Task spills, reads back, and reports to the chief.
#[cfg(unix)]
#[tokio::test]
async fn scripted_task_shell_spill_readback_keeps_chief_thread_clean() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let mut config = resolved_config(root.path());
    config.loaded.config.agent.orchestration = openside_config::Orchestration::Front;
    config.loaded.config.sandbox.required = false;
    config.bwrap = None;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "chief", PolicyMode::Full, Some(&workspace)).await;
    let secret = openside_vault::Secret::new("spill-secret-canary-418".into());
    daemon
        .shared
        .registry
        .redactors
        .write()
        .push(openside_vault::Redactor::new([&secret]));
    let body = format!(
        "spill-secret-canary-418\n{}END-OF-PRIVATE-BODY",
        "PRIVATE-WORKER-TEXT-🙂\n".repeat(5000)
    );
    std::fs::write(workspace.join("input.txt"), body).unwrap();
    daemon
        .shared
        .emit(
            &session,
            None,
            EventKind::SubagentStarted {
                subagent_id: "spill-task".into(),
                parent_tool_call_id: "start-spill".into(),
                task: "Inspect a large result and report its size".into(),
                model: "test/model".into(),
                tools: vec!["bash".into()],
                background: true,
            },
        )
        .await
        .unwrap();
    let parent = host(&daemon, &session).await;
    let invoke = |name: &str, args: Value, id: &str| ToolCall {
        subagent: Some("spill-task".into()),
        ..call(name, args, id)
    };
    let report = done(
        parent
            .call(invoke(
                "bash",
                json!({"command":"cat input.txt"}),
                "large-shell",
            ))
            .await,
    );
    let reference = report["output_ref"].as_str().unwrap();
    assert!(report["bytes"].as_u64().unwrap() > 100_000);
    assert!(!report.to_string().contains("spill-secret-canary-418"));
    let mut complete = String::new();
    loop {
        let page = done(
            parent
                .call(invoke(
                    "read",
                    json!({"path":reference,"offset":complete.len(),"limit":4096}),
                    &format!("range-{}", complete.len()),
                ))
                .await,
        );
        complete.push_str(page["content"].as_str().unwrap());
        if page["more"] == false {
            break;
        }
    }
    let stored: Value = serde_json::from_str(&complete).unwrap();
    assert!(stored["stdout"]
        .as_str()
        .unwrap()
        .ends_with("END-OF-PRIVATE-BODY"));
    assert!(stored["stdout"].as_str().unwrap().contains("[REDACTED]"));
    assert!(!complete.contains("spill-secret-canary-418"));
    let on_disk = std::fs::read_to_string(
        crate::artifacts::session_dir(&daemon.shared, &session)
            .join(tool_output::DIRECTORY)
            .join(&reference[7..]),
    )
    .unwrap();
    assert_eq!(on_disk, complete);
    assert!(matches!(
        parent
            .call(call("read", json!({"path":reference}), "front-read"))
            .await,
        ToolOutcome::Denied { .. }
    ));
    assert!(matches!(
        parent
            .call(invoke(
                "read",
                json!({"path":"input.txt"}),
                "ungranted-file"
            ))
            .await,
        ToolOutcome::Denied { .. }
    ));
    done(
        parent
            .call(call(
                "send_message",
                json!({"text":"The large result was inspected successfully."}),
                "chief-report",
            ))
            .await,
    );
    let events = daemon.shared.db.events(&session, 0).await.unwrap();
    let thread = serde_json::to_string(&events).unwrap();
    for private in [
        "PRIVATE-WORKER-TEXT",
        "END-OF-PRIVATE-BODY",
        "spill-secret-canary-418",
    ] {
        assert!(
            !thread.contains(private),
            "chief event stream leaked {private}"
        );
    }
    assert!(thread.contains("The large result was inspected successfully."));
    assert!(events
        .iter()
        .any(|event| matches!(&event.kind, EventKind::SubagentProgress { .. })));
    daemon.shutdown().await;
}

#[tokio::test]
async fn ultrabrowse_implicit_read_does_not_grant_filesystem_access() {
    let root = tempfile::tempdir().unwrap();
    let mut config = resolved_config(root.path());
    config.loaded.config.agent.profile = openside_config::AgentProfile::Ultrabrowse;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "research", PolicyMode::Full, Some(root.path())).await;
    let env = task_env(&daemon, &session).await;
    std::fs::write(root.path().join("existing.txt"), "not granted").unwrap();
    assert!(crate::profiles::allows(
        openside_config::AgentProfile::Ultrabrowse,
        "read"
    ));
    let reader = daemon.shared.registry.get("read").unwrap();
    assert!(reader
        .actions(&json!({"path":"existing.txt"}), &env)
        .await
        .is_err());
    let report = tool_output::prepare(&env, json!({"text":"x".repeat(40000)}))
        .await
        .unwrap();
    assert!(reader
        .actions(&json!({"path":report["output_ref"]}), &env)
        .await
        .is_ok_and(|actions| actions.is_empty()));
    daemon.shutdown().await;
}
