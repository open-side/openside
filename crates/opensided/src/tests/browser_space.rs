//! Daemon-level acceptance for isolated browser spaces (`browser_space: "isolated"`): storage
//! is separate from the shared profile and from every other space, the vault fills inside a
//! space, browsing is not indexed into memory, archive and deletion wipe the space, and a
//! browser that did not survive a daemon restart reports `browser_space_reset` once.
//! Real Chromium, loopback fixtures and temporary directories only.

use super::browser_tabs::alive;
use super::*;
use axum::{response::Html, routing::get, Router};
use openside_browser::Browser;
use openside_proto::MemoryKind;
use openside_vault::{AccessPolicy, AllowedOrigin, Entry, PassphraseLock, Secret, Vault};
use std::path::PathBuf;

const PAGE: &str = "<!doctype html><title>Space</title><h1>Space page</h1>\
<form onsubmit=\"event.preventDefault();document.body.innerHTML='<h1>Signed in</h1>'\">\
<label>Username<input id=username autocomplete=username></label>\
<label>Password<input id=password type=password autocomplete=current-password></label>\
<button>Sign in</button></form><a id=download href=/download download>Download</a>";
const USERNAME: &str = "username-canary-space-5531";

async fn site() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let router = Router::new()
        .route("/", get(|| async { Html(PAGE) }))
        .route("/isolated", get(|| async { Html(PAGE) }))
        .route("/login", get(|| async { Html(PAGE) }))
        .route("/otp", get(|| async { Html("<!doctype html><title>Email verification</title><form><label>Email code<input name=code autocomplete=one-time-code></label><button>Verify</button></form>") }))
        .route("/choice", get(|| async { Html("<!doctype html><title>Choose verification</title><h2>Choose how to verify</h2><button>Text message</button><button>Email</button>") }))
        .route("/setup", get(|| async { Html("<!doctype html><title>Authenticator setup</title><a href='otpauth://totp/Space:alice?secret=JBSWY3DPEHPK3PXP&issuer=Space'>Add authenticator</a>") }))
        .route("/download", get(|| async {
            ([
                (axum::http::header::CONTENT_TYPE, "application/octet-stream"),
                (axum::http::header::CONTENT_DISPOSITION, "attachment; filename=space.txt"),
            ], "synthetic private space download\n")
        }));
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (origin, task)
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

async fn open_tab(daemon: &Daemon, session: &SessionId, url: &str, id: &str) -> String {
    model(tool(daemon, session, "browser_open_tab", json!({"url":url}), id).await)["tab_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn shared_browser(daemon: &Daemon) -> Arc<Browser> {
    daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap()
}

async fn create(daemon: &Daemon, body: Value) -> (StatusCode, Value) {
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request("POST", "/v1/sessions", body, None))
        .await
        .unwrap();
    (response.status(), json_body(response).await)
}

async fn isolated_session(daemon: &Daemon) -> SessionId {
    let (status, body) = create(daemon, json!({"browser_space":"isolated"})).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["browser_space"], "isolated");
    SessionId::new(body["id"].as_str().unwrap())
}

async fn cookie(browser: &Browser, tab: &str) -> Value {
    browser.evaluate(tab, "document.cookie").await.unwrap()
}

