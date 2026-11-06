//! F-5: the first-run guide in the built web app, walked end to end in headless Chromium
//! against a throwaway daemon. Fake Chrome and Firefox profiles sit where the daemon looks for
//! them. Covers the one-time auto-open, welcome → browser import → passwords → AI subscription →
//! all set through public endpoints only, the dry-run preview before any write, the default-browser
//! prompt, skipping, and reopening from Settings › General.
//!
//! Captures: with `OPENSIDE_SCREENS_DIR` set, the flow is walked once per theme and viewport
//! (light/dark x 390x844 phone / 1440x900 desktop) and every step is saved as
//! `<theme>-<phone|desktop>-<step>.png`; without it, one desktop pass runs.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use futures_util::{FutureExt, SinkExt, StreamExt};
use openside_browser::{Browser, BrowserConfig};
use openside_e2e::Harness;
use openside_import::testing::{
    BookmarkEntry, ChromeProfile, CookieSpec, Encryption, FirefoxProfile, HistoryEntry,
};
use serde_json::{json, Value};
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};

const OWNER: &str = "onboarding-e2e";
const DAEMON: &str = env!("CARGO_BIN_EXE_e2e-opensided");
const COOKIE_VALUE: &str = "onboarding-cookie-value-canary";

fn bundle() -> Option<PathBuf> {
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist");
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
            eprintln!("SKIP: onboarding acceptance requires Chromium: {reason}");
            None
        }
    }
}

const HELPERS: &str = r#"(() => {
  window.__violations = [];
  document.addEventListener('securitypolicyviolation', (e) => window.__violations.push(e.violatedDirective));
  window.__ob = {
    fill(el, value) {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(el, value);
      el.dispatchEvent(new Event('input', { bubbles: true }));
    },
    // A button or link by visible text or aria-label.
    find(label, scope) {
      const root = scope ? document.querySelector(scope) : document;
      return root && Array.from(root.querySelectorAll('button, a')).find(
        (b) => (b.getAttribute('aria-label') || b.textContent).trim() === label);
    },
    press(label, scope) {
      const el = __ob.find(label, scope);
      if (!el || el.disabled) return false;
      el.click();
      return true;
    },
    enabled(label, scope) {
      const el = __ob.find(label, scope);
      return !!el && !el.disabled;
    },
    title() { return document.querySelector('.onb h1')?.textContent ?? null; },
    where() { return location.pathname + location.search; },
  };
  return true;
})()"#;

struct Viewport {
    name: &'static str,
    width: u32,
    height: u32,
    scale: f32,
    mobile: bool,
}

const PHONE: Viewport = Viewport {
    name: "phone",
    width: 390,
    height: 844,
    scale: 2.0,
    mobile: true,
};
const DESKTOP: Viewport = Viewport {
    name: "desktop",
    width: 1440,
    height: 900,
    scale: 1.0,
    mobile: false,
};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Viewport and colour-scheme emulation on its own DevTools session; captures go through it,
/// because the override is per session.
struct Emulation {
    socket: Socket,
    session: String,
    next: u64,
}

impl Emulation {
    async fn attach(ws_url: &str, tab: &str) -> Self {
        let (socket, _) = tokio_tungstenite::connect_async(ws_url).await.unwrap();
        let mut emulation = Self {
            socket,
            session: String::new(),
            next: 0,
        };
        let attached = emulation
            .call(
                "Target.attachToTarget",
                json!({"targetId":tab,"flatten":true}),
            )
            .await;
        emulation.session = attached["sessionId"].as_str().unwrap().to_owned();
        emulation
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        let mut request = json!({"id":id,"method":method,"params":params});
        if !self.session.is_empty() {
            request["sessionId"] = json!(self.session);
        }
        self.socket
            .send(Message::Text(request.to_string().into()))
            .await
            .unwrap();
        loop {
            let next = tokio::time::timeout(Duration::from_secs(10), self.socket.next())
                .await
                .expect("CDP reply")
                .unwrap()
                .unwrap();
            if let Message::Text(body) = next {
                let reply: Value = serde_json::from_str(body.as_str()).unwrap();
                if reply["id"] == id {
                    assert!(reply["error"].is_null(), "{method}: {reply}");
                    return reply["result"].clone();
                }
            }
        }
    }

