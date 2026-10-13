//! Daemon-level acceptance for B-7 ① ③ (tab ownership through the browser tools, distinct
//! `tab_asleep` denial), B-10 (incognito temporary profile, deletion, Vault denial) and the
//! daemon half of PP-23 ② (a SIGKILLed shared browser is relaunched and the agent is told).
//! Real Chromium, loopback fixtures and temporary directories only.

use super::*;
use axum::{response::Html, routing::get, Router};
use openside_browser::{Browser, BrowserConfig, OpenedBy};
use openside_vault::{AccessPolicy, AllowedOrigin, Entry, PassphraseLock, Secret, Totp, Vault};
use std::time::Instant;

const PAGE: &str = "<!doctype html><title>Fixture</title><h1>Fixture page</h1>\
<label>Username<input id=username></label><button>Continue</button>";

async fn site() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let router = Router::new()
        .route("/", get(|| async { Html(PAGE) }))
        .route(
            "/next",
            get(|| async { Html("<title>Next</title><h1>Next page</h1>") }),
        );
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (origin, task)
}

fn browser_config(root: &Path) -> Option<Resolved> {
    let mut config = resolved_config(root);
    match test_browser() {
        Ok(executable) => {
            config.loaded.config.browser.executable = Some(executable);
            Some(config)
        }
        Err(error) => {
            eprintln!("SKIP daemon browser tab tests: {error}");
            None
        }
    }
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

fn reason(outcome: ToolOutcome) -> String {
    let ToolOutcome::Denied { reason, .. } = outcome else {
        panic!("expected a denial: {outcome:?}");
    };
    reason
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

async fn incognito_session(daemon: &Daemon, name: &str) -> SessionId {
    let id = SessionId::new(name);
    let stored = id.clone();
    daemon
        .shared
        .db
        .store(move |store| {
            store.create_session_with_privacy(
                stored,
                "Private",
                std::any::type_name::<TestRuntime>(),
                PolicyMode::Full,
                None,
                true,
            )?;
            Ok(())
        })
        .await
        .unwrap();
    id
}

/// A process that exited is gone even while it waits to be reaped.
pub(super) fn alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => stat
                .rsplit_once(") ")
                .is_some_and(|(_, rest)| !rest.starts_with('Z')),
            Err(_) => false,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let output = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "state="])
            .output()
            .unwrap();
        assert!(
            output.status.success()
                || (output.status.code() == Some(1) && output.stderr.is_empty()),
            "process state probe failed: {output:?}"
        );
        output
            .stdout
            .iter()
            .find(|byte| !byte.is_ascii_whitespace())
            .is_some_and(|state| *state != b'Z')
    }
}

async fn exits(pid: u32) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while alive(pid) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("browser process {pid} survived"));
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

fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// B-7 ① through the agent tools: a user tab is never listed, navigated, clicked, read
/// through the tools, captured or closed by the agent. B-7 ③: an asleep agent tab is refused
/// with the distinct `tab_asleep` reason well inside 12 seconds.
#[tokio::test]
async fn user_tabs_are_read_only_and_asleep_tabs_are_refused_with_tab_asleep() {
    let root = tempfile::tempdir().unwrap();
    let Some(config) = browser_config(root.path()) else {
        return;
    };
    let (origin, fixture) = site().await;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "agent", PolicyMode::Full, None).await;
    let tab = open_tab(&daemon, &session, &format!("{origin}/"), "open").await;
    let browser = shared_browser(&daemon).await;
    let _kill = Kill(vec![browser.process_id()]);
    let user = browser
        .tabs()
        .await
        .into_iter()
        .find(|tab| tab.opened_by == OpenedBy::External)
        .expect("the browser's own start tab");
    browser.adopt_user_tab(&user.id).await.unwrap();
    drop(browser);

    for (name, args) in [
        ("browser_snapshot", json!({"tab_id":user.id})),
        (
            "browser_navigate",
            json!({"tab_id":user.id,"url":format!("{origin}/next")}),
        ),
        (
            "browser_act",
            json!({"tab_id":user.id,"target":"e1","action":"click"}),
        ),
        ("browser_screenshot", json!({"tab_id":user.id})),
        ("browser_close_tab", json!({"tab_id":user.id})),
        (
            "browser_repl",
            json!({"tab_id":user.id,"code":"await page.url()"}),
        ),
    ] {
        let outcome = tool(&daemon, &session, name, args, &format!("user-{name}")).await;
        assert!(
            matches!(outcome, ToolOutcome::Denied { .. }),
            "{name} on a user tab must be denied: {outcome:?}"
        );
    }
    let listed = model(tool(&daemon, &session, "browser_tabs", json!({}), "tabs").await);
    assert_eq!(listed.as_array().unwrap().len(), 1, "{listed}");
    assert_eq!(listed[0]["tab_id"], tab);
    let browser = shared_browser(&daemon).await;
    let still = browser
        .tabs()
        .await
        .into_iter()
        .find(|item| item.id == user.id)
        .expect("the user tab stays open");
    assert_eq!(still.opened_by, OpenedBy::User);
    assert_eq!(still.url, user.url, "the user tab was not navigated");

    browser.mark_asleep(&tab, true, false).await.unwrap();
    drop(browser);
    for (name, args) in [
        ("browser_snapshot", json!({"tab_id":tab})),
        (
            "browser_navigate",
            json!({"tab_id":tab,"url":format!("{origin}/next")}),
        ),
    ] {
        let started = Instant::now();
        let denial = reason(tool(&daemon, &session, name, args, &format!("asleep-{name}")).await);
        assert!(denial.starts_with("tab_asleep"), "{name}: {denial}");
        assert!(started.elapsed() < Duration::from_secs(12));
    }
    let listed = model(tool(&daemon, &session, "browser_tabs", json!({}), "tabs-asleep").await);
    assert_eq!(listed[0]["asleep"], true);
    assert_eq!(
        listed[0]["group"],
        Value::Null,
        "stock Chromium has no tab groups"
    );
    shared_browser(&daemon)
        .await
        .mark_asleep(&tab, false, false)
        .await
        .unwrap();
    let snapshot = model(
        tool(
            &daemon,
            &session,
            "browser_snapshot",
            json!({"tab_id":tab}),
            "awake",
        )
        .await,
    );
    assert!(snapshot.to_string().contains("Fixture page"), "{snapshot}");
    daemon.shutdown().await;
    fixture.abort();
}

