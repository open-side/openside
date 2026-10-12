use super::*;
use axum::{response::Html, routing::get, Router};
use openside_vault::{Redactor, Secret};

async fn send(router: &axum::Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(request(method, path, body, None))
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, body)
}

async fn tool(
    daemon: &Daemon,
    session: &SessionId,
    name: &str,
    args: Value,
    id: &str,
) -> ToolOutcome {
    host(daemon, session).await.call(call(name, args, id)).await
}

fn model(outcome: ToolOutcome) -> Value {
    let ToolOutcome::Done { for_model_json } = outcome else {
        panic!("expected completed tool: {outcome:?}");
    };
    serde_json::from_str(&for_model_json).unwrap()
}

/// Every byte under the data dir, so a rejected value can be proven absent from storage.
fn data_bytes(daemon: &Daemon) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut stack = vec![daemon.shared.config.data_dir.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                bytes.extend(std::fs::read(path).unwrap_or_default());
            }
        }
    }
    bytes
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

#[tokio::test]
async fn memory_api_crud_history_health_secret_and_scopes() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let router = api::router(daemon.shared.clone(), true);

    let (status, health) = send(&router, "GET", "/v1/memory/health", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(health["mode"], "lexical_only");
    assert!(health["problem"]
        .as_str()
        .unwrap()
        .contains("tokenizer.json"));
    assert!(health["next_step"]
        .as_str()
        .unwrap()
        .contains("openside memory model fetch"));

    let (status, created) = send(
        &router,
        "POST",
        "/v1/memory",
        json!({"content":"The quarterly report goes to the finance team","tags":["work"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["retention"], "permanent");
    assert_eq!(created["version"], 1);
    let id = created["id"].as_str().unwrap().to_owned();

    let (status, found) = send(
        &router,
        "GET",
        "/v1/memory?q=quarterly&kind=note&limit=5",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(found["mode"], "lexical_only");
    assert_eq!(found["hits"][0]["memory"]["id"], id);
    let (_, none) = send(
        &router,
        "GET",
        "/v1/memory?q=quarterly&kind=history",
        json!({}),
    )
    .await;
    assert!(none["hits"].as_array().unwrap().is_empty());

    let (status, updated) = send(
        &router,
        "PATCH",
        &format!("/v1/memory/{id}"),
        json!({"content":"The annual report goes to the finance team","retention":"days_90"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["version"], 2);
    assert_eq!(updated["retention"], "days_90");
    assert!(updated["expires_at"].is_string());

    let (status, history) = send(
        &router,
        "GET",
        &format!("/v1/memory/{id}/history"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let versions = history["versions"].as_array().unwrap();
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0]["op"], "create");
    assert_eq!(versions[1]["op"], "update");
    assert_eq!(versions[1]["actor"], "api");
    assert!(versions[1]["at"].is_string());

    const CANARY: &str = "sk-memapicanary0123456789abcdefABCDEF";
    for body in [
        json!({"content":format!("my key is {CANARY}")}),
        json!({"content":"login","tags":[format!("password={CANARY}")]}),
        json!({"content":"card 4111 1111 1111 1111"}),
    ] {
        let (status, error) = send(&router, "POST", "/v1/memory", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
        assert_eq!(error["code"], "secret_detected");
        assert!(!error.to_string().contains(CANARY));
    }
    // Multibyte text next to '&' once panicked the entity decoder inside the screen.
    let (status, created) = send(
        &router,
        "POST",
        "/v1/memory",
        json!({"content":"R&D 회의: &가나다라마바사 東京&大阪","tags":["&é"]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let (status, error) = send(
        &router,
        "PATCH",
        &format!("/v1/memory/{id}"),
        json!({"content":CANARY}),
    )
    .await;
    assert_eq!(
        (status, error["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("secret_detected"))
    );

    let (status, removed) = send(&router, "DELETE", &format!("/v1/memory/{id}"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(removed["id"], id);
    for method in ["GET", "PATCH", "DELETE"] {
        let (status, _) = send(&router, method, &format!("/v1/memory/{id}"), json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method}");
    }
    let (_, history) = send(
        &router,
        "GET",
        &format!("/v1/memory/{id}/history"),
        json!({}),
    )
    .await;
    assert_eq!(history["versions"][2]["op"], "delete");
    assert!(
        history["versions"][0]["content"].is_null(),
        "delete scrubs snapshots"
    );

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
    let status = |method: &'static str, path: String, token: String| {
        let remote = remote.clone();
        async move {
            remote
                .oneshot(request(
                    method,
                    &path,
                    json!({"content":"remote note"}),
                    Some(&token),
                ))
                .await
                .unwrap()
                .status()
        }
    };
    let live = daemon
        .shared
        .memory
        .run(|store| store.create(openside_memory::NewMemory::note("scope probe"), "test"))
        .await
        .unwrap()
        .id;
    for (method, path) in [
        ("GET", "/v1/memory?q=note".to_owned()),
        ("GET", "/v1/memory/health".to_owned()),
        ("GET", format!("/v1/memory/{live}")),
        ("GET", format!("/v1/memory/{live}/history")),
    ] {
        assert_eq!(
            status(method, path.clone(), read.clone()).await,
            StatusCode::OK,
            "{method} {path}"
        );
    }
    for (method, path) in [
        ("POST", "/v1/memory".to_owned()),
        ("PATCH", format!("/v1/memory/{live}")),
        ("DELETE", format!("/v1/memory/{live}")),
    ] {
        assert_eq!(
            status(method, path.clone(), read.clone()).await,
            StatusCode::FORBIDDEN,
            "{method} {path}"
        );
    }
    assert_eq!(
        status("GET", "/v1/memory".into(), "not-a-token".into()).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status("PATCH", format!("/v1/memory/{live}"), admin.clone()).await,
        StatusCode::OK
    );
    assert_eq!(
        status("POST", "/v1/memory".into(), admin).await,
        StatusCode::CREATED
    );

    let bytes = data_bytes(&daemon);
    assert!(!contains(&bytes, CANARY));
    assert!(!contains(&bytes, "4111 1111 1111 1111"));
    daemon.shutdown().await;
}

#[tokio::test]
async fn memory_tools_remember_across_sessions_with_tiers_and_vault_redaction() {
    let root = tempfile::tempdir().unwrap();
    let daemon = daemon(root.path(), &TestRuntime::default()).await;
    let guard = create_session(&daemon, "guard", PolicyMode::Guard, None).await;
    let readonly = create_session(&daemon, "readonly", PolicyMode::ReadOnly, None).await;

    let guard_specs: Vec<_> = host(&daemon, &guard)
        .await
        .specs()
        .into_iter()
        .map(|spec| spec.name)
        .collect();
    assert!(guard_specs.iter().any(|name| name == "memory_search"));
    assert!(guard_specs.iter().any(|name| name == "memory_write"));
    let readonly_specs: Vec<_> = host(&daemon, &readonly)
        .await
        .specs()
        .into_iter()
        .map(|spec| spec.name)
        .collect();
    assert!(readonly_specs.iter().any(|name| name == "memory_search"));
    assert!(
        !readonly_specs.iter().any(|name| name == "memory_write"),
        "write tier hidden in read-only"
    );

    let stored = model(
        tool(
            &daemon,
            &guard,
            "memory_write",
            json!({"content":"Preferred airline seat is the aisle","tags":["travel"],"retention":"days_30"}),
            "write-1",
        )
        .await,
    );
    assert_eq!(stored["stored"], true);
    assert_eq!(stored["retention"], "days_30");
    let id = stored["id"].as_str().unwrap().to_owned();
    let multilingual = model(
        tool(
            &daemon,
            &guard,
            "memory_write",
            json!({"content":"R&D 팀 메모 &가나다라마바사 ab&😀😀😀"}),
            "write-multilingual",
        )
        .await,
    );
    assert_eq!(multilingual["stored"], true, "{multilingual}");
    let history = daemon
        .shared
        .memory
        .run({
            let id = id.clone();
            move |store| store.history(&id)
        })
        .await
        .unwrap();
    assert_eq!(history.versions[0].actor, "agent:guard");

    let found = model(
        tool(
            &daemon,
            &readonly,
            "memory_search",
            json!({"query":"airline seat"}),
            "search-1",
        )
        .await,
    );
    assert_eq!(found["results"][0]["id"], id);
    assert_eq!(
        found["results"][0]["content"],
        "Preferred airline seat is the aisle"
    );

    const CANARY: &str = "ghp_memtoolcanary0123456789abcdefABCDEF";
    let outcome = tool(
        &daemon,
        &guard,
        "memory_write",
        json!({"content":format!("token {CANARY}")}),
        "write-secret",
    )
    .await;
    assert!(
        matches!(&outcome, ToolOutcome::Denied { reason, .. } if reason == "secret_detected"),
        "{outcome:?}"
    );

    // A value the vault redactor knows is a secret even without a recognisable shape.
    const VAULT_VALUE: &str = "plain-looking-vault-value-7731";
    daemon
        .shared
        .registry
        .redactors
        .write()
        .push(Redactor::new([&Secret::new(VAULT_VALUE.into())]));
    let outcome = tool(
        &daemon,
        &guard,
        "memory_write",
        json!({"content":format!("remember {VAULT_VALUE}")}),
        "write-vault",
    )
    .await;
    assert!(
        matches!(&outcome, ToolOutcome::Denied { reason, .. } if reason == "secret_detected"),
        "{outcome:?}"
    );

    // Memories written before a value became known are masked on the way out.
    model(
        tool(
            &daemon,
            &guard,
            "memory_write",
            json!({"content":"door code hint is sunflower-4402"}),
            "write-later",
        )
        .await,
    );
    daemon
        .shared
        .registry
        .redactors
        .write()
        .push(Redactor::new([&Secret::new("sunflower-4402".into())]));
    let found = model(
        tool(
            &daemon,
            &guard,
            "memory_search",
            json!({"query":"door code"}),
            "search-2",
        )
        .await,
    );
    assert!(!found.to_string().contains("sunflower-4402"), "{found}");
    let (_, api) = send(
        &api::router(daemon.shared.clone(), true),
        "GET",
        "/v1/memory?q=door",
        json!({}),
    )
    .await;
    assert!(!api.to_string().contains("sunflower-4402"), "{api}");

    let bytes = data_bytes(&daemon);
    assert!(
        !contains(&bytes, CANARY),
        "rejected secret reached the data dir (events, audit or memory)"
    );
    assert!(!contains(&bytes, VAULT_VALUE));
    daemon.shutdown().await;
}

#[tokio::test]
async fn unusable_model_keeps_search_lexical_and_reports_the_problem() {
    let root = tempfile::tempdir().unwrap();
    let mut config = resolved_config(root.path());
    let model_dir = root.path().join("model");
    std::fs::create_dir_all(&model_dir).unwrap();
    std::fs::write(
        model_dir.join("tokenizer.json"),
        b"{\"model\":{\"type\":\"Unigram\"}}",
    )
    .unwrap();
    std::fs::write(model_dir.join("model.safetensors"), b"not safetensors").unwrap();
    config.loaded.config.memory.embedding.model_dir = Some(model_dir);
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let health = daemon.shared.memory.health().await;
    assert_eq!(health.mode, MemorySearchMode::LexicalOnly);
    assert!(health.problem.is_some());
    assert!(health.next_step.is_some());
    daemon.shutdown().await;
}

const PAGE: &str = "<!doctype html><html><head><title>Fixture Garden</title></head><body><h1>Tomato planting guide</h1><p>Plant seedlings after the last frost.</p></body></html>";

#[tokio::test]
async fn browsing_history_is_indexed_except_incognito_sessions_and_secret_filled_documents() {
    let root = tempfile::tempdir().unwrap();
    let executable = match test_browser(root.path()) {
        Ok(executable) => executable,
        Err(error) => {
            eprintln!("SKIP browsing history indexing test: {error}");
            return;
        }
    };
    let config = || {
        let mut config = resolved_config(root.path());
        config.loaded.config.browser.executable = Some(executable.clone());
        config.loaded.config.models.default = Some(std::any::type_name::<TestRuntime>().into());
        config
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let router = Router::new()
        .route("/garden", get(|| async { Html(PAGE) }))
        .route(
            "/private",
            get(|| async { Html("<title>Private Page</title><p>hidden</p>") }),
        )
        .route(
            "/filled",
            get(|| async { Html("<title>Filled Page</title><input id=name>") }),
        )
        .route(
            "/after",
            get(|| async { Html("<title>After Page</title><p>fresh document</p>") }),
        );
    let fixture = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let daemon = Daemon::open_runtime(config(), TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "browse", PolicyMode::Guard, None).await;
    let history = |daemon: &Daemon| {
        let memory = daemon.shared.memory.clone();
        async move {
            memory
                .run(|store| store.search("", Some(MemoryKind::History), 100))
                .await
                .unwrap()
                .hits
                .into_iter()
                .map(|hit| hit.memory)
                .collect::<Vec<_>>()
        }
    };
    let private_urls = |visits: &[Memory]| {
        visits
            .iter()
            .filter(|visit| visit.url.as_deref().unwrap().contains("/private"))
            .count()
    };

    let opened = model(
        tool(
            &daemon,
            &session,
            "browser_open_tab",
            json!({"url":format!("{origin}/garden?token=abc")}),
            "open",
        )
        .await,
    );
    let tab = opened["tab_id"].as_str().unwrap().to_owned();
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap();
    let _cleanup = BrowserCleanup(browser.process_id());
    model(
        tool(
            &daemon,
            &session,
            "browser_snapshot",
            json!({"tab_id":tab}),
            "snap",
        )
        .await,
    );
    let visits = history(&daemon).await;
    assert_eq!(visits.len(), 1, "{visits:?}");
    let visit = &visits[0];
    assert!(visit
        .url
        .as_deref()
        .unwrap()
        .starts_with(&format!("{origin}/garden")));
    assert!(
        !visit.url.as_deref().unwrap().contains("abc"),
        "public_url form drops query values"
    );
    assert_eq!(visit.title.as_deref(), Some("Fixture Garden"));
    assert!(visit.content.contains("Plant seedlings"));
    assert_eq!(visit.retention, Retention::Days30);
    assert!(visit.expires_at.is_some());
    let found = model(
        tool(
            &daemon,
            &session,
            "memory_search",
            json!({"query":"tomato","kind":"history"}),
            "recall",
        )
        .await,
    );
    assert_eq!(found["results"][0]["source"], "web_page");
    assert_eq!(found["results"][0]["untrusted"], true);

    // Privacy is chosen at session creation through the public API and stored durably.
    let router = api::router(daemon.shared.clone(), true);
    let response = router
        .clone()
        .oneshot(request(
            "POST",
            "/v1/sessions",
            json!({"title":"Private","incognito":true}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = json_body(response).await;
    assert_eq!(created["incognito"], true);
    let private = SessionId::new(created["id"].as_str().unwrap());
    let private_tab = model(
        tool(
            &daemon,
            &private,
            "browser_open_tab",
            json!({"url":format!("{origin}/private")}),
            "p-open",
        )
        .await,
    )["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    model(
        tool(
            &daemon,
            &private,
            "browser_snapshot",
            json!({"tab_id":private_tab}),
            "p-snap",
        )
        .await,
    );
    model(
        tool(
            &daemon,
            &private,
            "browser_navigate",
            json!({"tab_id":private_tab,"url":format!("{origin}/private?x=1")}),
            "p-nav",
        )
        .await,
    );
    assert_eq!(private_urls(&history(&daemon).await), 0);

    // The taint check is bound to the document whose content would be indexed.
    model(
        tool(
            &daemon,
            &session,
            "browser_navigate",
            json!({"tab_id":tab,"url":format!("{origin}/filled")}),
            "nav-filled",
        )
        .await,
    );
    let filled = history(&daemon)
        .await
        .into_iter()
        .find(|visit| visit.url.as_deref().unwrap().contains("/filled"))
        .unwrap();
    daemon
        .shared
        .memory
        .run(move |store| store.delete(&filled.id, "test"))
        .await
        .unwrap();
    daemon
        .shared
        .registry
        .browser
        .mark_secret(&daemon.shared, &browser, &tab)
        .await
        .unwrap();
    model(
        tool(
            &daemon,
            &session,
            "browser_snapshot",
            json!({"tab_id":tab}),
            "snap-filled",
        )
        .await,
    );
    assert!(history(&daemon).await.iter().all(|visit| !visit
        .url
        .as_deref()
        .unwrap()
        .contains("/filled")));
    model(
        tool(
            &daemon,
            &session,
            "browser_navigate",
            json!({"tab_id":tab,"url":format!("{origin}/after")}),
            "nav-after",
        )
        .await,
    );
    assert!(
        history(&daemon)
            .await
            .iter()
            .any(|visit| visit.url.as_deref().unwrap().contains("/after")),
        "a new document is indexed again"
    );

    // A snapshot captured before a navigation must not be stored under the new document.
    let state = daemon.shared.db.state(&session).await.unwrap();
    let env = crate::tools::ToolEnv::new(daemon.shared.clone(), &state);
    let stale_document = browser.document_id(&tab).await.unwrap();
    let stale_snapshot = browser.snapshot(&tab).await.unwrap();
    assert!(stale_snapshot.text.contains("fresh document"));
    browser
        .navigate(session.as_str(), &tab, &format!("{origin}/garden"))
        .await
        .unwrap();
    let before = history(&daemon).await;
    crate::tools::memory::index_page(
        &env,
        &browser,
        &tab,
        Some((&stale_snapshot, &stale_document)),
    )
    .await;
    let after = history(&daemon).await;
    assert_eq!(
        before.iter().map(|visit| visit.version).collect::<Vec<_>>(),
        after.iter().map(|visit| visit.version).collect::<Vec<_>>(),
        "a capture from a replaced document changes nothing"
    );
    assert!(after
        .iter()
        .all(|visit| !visit.content.contains("fresh document")));
    let document = browser.document_id(&tab).await.unwrap();
    let snapshot = browser.snapshot(&tab).await.unwrap();
    crate::tools::memory::index_page(&env, &browser, &tab, Some((&snapshot, &document))).await;
    let garden = history(&daemon)
        .await
        .into_iter()
        .find(|visit| visit.url.as_deref().unwrap().ends_with("/garden"))
        .unwrap();
    let earlier = before.iter().find(|visit| visit.id == garden.id).unwrap();
    assert_eq!(
        garden.version,
        earlier.version + 1,
        "a matching capture is indexed"
    );

    drop(browser);
    daemon.shutdown().await;
    drop(daemon);

    // After a restart the private session is still private without any re-registration.
    let daemon = Daemon::open_runtime(config(), TestRuntime::default())
        .await
        .unwrap();
    assert!(daemon.shared.db.state(&private).await.unwrap().incognito);
    let reopened = model(
        tool(
            &daemon,
            &private,
            "browser_open_tab",
            json!({"url":format!("{origin}/private")}),
            "p-open-2",
        )
        .await,
    )["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    // Incognito sessions browse in their own temporary-profile browser (B-10).
    let browser = daemon
        .shared
        .registry
        .browser
        .for_session(&daemon.shared, &private)
        .await
        .map_err(|error| error.0)
        .unwrap();
    let _cleanup_restarted = BrowserCleanup(browser.process_id());
    drop(browser);
    model(
        tool(
            &daemon,
            &private,
            "browser_snapshot",
            json!({"tab_id":reopened}),
            "p-snap-2",
        )
        .await,
    );
    assert_eq!(private_urls(&history(&daemon).await), 0);
    daemon.shutdown().await;
    fixture.abort();
}

struct BrowserCleanup(u32);
impl Drop for BrowserCleanup {
    fn drop(&mut self) {
        #[cfg(unix)]
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(i32::try_from(self.0).unwrap()),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
}
