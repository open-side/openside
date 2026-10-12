use super::*;
use openside_skills::Store;

async fn send(router: &axum::Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(request(method, path, body, None))
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    // Extractor rejections (unknown JSON fields) are plain text, not an ApiError.
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

fn library(daemon: &Daemon) -> std::path::PathBuf {
    daemon.shared.config.data_dir.join("skills")
}

fn names(list: &Value) -> Vec<&str> {
    list["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|skill| skill["name"].as_str().unwrap())
        .collect()
}

fn write_skill(root: &Path, name: &str, description: &str, body: &str) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n{body}"),
    )
    .unwrap();
}

#[tokio::test]
async fn skills_api_create_edit_toggle_delete_and_mention_lookup() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let router = api::router(daemon.shared.clone(), true);

    let (status, empty) = send(&router, "GET", "/v1/skills", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(names(&empty).is_empty() && empty["problems"].as_array().unwrap().is_empty());

    let (status, created) = send(
        &router,
        "POST",
        "/v1/skills",
        json!({
            "name": "pdf-tools",
            "description": "Extract text from PDF files",
            "body": "# PDF\nRun pdftotext.\n",
            "resources": [{"path": "references/usage.md", "content": "usage notes"}]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["source"], "user");
    assert_eq!(created["enabled"], true);
    assert_eq!(created["resources"][0]["path"], "references/usage.md");
    for (name, description) in [
        ("notes", "Keep meeting notes"),
        ("tool-box", "Misc helpers; mentions pdf in text"),
    ] {
        let (status, _) = send(
            &router,
            "POST",
            "/v1/skills",
            json!({"name": name, "description": description, "body": "body"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    let (status, duplicate) = send(
        &router,
        "POST",
        "/v1/skills",
        json!({"name": "pdf-tools", "description": "again", "body": ""}),
    )
    .await;
    assert_eq!(
        (status, duplicate["code"].as_str()),
        (StatusCode::CONFLICT, Some("exists"))
    );
    for bad in [
        json!({"name": "Bad Name", "description": "x"}),
        json!({"name": "ok", "description": ""}),
        json!({"name": "ok", "description": "x", "resources": [{"path": "../escape", "content": "x"}]}),
        json!({"name": "ok", "description": "x", "resources": [{"path": "SKILL.md", "content": "x"}]}),
        json!({"name": "ok", "description": "x", "unknown": 1}),
    ] {
        let (status, _) = send(&router, "POST", "/v1/skills", bad.clone()).await;
        assert!(
            status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
            "{bad} -> {status}"
        );
    }
    assert!(!library(&daemon).join("ok").exists());

    let (_, all) = send(&router, "GET", "/v1/skills", json!({})).await;
    assert_eq!(names(&all), ["notes", "pdf-tools", "tool-box"]);

    // Mention picker: name prefix first, then name substring, then description.
    let (_, mention) = send(&router, "GET", "/v1/skills?q=%2Fpdf", json!({})).await;
    assert_eq!(names(&mention), ["pdf-tools", "tool-box"]);
    let (_, mention) = send(&router, "GET", "/v1/skills?q=TOOL", json!({})).await;
    assert_eq!(names(&mention), ["tool-box", "pdf-tools"]);
    let (_, none) = send(&router, "GET", "/v1/skills?q=zzz", json!({})).await;
    assert!(names(&none).is_empty());

    let (status, detail) = send(&router, "GET", "/v1/skills/pdf-tools", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["body"], "# PDF\nRun pdftotext.\n");
    let (status, _) = send(&router, "GET", "/v1/skills/missing", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(&router, "GET", "/v1/skills/Bad%20Name", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, edited) = send(
        &router,
        "PATCH",
        "/v1/skills/pdf-tools",
        json!({"description": "Read \"PDF\": text and tables", "body": "new body\n"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(edited["description"], "Read \"PDF\": text and tables");
    assert_eq!(edited["body"], "new body\n");

    let (status, off) = send(
        &router,
        "PATCH",
        "/v1/skills/notes",
        json!({"enabled": false}),
    )
    .await;
    assert_eq!((status, &off["enabled"]), (StatusCode::OK, &json!(false)));
    let (_, enabled_only) = send(&router, "GET", "/v1/skills?enabled=true", json!({})).await;
    assert_eq!(names(&enabled_only), ["pdf-tools", "tool-box"]);
    let (_, disabled_only) = send(&router, "GET", "/v1/skills?enabled=false", json!({})).await;
    assert_eq!(names(&disabled_only), ["notes"]);

    let (status, removed) = send(&router, "DELETE", "/v1/skills/tool-box", json!({})).await;
    assert_eq!(
        (status, removed["name"].as_str()),
        (StatusCode::OK, Some("tool-box"))
    );
    let (status, _) = send(&router, "GET", "/v1/skills/tool-box", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(&router, "DELETE", "/v1/skills/tool-box", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    daemon.shutdown().await;
}

#[tokio::test]
async fn invalid_skill_directories_are_reported_without_breaking_the_listing() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let router = api::router(daemon.shared.clone(), true);
    let library = library(&daemon);
    write_skill(&library, "healthy", "Works", "body");
    for (name, text) in [
        ("no-front", "just markdown"),
        ("bad-yaml", "---\nname: bad-yaml\ndescription: [oops\n---\n"),
        ("wrong-name", "---\nname: another\ndescription: x\n---\n"),
        (
            "huge",
            &format!(
                "---\nname: huge\ndescription: x\n---\n{}",
                "x".repeat(100_000)
            ),
        ),
    ] {
        std::fs::create_dir_all(library.join(name)).unwrap();
        std::fs::write(library.join(name).join("SKILL.md"), text).unwrap();
    }
    let (status, list) = send(&router, "GET", "/v1/skills", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(names(&list), ["healthy"]);
    let mut problems: Vec<_> = list["problems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    problems.sort_unstable();
    assert_eq!(problems, ["bad-yaml", "huge", "no-front", "wrong-name"]);
    // Mention lookup keeps working too.
    let (status, mention) = send(&router, "GET", "/v1/skills?q=heal", json!({})).await;
    assert_eq!((status, names(&mention)), (StatusCode::OK, vec!["healthy"]));
    daemon.shutdown().await;
}

#[tokio::test]
async fn project_skills_follow_the_session_workspace_and_stay_read_only() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("repo");
    write_skill(
        &workspace.join(".openside/skills"),
        "repo-helper",
        "Project helper",
        "p",
    );
    std::fs::create_dir_all(&workspace).unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    create_session(&daemon, "with-repo", PolicyMode::Guard, Some(&workspace)).await;
    create_session(&daemon, "no-repo", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), true);

    let (_, plain) = send(&router, "GET", "/v1/skills", json!({})).await;
    assert!(names(&plain).is_empty(), "project skills need a session");
    let (_, no_repo) = send(&router, "GET", "/v1/skills?session=no-repo", json!({})).await;
    assert!(names(&no_repo).is_empty());
    let (status, list) = send(&router, "GET", "/v1/skills?session=with-repo", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(names(&list), ["repo-helper"]);
    assert_eq!(list["skills"][0]["source"], "project");
    let (status, _) = send(&router, "GET", "/v1/skills?session=ghost", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(&router, "GET", "/v1/skills/repo-helper", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, detail) = send(
        &router,
        "GET",
        "/v1/skills/repo-helper?session=with-repo",
        json!({}),
    )
    .await;
    assert_eq!(
        (status, detail["body"].as_str()),
        (StatusCode::OK, Some("p"))
    );

    for (method, body) in [
        ("PATCH", json!({"body": "rewritten"})),
        ("PATCH", json!({"description": "x", "enabled": false})),
        ("DELETE", json!({})),
    ] {
        let (status, error) = send(
            &router,
            method,
            "/v1/skills/repo-helper?session=with-repo",
            body,
        )
        .await;
        assert_eq!(
            (status, error["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("read_only")),
            "{method}"
        );
    }
    let (_, still) = send(
        &router,
        "GET",
        "/v1/skills/repo-helper?session=with-repo",
        json!({}),
    )
    .await;
    assert_eq!(
        (still["body"].as_str(), still["enabled"].clone()),
        (Some("p"), json!(true))
    );

    // The user can still refuse a repository's skill: switching it off writes only the
    // user's own state, never the repository.
    let (status, off) = send(
        &router,
        "PATCH",
        "/v1/skills/repo-helper?session=with-repo",
        json!({"enabled": false}),
    )
    .await;
    assert_eq!(
        (status, &off["enabled"], &off["source"]),
        (StatusCode::OK, &json!(false), &json!("project"))
    );
    assert!(
        crate::skills::index_part(&daemon.shared, workspace.to_str())
            .await
            .is_none()
    );
    let (status, _) = send(
        &router,
        "PATCH",
        "/v1/skills/repo-helper?session=with-repo",
        json!({"enabled": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(workspace
        .join(".openside/skills/repo-helper/SKILL.md")
        .is_file());

    // The prompt marks repository skills as untrusted; user skills carry no marker.
    let index = crate::skills::index_part(&daemon.shared, workspace.to_str())
        .await
        .unwrap();
    assert!(
        index.contains("- repo-helper: [project] Project helper"),
        "{index}"
    );
    assert!(index.contains("untrusted"), "{index}");

    // A user skill of the same name shadows the project's.
    write_skill(&library(&daemon), "repo-helper", "User version", "u");
    let (_, shadowed) = send(
        &router,
        "GET",
        "/v1/skills/repo-helper?session=with-repo",
        json!({}),
    )
    .await;
    assert_eq!(shadowed["source"], "user");
    daemon.shutdown().await;
    drop(daemon);

    let mut config = resolved_config(root.path());
    config.loaded.config.skills.project = false;
    let daemon = Daemon::open_runtime(config, runtime.clone()).await.unwrap();
    let router = api::router(daemon.shared.clone(), true);
    std::fs::remove_dir_all(library(&daemon).join("repo-helper")).unwrap();
    let (_, off) = send(&router, "GET", "/v1/skills?session=with-repo", json!({})).await;
    assert!(
        names(&off).is_empty(),
        "skills.project = false hides project skills"
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn skills_routes_need_read_for_lists_and_admin_for_changes() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    write_skill(&library(&daemon), "probe", "Scope probe", "body");
    let (_, read) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Read])
        .await
        .unwrap();
    let (_, admin) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Admin])
        .await
        .unwrap();
    let remote = api::router(daemon.shared.clone(), false);
    let status = |method: &'static str, path: &'static str, body: Value, token: Option<String>| {
        let remote = remote.clone();
        async move {
            remote
                .oneshot(request(method, path, body, token.as_deref()))
                .await
                .unwrap()
                .status()
        }
    };
    for path in ["/v1/skills", "/v1/skills?q=pro", "/v1/skills/probe"] {
        assert_eq!(
            status("GET", path, json!({}), None).await,
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
        assert_eq!(
            status("GET", path, json!({}), Some(read.clone())).await,
            StatusCode::OK,
            "{path}"
        );
    }
    let create = json!({"name": "made", "description": "d"});
    for (method, path, body) in [
        ("POST", "/v1/skills", create.clone()),
        ("PATCH", "/v1/skills/probe", json!({"enabled": false})),
        ("DELETE", "/v1/skills/probe", json!({})),
    ] {
        assert_eq!(
            status(method, path, body, Some(read.clone())).await,
            StatusCode::FORBIDDEN,
            "{method} {path}"
        );
    }
    assert!(library(&daemon).join("probe").exists());
    assert_eq!(
        status("POST", "/v1/skills", create, Some(admin.clone())).await,
        StatusCode::CREATED
    );
    assert_eq!(
        status("DELETE", "/v1/skills/probe", json!({}), Some(admin)).await,
        StatusCode::OK
    );
    daemon.shutdown().await;
}

async fn model(daemon: &Daemon, session: &SessionId, args: Value, id: &str) -> ToolOutcome {
    host(daemon, session)
        .await
        .call(call("skill", args, id))
        .await
}

fn done(outcome: ToolOutcome) -> Value {
    let ToolOutcome::Done { for_model_json } = outcome else {
        panic!("expected a completed tool: {outcome:?}");
    };
    serde_json::from_str(&for_model_json).unwrap()
}

#[tokio::test]
async fn skill_index_offers_only_enabled_skills_and_the_tool_loads_them_on_demand() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let library = library(&daemon);
    std::fs::create_dir_all(library.join("alpha")).unwrap();
    std::fs::write(
        library.join("alpha/SKILL.md"),
        "---\nname: alpha\ndescription: >\n  Alpha does <b>things</b>\n  across lines\n---\nALPHA BODY\n",
    )
    .unwrap();
    write_skill(&library, "beta", "Beta is switched off", "BETA BODY\n");
    std::fs::create_dir_all(library.join("alpha/references")).unwrap();
    std::fs::write(library.join("alpha/references/guide.md"), "GUIDE TEXT").unwrap();
    std::fs::create_dir_all(library.join("broken")).unwrap();
    std::fs::write(library.join("broken/SKILL.md"), "no frontmatter").unwrap();
    Store::new(&library).set_enabled("beta", false).unwrap();

    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), true);
    let (status, _) = send(
        &router,
        "POST",
        "/v1/sessions/session/messages",
        json!({"message_id": "m1", "text": "hello", "mode": "queue"}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    until(|| !runtime.0.lock().opened.is_empty()).await;
    let spec = runtime.0.lock().opened[0].clone();
    assert!(spec.tools.iter().any(|tool| tool.name == "skill"));
    let index = spec
        .system_prompt_parts
        .iter()
        .find(|part| part.contains("<available_skills>"))
        .expect("index part");
    assert!(
        index.contains("- alpha: Alpha does bthings/b across lines"),
        "{index}"
    );
    assert!(
        !index.contains("beta") && !index.contains("broken"),
        "{index}"
    );
    assert!(
        !index.contains("ALPHA BODY"),
        "bodies stay out of the prompt: {index}"
    );
    assert_eq!(index.matches("<available_skills>").count(), 1);

    let loaded = done(model(&daemon, &session, json!({"name": "alpha"}), "load-1").await);
    assert_eq!(loaded["instructions"], "ALPHA BODY\n");
    assert_eq!(loaded["source"], "user");
    assert_eq!(loaded["resources"], json!(["references/guide.md"]));
    let resource = done(
        model(
            &daemon,
            &session,
            json!({"name": "alpha", "resource": "references/guide.md"}),
            "load-2",
        )
        .await,
    );
    assert_eq!(resource["content"], "GUIDE TEXT");

    for (args, id) in [
        (json!({"name": "beta"}), "disabled"),
        (json!({"name": "ghost"}), "missing"),
        (json!({"name": "broken"}), "invalid"),
    ] {
        match model(&daemon, &session, args, id).await {
            ToolOutcome::Denied { reason, .. } => assert_eq!(reason, "skill_not_found", "{id}"),
            other => panic!("{id}: {other:?}"),
        }
    }
    for resource in [
        "../../../etc/passwd",
        "/etc/passwd",
        ".secret",
        "references",
    ] {
        let outcome = model(
            &daemon,
            &session,
            json!({"name": "alpha", "resource": resource}),
            "escape",
        )
        .await;
        assert!(
            !matches!(outcome, ToolOutcome::Done { .. }),
            "{resource}: {outcome:?}"
        );
    }
    // Removing a skill after the index was built still stops loads.
    Store::new(&library).remove("alpha").unwrap();
    assert!(matches!(
        model(&daemon, &session, json!({"name": "alpha"}), "gone").await,
        ToolOutcome::Denied { .. }
    ));
    daemon.shutdown().await;
}

#[tokio::test]
async fn without_enabled_skills_the_prompt_has_no_index() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    write_skill(&library(&daemon), "only", "Only skill", "body");
    Store::new(library(&daemon))
        .set_enabled("only", false)
        .unwrap();
    create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let router = api::router(daemon.shared.clone(), true);
    send(
        &router,
        "POST",
        "/v1/sessions/session/messages",
        json!({"message_id": "m1", "text": "hello", "mode": "queue"}),
    )
    .await;
    until(|| !runtime.0.lock().opened.is_empty()).await;
    let spec = runtime.0.lock().opened[0].clone();
    assert!(spec
        .system_prompt_parts
        .iter()
        .all(|part| !part.contains("<available_skills>")));
    daemon.shutdown().await;
}

#[tokio::test]
async fn skill_tool_is_available_in_read_only_sessions() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    write_skill(&library(&daemon), "reader", "Read only use", "body");
    let session = create_session(&daemon, "readonly", PolicyMode::ReadOnly, None).await;
    let specs: Vec<_> = host(&daemon, &session)
        .await
        .specs()
        .into_iter()
        .map(|spec| spec.name)
        .collect();
    assert!(specs.iter().any(|name| name == "skill"));
    let loaded = done(model(&daemon, &session, json!({"name": "reader"}), "ro").await);
    assert_eq!(loaded["instructions"], "body");
    daemon.shutdown().await;
}

#[tokio::test]
async fn skill_tool_reports_the_source_of_project_skills_and_honors_switching_them_off() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("repo");
    write_skill(
        &workspace.join(".openside/skills"),
        "repo-helper",
        "Project helper",
        "REPO BODY",
    );
    std::fs::write(
        workspace.join(".openside/skills/repo-helper/note.md"),
        "NOTE",
    )
    .unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let session = create_session(&daemon, "s", PolicyMode::Guard, Some(&workspace)).await;
    let loaded = done(model(&daemon, &session, json!({"name": "repo-helper"}), "p1").await);
    assert_eq!(
        (loaded["source"].as_str(), loaded["instructions"].as_str()),
        (Some("project"), Some("REPO BODY"))
    );
    let resource = done(
        model(
            &daemon,
            &session,
            json!({"name": "repo-helper", "resource": "note.md"}),
            "p2",
        )
        .await,
    );
    assert_eq!(
        (resource["source"].as_str(), resource["content"].as_str()),
        (Some("project"), Some("NOTE"))
    );

    let router = api::router(daemon.shared.clone(), true);
    let (status, _) = send(
        &router,
        "PATCH",
        "/v1/skills/repo-helper?session=s",
        json!({"enabled": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(matches!(
        model(&daemon, &session, json!({"name": "repo-helper"}), "p3").await,
        ToolOutcome::Denied { .. }
    ));
    daemon.shutdown().await;
}