/// B-10: an incognito session browses in its own Chromium on a 0700 temporary profile under
/// the runtime directory, isolated from the shared profile; the Vault refuses it with
/// `vault_incognito`; the profile is deleted when the session is deleted, when the daemon
/// shuts down, and by the next start after a crash.
#[tokio::test]
async fn incognito_sessions_use_a_deleted_temporary_profile_and_cannot_use_the_vault() {
    let root = tempfile::tempdir().unwrap();
    let Some(mut config) = browser_config(root.path()) else {
        return;
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
                Secret::new("username-canary-incognito-6120".into()),
                Secret::new("password-canary-incognito-6120".into()),
                Some(Totp::parse(Secret::new("JBSWY3DPEHPK3PXP".into())).unwrap()),
                AccessPolicy::Always,
            )
            .unwrap(),
        )
        .unwrap();
    drop(vault);
    let pass_file = root.path().join("passphrase.txt");
    std::fs::write(&pass_file, "fixture-passphrase\n").unwrap();
    config.loaded.config.vault.path = Some(vault_path);
    config.loaded.config.vault.passphrase_cmd =
        Some(vec!["cat".into(), pass_file.to_string_lossy().into_owned()]);
    let executable = config.loaded.config.browser.executable.clone().unwrap();
    let incognito_root = config.loaded.paths.runtime_dir.join("incognito-browsers");
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let normal = create_session(&daemon, "normal", PolicyMode::Full, None).await;
    let private = incognito_session(&daemon, "private").await;
    let normal_tab = open_tab(&daemon, &normal, &format!("{origin}/"), "n-open").await;
    let private_tab = open_tab(&daemon, &private, &format!("{origin}/"), "p-open").await;

    let shared = shared_browser(&daemon).await;
    let incognito = daemon
        .shared
        .registry
        .browser
        .for_session(&daemon.shared, &private)
        .await
        .map_err(|error| error.0)
        .unwrap();
    let mut kill = Kill(vec![shared.process_id(), incognito.process_id()]);
    assert_ne!(shared.process_id(), incognito.process_id());
    let directory = incognito_root.join("private");
    assert_eq!(
        incognito.profile_path(),
        std::fs::canonicalize(directory.join("profile")).unwrap()
    );
    assert_ne!(shared.profile_path(), incognito.profile_path());
    assert_eq!(mode(&directory), 0o700);
    assert_eq!(mode(&directory.join("profile")), 0o700);
    // Storage is isolated: a cookie set in the incognito profile never reaches the shared one.
    assert_eq!(
        incognito
            .evaluate(
                &private_tab,
                "document.cookie = 'incognito=1; path=/'; document.cookie"
            )
            .await
            .unwrap(),
        json!("incognito=1")
    );
    assert_eq!(
        shared
            .evaluate(&normal_tab, "document.cookie")
            .await
            .unwrap(),
        json!("")
    );
    assert!(shared.tabs().await.iter().all(|tab| tab.id != private_tab));
    let private_pid = incognito.process_id();
    drop((shared, incognito));
    let listed = model(tool(&daemon, &private, "browser_tabs", json!({}), "p-tabs").await);
    assert_eq!(listed.as_array().unwrap().len(), 1, "{listed}");
    assert_eq!(listed[0]["tab_id"], private_tab);
    assert!(matches!(
        tool(
            &daemon,
            &normal,
            "browser_snapshot",
            json!({"tab_id":private_tab}),
            "cross"
        )
        .await,
        ToolOutcome::Denied { .. }
    ));

    let specs = host(&daemon, &private).await.specs();
    for name in ["vault_login", "vault_otp", "vault_passkey"] {
        if !specs.iter().any(|spec| spec.name == name) {
            continue;
        }
        let args =
            json!({"tab_id":private_tab,"site":"fixture","username_ref":"e1","password_ref":"e2"});
        assert_eq!(
            reason(tool(&daemon, &private, name, args, &format!("p-{name}")).await),
            "vault_incognito",
            "{name}"
        );
    }
    assert!(
        specs.iter().any(|spec| spec.name == "vault_login"),
        "the vault is configured"
    );
    let normal_denial = tool(&daemon, &normal, "vault_login", json!({}), "n-vault").await;
    assert!(
        !matches!(&normal_denial, ToolOutcome::Denied { reason, .. } if reason == "vault_incognito"),
        "{normal_denial:?}"
    );
    let audit = std::fs::read_to_string(daemon.shared.config.data_dir.join("vault-audit.jsonl"))
        .unwrap_or_default();
    assert!(
        !audit.contains("private"),
        "an incognito session never reached the vault: {audit}"
    );

    // Session end: deleting the session terminates its Chromium and deletes the profile.
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request("DELETE", "/v1/sessions/private", json!({}), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(!directory.exists(), "the incognito profile must be deleted");
    exits(private_pid).await;

    // Daemon shutdown deletes every live incognito profile.
    let second = incognito_session(&daemon, "second").await;
    open_tab(&daemon, &second, &format!("{origin}/"), "s-open").await;
    let second_pid = daemon
        .shared
        .registry
        .browser
        .for_session(&daemon.shared, &second)
        .await
        .map_err(|error| error.0)
        .unwrap()
        .process_id();
    kill.0.push(second_pid);
    assert!(incognito_root.join("second/profile").is_dir());
    daemon.shutdown().await;
    drop(daemon);
    assert!(
        !incognito_root.exists(),
        "shutdown leaves no incognito profile"
    );
    exits(second_pid).await;

    // Crash recovery: a Chromium and profile left by a daemon that died mid-session are
    // terminated and deleted by the next start.
    let crashed = incognito_root.join("crashed");
    let mut leftover = BrowserConfig::new(executable);
    leftover.profile_dir = Some(crashed.join("profile"));
    leftover.detached = true;
    // Detached only tests outliving a dropped controller; it must not outlive the test
    // process if that process is killed without unwinding.
    leftover.die_with_spawner = true;
    let (browser, _events) = Browser::launch(leftover).await.unwrap();
    std::fs::write(
        crashed.join("endpoint.json"),
        serde_json::to_vec(&browser.endpoint()).unwrap(),
    )
    .unwrap();
    let crashed_pid = browser.process_id();
    kill.0.push(crashed_pid);
    drop(browser);
    assert!(
        alive(crashed_pid),
        "a detached browser outlives its controller"
    );
    let mut config = resolved_config(root.path());
    config.loaded.config.browser.executable = Some(test_browser().unwrap());
    let restarted = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    assert!(
        !crashed.exists(),
        "startup deletes a crashed incognito profile"
    );
    exits(crashed_pid).await;
    restarted.shutdown().await;
    fixture.abort();
}