    async fn set(&mut self, viewport: &Viewport, scheme: &str) {
        self.call(
            "Emulation.setDeviceMetricsOverride",
            json!({"width":viewport.width,"height":viewport.height,
                "deviceScaleFactor":viewport.scale,"mobile":viewport.mobile}),
        )
        .await;
        self.call(
            "Emulation.setTouchEmulationEnabled",
            json!({"enabled":viewport.mobile,"maxTouchPoints":5}),
        )
        .await;
        self.call(
            "Emulation.setEmulatedMedia",
            json!({"features":[{"name":"prefers-color-scheme","value":scheme}]}),
        )
        .await;
    }

    async fn capture(&mut self) -> Vec<u8> {
        use base64::Engine;
        let shot = self
            .call(
                "Page.captureScreenshot",
                json!({"format":"png","captureBeyondViewport":false}),
            )
            .await;
        base64::engine::general_purpose::STANDARD
            .decode(shot["data"].as_str().unwrap())
            .unwrap()
    }
}

struct Ui {
    browser: Browser,
    tab: String,
    origin: String,
    emulation: Emulation,
    /// `<out>/<theme>-<viewport>-` of the pass being captured, when capturing.
    prefix: Option<PathBuf>,
}

impl Ui {
    async fn eval(&self, expression: &str) -> Value {
        self.browser
            .evaluate(&self.tab, expression)
            .await
            .unwrap_or_else(|e| panic!("evaluate {expression}: {e:?}"))
    }

