//! F-5: the first-run guide in the built web app, walked end to end in headless Chromium
//! against a throwaway daemon. Fake Chrome and Firefox profiles sit where the daemon looks for
//! them, a fake agent configuration directory is detected, and the "phone" is a pairing code
//! exchanged over TCP. Covers the one-time auto-open, every step through public endpoints only,
//! the dry-run preview before any write, skipping, and reopening from Settings.
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
const CLI: &str = env!("CARGO_BIN_EXE_e2e-openside");
const PASSPHRASE: &str = "ONBOARDING-VAULT-PASSPHRASE-CANARY-77";
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
    choose(text) {
      const row = Array.from(document.querySelectorAll('.onb-choice')).find((r) => r.textContent.includes(text));
      if (!row) return false;
      row.querySelector('input').click();
      return true;
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
    let chrome = root.join("config/google-chrome");
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
    let firefox = root.join("home/.mozilla/firefox");
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

/// Back to a daemon nobody has set up: the guide pending, no skill, nothing imported.
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
    let _ = std::fs::remove_dir_all(harness.root().join("home/.claude/skills"));
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

/// One full run of the guide on `ui`, which is already open on a pending daemon at `/`.
async fn walk(ui: &mut Ui, harness: &Harness) {
    // First visit: the app opens the guide by itself.
    ui.wait("guide opened by the app", "__ob.where() === '/welcome'")
        .await;
    ui.on_step("Welcome to openside").await;
    assert_eq!(
        ui.eval("!!document.querySelector('.shell[data-focus]') && getComputedStyle(document.querySelector('.shell-sidebar')).display")
            .await,
        "none",
        "the conversation list is hidden while the guide is open"
    );
    ui.snap("1-welcome").await;

    // Agents: the fake `~/.claude` is detected; the skill is installed on request.
    ui.press("Get started", None).await;
    ui.on_step("Connect your agents").await;
    ui.wait(
        "Claude Code found",
        "Array.from(document.querySelectorAll('.onb article')).some((a) => a.textContent.includes('Claude Code') && a.textContent.includes('Found'))",
    )
    .await;
    let skill = harness
        .root()
        .join("home/.claude/skills/openside-browser/SKILL.md");
    assert!(!skill.exists());
    ui.snap("2-agents").await;
    ui.press("Install the browser skill for Claude Code", None)
        .await;
    ui.wait(
        "skill installed",
        "Array.from(document.querySelectorAll('.onb article')).some((a) => a.textContent.includes('Claude Code') && a.textContent.includes('Skill installed'))",
    )
    .await;
    assert!(skill.is_file(), "{} was not written", skill.display());
    ui.snap("3-agents-installed").await;

    // Import: the sealed profile explains itself instead of writing anything.
    ui.press("Continue", None).await;
    ui.on_step("Bring your browser data").await;
    ui.wait(
        "profiles",
        "document.querySelectorAll('.onb-choice').length === 3",
    )
    .await;
    assert_eq!(
        ui.eval("__ob.enabled('Import')").await,
        false,
        "no write before a preview"
    );
    ui.snap("4-import").await;
    assert_eq!(ui.eval("__ob.choose('Chrome · Sealed')").await, true);
    ui.press("Preview", None).await;
    ui.wait(
        "sealed cookies refused",
        "!!document.querySelector('.onb [role=alert]')",
    )
    .await;
    let problem = ui.text_of(".onb-problem").await;
    assert!(problem.contains("keyring"), "{problem}");
    assert!(!problem.contains(&harness.root().display().to_string()));
    ui.snap("5-import-error").await;
    ui.press("Leave cookies out", None).await;
    ui.press("Preview", None).await;
    ui.wait(
        "history-only preview",
        "!!document.querySelector('.onb-counts')",
    )
    .await;
    assert!(ui
        .text_of(".onb-counts")
        .await
        .contains("1 history entry to import"));
    assert!(!ui.text_of(".onb-counts").await.contains("Cookies"));

    // The profile that has everything: preview first, then the write.
    assert_eq!(ui.eval("__ob.choose('Chrome · Personal')").await, true);
    assert_eq!(
        ui.eval("!!document.querySelector('.onb-counts')").await,
        false
    );
    ui.press("Preview", None).await;
    ui.wait("full preview", "!!document.querySelector('.onb-counts')")
        .await;
    let counts = ui.text_of(".onb-counts").await;
    for expected in [
        "2 history entries to import",
        "1 bookmark to import",
        "2 cookies to import",
    ] {
        assert!(counts.contains(expected), "{expected} not in {counts}");
    }
    assert!(
        !harness.data_dir.join("browser-profile").exists(),
        "a preview wrote a browser profile"
    );
    ui.snap("6-import-preview").await;
    ui.press("Import", None).await;
    ui.wait(
        "import result",
        "!!document.querySelector('.onb-result [aria-label=Imported]')",
    )
    .await;
    let imported = ui.text_of(".onb-result").await;
    assert!(
        imported.contains("Imported from Chrome · Personal"),
        "{imported}"
    );
    assert!(imported.contains("2 imported"), "{imported}");
    let profile = harness.data_dir.join("browser-profile/Default");
    assert!(profile.is_dir(), "{} missing", profile.display());
    ui.snap("7-import-done").await;

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

    // Vault: opens over the guide and returns to the same step once it is set up.
    ui.press("Continue", None).await;
    ui.on_step("Keep logins in the vault").await;
    ui.wait("vault status", "!!__ob.find('Set up the vault')")
        .await;
    ui.snap("8-vault").await;
    ui.press("Set up the vault", None).await;
    ui.wait(
        "vault dialog over the guide",
        "__ob.where() === '/vault' && !!document.querySelector('form[aria-label=\"Set up vault\"]') && __ob.title() === 'Keep logins in the vault'",
    )
    .await;
    ui.eval(&format!(
        "(() => {{ const f = document.querySelector('form[aria-label=\"Set up vault\"]'); \
         f.querySelectorAll('input[type=password]').forEach((i) => __ob.fill(i, {p})); \
         Array.from(f.querySelectorAll('button')).find((b) => b.textContent.trim() === 'Set up vault').click(); return true }})()",
        p = json!(PASSPHRASE)
    ))
    .await;
    ui.wait(
        "vault unlocked",
        "!!document.querySelector('[data-section=vault] article[data-state=unlocked]')",
    )
    .await;
    ui.snap("9-vault-dialog").await;
    ui.press("Close", None).await;
    ui.wait(
        "back on the guide",
        "__ob.where() === '/welcome?step=vault' && !!__ob.find('Open the vault')",
    )
    .await;
    ui.snap("10-vault-ready").await;

    // Pair: the page shows the command; running it and scanning the code makes the phone appear.
    ui.press("Continue", None).await;
    ui.on_step("Pair your phone").await;
    ui.wait(
        "waiting",
        "document.body.innerText.includes('Waiting for a new device')",
    )
    .await;
    assert!(ui
        .text_of(".onb-command")
        .await
        .contains("openside pair --name \"My phone\""));
    ui.snap("11-pair").await;
    let offer = harness
        .cli(CLI, &["--json", "pair", "--name", "My phone"], None)
        .await;
    assert!(
        offer.status.success(),
        "{}",
        String::from_utf8_lossy(&offer.stderr)
    );
    // `--json` ends every command with a summary record; the payload is the first line.
    let first = offer.stdout.split(|byte| *byte == b'\n').next().unwrap();
    let offer: Value = serde_json::from_slice(first).unwrap();
    let exchanged = harness
        .tcp_request(
            "POST",
            "/v1/pairing/exchange",
            None,
            Some(json!({"code": offer["code"]})),
        )
        .await;
    assert_eq!(exchanged.status, 201, "{}", exchanged.body);
    ui.wait(
        "phone noticed",
        "document.body.innerText.includes('My phone is paired')",
    )
    .await;
    ui.snap("12-pair-done").await;

    // Done: the recap lists what happened, and finishing records it on the daemon.
    ui.press("Continue", None).await;
    ui.on_step("You’re all set").await;
    let recap = ui.text_of(".onb-recap").await;
    for expected in [
        "Browser skill for Claude Code",
        "Imported from Chrome · Personal",
        "Set up",
        "My phone paired",
    ] {
        assert!(recap.contains(expected), "{expected} not in {recap}");
    }
    assert!(!recap.contains("Skipped"), "{recap}");
    ui.snap("13-done").await;
    assert_eq!(onboarding_state(harness).await, "pending");
    ui.press("Open openside", None).await;
    ui.wait(
        "app home",
        "__ob.where() === '/' && !!document.querySelector('.composer')",
    )
    .await;
    assert_eq!(onboarding_state(harness).await, "completed");

    // Shown once: a reload stays on the app.
    ui.goto("/").await;
    tokio::time::sleep(Duration::from_millis(1_000)).await;
    assert_eq!(ui.eval("__ob.where()").await, "/");

    // Reopened from Settings; leaving it again does not turn "completed" into "skipped".
    ui.press("Settings", None).await;
    let entry = "Array.from(document.querySelectorAll('.workspace-entry')).find((b) => b.textContent.includes('Setup guide'))";
    ui.wait("settings dialog", &format!("!!{entry}")).await;
    ui.snap("14-settings").await;
    assert_eq!(ui.eval(&format!("{entry}.click() ?? true")).await, true);
    ui.wait("guide from settings", "__ob.where() === '/welcome'")
        .await;
    ui.press("Skip setup", None).await;
    ui.wait("app again", "__ob.where() === '/'").await;
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
    // The agent's configuration directory exists, so it counts as installed.
    std::fs::create_dir_all(root.join("home/.claude")).unwrap();
    // Pinned: `auto` would reach the developer's real keyring; skills stay inside the root.
    harness.append_config(&format!(
        "[ui]\ndir = {}\n[vault]\nunlock = \"passphrase\"\n[skills]\ndir = {}\n[browser]\nexecutable = {}\n",
        json!(dist),
        json!(root.join("skill-library")),
        json!(executable)
    ));
    harness.launch().await;

    // Admin over TCP, as a phone or another computer would hold; no localStorage scopes are
    // stored, like a pasted token.
    let paired = harness
        .request(
            "POST",
            "/v1/devices/pair",
            Some(json!({"name":"onboarding e2e","scopes":["read","approve","admin"]})),
        )
        .await;
    assert_eq!(paired.status, 201, "{}", paired.body);
    let token = paired.body["token"].as_str().unwrap().to_owned();
    let port = std::fs::read_to_string(harness.data_dir.join("opensided.port"))
        .unwrap()
        .trim()
        .to_owned();
    let origin = format!("http://127.0.0.1:{port}");

    let (browser, _events) = Browser::launch(BrowserConfig::new(executable))
        .await
        .unwrap();
    let tab = browser
        .create_agent_tab(OWNER, &format!("{origin}/"))
        .await
        .unwrap()
        .id;
    let emulation = Emulation::attach(&browser.endpoint().ws_url, &tab).await;
    let mut ui = Ui {
        browser,
        tab,
        origin,
        emulation,
        prefix: None,
    };
    ui.wait("token screen", "!!document.querySelector('.token-entry')")
        .await;
    ui.eval(&format!(
        "localStorage.setItem('openside.token', {}); true",
        json!(token)
    ))
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
    ui.wait("app", "__ob.where() === '/'").await;
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
