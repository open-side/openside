//! Web UI management acceptance: the built PWA served by the real daemon, driven in
//! headless Chromium over the loopback TCP listener as a paired device. Covers the
//! `/pair` deep link (F-2 web), the device list, the vault audit view (C-4) and the
//! schema-driven settings screen with the local-socket-only rule (PP-13), and a
//! phone-width (390x844) vault setup that adds an authenticator from its setup key.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{path::PathBuf, time::Duration};

use openside_browser::{Browser, BrowserConfig};
use openside_e2e::Harness;
use openside_vault::{AccessPolicy, AllowedOrigin, Entry, PassphraseLock, Secret, Vault};
use serde_json::{json, Value};

const OWNER: &str = "web-admin-e2e";
const PASSWORD: &str = "WEB-ADMIN-VAULT-PASSWORD-CANARY-5d1e";

fn bundle() -> Option<PathBuf> {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist");
    if !dist.join("index.html").is_file() {
        eprintln!("SKIP: web UI bundle is missing; run pnpm -C web install && pnpm -C web build");
        return None;
    }
    Some(dist.canonicalize().unwrap())
}

fn chromium() -> Option<PathBuf> {
    match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(path) => Some(path),
        Err(reason) => {
            eprintln!("SKIP: web UI acceptance requires Chromium: {reason}");
            None
        }
    }
}

struct Page<'a> {
    browser: &'a Browser,
    tab: String,
}

impl Page<'_> {
    async fn eval(&self, expression: &str) -> Value {
        self.browser
            .evaluate(&self.tab, expression)
            .await
            .unwrap_or_else(|e| panic!("evaluate {expression}: {e:?}"))
    }

    /// Poll a page predicate; on timeout, fail with the visible page text.
    async fn wait(&self, what: &str, predicate: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            if self.eval(predicate).await == true {
                return;
            }
            if tokio::time::Instant::now() > deadline {
                let text = self
                    .eval("document.body ? document.body.innerText : ''")
                    .await;
                let saves = self.eval("window.__configSaves ?? []").await;
                panic!("timed out waiting for {what}; page text: {text}; config saves: {saves}");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Client-side navigation as a user would: the sidebar's Workspace link opens the
    /// Workspace dialog, whose tile opens the section.
    async fn open_section(&self, label: &str, ready: &str) {
        self.eval(
            "Array.from(document.querySelectorAll('nav.manage-nav a')).find((a) => a.textContent === 'Workspace').click()",
        )
        .await;
        let tile = format!(
            "Array.from(document.querySelectorAll('[role=dialog] a.workspace-tile')).find((a) => a.querySelector('strong').textContent === {})",
            json!(label)
        );
        self.wait("Workspace dialog", &format!("!!{tile}")).await;
        self.eval(&format!("{tile}.click()")).await;
        self.wait(label, ready).await;
    }
}

// React tracks input values through the native setter; assigning `.value`
// directly would be ignored, so test input goes through the prototype setter.
const HELPERS: &str = r#"(() => {
  window.__violations = [];
  document.addEventListener('securitypolicyviolation', (e) => window.__violations.push({
    directive: e.violatedDirective, inline: e.blockedURI === 'inline',
    source: new URL(e.sourceFile || location.href, location.href).pathname, line: e.lineNumber,
  }));
  window.__os = {
    fill(el, value) {
      const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype
        : el instanceof HTMLSelectElement ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, value);
      el.dispatchEvent(new Event(el instanceof HTMLSelectElement ? 'change' : 'input', { bubbles: true }));
    },
    button(scope, text) {
      return Array.from(scope.querySelectorAll('button')).find((b) => b.textContent.trim() === text);
    },
  };
  return true;
})()"#;

fn config_file(harness: &Harness) -> String {
    std::fs::read_to_string(harness.root().join("config/openside/config.toml")).unwrap()
}

