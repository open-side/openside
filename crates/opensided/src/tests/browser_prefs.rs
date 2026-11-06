//! B11: the person's browser settings through `/v1/browser/prefs`. A fake gateway covers the
//! daemon's mapping; the patched smoke drives a real headless patched browser and reads the
//! value back on chrome://settings.

use super::*;
use crate::browser_prefs_api::{read, update, PrefsGateway};
use openside_browser::{BrowserError, BrowserPrefUpdate, CookiePolicy};

struct FakeGateway {
    supported: bool,
    state: Mutex<openside_browser::BrowserPrefs>,
    writes: Mutex<Vec<BrowserPrefUpdate>>,
    refuse: Option<&'static str>,
    /// Writes accepted before `refuse` applies.
    accept: usize,
}

impl FakeGateway {
    fn new() -> Self {
        Self {
            supported: true,
            state: Mutex::new(openside_browser::BrowserPrefs {
                spell_check: true,
                accept_languages: vec!["ko".into(), "en-US".into()],
                third_party_cookies: CookiePolicy::Allow,
                auto_picture_in_picture: Some(true),
                default_zoom_percent: Some(100),
                search_engine: openside_browser::SearchEngines {
                    current: Some(1),
                    options: vec![
                        openside_browser::SearchEngine {
                            id: 1,
                            name: "Google".into(),
                        },
                        openside_browser::SearchEngine {
                            id: 68,
                            name: "Naver".into(),
                        },
                    ],
                },
                mini_popup: Some(false),
            }),
            writes: Mutex::new(Vec::new()),
            refuse: None,
            accept: 0,
        }
    }
}

impl PrefsGateway for FakeGateway {
    fn supports_prefs(&self) -> bool {
        self.supported
    }
    async fn prefs(&self) -> Result<openside_browser::BrowserPrefs, BrowserError> {
        Ok(self.state.lock().clone())
    }
    async fn set_pref(
        &self,
        update: &BrowserPrefUpdate,
    ) -> Result<openside_browser::BrowserPrefs, BrowserError> {
        if let Some(code) = self
            .refuse
            .filter(|_| self.writes.lock().len() >= self.accept)
        {
            return Err(match code {
                "disconnected" => BrowserError::Disconnected,
                code => BrowserError::Gateway {
                    code: code.into(),
                    message: "refused".into(),
                },
            });
        }
        self.writes.lock().push(update.clone());
        let mut state = self.state.lock();
        match update {
            BrowserPrefUpdate::SpellCheck(on) => state.spell_check = *on,
            BrowserPrefUpdate::AcceptLanguages(tags) => state.accept_languages = tags.clone(),
            BrowserPrefUpdate::ThirdPartyCookies(policy) => state.third_party_cookies = *policy,
            BrowserPrefUpdate::AutoPictureInPicture(on) => {
                state.auto_picture_in_picture = Some(*on)
            }
            BrowserPrefUpdate::DefaultZoomPercent(percent) => {
                state.default_zoom_percent = Some(*percent)
            }
            BrowserPrefUpdate::SearchEngine(id) => state.search_engine.current = Some(*id),
            BrowserPrefUpdate::MiniPopup(on) => state.mini_popup = Some(*on),
        }
        Ok(state.clone())
    }
}

#[tokio::test]
async fn stock_browsers_report_unsupported_instead_of_values() {
    let none: Option<&FakeGateway> = None;
    let state = read(none).await.unwrap();
    assert_eq!(
        (state.status, state.prefs),
        (BrowserPrefsStatus::Unsupported, None)
    );
    let fake = FakeGateway {
        supported: false,
        ..FakeGateway::new()
    };
    assert_eq!(
        read(Some(&fake)).await.unwrap().status,
        BrowserPrefsStatus::Unsupported
    );
    let mut applied = Vec::new();
    let refused = update(
        Some(&fake),
        UpdateBrowserPrefsRequest {
            spell_check: Some(false),
            ..Default::default()
        },
        &mut applied,
    )
    .await
    .unwrap_err();
    assert_eq!(
        (refused.0, refused.1),
        (StatusCode::CONFLICT, "browser_prefs_unsupported")
    );
    assert!(fake.writes.lock().is_empty());
    assert!(applied.is_empty());
}