    async fn wait(&self, what: &str, predicate: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        loop {
            if self.eval(predicate).await == true {
                return;
            }
            if tokio::time::Instant::now() > deadline {
                let text = self
                    .eval("document.body ? document.body.innerText : ''")
                    .await;
                panic!("timed out waiting for {what}; page text: {text}");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn goto(&self, path: &str) {
        let url = format!("{}{path}", self.origin);
        self.browser
            .navigate(OWNER, &self.tab, &url)
            .await
            .unwrap_or_else(|e| panic!("navigate {path}: {e:?}"));
        self.wait(
            "document",
            "document.readyState === 'complete' && !!document.querySelector('.shell')",
        )
        .await;
        self.eval(HELPERS).await;
    }

    async fn press(&self, label: &str, scope: Option<&str>) {
        let args = format!("{}, {}", json!(label), json!(scope));
        self.wait(&format!("button {label}"), &format!("__ob.enabled({args})"))
            .await;
        assert_eq!(self.eval(&format!("__ob.press({args})")).await, true);
    }

    async fn on_step(&self, title: &str) {
        self.wait(
            &format!("step {title}"),
            &format!("__ob.title() === {}", json!(title)),
        )
        .await;
    }

    async fn text_of(&self, selector: &str) -> String {
        self.eval(&format!(
            "document.querySelector({})?.textContent ?? ''",
            json!(selector)
        ))
        .await
        .as_str()
        .unwrap()
        .to_owned()
    }

    /// Fonts loaded and transitions settled, then a capture when this pass records.
    async fn snap(&mut self, name: &str) {
        let Some(prefix) = self.prefix.clone() else {
            return;
        };
        self.eval("document.fonts.ready.then(() => true)").await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        let png = self.emulation.capture().await;
        let file = PathBuf::from(format!("{}{name}.png", prefix.display()));
        std::fs::write(&file, png).unwrap();
        eprintln!("captured {}", file.display());
    }
}

fn history(urls: &[&str]) -> Vec<HistoryEntry> {
    urls.iter()
        .map(|url| HistoryEntry {
            url: (*url).into(),
            title: Some("Visited".into()),
            visits_unix_ms: vec![1_700_000_000_000],
        })
        .collect()
}

/// Browser profiles where the daemon's HOME/XDG_CONFIG_HOME point, as a person's would be.
fn seed_browsers(root: &Path) {
    let chrome = root.join(if cfg!(target_os = "macos") {
        "home/Library/Application Support/Google/Chrome"
    } else {
        "config/google-chrome"
    });
    ChromeProfile {
        history: history(&["https://example.test/a", "https://example.test/b"]),
        bookmarks: vec![BookmarkEntry {
            folder: vec!["Work".into()],
            title: "Example".into(),
            url: "https://example.test/".into(),
        }],
        cookies: vec![
            CookieSpec::new("example.test", "sid", COOKIE_VALUE),
            CookieSpec::new("example.test", "theme", "dark"),
        ],
        // Plain Mac cookies exercise UI delivery, not the user's Safe Storage account.
        // The encrypted V10 CLI fixture can supply its synthetic key; the UI cannot.
        encryption: if cfg!(target_os = "macos") {
            Encryption::Plain
        } else {
            Encryption::V10
        },
        ..ChromeProfile::default()
    }
    .write(&chrome.join("Default"));
    // Cookies sealed by the desktop keyring: the API cannot take a keyring password command.
    ChromeProfile {
        history: history(&["https://sealed.test/"]),
        cookies: vec![CookieSpec::new("sealed.test", "sid", "sealed")],
        encryption: Encryption::V11("keyring-password".into()),
        ..ChromeProfile::default()
    }
    .write(&chrome.join("Profile 2"));
    std::fs::write(
        chrome.join("Local State"),
        r#"{"profile":{"info_cache":{"Default":{"name":"Personal"},"Profile 2":{"name":"Sealed"}}}}"#,
    )
    .unwrap();
    let firefox = root.join(if cfg!(target_os = "macos") {
        "home/Library/Application Support/Firefox"
    } else {
        "home/.mozilla/firefox"
    });
    FirefoxProfile {
        history: history(&["https://firefox.test/"]),
        ..FirefoxProfile::default()
    }
    .write(&firefox.join("abcd.default-release"));
    std::fs::write(
        firefox.join("profiles.ini"),
        "[Profile0]\nName=default-release\nIsRelative=1\nPath=abcd.default-release\n",
    )
    .unwrap();
}

/// Back to a daemon nobody has set up: the guide pending, no vault, nothing imported.
async fn reset(harness: &Harness) {
    // Locked first: the unlocked vault lives in daemon memory, the file alone decides "set up".
    let _ = harness.request("POST", "/v1/vault/lock", None).await;
    for path in [
        harness.data_dir.join("onboarding.json"),
        harness.data_dir.join("audit.jsonl"),
        harness.data_dir.join("vault.db"),
    ] {
        let _ = std::fs::remove_file(path);
    }
    let _ = std::fs::remove_dir_all(harness.data_dir.join("browser-profile"));
}

async fn onboarding_state(harness: &Harness) -> String {
    let response = harness.request("GET", "/v1/onboarding", None).await;
    assert_eq!(response.status, 200, "{}", response.body);
    response.body["state"].as_str().unwrap().to_owned()
}

fn audit(harness: &Harness, tool: &str) -> Vec<Value> {
    std::fs::read_to_string(harness.data_dir.join("audit.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|line| line["tool"] == tool)
        .collect()
}

/// Skip the step the guide is on, or continue when the person already did the work.
const NEXT: &str = "(__ob.press('Continue') || __ob.press('Skip'))";

/// One full run of the guide on `ui`, which is already open on a pending daemon at `/`.
async fn walk(ui: &mut Ui, harness: &Harness) {
    // First visit: the app opens the guide by itself, on the welcome window.
    ui.wait("guide opened by the app", "__ob.where() === '/welcome'")
        .await;
    ui.on_step("An AI assistant that works in your browser.")
        .await;
    assert_eq!(
        ui.eval("!!document.querySelector('.shell[data-focus]') && getComputedStyle(document.querySelector('.shell-sidebar')).display")
            .await,
        "none",
        "the conversation list is hidden while the guide is open"
    );
    assert_eq!(
        ui.eval("!!document.querySelector('.onb-pager')").await,
        false,
        "welcome stands outside the step count"
    );
    assert_eq!(ui.eval("__ob.enabled('Skip setup')").await, true);
    ui.snap("1-welcome").await;
    ui.press("Get started", None).await;

    // Browser import: every profile listed and preselected, passwords left to their own step.
    ui.on_step("Import from another browser").await;
    assert_eq!(ui.eval("__ob.where()").await, "/welcome?step=import");
    ui.wait(
        "three browser profiles with their counts",
        "document.querySelectorAll('.onb-profile').length === 3 && !document.body.innerText.includes('Counting')",
    )
    .await;
    assert_eq!(
        ui.eval(
            "Array.from(document.querySelectorAll('.onb-profile input')).every((i) => i.checked)"
        )
        .await,
        true,
        "every browser is preselected"
    );
    assert_eq!(
        ui.eval("document.querySelector('.onb-pager [aria-current=step]') === document.querySelector('.onb-pager li')")
            .await,
        true,
        "import is the first of the counted steps"
    );
    let body = ui.text_of(".onb").await;
    assert!(body.contains("Chrome · Personal"), "{body}");
    assert!(body.contains("Firefox · default-release"), "{body}");
    assert!(
        !body.contains("openside import") && !body.contains("openside pair"),
        "the guide must not tell the person to run a command: {body}"
    );
    assert!(
        !harness.data_dir.join("browser-profile").exists(),
        "looking at what is there wrote a browser profile"
    );
    assert_eq!(
        ui.eval("!!document.querySelector('form[aria-label=Vault]')")
            .await,
        false,
        "passwords have their own step, so no vault form here"
    );
    assert_eq!(ui.eval("__ob.enabled('Skip')").await, true);
    ui.snap("2-import").await;

    // The footer's Import brings everything over.
    ui.press("Import", Some(".onb-footer")).await;
    ui.wait(
        "import result",
        "!!document.querySelector('.onb-result [aria-label=Imported]')",
    )
    .await;
    let imported = ui.text_of(".onb-result").await;
    assert!(imported.contains("Imported from 3 profiles"), "{imported}");
    assert!(imported.contains("History"), "{imported}");
    assert_eq!(
        ui.eval("!!__ob.find('Skip')").await,
        false,
        "Skip gives way to Continue once something was imported"
    );
    // Later passes reset the data directory under a browser that is already running, which
    // does not recreate its profile, so only the first pass can see the folder appear.
    static FIRST_PASS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
    if FIRST_PASS.swap(false, std::sync::atomic::Ordering::SeqCst) {
        let profile = harness.data_dir.join("browser-profile/Default");
        assert!(profile.is_dir(), "{} missing", profile.display());
    }
    ui.snap("3-import-done").await;

    // Audit: what happened, never a cookie value.
    let lines = audit(harness, "import");
    let decisions: Vec<_> = lines
        .iter()
        .map(|line| line["decision"].as_str().unwrap())
        .collect();
    assert!(
        decisions.contains(&"previewed") && decisions.contains(&"imported"),
        "{decisions:?}"
    );
    let raw = std::fs::read_to_string(harness.data_dir.join("audit.jsonl")).unwrap();
    assert!(!raw.contains(COOKIE_VALUE));

    // Passwords: the import sheet opens and closes without writing anything; the step is skippable.
    ui.press("Continue", Some(".onb-footer")).await;
    ui.on_step("Let openside fill in your passwords").await;
    ui.snap("4-passwords").await;
    ui.press("Import", Some(".onb-footer")).await;
    ui.wait(
        "vault import sheet",
        "document.body.innerText.includes('Import credentials to openside Vault') && !!__ob.find('Not now')",
    )
    .await;
    ui.snap("5-vault-sheet").await;
    ui.press("Not now", None).await;
    ui.wait(
        "sheet closed",
        "!document.body.innerText.includes('Import credentials to openside Vault')",
    )
    .await;
    assert!(
        !harness.data_dir.join("vault.db").exists(),
        "closing the sheet set up a vault"
    );
    ui.press("Skip", Some(".onb-footer")).await;

    // AI subscription: ChatGPT is preselected; connecting one is optional.
    ui.on_step("Choose your AI subscription").await;
    ui.wait(
        "subscription choices",
        "document.querySelectorAll('.onb .models-choice').length > 0",
    )
    .await;
    ui.snap("6-subscription").await;
    assert_eq!(ui.eval(NEXT).await, true);

    // All set: finishing records it on the daemon, after the default-browser prompt.
    ui.on_step("You’re all set!").await;
    assert_eq!(
        ui.eval("!!document.querySelector('.onb-pager')").await,
        false
    );
    assert_eq!(
        ui.eval("document.querySelector('.onb-option input')?.checked")
            .await,
        true,
        "the default-browser option starts ticked"
    );
    ui.snap("7-done").await;
    assert_eq!(onboarding_state(harness).await, "pending");
    ui.press("Get started", Some(".onb-footer")).await;
    ui.wait(
        "default-browser prompt",
        "document.body.innerText.includes('Make openside your default browser?')",
    )
    .await;
    ui.snap("8-default-browser").await;
    ui.press("Not now", None).await;
    ui.wait(
        "app home",
        "__ob.where() === '/' && !!document.querySelector('main.home')",
    )
    .await;
    assert_eq!(onboarding_state(harness).await, "completed");

    // Shown once: a reload stays on the app.
    ui.goto("/").await;
    tokio::time::sleep(Duration::from_millis(1_000)).await;
    assert_eq!(ui.eval("__ob.where()").await, "/");

    // Reopened from Settings › General; leaving it again does not turn "completed" into "skipped".
    ui.goto("/settings/general").await;
    ui.wait("settings general", "!!__ob.find('Run setup again')")
        .await;
    ui.snap("9-settings").await;
    ui.press("Run setup again", None).await;
    ui.wait("guide from settings", "__ob.where() === '/welcome'")
        .await;
    ui.press("Skip setup", None).await;
    ui.wait(
        "app again",
        "__ob.where() === '/' && !!document.querySelector('main.home')",
    )
    .await;
    assert_eq!(onboarding_state(harness).await, "completed");
}

#[tokio::test]
async fn first_run_guide_is_walked_through_public_endpoints() {
    let (Some(dist), Some(executable)) = (bundle(), chromium()) else {
        return;
    };
    let Some(mut harness) = Harness::prepare(
        DAEMON,
        &json!({"responses":[{"parts":[{"type":"text","text":"unused"}]}]}),
    ) else {
        return;
    };
    let root = harness.root().to_owned();
    seed_browsers(&root);
    // Pinned: `auto` would reach the developer's real keyring; skills stay inside the root.
    harness.append_config(&format!(
        "[ui]\ndir = {}\n[vault]\nunlock = \"passphrase\"\n[skills]\ndir = {}\n[browser]\nexecutable = {}\n",
        json!(dist),
        json!(root.join("skill-library")),
        json!(executable)
    ));
    harness.launch().await;

    // The desktop launcher's way in: the socket hands out the per-boot local secret and an owner
    // device, which the app takes from the URL fragment. Import is refused without it.
    let bootstrap = harness
        .request("GET", "/v1/remote/local-access", None)
        .await;
    assert_eq!(bootstrap.status, 200, "{}", bootstrap.body);
    let launch = bootstrap.body["url"].as_str().unwrap().to_owned();
    let origin = launch
        .split('#')
        .next()
        .unwrap()
        .trim_end_matches('/')
        .to_owned();

    let (browser, _events) = Browser::launch(BrowserConfig::new(executable))
        .await
        .unwrap();
    let tab = browser.create_agent_tab(OWNER, &launch).await.unwrap().id;
    let emulation = Emulation::attach(&browser.endpoint().ws_url, &tab).await;
    let mut ui = Ui {
        browser,
        tab,
        origin,
        emulation,
        prefix: None,
    };
    ui.wait(
        "local bootstrap",
        "!!document.querySelector('.shell') && location.hash === ''",
    )
    .await;

    // A failed assertion must not leave Chromium behind: it would hold the heavy-run lock.
    let outcome = std::panic::AssertUnwindSafe(scenario(&mut ui, &harness))
        .catch_unwind()
        .await;
    ui.browser.shutdown().await.unwrap();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

async fn scenario(ui: &mut Ui, harness: &Harness) {
    let out = std::env::var_os("OPENSIDE_SCREENS_DIR").map(PathBuf::from);
    if let Some(out) = &out {
        std::fs::create_dir_all(out).unwrap();
    }
    let passes: Vec<(&Viewport, &str)> = if out.is_some() {
        vec![
            (&DESKTOP, "light"),
            (&DESKTOP, "dark"),
            (&PHONE, "light"),
            (&PHONE, "dark"),
        ]
    } else {
        vec![(&DESKTOP, "light")]
    };
    for (viewport, scheme) in passes {
        reset(harness).await;
        ui.emulation.set(viewport, scheme).await;
        ui.prefix = out
            .as_ref()
            .map(|out| out.join(format!("{scheme}-{}-", viewport.name)));
        ui.goto("/").await;
        walk(ui, harness).await;
    }

    // Skipping: stored once, never asked again.
    reset(harness).await;
    ui.prefix = None;
    ui.goto("/").await;
    ui.wait("guide", "__ob.where() === '/welcome'").await;
    ui.press("Skip setup", None).await;
    ui.wait(
        "app",
        "__ob.where() === '/' && !!document.querySelector('main.home')",
    )
    .await;
    assert_eq!(onboarding_state(harness).await, "skipped");
    ui.goto("/").await;
    tokio::time::sleep(Duration::from_millis(1_000)).await;
    assert_eq!(ui.eval("__ob.where()").await, "/");

    // A deep link is not interrupted even while the guide is pending.
    reset(harness).await;
    assert_eq!(onboarding_state(harness).await, "pending");
    ui.goto("/sessions/none").await;
    tokio::time::sleep(Duration::from_millis(1_000)).await;
    assert_eq!(ui.eval("__ob.where()").await, "/sessions/none");

    assert_eq!(
        ui.eval("window.__violations ? window.__violations.length : 0")
            .await,
        0,
        "CSP violations in the UI"
    );
}