#[tokio::test]
async fn paired_web_device_manages_devices_vault_audit_and_settings() {
    let (Some(dist), Some(executable)) = (bundle(), chromium()) else {
        return;
    };
    let Some(mut harness) = Harness::prepare(
        env!("CARGO_BIN_EXE_e2e-opensided"),
        &json!({"responses":[{"parts":[{"type":"text","text":"unused"}]}]}),
    ) else {
        return;
    };
    let vault_path = harness.root().join("admin.vault");
    let mut vault = Vault::create(
        &vault_path,
        harness.root().join("setup.audit.jsonl"),
        &Secret::new("disposable-admin-passphrase".into()),
        PassphraseLock::default(),
    )
    .unwrap();
    vault
        .put(
            Entry::new(
                "fixture".into(),
                vec![AllowedOrigin::parse("https://fixture.test").unwrap()],
                Secret::new("fixture-user".into()),
                Secret::new(PASSWORD.into()),
                None,
                AccessPolicy::Always,
            )
            .unwrap(),
        )
        .unwrap();
    drop(vault);
    let unlock = harness.root().join("unlock.sh");
    std::fs::write(
        &unlock,
        "#!/bin/sh\nprintf '%s\\n' 'disposable-admin-passphrase'\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&unlock, std::fs::Permissions::from_mode(0o700)).unwrap();
    harness.append_config(&format!(
        "[ui]\ndir = {}\n[vault]\npath = {}\npassphrase_cmd = [{}]\n",
        json!(dist),
        json!(vault_path),
        json!(unlock)
    ));
    harness.launch().await;
    let onboarding = harness
        .request("PUT", "/v1/onboarding", Some(json!({"state":"skipped"})))
        .await;
    assert_eq!(onboarding.status, 200, "{}", onboarding.body);
    let port = std::fs::read_to_string(harness.data_dir.join("opensided.port"))
        .unwrap()
        .trim()
        .to_owned();

    // `openside pair` equivalent: only the local socket can issue a pairing offer.
    let offer = harness
        .request(
            "POST",
            "/v1/pairing",
            Some(json!({"name":"Web admin e2e","scopes":["read","admin"]})),
        )
        .await;
    assert_eq!(offer.status, 201, "{}", offer.body);
    let path = offer.body["path"].as_str().unwrap();
    assert!(path.starts_with("/pair#"), "{path}");

    let (browser, _events) = Browser::launch(BrowserConfig::new(executable))
        .await
        .unwrap();
    let tab = browser
        .create_agent_tab(OWNER, &format!("http://127.0.0.1:{port}{path}"))
        .await
        .unwrap();
    let page = Page {
        browser: &browser,
        tab: tab.id.clone(),
    };
    page.wait(
        "app after pairing",
        "!!document.querySelector('.session-list')",
    )
    .await;
    page.eval(HELPERS).await;
    assert_eq!(
        page.eval("location.pathname + location.hash").await,
        "/agents/chief"
    );
    let device = page.eval("localStorage.getItem('openside.device')").await;
    let device = device.as_str().expect("device id stored").to_owned();
    assert!(page
        .eval("localStorage.getItem('openside.token')")
        .await
        .is_string());
    // Single use: the code is spent, so a second exchange fails.
    let code = path.trim_start_matches("/pair#");
    let replay = harness
        .request("POST", "/v1/pairing/exchange", Some(json!({"code":code})))
        .await;
    assert_eq!(replay.status, 404, "{}", replay.body);

    page.open_section(
        "Devices",
        &format!(
            "!!document.querySelector('[role=dialog] [data-section=devices] li[data-device={}]')",
            json!(device)
        ),
    )
    .await;
    assert_eq!(
        page.eval(&format!(
            "document.querySelector('li[data-device={}]').textContent.includes('This device')",
            json!(device)
        ))
        .await,
        true
    );

    // Vault: a policy change made in the UI is audited by the vault and read back in the UI.
    page.open_section(
        "Vault",
        "!!document.querySelector('[data-section=vault] [role=combobox][aria-label=\"Use policy for fixture\"]')",
    )
    .await;
    page.eval(
        "document.querySelector('[data-section=vault] [role=combobox][aria-label=\"Use policy for fixture\"]').click()",
    )
    .await;
    page.wait(
        "vault policy choices",
        "Array.from(document.querySelectorAll('[role=option]')).some(option => option.textContent.trim() === 'Never')",
    )
    .await;
    page.eval("Array.from(document.querySelectorAll('[role=option]')).find(option => option.textContent.trim() === 'Never').click()").await;
    // Matching effective policies do not repeat the selector label; verify the saved metadata.
    page.wait(
        "persisted and enforced item policy",
        "fetch('/v1/vault/items', { headers: { Authorization: 'Bearer ' + localStorage.getItem('openside.token') } }) \
         .then((r) => r.json()).then((data) => data.items.some((item) => item.id === 'fixture' && item.policy === 'never' && item.effective_policy === 'never') \
         && document.querySelector('[data-section=vault] [role=combobox][aria-label=\"Use policy for fixture\"]').textContent.trim() === 'Never')",
    )
    .await;
    page.eval(
        "(() => { const f = document.querySelector('form[aria-label=\"Audit filters\"]'); \
         __os.fill(f.querySelector('input'), 'fixture'); __os.button(f, 'Filter').click(); return true })()",
    )
    .await;
    page.wait(
        "audit record of the policy change",
        "Array.from(document.querySelectorAll('ol.audit-list li[data-item=fixture]')).some((r) => \
         r.dataset.result === 'updated' && r.textContent.includes('Policy change') && r.textContent.includes('never'))",
    )
    .await;
    assert_eq!(
        page.eval("document.body.innerText.includes('WEB-ADMIN-VAULT-PASSWORD')")
            .await,
        false
    );

    // Settings over TCP: a remote-editable key saves; a local-only key is shown locked.
    page.open_section(
        "Configuration",
        "!!document.getElementById('setting-tickets.confirm_ttl_secs')",
    )
    .await;
    assert_eq!(
        page.eval("document.querySelector('[data-key=\"policy.mode\"]').dataset.locked === 'true' && document.getElementById('setting-policy.mode').disabled")
            .await,
        true
    );
    // Persistence can finish before the browser receives the save response.
    page.eval(
        "(() => { const fetch = window.fetch; let first = true; window.__configSaves = []; \
         window.fetch = async (...args) => { const response = await fetch(...args); \
         if (args[0].endsWith('/v1/settings/config') && args[1]?.method === 'PUT') { \
         const doc = JSON.parse(args[1].body).config; \
         window.__configSaves.push({ mode: doc.policy.mode, ttl: doc.tickets.confirm_ttl_secs, status: response.status, \
         ...(!response.ok ? { code: (await response.clone().json()).code } : {}) }); \
         const delay = first ? 1000 : 1500; first = false; \
         await new Promise((resolve) => setTimeout(resolve, delay)); \
         } return response; }; return true; })()",
    )
    .await;
    page.eval(
        "(() => { __os.fill(document.getElementById('setting-tickets.confirm_ttl_secs'), '1234'); \
         __os.button(document.querySelector('[data-section=config]'), 'Save (1)').click(); return true })()",
    )
    .await;
    // Switch editors while the browser's first save response is still pending.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let stored = loop {
        let stored = harness.request("GET", "/v1/settings/config", None).await;
        if stored.body["config"]["tickets"]["confirm_ttl_secs"] == 1234 {
            break stored;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "setting not saved: {}",
            stored.body
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(stored.body["config"]["tickets"]["confirm_ttl_secs"], 1234);
    assert_eq!(stored.body["config"]["policy"]["mode"], "full");
    assert!(config_file(&harness).contains("confirm_ttl_secs = 1234"));

    // The JSON tab can express a local-only change; the daemon refuses it over TCP.
    page.eval("__os.button(document.querySelector('[data-section=config]'), 'JSON').click()")
        .await;
    page.wait(
        "JSON editor",
        "!!document.querySelector('[data-section=config] textarea')",
    )
    .await;
    page.eval(
        "(() => { const t = document.querySelector('[data-section=config] textarea'); const doc = JSON.parse(t.value); \
         doc.policy.mode = 'read-only'; __os.fill(t, JSON.stringify(doc)); \
         __os.button(document.querySelector('[data-section=config]'), 'Save JSON').click(); return true })()",
    )
    .await;
    page.wait(
        "local authority refusal",
        "window.__configSaves.some(s => s.status === 403 && s.code === 'local_access_required') \
         && Array.from(document.querySelectorAll('[data-section=config] [role=alert]')).some((a) => \
         a.textContent.includes('Reopen openside from the computer it runs on'))",
    )
    .await;
    let after = harness.request("GET", "/v1/settings/config", None).await;
    assert_eq!(after.body["config"]["policy"]["mode"], "full");
    assert_eq!(after.body["config"]["tickets"]["confirm_ttl_secs"], 1234);
    assert_eq!(
        page.eval("window.__configSaves").await,
        json!([
            {"mode":"full","ttl":1234,"status":200},
            {"mode":"read-only","ttl":600,"status":403,"code":"local_access_required"},
        ])
    );
    assert!(config_file(&harness).contains("mode = \"full\""));
    assert_eq!(page.eval("window.__violations").await, json!([]));

    browser.shutdown().await.unwrap();
    harness.assert_no_canaries(&[PASSWORD]);
    harness.kill().await;
}

/// A second DevTools session on the tab that emulates a phone; the override lasts as long as
/// the returned socket stays open.
async fn phone_viewport(
    ws_url: &str,
    tab: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let (mut socket, _) = tokio_tungstenite::connect_async(ws_url).await.unwrap();
    let mut session = None::<String>;
    let calls = [
        (
            "Target.attachToTarget",
            json!({"targetId":tab,"flatten":true}),
        ),
        (
            "Emulation.setDeviceMetricsOverride",
            json!({"width":390,"height":844,"deviceScaleFactor":3,"mobile":true}),
        ),
        (
            "Emulation.setTouchEmulationEnabled",
            json!({"enabled":true,"maxTouchPoints":5}),
        ),
    ];
    for (id, (method, params)) in calls.into_iter().enumerate() {
        let mut request = json!({"id":id,"method":method,"params":params});
        if let Some(session) = &session {
            request["sessionId"] = json!(session);
        }
        socket
            .send(Message::Text(request.to_string().into()))
            .await
            .unwrap();
        let reply = loop {
            let next = tokio::time::timeout(Duration::from_secs(10), socket.next())
                .await
                .expect("CDP reply")
                .unwrap()
                .unwrap();
            if let Message::Text(body) = next {
                let reply: Value = serde_json::from_str(body.as_str()).unwrap();
                if reply["id"] == id {
                    break reply;
                }
            }
        };
        assert!(reply["error"].is_null(), "{method}: {reply}");
        if session.is_none() {
            session = Some(reply["result"]["sessionId"].as_str().unwrap().to_owned());
        }
    }
    socket
}

/// A phone paired with the admin scope sets up a passphrase vault and adds an authenticator
/// from its setup key at 390x844; the key travels only to the daemon and is never shown again.
#[tokio::test]
async fn paired_phone_adds_an_authenticator_to_the_vault() {
    const SEED: &str = "JBSWY3DPEHPK3PXP";
    const PASSPHRASE: &str = "PHONE-VAULT-PASSPHRASE-CANARY-41";
    let (Some(dist), Some(executable)) = (bundle(), chromium()) else {
        return;
    };
    let Some(mut harness) = Harness::prepare(
        env!("CARGO_BIN_EXE_e2e-opensided"),
        &json!({"responses":[{"parts":[{"type":"text","text":"unused"}]}]}),
    ) else {
        return;
    };
    // Pinned: `auto` would reach the developer's real keyring.
    harness.append_config(&format!(
        "[ui]\ndir = {}\n[vault]\nunlock = \"passphrase\"\n",
        json!(dist)
    ));
    harness.launch().await;
    let onboarding = harness
        .request("PUT", "/v1/onboarding", Some(json!({"state":"skipped"})))
        .await;
    assert_eq!(onboarding.status, 200, "{}", onboarding.body);
    let port = std::fs::read_to_string(harness.data_dir.join("opensided.port"))
        .unwrap()
        .trim()
        .to_owned();
    let offer = harness
        .request(
            "POST",
            "/v1/pairing",
            Some(json!({"name":"Phone e2e","scopes":["read","admin"]})),
        )
        .await;
    assert_eq!(offer.status, 201, "{}", offer.body);
    let path = offer.body["path"].as_str().unwrap().to_owned();

    let (browser, _events) = Browser::launch(BrowserConfig::new(executable))
        .await
        .unwrap();
    let tab = browser
        .create_agent_tab(OWNER, &format!("http://127.0.0.1:{port}{path}"))
        .await
        .unwrap();
    let _phone = phone_viewport(&browser.endpoint().ws_url, &tab.id).await;
    let page = Page {
        browser: &browser,
        tab: tab.id.clone(),
    };
    page.wait(
        "app after pairing",
        "!!document.querySelector('.session-list') || !!localStorage.getItem('openside.token')",
    )
    .await;
    page.eval("location.assign('/vault')").await;
    page.wait(
        "vault status at phone width",
        "window.innerWidth === 390 && !!document.querySelector('[data-section=vault] article[data-state=uninitialized]')",
    )
    .await;
    page.eval(HELPERS).await;

    page.eval(&format!(
        "(() => {{ const f = document.querySelector('form[aria-label=\"Set up vault\"]'); \
         f.querySelectorAll('input[type=password]').forEach((i) => __os.fill(i, {p})); \
         __os.button(f, 'Set up vault').click(); return true }})()",
        p = json!(PASSPHRASE)
    ))
    .await;
    page.wait(
        "unlocked vault",
        "!!document.querySelector('[data-section=vault] article[data-state=unlocked]')",
    )
    .await;

    page.eval(
        "__os.button(document.querySelector('[data-section=vault]'), 'Authenticator').click()",
    )
    .await;
    page.wait(
        "authenticator form",
        "!!document.querySelector('form[aria-label=\"New authenticator\"]')",
    )
    .await;
    page.eval(&format!(
        "(() => {{ const f = document.querySelector('form[aria-label=\"New authenticator\"]'); const i = f.querySelectorAll('input'); \
         __os.fill(i[0], 'https://github.com'); __os.fill(f.querySelector('input[type=password]'), {s}); return true }})()",
        s = json!(SEED)
    ))
    .await;
    // The sheet fits the phone: nothing in the form is wider than the viewport.
    assert_eq!(
        page.eval(
            "(() => { const f = document.querySelector('form[aria-label=\"New authenticator\"]'); \
             const r = f.getBoundingClientRect(); return r.left >= 0 && r.right <= 390 && \
             Array.from(f.querySelectorAll('input, button')).every((e) => e.getBoundingClientRect().right <= 390) })()"
        )
        .await,
        true
    );
    page.eval("__os.button(document.querySelector('form[aria-label=\"New authenticator\"]'), 'Add').click()")
        .await;
    page.wait(
        "added authenticator",
        "!!document.querySelector('[data-section=vault] article[data-item=otp-github-com]') && \
         document.querySelector('[data-section=vault] article[data-item=otp-github-com]').textContent.includes('one-time codes')",
    )
    .await;
    assert_eq!(
        page.eval("document.documentElement.scrollWidth <= 390")
            .await,
        true,
        "no horizontal scrolling at phone width"
    );
    assert_eq!(
        page.eval(&format!(
            "document.body.innerText.includes({})",
            json!(SEED)
        ))
        .await,
        false
    );

    let items = harness.request("GET", "/v1/vault/items", None).await;
    assert_eq!(items.status, 200, "{}", items.body);
    let item = items.body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == "otp-github-com")
        .expect("authenticator item")
        .clone();
    assert_eq!(item["has_totp"], true);
    assert_eq!(item["origins"], json!(["https://github.com:443"]));
    assert_eq!(page.eval("window.__violations").await, json!([]));

    browser.shutdown().await.unwrap();
    harness.assert_no_canaries(&[SEED, PASSPHRASE]);
    harness.kill().await;
}