#[tokio::test]
async fn updates_apply_in_field_order_and_return_the_browser_read_back() {
    let fake = FakeGateway::new();
    let state = read(Some(&fake)).await.unwrap();
    assert_eq!(state.status, BrowserPrefsStatus::Available);
    let prefs = state.prefs.unwrap();
    assert!(prefs.spell_check);
    assert_eq!(prefs.search_engine.options[1].name, "Naver");
    assert_eq!(prefs.mini_popup, Some(false));

    let mut applied = Vec::new();
    let state = update(
        Some(&fake),
        UpdateBrowserPrefsRequest {
            mini_popup: Some(true),
            search_engine: Some(68),
            spell_check: Some(false),
            third_party_cookies: Some(ThirdPartyCookies::Block),
            default_zoom_percent: Some(125),
            ..Default::default()
        },
        &mut applied,
    )
    .await
    .unwrap();
    assert_eq!(
        *fake.writes.lock(),
        [
            BrowserPrefUpdate::SpellCheck(false),
            BrowserPrefUpdate::ThirdPartyCookies(CookiePolicy::Block),
            BrowserPrefUpdate::DefaultZoomPercent(125),
            BrowserPrefUpdate::SearchEngine(68),
            BrowserPrefUpdate::MiniPopup(true),
        ]
    );
    assert_eq!(
        applied,
        [
            "spell_check",
            "third_party_cookies",
            "default_zoom_percent",
            "search_engine",
            "mini_popup"
        ]
    );
    let prefs = state.prefs.unwrap();
    assert!(!prefs.spell_check);
    assert_eq!(prefs.third_party_cookies, ThirdPartyCookies::Block);
    assert_eq!(prefs.default_zoom_percent, Some(125));
    assert_eq!(prefs.search_engine.current, Some(68));
    assert_eq!(prefs.mini_popup, Some(true));

    // An empty change is a read, never a write.
    fake.writes.lock().clear();
    let mut applied = Vec::new();
    let state = update(
        Some(&fake),
        UpdateBrowserPrefsRequest::default(),
        &mut applied,
    )
    .await
    .unwrap();
    assert!(!state.prefs.unwrap().spell_check);
    assert!(fake.writes.lock().is_empty());
    assert!(applied.is_empty());

    // A later field's refusal leaves the earlier ones applied, and reported as applied.
    let fake = FakeGateway {
        refuse: Some("invalid_request"),
        accept: 1,
        ..FakeGateway::new()
    };
    let mut applied = Vec::new();
    let error = update(
        Some(&fake),
        UpdateBrowserPrefsRequest {
            spell_check: Some(false),
            default_zoom_percent: Some(101),
            ..Default::default()
        },
        &mut applied,
    )
    .await
    .unwrap_err();
    assert_eq!(error.1, "invalid_value");
    assert_eq!(applied, ["spell_check"]);
}

#[tokio::test]
async fn browser_refusals_map_to_stable_api_errors() {
    for (code, status, api) in [
        ("invalid_request", StatusCode::BAD_REQUEST, "invalid_value"),
        (
            "policy_denied",
            StatusCode::CONFLICT,
            "browser_pref_managed",
        ),
        (
            "unsupported",
            StatusCode::CONFLICT,
            "browser_prefs_unsupported",
        ),
        (
            "disconnected",
            StatusCode::SERVICE_UNAVAILABLE,
            "browser_disconnected",
        ),
        ("timeout", StatusCode::SERVICE_UNAVAILABLE, "browser_busy"),
        (
            "resource_exhausted",
            StatusCode::SERVICE_UNAVAILABLE,
            "browser_busy",
        ),
        (
            "internal",
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
        ),
    ] {
        let fake = FakeGateway {
            refuse: Some(code),
            ..FakeGateway::new()
        };
        let mut applied = Vec::new();
        let error = update(
            Some(&fake),
            UpdateBrowserPrefsRequest {
                default_zoom_percent: Some(101),
                ..Default::default()
            },
            &mut applied,
        )
        .await
        .unwrap_err();
        assert_eq!((error.0, error.1), (status, api), "{code}");
        assert!(applied.is_empty());
    }
}