/// PP-23 ② (daemon): after Chromium is SIGKILLed mid-session, the next poll relaunches it on
/// the same managed profile, owners receive `tab.closed` for the tabs that died, a call naming
/// a dead tab (tool or REPL) is refused once with `browser_restarted`, and the next browser
/// call works. A relaunched browser that dies again at once is relaunched only after a
/// backoff, during which calls get `browser_restarting`.
#[tokio::test]
async fn killed_browser_is_relaunched_and_the_agent_is_told_its_tabs_are_gone() {
    let root = tempfile::tempdir().unwrap();
    let Some(config) = browser_config(root.path()) else {
        return;
    };
    let (origin, fixture) = site().await;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "agent", PolicyMode::Full, None).await;
    let tab = open_tab(&daemon, &session, &format!("{origin}/"), "open").await;
    let repl_tab = open_tab(&daemon, &session, &format!("{origin}/"), "open-repl").await;
    let browser = shared_browser(&daemon).await;
    let first_pid = browser.process_id();
    let mut kill = Kill(vec![first_pid]);
    nix::sys::signal::killpg(
        nix::unistd::Pid::from_raw(i32::try_from(first_pid).unwrap()),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while browser.is_connected() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    drop(browser);

    daemon
        .shared
        .registry
        .browser
        .poll(&daemon.shared)
        .await
        .unwrap();
    let relaunched = shared_browser(&daemon).await;
    assert!(relaunched.is_connected());
    assert_ne!(relaunched.process_id(), first_pid);
    kill.0.push(relaunched.process_id());
    let saved: Value = serde_json::from_slice(
        &std::fs::read(daemon.shared.config.data_dir.join("browser-state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["endpoint"]["pid"], relaunched.process_id());
    drop(relaunched);
    wait_event(
        &daemon,
        &session,
        |kind| matches!(kind, EventKind::TabClosed { tab_id } if tab_id == &tab),
    )
    .await;

    let denial = reason(
        tool(
            &daemon,
            &session,
            "browser_snapshot",
            json!({"tab_id":tab}),
            "dead",
        )
        .await,
    );
    assert!(denial.starts_with("browser_restarted"), "{denial}");
    assert!(denial.contains("browser_open_tab"), "{denial}");
    // Told once; the entry is pruned and the id is just an unknown tab from then on.
    let again = reason(
        tool(
            &daemon,
            &session,
            "browser_snapshot",
            json!({"tab_id":tab}),
            "dead-again",
        )
        .await,
    );
    assert!(!again.starts_with("browser_restarted"), "{again}");
    let repl = reason(
        tool(
            &daemon,
            &session,
            "browser_repl",
            json!({"tab_id":repl_tab,"code":"await page.url()"}),
            "dead-repl",
        )
        .await,
    );
    assert!(repl.starts_with("browser_restarted"), "{repl}");
    let fresh = open_tab(&daemon, &session, &format!("{origin}/next"), "reopen").await;
    let snapshot = model(
        tool(
            &daemon,
            &session,
            "browser_snapshot",
            json!({"tab_id":fresh}),
            "snap",
        )
        .await,
    );
    assert!(snapshot.to_string().contains("Next page"), "{snapshot}");

    // Crash loop: the relaunched browser dies again right away.
    let browser = shared_browser(&daemon).await;
    let second_pid = browser.process_id();
    nix::sys::signal::killpg(
        nix::unistd::Pid::from_raw(i32::try_from(second_pid).unwrap()),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while browser.is_connected() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    drop(browser);
    daemon
        .shared
        .registry
        .browser
        .poll(&daemon.shared)
        .await
        .unwrap();
    let waiting = tool(
        &daemon,
        &session,
        "browser_open_tab",
        json!({"url":format!("{origin}/")}),
        "loop",
    )
    .await;
    assert!(
        format!("{waiting:?}").contains("browser_restarting"),
        "{waiting:?}"
    );
    assert!(
        daemon.shared.registry.browser.existing().await.is_err(),
        "no relaunch during the backoff"
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    daemon
        .shared
        .registry
        .browser
        .poll(&daemon.shared)
        .await
        .unwrap();
    let third = shared_browser(&daemon).await;
    assert!(third.is_connected());
    kill.0.push(third.process_id());
    drop(third);
    open_tab(&daemon, &session, &format!("{origin}/"), "after-backoff").await;
    daemon.shutdown().await;
    fixture.abort();
}

/// Incognito sessions get the shared browser's treatment: their browser events reach a
/// running agent as steer notices; a device-approval human step bound to an incognito tab
/// resolves when its condition is met in the incognito browser; a secret_input human step is
/// refused with `vault_incognito`; a killed incognito browser is replaced by a fresh profile
/// and its tabs are reported closed and refused with `browser_restarted`.
#[tokio::test]
async fn incognito_browser_events_steer_and_human_steps_bind_incognito_tabs() {
    let root = tempfile::tempdir().unwrap();
    let Some(config) = browser_config(root.path()) else {
        return;
    };
    let (origin, fixture) = site().await;
    let runtime = TestRuntime::default();
    let daemon = Daemon::open_runtime(config, runtime.clone()).await.unwrap();
    let private = incognito_session(&daemon, "private").await;
    let router = api::router(daemon.shared.clone(), true);
    let accepted = router
        .clone()
        .oneshot(request(
            "POST",
            "/v1/sessions/private/messages",
            json!({"message_id":"run","text":"browse","mode":"queue"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    wait_event(&daemon, &private, |kind| {
        matches!(kind, EventKind::TurnStarted {})
    })
    .await;

    let tab = open_tab(&daemon, &private, &format!("{origin}/"), "open").await;
    let incognito = daemon
        .shared
        .registry
        .browser
        .for_session(&daemon.shared, &private)
        .await
        .map_err(|error| error.0)
        .unwrap();
    let mut kill = Kill(vec![incognito.process_id()]);
    drop(incognito);

    // Browser notices are lower-priority internal work, never owner steering.
    let closing = open_tab(&daemon, &private, &format!("{origin}/"), "open-closing").await;
    model(
        tool(
            &daemon,
            &private,
            "browser_close_tab",
            json!({"tab_id":closing}),
            "close",
        )
        .await,
    );
    let closed = format!("[browser event] Tab {closing} was closed");
    wait_event(&daemon, &private, |kind| {
        matches!(kind, EventKind::MessageQueued { text, mode: QueueMode::Queue, author_kind: openside_proto::MessageAuthorKind::Internal, .. } if text.starts_with(&closed))
    })
    .await;

    let ToolOutcome::Deferred { ticket: device, .. } = tool(
        &daemon,
        &private,
        "request_human_step",
        json!({"kind":"device_approval","summary":"Approve on your device","tab_id":tab,"resume_when":{"text_visible":"Next page"}}),
        "device",
    )
    .await
    else {
        panic!("expected a device approval ticket");
    };
    model(
        tool(
            &daemon,
            &private,
            "browser_navigate",
            json!({"tab_id":tab,"url":format!("{origin}/next")}),
            "nav",
        )
        .await,
    );
    daemon
        .shared
        .registry
        .browser
        .poll(&daemon.shared)
        .await
        .unwrap();
    wait_event(&daemon, &private, |kind| {
        matches!(kind, EventKind::TicketResolved { ticket_id, resolution, .. } if ticket_id == &device && resolution["status"] == "done")
    })
    .await;
    assert_eq!(
        reason(
            tool(
                &daemon,
                &private,
                "request_human_step",
                json!({"kind":"secret_input","summary":"Enter code","tab_id":tab,"origin":origin,"field_ref":"e1"}),
                "secret",
            )
            .await
        ),
        "vault_incognito"
    );

    // Killing the incognito browser: its tabs are lost and the next call gets a fresh profile.
    let lost = open_tab(&daemon, &private, &format!("{origin}/"), "open-lost").await;
    let dead = daemon
        .shared
        .registry
        .browser
        .for_session(&daemon.shared, &private)
        .await
        .map_err(|error| error.0)
        .unwrap();
    let dead_pid = dead.process_id();
    nix::sys::signal::killpg(
        nix::unistd::Pid::from_raw(i32::try_from(dead_pid).unwrap()),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while dead.is_connected() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    drop(dead);
    let fresh = open_tab(&daemon, &private, &format!("{origin}/"), "open-fresh").await;
    let replacement = daemon
        .shared
        .registry
        .browser
        .for_session(&daemon.shared, &private)
        .await
        .map_err(|error| error.0)
        .unwrap();
    assert_ne!(replacement.process_id(), dead_pid);
    kill.0.push(replacement.process_id());
    assert!(replacement.tabs().await.iter().all(|item| item.id != lost));
    drop(replacement);
    wait_event(
        &daemon,
        &private,
        |kind| matches!(kind, EventKind::TabClosed { tab_id } if tab_id == &lost),
    )
    .await;
    let denial = reason(
        tool(
            &daemon,
            &private,
            "browser_snapshot",
            json!({"tab_id":lost}),
            "lost",
        )
        .await,
    );
    assert!(denial.starts_with("browser_restarted"), "{denial}");
    model(
        tool(
            &daemon,
            &private,
            "browser_snapshot",
            json!({"tab_id":fresh}),
            "fresh",
        )
        .await,
    );
    daemon.shutdown().await;
    fixture.abort();
}

/// The startup sweep of crashed incognito profiles only ever touches real directories under
/// the incognito root: a planted `endpoint.json` naming a browser outside the root, an
/// `endpoint.json` that is a symlink, and a symlinked entry are all removed without stopping
/// that browser or deleting anything they point at.
#[tokio::test]
async fn incognito_sweep_never_follows_planted_endpoints_or_symlinks() {
    let root = tempfile::tempdir().unwrap();
    let Some(config) = browser_config(root.path()) else {
        return;
    };
    let executable = config.loaded.config.browser.executable.clone().unwrap();
    let incognito_root = config.loaded.paths.runtime_dir.join("incognito-browsers");
    let outside = root.path().join("outside");
    let mut victim = BrowserConfig::new(executable);
    victim.profile_dir = Some(outside.join("profile"));
    let (browser, _events) = Browser::launch(victim).await.unwrap();
    let _kill = Kill(vec![browser.process_id()]);
    let endpoint = serde_json::to_vec(&browser.endpoint()).unwrap();
    std::fs::write(outside.join("keep.txt"), "outside").unwrap();

    let planted = incognito_root.join("planted");
    std::fs::create_dir_all(planted.join("profile")).unwrap();
    std::fs::write(planted.join("endpoint.json"), &endpoint).unwrap();
    let linked = incognito_root.join("linked-endpoint");
    std::fs::create_dir_all(linked.join("profile")).unwrap();
    std::fs::write(outside.join("endpoint.json"), &endpoint).unwrap();
    std::os::unix::fs::symlink(outside.join("endpoint.json"), linked.join("endpoint.json"))
        .unwrap();
    std::os::unix::fs::symlink(&outside, incognito_root.join("link")).unwrap();
    let inner = incognito_root.join("inner-link");
    std::fs::create_dir_all(&inner).unwrap();
    std::os::unix::fs::symlink(outside.join("profile"), inner.join("profile")).unwrap();
    std::fs::write(inner.join("endpoint.json"), &endpoint).unwrap();

    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    for entry in [&planted, &linked, &inner, &incognito_root.join("link")] {
        assert!(
            std::fs::symlink_metadata(entry).is_err(),
            "{} was swept",
            entry.display()
        );
    }
    assert!(
        browser.is_connected(),
        "a browser outside the incognito root is never stopped"
    );
    assert!(!browser.tabs().await.is_empty());
    assert_eq!(
        std::fs::read_to_string(outside.join("keep.txt")).unwrap(),
        "outside"
    );
    assert!(outside.join("profile").is_dir());
    assert!(outside.join("endpoint.json").is_file());
    daemon.shutdown().await;
    browser.shutdown().await.unwrap();
}

/// The daemon's browser is detached so it survives a daemon restart; a test process that dies
/// without unwinding must still not leave it running. `launch_config` ties it to the launching
/// test thread: with no Drop run at all (daemon and runtime leaked), Chromium still exits.
#[cfg(target_os = "linux")]
#[test]
fn detached_test_browser_dies_with_the_thread_that_launched_it() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().to_owned();
    let pid = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let pid = runtime.block_on(async {
            let config = browser_config(&path)?;
            let daemon = Daemon::open_runtime(config, TestRuntime::default())
                .await
                .unwrap();
            let session = create_session(&daemon, "agent", PolicyMode::Full, None).await;
            let (origin, fixture) = site().await;
            open_tab(&daemon, &session, &format!("{origin}/"), "open").await;
            std::mem::forget(fixture);
            let pid = shared_browser(&daemon).await.process_id();
            // Simulates an abort: no destructor of the daemon ever runs.
            std::mem::forget(daemon);
            Some(pid)
        });
        std::mem::forget(runtime);
        pid
    })
    .join()
    .unwrap();
    let Some(pid) = pid else {
        return;
    };
    let _kill = Kill(vec![pid]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while alive(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !alive(pid),
        "the detached test browser outlived its test thread"
    );
}

/// Shutdown cannot wait out every stray `Arc<Browser>` clone — an ungated state probe or a
/// closing session task may still hold one. A straggler must not keep Chromium alive: the
/// daemon is going down, so the browser goes down through the shared handle.
#[tokio::test]
async fn shutdown_kills_the_browser_even_with_a_straggler_handle() {
    let root = tempfile::tempdir().unwrap();
    let Some(config) = browser_config(root.path()) else {
        return;
    };
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "agent", PolicyMode::Full, None).await;
    let (origin, fixture) = site().await;
    open_tab(&daemon, &session, &format!("{origin}/"), "open").await;
    let held = shared_browser(&daemon).await;
    let pid = held.process_id();
    let _kill = Kill(vec![pid]);
    daemon.shutdown().await;
    exits(pid).await;
    drop(held);
    drop(daemon);
    drop(fixture);
}