struct Kill(Vec<u32>);
impl Drop for Kill {
    fn drop(&mut self) {
        for pid in &self.0 {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(i32::try_from(*pid).unwrap()),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
    }
}

async fn download(browser: &Browser, owner: &SessionId, tab: &str) -> PathBuf {
    let context = browser.space(owner.as_str()).await.unwrap();
    let directory = browser.space_downloads(&context);
    browser
        .evaluate(tab, "document.getElementById('download').click(); true")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if std::fs::read_dir(&directory).unwrap().any(|entry| {
                std::fs::read_to_string(entry.unwrap().path())
                    .ok()
                    .as_deref()
                    == Some("synthetic private space download\n")
            }) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    directory
}
#[tokio::test]
async fn isolated_spaces_separate_storage_fill_the_vault_and_are_wiped_on_archive_and_delete() {
    let root = tempfile::tempdir().unwrap();
    let executable = match test_browser() {
        Ok(executable) => executable,
        Err(error) => {
            eprintln!("SKIP isolated browser space test: {error}");
            return;
        }
    };
    let (origin, fixture) = site().await;
    let passphrase = Secret::new("fixture-passphrase".into());
    let vault_path = root.path().join("vault.db");
    let mut vault = Vault::create(
        &vault_path,
        root.path().join("setup-audit.jsonl"),
        &passphrase,
        PassphraseLock::default(),
    )
    .unwrap();
    vault
        .put(
            Entry::new(
                "fixture".into(),
                vec![AllowedOrigin::parse(&origin).unwrap()],
                Secret::new(USERNAME.into()),
                Secret::new("password-canary-space-5531".into()),
                None,
                AccessPolicy::Always,
            )
            .unwrap(),
        )
        .unwrap();
    drop(vault);
    let pass_file = root.path().join("passphrase.txt");
    std::fs::write(&pass_file, "fixture-passphrase\n").unwrap();
    let config = || {
        let mut config = resolved_config(root.path());
        config.loaded.config.browser.executable = Some(executable.clone());
        config.loaded.config.models.default = Some(std::any::type_name::<TestRuntime>().into());
        config.loaded.config.vault.path = Some(vault_path.clone());
        config.loaded.config.vault.passphrase_cmd =
            Some(vec!["cat".into(), pass_file.to_string_lossy().into_owned()]);
        config
    };
    let daemon = Daemon::open_runtime(config(), TestRuntime::default())
        .await
        .unwrap();

    // Incognito already has its own browser; asking for both is a contradiction.
    let (status, body) = create(
        &daemon,
        json!({"incognito":true,"browser_space":"isolated"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_browser_space");
    let (status, body) = create(&daemon, json!({})).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        body["browser_space"], "shared",
        "the shared profile stays the default"
    );
    let normal = SessionId::new(body["id"].as_str().unwrap());
    let first = isolated_session(&daemon).await;
    let second = isolated_session(&daemon).await;

    let normal_tab = open_tab(&daemon, &normal, &format!("{origin}/"), "n-open").await;
    let first_tab = open_tab(&daemon, &first, &format!("{origin}/isolated"), "a-open").await;
    let second_tab = open_tab(&daemon, &second, &format!("{origin}/isolated"), "b-open").await;
    let browser = shared_browser(&daemon).await;
    let mut kill = Kill(vec![browser.process_id()]);
    for session in [&first, &second] {
        let own = daemon
            .shared
            .registry
            .browser
            .for_session(&daemon.shared, session)
            .await
            .map_err(|error| error.0)
            .unwrap();
        assert_eq!(
            own.process_id(),
            browser.process_id(),
            "spaces live in the shared Chromium"
        );
    }
    let tabs = browser.tabs().await;
    let context = |id: &str| {
        tabs.iter()
            .find(|tab| tab.id == id)
            .unwrap()
            .browser_context_id
            .clone()
    };
    assert_eq!(context(&normal_tab), None);
    assert_eq!(context(&first_tab), browser.space(first.as_str()).await);
    assert_eq!(context(&second_tab), browser.space(second.as_str()).await);
    assert!(context(&first_tab).is_some() && context(&first_tab) != context(&second_tab));

    // Same origin, three storage jars.
    assert_eq!(
        browser
            .evaluate(&first_tab, "document.cookie = 'space=first; path=/'; localStorage.setItem('k', 'first'); document.cookie")
            .await
            .unwrap(),
        json!("space=first")
    );
    assert_eq!(cookie(&browser, &normal_tab).await, json!(""));
    assert_eq!(cookie(&browser, &second_tab).await, json!(""));
    assert_eq!(
        browser
            .evaluate(&second_tab, "localStorage.getItem('k')")
            .await
            .unwrap(),
        Value::Null
    );
    browser
        .evaluate(&normal_tab, "document.cookie = 'space=shared; path=/'")
        .await
        .unwrap();
    assert_eq!(cookie(&browser, &first_tab).await, json!("space=first"));

    // Ownership and policy are unchanged: another session cannot reach a space tab.
    assert!(matches!(
        tool(
            &daemon,
            &normal,
            "browser_snapshot",
            json!({"tab_id":first_tab}),
            "cross"
        )
        .await,
        ToolOutcome::Denied { .. }
    ));

    // The vault fills inside the space.
    let username =
        super::browser::reference(&daemon, &first, &first_tab, "Username", "a-user").await;
    let password =
        super::browser::reference(&daemon, &first, &first_tab, "Password", "a-pass").await;
    assert_eq!(
        model(
            tool(
                &daemon,
                &first,
                "vault_login",
                json!({"tab_id":first_tab,"site":"fixture","username_ref":username,"password_ref":password}),
                "a-vault",
            )
            .await
        ),
        json!({"status":"filled"})
    );
    assert_eq!(
        browser
            .evaluate(&first_tab, "document.getElementById('username').value")
            .await
            .unwrap(),
        json!(USERNAME)
    );

    // Space browsing leaves no history; shared browsing still does.
    let visits = daemon
        .shared
        .memory
        .run(|store| store.search("", Some(MemoryKind::History), 100))
        .await
        .unwrap()
        .hits;
    assert!(!visits.is_empty(), "the shared session is indexed");
    assert!(
        visits.iter().all(|hit| !hit
            .memory
            .url
            .as_deref()
            .unwrap_or_default()
            .contains("/isolated")),
        "isolated sessions are not indexed"
    );

    let private_downloads = download(&browser, &first, &first_tab).await;
    // Archive disposes the space: its tab closes and its data is gone.
    let router = api::router(daemon.shared.clone(), true);
    let path = format!("/v1/sessions/{first}");
    let response = router
        .clone()
        .oneshot(request("PATCH", &path, json!({"archived":true}), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(browser.space(first.as_str()).await, None);
    assert!(browser.tabs().await.iter().all(|tab| tab.id != first_tab));
    assert!(
        !private_downloads.exists(),
        "archive wipes private downloads"
    );
    assert_eq!(browser.spaces().await.len(), 1);
    assert_eq!(
        browser.prune_spaces().await.unwrap(),
        0,
        "no context is left behind"
    );
    // Archived: browsing (tool or REPL, both through `for_session`) is refused and recreates
    // nothing, and a repeated archive changes nothing either.
    let tabs_before = browser.tabs().await.len();
    let refused = tool(
        &daemon,
        &first,
        "browser_open_tab",
        json!({"url":format!("{origin}/isolated")}),
        "a-archived",
    )
    .await;
    assert!(
        format!("{refused:?}").contains("session_archived"),
        "{refused:?}"
    );
    let repl = daemon
        .shared
        .registry
        .browser
        .for_session(&daemon.shared, &first)
        .await;
    assert!(repl.is_err_and(|error| error.0.starts_with("session_archived")));
    let response = router
        .clone()
        .oneshot(request("PATCH", &path, json!({"archived":true}), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        browser.space(first.as_str()).await,
        None,
        "no space recreated"
    );
    assert_eq!(
        browser.tabs().await.len(),
        tabs_before,
        "no tab opened anywhere"
    );
    // A tab-creating path that bypasses `for_session` (the vault ceremony) fails closed
    // instead of falling back to the shared profile.
    assert!(matches!(
        browser
            .create_agent_tab(first.as_str(), "about:blank")
            .await,
        Err(openside_browser::BrowserError::SpaceClosed(_))
    ));
    assert_eq!(browser.tabs().await.len(), tabs_before);
    // Unarchived and used again: a fresh, empty space, without a reset notice.
    let response = router
        .clone()
        .oneshot(request("PATCH", &path, json!({"archived":false}), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let fresh = open_tab(&daemon, &first, &format!("{origin}/isolated"), "a-again").await;
    assert_eq!(cookie(&browser, &fresh).await, json!(""));
    assert_eq!(
        browser
            .evaluate(&fresh, "localStorage.getItem('k')")
            .await
            .unwrap(),
        Value::Null
    );

    // Deletion disposes it too.
    let response = router
        .clone()
        .oneshot(request(
            "DELETE",
            &format!("/v1/sessions/{second}"),
            json!({}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(browser.space(second.as_str()).await, None);
    assert!(browser.tabs().await.iter().all(|tab| tab.id != second_tab));
    assert_eq!(browser.spaces().await.len(), 1);
    assert_eq!(cookie(&browser, &normal_tab).await, json!("space=shared"));

    // A clean daemon stop ends Chromium and with it the space; the next start tells the
    // session once, then it continues in a new empty space.
    let private_downloads = download(&browser, &first, &fresh).await;
    let pid = browser.process_id();
    drop(browser);
    daemon.shutdown().await;
    drop(daemon);
    tokio::time::timeout(Duration::from_secs(10), async {
        while alive(pid) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the browser exits with the daemon");
    assert!(
        !private_downloads.exists(),
        "clean shutdown wipes private downloads"
    );
    std::fs::create_dir_all(&private_downloads).unwrap();
    std::fs::write(private_downloads.join("crash-leftover"), "private-download").unwrap();
    let restarted = Daemon::open_runtime(config(), TestRuntime::default())
        .await
        .unwrap();
    assert!(
        !private_downloads.exists(),
        "dead endpoint recovery sweeps stale downloads before browsing"
    );
    let reset = tool(
        &restarted,
        &first,
        "browser_open_tab",
        json!({"url":format!("{origin}/isolated")}),
        "a-reset",
    )
    .await;
    assert!(
        format!("{reset:?}").contains("browser_space_reset"),
        "{reset:?}"
    );
    let browser = shared_browser(&restarted).await;
    kill.0.push(browser.process_id());
    let tab = open_tab(&restarted, &first, &format!("{origin}/isolated"), "a-after").await;
    assert_eq!(cookie(&browser, &tab).await, json!(""));
    assert!(browser.space(first.as_str()).await.is_some());
    let pid = browser.process_id();
    drop(browser);
    restarted.shutdown().await;
    drop(restarted);
    tokio::time::timeout(Duration::from_secs(10), async {
        while alive(pid) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("no browser process is left behind");

    // A malformed state file names no browser and no space: the start survives it, nothing
    // is restored, and the isolated session still browses only in a fresh space of its own.
    let state = openside_config::data_layout::DataPath::BrowserState.path(&config().data_dir);
    std::fs::write(&state, b"{\"endpoint\": 7, \"spaces\": [garbage").unwrap();
    let third = Daemon::open_runtime(config(), TestRuntime::default())
        .await
        .unwrap();
    let tab = open_tab(&third, &first, &format!("{origin}/isolated"), "a-malformed").await;
    let browser = shared_browser(&third).await;
    kill.0.push(browser.process_id());
    let context = browser.space(first.as_str()).await;
    assert!(context.is_some());
    let listed = browser.tabs().await;
    let entry = listed.iter().find(|entry| entry.id == tab).unwrap();
    assert_eq!(
        entry.browser_context_id, context,
        "never the shared profile"
    );
    assert_eq!(cookie(&browser, &tab).await, json!(""));
    let pid = browser.process_id();
    drop(browser);
    third.shutdown().await;
    tokio::time::timeout(Duration::from_secs(10), async {
        while alive(pid) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("no browser process is left behind");
    fixture.abort();
}

#[tokio::test]
async fn prewarm_waiting_for_the_lifecycle_gate_cannot_create_an_archived_or_deleted_space() {
    let root = tempfile::tempdir().unwrap();
    let Some(mut config) = super::browser::browser_config(root.path()) else {
        return;
    };
    config.loaded.config.models.default = Some(std::any::type_name::<TestRuntime>().into());
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let browser = daemon
        .shared
        .registry
        .browser
        .get(&daemon.shared)
        .await
        .map_err(|error| error.0)
        .unwrap();
    let _kill = Kill(vec![browser.process_id()]);
    for deleted in [false, true] {
        let session = isolated_session(&daemon).await;
        let gate = daemon.shared.registry.browser.gate.lock().await;
        let mut prewarm = std::pin::pin!(daemon
            .shared
            .registry
            .browser
            .prewarm_tab(&daemon.shared, &session));
        assert!(futures_util::poll!(prewarm.as_mut()).is_pending());
        let transition = if deleted {
            EventKind::SessionDeleted {}
        } else {
            EventKind::SessionUpdated {
                mode: None,
                title: None,
                pinned: None,
                archived: Some(true),
            }
        };
        daemon
            .shared
            .emit(&session, None, transition)
            .await
            .unwrap();
        drop(gate);
        prewarm.await;
        assert_eq!(browser.space(session.as_str()).await, None);
        assert!(browser
            .tabs()
            .await
            .iter()
            .all(|tab| tab.owner_session_id.as_deref() != Some(session.as_str())));
    }
    drop(browser);
    daemon.shutdown().await;
}

#[tokio::test]
async fn closing_spaces_refuse_pending_login_choice_otp_and_registration_answers() {
    use super::browser::reference;
    use super::otp::{otp_config, ticket};
    use futures_util::FutureExt;
    let root = tempfile::tempdir().unwrap();
    let Some(mut config) = otp_config(root.path(), vec![]) else {
        return;
    };
    config.loaded.config.tickets.confirm_ttl_secs = 60;
    config.loaded.config.models.default = Some(std::any::type_name::<TestRuntime>().into());
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let (origin, fixture) = site().await;
    let (status, body) = create(&daemon, json!({"browser_space":"isolated","mode":"full"})).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let session = SessionId::new(body["id"].as_str().unwrap());
    let login = open_tab(
        &daemon,
        &session,
        &format!("{origin}/login"),
        "closing-login",
    )
    .await;
    let otp = open_tab(&daemon, &session, &format!("{origin}/otp"), "closing-otp").await;
    let choice = open_tab(
        &daemon,
        &session,
        &format!("{origin}/choice"),
        "closing-choice",
    )
    .await;
    let setup = open_tab(
        &daemon,
        &session,
        &format!("{origin}/setup"),
        "closing-setup",
    )
    .await;
    let field = reference(&daemon, &session, &otp, "Email code", "closing-code-ref").await;
    let mut pending = Vec::new();
    for (name, args, value) in [
        (
            "request_login",
            json!({"tab_id":login}),
            json!({"values":{"f0":"space-user","f1":"space-password"},"save":true}),
        ),
        (
            "request_otp",
            json!({"tab_id":otp,"field_ref":field,"channel":"email"}),
            json!("778811"),
        ),
        (
            "request_choice",
            json!({"tab_id":choice}),
            json!({"option":"o0"}),
        ),
        (
            "register_authenticator",
            json!({"tab_id":setup}),
            json!(true),
        ),
    ] {
        let outcome = tool(&daemon, &session, name, args, name).await;
        let ToolOutcome::Deferred { ticket, .. } = outcome else {
            panic!("{name}: expected pending human step, got {outcome:?}");
        };
        let expected = if matches!(name, "request_login" | "request_choice") {
            "target_unavailable"
        } else {
            "secret_input_failed"
        };
        pending.push((ticket, value, expected));
    }
    let (_, approver) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::HumanStep, CapabilityScope::Approve])
        .await
        .unwrap();
    let browser = shared_browser(&daemon).await;
    let _kill = Kill(vec![browser.process_id()]);
    assert!(Box::pin(browser.close_space(session.as_str()))
        .now_or_never()
        .is_none());
    assert_eq!(browser.closing_spaces().await, 1);
    assert!(browser.tabs().await.iter().any(|tab| tab.id == login));
    browser.record_protocol(true);
    for (id, value, expected) in &pending {
        let (status, body) = super::otp::resolve_as(&daemon, &approver, id, value.clone()).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["code"], *expected);
        assert_eq!(ticket(&daemon, id).await["status"], "pending");
    }
    assert!(
        browser
            .take_protocol_log()
            .iter()
            .all(|(method, _)| !matches!(
                method.as_str(),
                "Input.insertText" | "Input.dispatchKeyEvent" | "Runtime.callFunctionOn"
            )),
        "closed targets cannot receive input or vault fills"
    );
    browser.record_protocol(false);
    assert_eq!(
        browser
            .evaluate(&login, "document.getElementById('password').value")
            .await
            .unwrap(),
        json!("")
    );
    assert_eq!(
        browser
            .evaluate(&otp, "document.querySelector('input').value")
            .await
            .unwrap(),
        json!("")
    );
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request("GET", "/v1/vault/items", json!({}), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["items"], json!([]));
    browser.open_space(session.as_str()).await.unwrap();
    for (id, value, expected) in &pending {
        let (status, body) = super::otp::resolve_as(&daemon, &approver, id, value.clone()).await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "replacement context must not authorize old targets: {body}"
        );
        assert_eq!(body["code"], *expected);
        let response = api::router(daemon.shared.clone(), true)
            .oneshot(request(
                "POST",
                &format!("/v1/tickets/{id}/reject"),
                json!({}),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    browser.settle_spaces().await.unwrap();
    assert_eq!(browser.closing_spaces().await, 0);
    assert!(browser.tabs().await.iter().all(|tab| tab.id != login));
    drop(browser);
    daemon.shutdown().await;
    fixture.abort();
}

#[tokio::test]
async fn owner_can_complete_a_popup_login_in_its_isolated_space_but_other_sessions_cannot() {
    use super::browser::resolve_value;
    let root = tempfile::tempdir().unwrap();
    let Some(mut config) = super::otp::otp_config(root.path(), vec![]) else {
        return;
    };
    config.loaded.config.tickets.confirm_ttl_secs = 60;
    config.loaded.config.models.default = Some(std::any::type_name::<TestRuntime>().into());
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let (origin, fixture) = site().await;
    let (status, body) = create(&daemon, json!({"browser_space":"isolated","mode":"full"})).await;
    assert_eq!(status, StatusCode::CREATED);
    let session = SessionId::new(body["id"].as_str().unwrap());
    let other = isolated_session(&daemon).await;
    let parent = open_tab(&daemon, &session, &format!("{origin}/"), "popup-parent").await;
    let browser = shared_browser(&daemon).await;
    let _kill = Kill(vec![browser.process_id()]);
    browser
        .evaluate(&parent, &format!("window.open('{origin}/login'); true"))
        .await
        .unwrap();
    let popup = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(tab) = browser.tabs().await.into_iter().find(|tab| {
                tab.id != parent
                    && tab.opened_by == openside_browser::OpenedBy::External
                    && tab.owner_session_id.as_deref() == Some(session.as_str())
            }) {
                return tab;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        popup.browser_context_id,
        browser.space(session.as_str()).await
    );
    assert!(
        crate::tools::browser::Controller::owned(&browser, &other, &popup.id)
            .await
            .is_err()
    );
    model(
        tool(
            &daemon,
            &session,
            "browser_snapshot",
            json!({"tab_id":popup.id}),
            "popup-read",
        )
        .await,
    );
    let outcome = tool(
        &daemon,
        &session,
        "request_login",
        json!({"tab_id":popup.id}),
        "popup-login",
    )
    .await;
    let ToolOutcome::Deferred { ticket, .. } = outcome else {
        panic!("popup login must open a human-step ticket: {outcome:?}");
    };
    let response = resolve_value(
        &daemon,
        &ticket,
        json!({"values":{"f0":"popup-user","f1":"popup-password"},"save":false}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["resolution"]["next"], "done");
    assert_eq!(
        browser
            .evaluate(&popup.id, "document.body.innerText")
            .await
            .unwrap(),
        json!("Signed in")
    );
    drop(browser);
    daemon.shutdown().await;
    fixture.abort();
}

#[tokio::test]
async fn browser_get_prunes_orphan_downloads_without_erasing_live_spaces() {
    let root = tempfile::tempdir().unwrap();
    let Some(mut config) = super::browser::browser_config(root.path()) else {
        return;
    };
    config.loaded.config.models.default = Some(std::any::type_name::<TestRuntime>().into());
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let (origin, fixture) = site().await;
    let session = isolated_session(&daemon).await;
    let tab = open_tab(&daemon, &session, &format!("{origin}/"), "downloads-live").await;
    let browser = shared_browser(&daemon).await;
    let mut kill = Kill(vec![browser.process_id()]);
    let downloads = download(&browser, &session, &tab).await;
    let orphan = browser.profile_path().join("space-downloads/orphan");
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("fixture"), "private-download").unwrap();
    drop(
        daemon
            .shared
            .registry
            .browser
            .get(&daemon.shared)
            .await
            .map_err(|error| error.0)
            .unwrap(),
    );
    assert!(!orphan.exists());
    assert!(std::fs::read_dir(&downloads).unwrap().next().is_some());
    browser.terminate().await.unwrap();
    assert!(!downloads.exists());
    std::fs::create_dir_all(&downloads).unwrap();
    std::fs::write(downloads.join("leftover"), "private-download").unwrap();
    let replacement = daemon
        .shared
        .registry
        .browser
        .get(&daemon.shared)
        .await
        .map_err(|error| error.0)
        .unwrap();
    kill.0.push(replacement.process_id());
    assert!(
        !downloads.exists(),
        "a dead browser's private files cannot survive get/relaunch"
    );
    drop(replacement);
    drop(browser);
    daemon.shutdown().await;
    fixture.abort();
}