#[tokio::test]
async fn without_a_running_browser_nothing_is_launched() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let router = api::router(daemon.shared.clone(), true);
    let response = router
        .clone()
        .oneshot(request("GET", "/v1/browser/prefs", json!({}), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await,
        json!({"status":"not_running","prefs":null})
    );
    let response = router
        .clone()
        .oneshot(request(
            "PATCH",
            "/v1/browser/prefs",
            json!({"spell_check":false}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(response).await["code"], "browser_not_running");
    let response = router
        .clone()
        .oneshot(request(
            "PATCH",
            "/v1/browser/prefs",
            json!({"download_directory":"/tmp"}),
            None,
        ))
        .await
        .unwrap();
    assert!(response.status().is_client_error(), "{}", response.status());
    let response = router
        .clone()
        .oneshot(request(
            "POST",
            "/v1/browser/person/native-tools",
            json!({"tool":"devtools"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(response).await["code"], "browser_not_running");
    let response = router
        .oneshot(request(
            "POST",
            "/v1/browser/person/native-tools",
            json!({"tool":"bookmarks"}),
            None,
        ))
        .await
        .unwrap();
    assert!(response.status().is_client_error(), "{}", response.status());
    assert!(daemon.shared.registry.browser.existing().await.is_err());
    daemon.shutdown().await;
}

/// `openside-browser` started a stock browser: it runs, so the settings are unsupported rather
/// than "open the browser".
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_running_stock_person_browser_reports_unsupported() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let person_runtime = daemon
        .shared
        .config
        .loaded
        .paths
        .runtime_dir
        .join("person-browser");
    std::fs::create_dir_all(&person_runtime).unwrap();
    std::fs::set_permissions(&person_runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut browser = std::process::Command::new("sh")
        // A loop, not one command: the shell would exec it and lose the argument.
        .args([
            "-c",
            "while :; do sleep 1; done",
            "sh",
            "--openside-gateway-role=person",
        ])
        .spawn()
        .unwrap();
    let record = person_runtime.join("launch.json");
    std::fs::write(
        &record,
        json!({"pid": browser.id(), "patched": false}).to_string(),
    )
    .unwrap();
    std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o600)).unwrap();
    let router = api::router(daemon.shared.clone(), true);
    let call = |method: &'static str, path: &'static str, body: Value| {
        router.clone().oneshot(request(method, path, body, None))
    };

    // The launcher takes the directory from the daemon instead of deriving it.
    let response = call("GET", "/v1/remote/local-access", Value::Null)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await["person_browser_runtime_dir"],
        person_runtime.to_str().unwrap()
    );
    let response = call("GET", "/v1/browser/prefs", json!({})).await.unwrap();
    assert_eq!(
        json_body(response).await,
        json!({"status":"unsupported","prefs":null})
    );
    let response = call("PATCH", "/v1/browser/prefs", json!({"spell_check":false}))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body(response).await["code"],
        "browser_prefs_unsupported"
    );
    let response = call(
        "POST",
        "/v1/browser/person/native-tools",
        json!({"tool":"task_manager"}),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body(response).await["code"],
        "native_tools_unsupported"
    );

    // Once that browser exits, it is simply not running.
    browser.kill().unwrap();
    browser.wait().unwrap();
    let response = call("GET", "/v1/browser/prefs", json!({})).await.unwrap();
    assert_eq!(
        json_body(response).await,
        json!({"status":"not_running","prefs":null})
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn remote_devices_read_with_read_and_change_only_with_admin() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let tcp = api::router(daemon.shared.clone(), false);
    let token = |scopes| {
        let db = daemon.shared.db.clone();
        async move {
            db.pair_as(
                "phone".into(),
                scopes,
                openside_proto::DeviceKind::PersonDevice,
            )
            .await
            .unwrap()
            .1
        }
    };
    let read = token(vec![openside_proto::CapabilityScope::Read]).await;
    let admin = token(vec![openside_proto::CapabilityScope::Admin]).await;
    let call = |method: &str, token: Option<&str>| {
        tcp.clone().oneshot(request(
            method,
            "/v1/browser/prefs",
            json!({"spell_check":false}),
            token,
        ))
    };

    assert_eq!(
        call("GET", None).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call("GET", Some(&read)).await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(
        call("PATCH", None).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call("PATCH", Some(&read)).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    // Admin passes the scope gate and reaches the handler, which never launches a browser.
    let response = call("PATCH", Some(&admin)).await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(response).await["code"], "browser_not_running");
    daemon.shutdown().await;
}

#[cfg(target_os = "linux")]
struct Killer(u32);

#[cfg(target_os = "linux")]
impl Drop for Killer {
    fn drop(&mut self) {
        let pid = nix::unistd::Pid::from_raw(i32::try_from(self.0).unwrap());
        let _ = nix::sys::signal::killpg(pid, nix::sys::signal::Signal::SIGKILL);
        let _ = nix::sys::wait::waitpid(pid, None);
    }
}

/// A plain CDP client on a chrome://settings page, separate from the daemon's controller.
#[cfg(target_os = "linux")]
struct SettingsPage {
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    session: String,
    next: u64,
}

#[cfg(target_os = "linux")]
impl SettingsPage {
    async fn open(browser: &openside_browser::Browser, url: &str) -> Self {
        let (socket, _) = tokio_tungstenite::connect_async(browser.endpoint().ws_url)
            .await
            .unwrap();
        let mut page = Self {
            socket,
            session: String::new(),
            next: 0,
        };
        let target = page
            .call(
                None,
                "Target.createTarget",
                json!({"url":url,"background":true}),
            )
            .await["targetId"]
            .clone();
        page.session = page
            .call(
                None,
                "Target.attachToTarget",
                json!({"targetId":target,"flatten":true}),
            )
            .await["sessionId"]
            .as_str()
            .unwrap()
            .to_owned();
        page
    }

    async fn call(&mut self, session: Option<&str>, method: &str, params: Value) -> Value {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        self.next += 1;
        let mut message = json!({"id":self.next,"method":method,"params":params});
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        self.socket
            .send(Message::Text(message.to_string().into()))
            .await
            .unwrap();
        let reply = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Message::Text(text) = self.socket.next().await.unwrap().unwrap() {
                    let reply: Value = serde_json::from_str(&text).unwrap();
                    if reply["id"] == self.next {
                        return reply;
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{method} did not answer"));
        assert!(reply.get("error").is_none(), "{method}: {reply}");
        reply["result"].clone()
    }

    /// What the settings page itself observes, through its own settingsPrivate binding.
    async fn spell_check(&mut self) -> Value {
        let expression = r#"(async () => {
            for (let i = 0; i < 100 && !(window.chrome && chrome.settingsPrivate); i++) {
                await new Promise(r => setTimeout(r, 100));
            }
            const pref = await new Promise(r =>
                chrome.settingsPrivate.getPref('browser.enable_spellchecking', r));
            return pref.value;
        })()"#;
        let session = self.session.clone();
        self.call(
            Some(&session),
            "Runtime.evaluate",
            json!({"expression":expression,"awaitPromise":true,"returnByValue":true}),
        )
        .await["result"]["value"]
            .clone()
    }
}

/// The person's own browser as `tools/openside-browser` starts it, publishing its person-role
/// gateway where the daemon looks for it.
#[cfg(target_os = "linux")]
async fn launch_person_browser(
    executable: &std::ffi::OsStr,
    daemon_runtime: &std::path::Path,
    root: &std::path::Path,
) -> openside_browser::Browser {
    use std::os::unix::fs::PermissionsExt;
    let runtime = daemon_runtime.join("person-browser");
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
    let launcher = root.join("person-launcher");
    std::fs::write(
        &launcher,
        format!(
            "#!/bin/sh\nexec '{}' --openside-gateway-instance-id=person-0123456789abcdef \
             '--openside-gateway-runtime-dir={}' --openside-gateway-role=person \
             --profile-directory=Default \"$@\"\n",
            executable.to_string_lossy(),
            runtime.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = openside_browser::BrowserConfig::new(launcher);
    config.profile_dir = Some(root.join("person-profile"));
    config.die_with_spawner = true;
    openside_browser::Browser::launch(config).await.unwrap().0
}

/// Two patched browsers, as on a real desktop: the daemon's agent browser and the person's own.
/// Settings and native tools reach the person's browser; the agent profile is never touched.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn patched_settings_and_native_tools_reach_the_person_browser_not_the_agent_browser() {
    use std::os::unix::fs::PermissionsExt;
    let Some(executable) = std::env::var_os("OPENSIDE_PATCHED_BROWSER") else {
        eprintln!("SKIP patched browser prefs: OPENSIDE_PATCHED_BROWSER is not set");
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let mut config = resolved_config(root.path());
    config.loaded.config.browser.executable = Some(std::path::PathBuf::from(&executable));
    config.loaded.config.browser.gateway = openside_config::BrowserGatewaySetting::Auto;
    let daemon_runtime = config.loaded.paths.runtime_dir.clone();
    std::fs::create_dir_all(&daemon_runtime).unwrap();
    std::fs::set_permissions(&daemon_runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let agent = daemon
        .shared
        .registry
        .browser
        .get(&daemon.shared)
        .await
        .map_err(|error| error.0)
        .unwrap();
    let _kill_agent = Killer(agent.process_id());
    assert!(agent
        .gateway()
        .is_some_and(|gateway| !gateway.supports_prefs()));
    let router = api::router(daemon.shared.clone(), true);
    let call = |method: &'static str, path: &'static str, body: Value| {
        router.clone().oneshot(request(method, path, body, None))
    };

    // The agent browser runs, the person's does not: nothing to read, nothing falls back.
    let response = call("GET", "/v1/browser/prefs", json!({})).await.unwrap();
    assert_eq!(
        json_body(response).await,
        json!({"status":"not_running","prefs":null})
    );
    let response = call(
        "POST",
        "/v1/browser/person/native-tools",
        json!({"tool":"task_manager"}),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(response).await["code"], "browser_not_running");

    let person = launch_person_browser(&executable, &daemon_runtime, root.path()).await;
    let kill_person = Killer(person.process_id());
    let mut person_settings = SettingsPage::open(&person, "chrome://settings/languages").await;
    let mut agent_settings = SettingsPage::open(&agent, "chrome://settings/languages").await;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let state = loop {
        let response = call("GET", "/v1/browser/prefs", json!({})).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let state = json_body(response).await;
        if state["status"] != "not_running" {
            break state;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "person gateway never appeared"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(state["status"], "available", "{state}");
    assert_eq!(state["prefs"]["spell_check"], true, "{state}");

    for on in [false, true] {
        let response = call("PATCH", "/v1/browser/prefs", json!({"spell_check":on}))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let state = json_body(response).await;
        assert_eq!(state["prefs"]["spell_check"], on, "{state}");
        assert_eq!(person_settings.spell_check().await, on);
        assert_eq!(agent_settings.spell_check().await, true);
    }
    // Each change is audited by setting name, never by value.
    let audited = || {
        let audit = std::fs::read_to_string(
            openside_config::data_layout::DataPath::Audit.path(&daemon.shared.config.data_dir),
        )
        .unwrap();
        let changes: Vec<Value> = audit
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|record| record["tool"] == "browser_prefs")
            .collect();
        for change in &changes {
            assert_eq!(change["decision"], "changed");
            let mut keys: Vec<&str> = change
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert_eq!(
                keys,
                ["complete", "decision", "device", "prefs", "tool", "ts"],
                "{change}"
            );
        }
        changes
    };
    let changes = audited();
    assert_eq!(changes.len(), 2);
    for change in &changes {
        assert_eq!(change["prefs"], json!(["spell_check"]));
        assert_eq!(change["complete"], true);
    }

    // The browser, not the daemon, rejects values outside its allowlist; nothing applied, nothing
    // audited.
    let response = call(
        "PATCH",
        "/v1/browser/prefs",
        json!({"default_zoom_percent":101}),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(audited().len(), 2);
    // An empty change writes nothing and is not audited either.
    let response = call("PATCH", "/v1/browser/prefs", json!({})).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(audited().len(), 2);
    // A later refusal still audits the field applied before it.
    let response = call(
        "PATCH",
        "/v1/browser/prefs",
        json!({"spell_check":true,"default_zoom_percent":101}),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let changes = audited();
    assert_eq!(changes.len(), 3);
    assert_eq!(changes[2]["prefs"], json!(["spell_check"]));
    assert_eq!(changes[2]["complete"], false);

    // Developer tools open in the person's browser, for its own tab.
    let devtools = |targets: Value| {
        targets["targetInfos"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|target| {
                target["url"]
                    .as_str()
                    .is_some_and(|url| url.starts_with("devtools://devtools/"))
            })
            .count()
    };
    let before = devtools(
        person_settings
            .call(None, "Target.getTargets", json!({}))
            .await,
    );
    for tool in ["devtools", "task_manager"] {
        let response = call(
            "POST",
            "/v1/browser/person/native-tools",
            json!({"tool":tool}),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT, "{tool}");
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while devtools(
        person_settings
            .call(None, "Target.getTargets", json!({}))
            .await,
    ) <= before
    {
        assert!(
            std::time::Instant::now() < deadline,
            "no DevTools window opened"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        devtools(
            agent_settings
                .call(None, "Target.getTargets", json!({}))
                .await
        ),
        0
    );

    // Closing the person's browser makes it not running again; the agent browser never stands in.
    drop(person_settings);
    drop(person);
    drop(kill_person);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let response = call("GET", "/v1/browser/prefs", json!({})).await.unwrap();
        if json_body(response).await["status"] == "not_running" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "closed person browser still answers"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    drop(agent_settings);
    drop(agent);
    daemon.shutdown().await;
}
