//! Screenshot harness for the browser surfaces of the web UI (visual QA, ignored by default):
//! the live browser card (running, cancel confirmation, minimized, done, folded, action
//! needed), the browser sheet with its tab list and zoom, the takeover card,
//! remote screen and its PiP, the inline code form, the `?ticket=` code sheet, the floating
//! code card over a conversation and over Settings, and the save-authenticator approval card.
//!
//! Everything is real: a hermetic daemon on a scripted model drives the agent Chromium against
//! loopback pages, so the preview and takeover show live screencast frames. Captures run in two
//! passes: the preview first, while nothing waits on the person, then the tickets.
//!
//! Run through `tools/qa/web-screens-browser.sh [out-dir]`, which rebuilds the web app first.
//! Env: `OPENSIDE_SCREENS_DIR` (default `/tmp/openside-qa/redesign/browser`),
//! `OPENSIDE_SCREENS_DIST` (the built app to serve, default `web/dist`),
//! `OPENSIDE_SCREENS_ONLY`, a comma-separated subset of: card-running, card-cancel,
//! card-minimized, browser-sheet, browser-tabs, browser-zoom, card-done, card-folded,
//! card-action, otp-inline, otp-sheet, otp-dock,
//! otp-dock-settings, takeover-card, takeover, takeover-pip, register-card, and
//! `OPENSIDE_SCREENS_PASSES`, a comma-separated subset of light-phone, light-desktop,
//! dark-phone, dark-desktop. A takeover admits a single remote-screen connection, so one run
//! captures `takeover` and `takeover-pip` in its first pass only; the wrapper script reruns
//! them for the other passes.
//! Files are named `<theme>-<phone|desktop>-<screen>.png`.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{path::PathBuf, time::Duration};

use axum::{response::Html, routing::get, Router};
use futures_util::{SinkExt, StreamExt};
use openside_browser::{Browser, BrowserConfig};
use openside_e2e::{Harness, SseReader};
use openside_proto::{Event, EventKind};
use openside_vault::{PassphraseLock, Secret, Vault};
use serde_json::{json, Value};
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};

const OWNER: &str = "web-screens-browser";
const DAEMON: &str = env!("CARGO_BIN_EXE_e2e-opensided");

fn text(text: &str) -> Value {
    json!({"parts":[{"type":"text","text":text}]})
}
fn tool(name: &str, args: Value, id: &str) -> Value {
    json!({"parts":[{"type":"tool_call","name":name,"args":args,"id":id}]})
}

const STYLE: &str = "<style>body{margin:0;font:16px/1.55 system-ui,sans-serif;color:#1d2433;background:#f6f7fb}\
header{display:flex;align-items:center;gap:12px;padding:18px 48px;background:#fff;border-bottom:1px solid #e3e6ee}\
header b{font-size:20px}nav{margin-left:auto;display:flex;gap:24px;color:#5a6478}\
main{max-width:880px;margin:40px auto;padding:0 32px}h1{font-size:40px;margin:0 0 12px}\
.cards{display:grid;grid-template-columns:repeat(3,1fr);gap:20px;margin-top:28px}\
.card{background:#fff;border:1px solid #e3e6ee;border-radius:14px;padding:22px}\
.card strong{display:block;font-size:28px;margin:8px 0}button,.btn{display:inline-block;padding:12px 22px;border:0;border-radius:10px;background:#3657d6;color:#fff;font:inherit}\
input{font:inherit;padding:12px 14px;border:1px solid #c9cfdb;border-radius:10px;width:240px}label{display:grid;gap:6px;margin:18px 0}\
code{background:#eef1f7;padding:4px 8px;border-radius:6px}</style>";

fn page(title: &str, body: &str) -> String {
    format!("<!doctype html><title>{title}</title>{STYLE}<header><b>Acme Cloud</b><nav><span>Product</span><span>Pricing</span><span>Docs</span></nav></header><main>{body}</main>")
}

/// Loopback pages for the agent's tabs; dropped with the test.
struct Site {
    origin: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Site {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Site {
    async fn start() -> Self {
        let pricing = page("Pricing · Acme Cloud", "<h1>Simple pricing</h1><p>Pick the plan that fits your team. Every plan includes unlimited projects.</p><div class='cards'><div class='card'>Starter<strong>$0</strong>For side projects</div><div class='card'>Team<strong>$24</strong>Per seat, monthly</div><div class='card'>Enterprise<strong>Custom</strong>SSO, audit log, SLA</div></div>");
        let docs = page("Release notes 0.4 — what changed in the scheduler, the vault and the web app", "<h1>Release notes 0.4</h1><p>Twelve fixes, three features and one breaking change. Routines now run on the daemon's own clock, the vault unlocks with a passkey, and the web app shows the agent's browser live.</p><p><span class='btn'>Read the migration guide</span></p>");
        let login = page("Sign in · Acme Cloud", "<h1>Check your phone</h1><p>We texted a 6-digit code to the number ending in 42.</p><form method='post' action='/login'><label>Verification code<input name='code' inputmode='numeric' autocomplete='one-time-code' maxlength='6'></label><button>Verify</button></form>");
        let captcha = page("Checkout · Acme Cloud", "<h1>One more step</h1><p>Confirm you are human to finish the checkout.</p><div class='card' style='width:300px;display:flex;align-items:center;gap:14px'><span style='width:26px;height:26px;border:2px solid #9aa3b5;border-radius:6px'></span>I'm not a robot</div>");
        let setup = page("Two-step verification · Acme Cloud", "<h1>Turn on two-step verification</h1><p>Scan the QR code with your authenticator app, or enter this setup link:</p><p><code>otpauth://totp/Acme:alice@acme.test?secret=JBSWY3DPEHPK3PXP&amp;issuer=Acme</code></p>");
        let app = Router::new()
            .route("/pricing", get(move || async move { Html(pricing) }))
            .route("/docs", get(move || async move { Html(docs) }))
            .route("/login", get(move || async move { Html(login) }))
            .route("/captcha", get(move || async move { Html(captcha) }))
            .route("/setup", get(move || async move { Html(setup) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        // Vault allowlists accept DNS origins, not IP literals.
        Self {
            origin: format!("http://localhost:{port}"),
            task,
        }
    }
}

struct Viewport {
    name: &'static str,
    width: u32,
    height: u32,
    scale: f32,
    mobile: bool,
}

const VIEWPORTS: [Viewport; 2] = [
    Viewport {
        name: "phone",
        width: 390,
        height: 844,
        scale: 2.0,
        mobile: true,
    },
    Viewport {
        name: "desktop",
        width: 1440,
        height: 900,
        scale: 1.0,
        mobile: false,
    },
];

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// A second DevTools session on the UI tab for viewport and colour-scheme emulation; the
/// overrides last as long as the socket (as in web_screens.rs).
struct Emulation {
    socket: Socket,
    session: String,
    next: u64,
    /// Requests and WebSockets of the UI tab that have not finished yet (CDP Network events),
    /// keyed by request id: tells which streams hold the page's connections to the daemon.
    open: std::collections::BTreeMap<String, String>,
}

impl Emulation {
    async fn attach(ws_url: &str, tab: &str) -> Self {
        let (socket, _) = tokio_tungstenite::connect_async(ws_url).await.unwrap();
        let mut emulation = Self {
            socket,
            session: String::new(),
            next: 0,
            open: std::collections::BTreeMap::new(),
        };
        let attached = emulation
            .call(
                "Target.attachToTarget",
                json!({"targetId":tab,"flatten":true}),
            )
            .await;
        emulation.session = attached["sessionId"].as_str().unwrap().to_owned();
        emulation.call("Network.enable", json!({})).await;
        emulation
    }

    fn note(&mut self, event: &Value) {
        let params = &event["params"];
        let id = params["requestId"].as_str().unwrap_or_default().to_owned();
        match event["method"].as_str().unwrap_or_default() {
            "Network.requestWillBeSent" => {
                let url = params["request"]["url"].as_str().unwrap_or_default();
                if !url.starts_with("data:") && !url.starts_with("blob:") {
                    let kind = params["type"].as_str().unwrap_or("?");
                    self.open.insert(id, format!("{kind} {url}"));
                }
            }
            "Network.webSocketCreated" => {
                let url = params["url"].as_str().unwrap_or_default();
                self.open.insert(id, format!("WebSocket {url}"));
            }
            "Network.loadingFinished" | "Network.loadingFailed" | "Network.webSocketClosed" => {
                self.open.remove(&id);
            }
            _ => {}
        }
    }

    /// Requests and WebSockets of the page still open now.
    async fn open_requests(&mut self) -> Vec<String> {
        while let Ok(Some(Ok(Message::Text(body)))) =
            tokio::time::timeout(Duration::from_millis(200), self.socket.next()).await
        {
            let event: Value = serde_json::from_str(body.as_str()).unwrap();
            self.note(&event);
        }
        self.open.values().cloned().collect()
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
                self.note(&reply);
            }
        }
    }

    async fn viewport(&mut self, viewport: &Viewport) {
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
    }

    async fn scheme(&mut self, scheme: &str) {
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

    /// Pauses a hung renderer and prints its JS stack: diagnoses the intermittent renderer hang
    /// (navigate and evaluate time out while the daemon still answers).
    async fn stack(&mut self) {
        for method in ["Debugger.enable", "Debugger.pause"] {
            self.next += 1;
            let request =
                json!({"id":self.next,"method":method,"params":{},"sessionId":self.session});
            self.socket
                .send(Message::Text(request.to_string().into()))
                .await
                .unwrap();
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        while let Ok(Some(Ok(message))) =
            tokio::time::timeout_at(deadline, self.socket.next()).await
        {
            if let Message::Text(body) = message {
                let reply: Value = serde_json::from_str(body.as_str()).unwrap();
                if reply["method"] == "Debugger.paused" {
                    for frame in reply["params"]["callFrames"].as_array().unwrap() {
                        eprintln!(
                            "  at {} {}:{}:{}",
                            frame["functionName"],
                            frame["url"],
                            frame["location"]["lineNumber"],
                            frame["location"]["columnNumber"]
                        );
                    }
                    return;
                }
            }
        }
        eprintln!("no Debugger.paused within 15s");
    }
}

struct Ui {
    browser: Browser,
    tab: String,
    origin: String,
    emulation: tokio::sync::Mutex<Emulation>,
}

impl Ui {
    async fn eval(&self, expression: &str) -> Value {
        self.browser
            .evaluate(&self.tab, expression)
            .await
            .unwrap_or_else(|e| panic!("evaluate {expression}: {e:?}"))
    }

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
                // Tells a stalled page network (fetches never settle) from a daemon that stopped
                // answering: the same URL from inside the page and from curl, plus the last API
                // responses the page saw.
                let page = self
                    .eval("(async () => { const t0 = performance.now(); const r = await Promise.race([fetch('/v1/health', {cache: 'no-store'}).then((r) => 'status ' + r.status, (e) => 'error ' + e), new Promise((ok) => setTimeout(() => ok('no answer in 5s'), 5000))]); const done = performance.getEntriesByType('resource').filter((e) => e.name.includes('/v1/')).slice(-8).map((e) => Math.round(e.startTime) + '+' + Math.round(e.duration) + ' ' + new URL(e.name).pathname); return { health: r, ms: Math.round(performance.now() - t0), now: Math.round(performance.now()), done } })()")
                    .await;
                let curl = std::process::Command::new("curl")
                    .args([
                        "-s",
                        "-o",
                        "/dev/null",
                        "-w",
                        "%{http_code} %{time_total}",
                        "-m",
                        "5",
                        &format!("{}/v1/health", self.origin),
                    ])
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
                let port = self.origin.rsplit(':').next().unwrap();
                let sockets = std::process::Command::new("ss")
                    .args([
                        "-tnpH",
                        "state",
                        "established",
                        &format!("( dport = :{port} )"),
                    ])
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
                let token = self
                    .eval("localStorage.getItem('openside.token') || ''")
                    .await;
                let auth = format!("Authorization: Bearer {}", token.as_str().unwrap_or(""));
                for path in [
                    "/v1/health",
                    "/v1/sessions",
                    "/v1/tickets",
                    "/v1/browser/tabs",
                ] {
                    let probe = std::process::Command::new("curl")
                        .args([
                            "-s",
                            "-o",
                            "/dev/null",
                            "-w",
                            "%{http_code} %{time_total}",
                            "-m",
                            "5",
                            "-H",
                            &auth,
                            &format!("{}{path}", self.origin),
                        ])
                        .output()
                        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
                    eprintln!("stall probe curl {path}: {probe:?}");
                }
                eprintln!("stall probe for {what}: page {page}; curl {curl:?}; sockets to the daemon:\n{}", sockets.unwrap_or_default());
                let open = self.emulation.lock().await.open_requests().await;
                eprintln!(
                    "stall probe: {} open requests/WebSockets of the page:\n  {}",
                    open.len(),
                    open.join("\n  ")
                );
                panic!("timed out waiting for {what}; page text: {text}");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn goto(&self, path: &str) {
        let url = format!("{}{path}", self.origin);
        for attempt in 1..=3 {
            match self.browser.navigate(OWNER, &self.tab, &url).await {
                Ok(_) => break,
                Err(error) if attempt < 3 => {
                    let probe = std::process::Command::new("curl")
                        .args([
                            "-s",
                            "-o",
                            "/dev/null",
                            "-w",
                            "%{http_code} %{time_total}",
                            "-m",
                            "5",
                            &url,
                        ])
                        .output()
                        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
                    let alive = tokio::time::timeout(
                        Duration::from_secs(3),
                        self.browser
                            .evaluate(&self.tab, "location.href + ' ' + document.readyState"),
                    )
                    .await;
                    eprintln!(
                        "navigate {path}: {error:?}; curl {probe:?}; page {alive:?}; retrying"
                    );
                    self.emulation.lock().await.stack().await;
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
                Err(error) => panic!("navigate {path}: {error:?}"),
            }
        }
        self.wait(
            "document",
            "document.readyState === 'complete' && !!document.querySelector('.shell')",
        )
        .await;
    }

    /// Clicks the visible button named `label` (accessible name or text) inside `scope`.
    async fn click(&self, scope: &str, label: &str) {
        let found = self
            .eval(&format!(
                "(() => {{ const b = Array.from(document.querySelectorAll({scope} + ' button, ' + {scope} + ' a')).find((b) => (b.getAttribute('aria-label') || b.textContent.trim()) === {label} && b.getClientRects().length); if (!b) return false; b.click(); return true }})()",
                scope = json!(scope),
                label = json!(label)
            ))
            .await;
        assert_eq!(found, true, "no {label:?} button in {scope}");
    }

    async fn snap(&self, path: &std::path::Path) {
        self.eval("document.fonts.ready.then(() => true)").await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        let png = self.emulation.lock().await.capture().await;
        std::fs::write(path, png).unwrap();
        eprintln!("captured {}", path.display());
    }
}

fn ended_ok<'a>(events: &'a [Event], call: &str) -> &'a Value {
    events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::ToolEnded {
                card: _,
                tool_call_id,
                result,
                is_error: false,
            } if tool_call_id == call => Some(result),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing successful {call}: {events:?}"))
}

fn field(snapshot: &Value, name: &str) -> String {
    let text = snapshot.as_str().expect("compact accessibility snapshot");
    let label = format!("\"{name}\"");
    let line = text
        .lines()
        .find(|line| line.contains(&label) && line.contains("textbox"))
        .unwrap_or_else(|| panic!("snapshot lacks {name}: {text}"));
    let start = line.find('[').unwrap() + 1;
    let end = line[start..].find(']').unwrap() + start;
    line[start..end].to_owned()
}

async fn submit(harness: &Harness, session: &str, id: &str, message: &str) {
    let response = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            Some(json!({"message_id":id,"text":message,"mode":"now"})),
        )
        .await;
    assert_eq!(response.status, 202, "{}", response.body);
}

async fn create(harness: &Harness, title: &str) -> String {
    let response = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"title":title,"mode":"full","workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(response.status, 201, "{}", response.body);
    response.body["id"].as_str().unwrap().to_owned()
}

/// Events through the settle that follows the final assistant message `final_text`.
async fn until_final(stream: &mut SseReader, final_text: &str) -> Vec<Event> {
    let mut events = Vec::new();
    loop {
        let event = stream.next().await;
        let done = matches!(event.kind, EventKind::SessionSettled {})
            && events.iter().any(|e: &Event| matches!(&e.kind, EventKind::AssistantFinal { text } if text == final_text));
        events.push(event);
        if done {
            return events;
        }
    }
}

/// PIDs of every scripted sidecar the harness started so far (one per session runtime), from
/// their stderr capture files.
fn sidecars(harness: &Harness) -> std::collections::HashSet<i32> {
    std::fs::read_dir(harness.root().join("stderr"))
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry
                .ok()?
                .file_name()
                .to_str()?
                .strip_prefix("sidecar-")?
                .strip_suffix(".stderr")?
                .parse()
                .ok()
        })
        .collect()
}

/// Kills the session runtime `pid` and waits until its session settled the interruption, so
/// the next message reopens a runtime on the current script at the saved cursor.
async fn restart_runtime(pid: i32, stream: &mut SseReader) {
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    let mut interrupted = false;
    loop {
        match stream.next().await.kind {
            EventKind::RuntimeError { code, .. } if code == "interrupted" => interrupted = true,
            EventKind::SessionSettled {} if interrupted => break,
            _ => {}
        }
    }
}

/// The id of the next ticket the session opens.
async fn opened(stream: &mut SseReader) -> String {
    loop {
        if let EventKind::TicketOpened { ticket } = stream.next().await.kind {
            return ticket.id.to_string();
        }
    }
}

#[tokio::test]
#[ignore = "visual QA harness; run tools/qa/web-screens-browser.sh"]
async fn web_screens_browser_capture() {
    let dist = std::env::var_os("OPENSIDE_SCREENS_DIST").map_or_else(
        || std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist"),
        PathBuf::from,
    );
    assert!(
        dist.join("index.html").is_file(),
        "{} is missing; run pnpm -C web build",
        dist.display()
    );
    let executable = openside_config::discover_browser(&openside_config::Config::default())
        .expect("headless Chromium");
    let out = PathBuf::from(
        std::env::var("OPENSIDE_SCREENS_DIR")
            .unwrap_or_else(|_| "/tmp/openside-qa/redesign/browser".into()),
    );
    std::fs::create_dir_all(&out).unwrap();
    let only: Vec<String> = std::env::var("OPENSIDE_SCREENS_ONLY")
        .map(|v| v.split(',').map(|s| s.trim().to_owned()).collect())
        .unwrap_or_default();
    let wanted = |screen: &str| only.is_empty() || only.iter().any(|s| s == screen);
    let passes: Vec<String> = std::env::var("OPENSIDE_SCREENS_PASSES")
        .map(|v| v.split(',').map(|s| s.trim().to_owned()).collect())
        .unwrap_or_default();
    let pass_wanted = |theme: &str, viewport: &str| {
        passes.is_empty() || passes.iter().any(|p| *p == format!("{theme}-{viewport}"))
    };

    let site = Site::start().await;
    let mut harness =
        Harness::prepare(DAEMON, &json!({"responses":[]})).expect("hermetic daemon prerequisites");
    // register_authenticator needs a vault it can unlock without a person.
    let vault_path = harness.root().join("screens.vault");
    drop(
        Vault::create(
            &vault_path,
            harness.root().join("setup.audit.jsonl"),
            &Secret::new("disposable-screens-passphrase".into()),
            PassphraseLock::default(),
        )
        .unwrap(),
    );
    let unlock = harness.root().join("unlock.sh");
    std::fs::write(
        &unlock,
        "#!/bin/sh\nprintf '%s\\n' 'disposable-screens-passphrase'\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&unlock, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    harness.append_config(&format!(
        "[ui]\ndir = {}\n[browser]\nexecutable = {}\n[vault]\npath = {}\npassphrase_cmd = [{}]\n",
        json!(dist.canonicalize().unwrap()),
        json!(executable),
        json!(vault_path),
        json!(unlock)
    ));
    harness.launch().await;

    // Every session runs its own scripted sidecar, which reads script.json when it starts and
    // keeps its cursor in the session snapshot. So each session's script is written just before
    // its runtime starts, and a later turn that needs the tab ids or field refs an earlier turn
    // returned kills that session's runtime (while nothing waits on the person) so the next
    // message restarts it on the extended script at the saved cursor.
    let origin = &site.origin;
    let browse = create(&harness, "Compare Acme plans").await;
    let login = create(&harness, "Sign in to Acme Cloud").await;
    let checkout = create(&harness, "Finish the Acme checkout").await;
    let setup = create(&harness, "Turn on Acme two-step").await;
    let mut browse_stream = harness.events(&browse, 0).await;
    let mut login_stream = harness.events(&login, 0).await;
    let mut checkout_stream = harness.events(&checkout, 0).await;
    let mut setup_stream = harness.events(&setup, 0).await;

    let compared = "Both pages are open. The Team plan is $24 per seat each month.";
    let rechecked =
        "Checked the pricing again: the Team plan is still $24 per seat. Nothing else changed.";
    let mut browse_script = vec![
        tool(
            "browser_open_tab",
            json!({"url":format!("{origin}/docs")}),
            "open-docs",
        ),
        tool(
            "browser_open_tab",
            json!({"url":format!("{origin}/pricing")}),
            "open-pricing",
        ),
        text(compared),
    ];
    harness.replace_script(&json!({"responses":browse_script}));
    let before = sidecars(&harness);
    submit(
        &harness,
        &browse,
        "b1",
        "Open Acme's pricing and the 0.4 release notes and compare the plans.",
    )
    .await;
    let events = until_final(&mut browse_stream, compared).await;
    let pricing_tab = ended_ok(&events, "open-pricing")["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let browse_pid = *sidecars(&harness).difference(&before).next().unwrap();
    // One slow browser call per capture pass keeps the run going while the card is captured;
    // the reply after it is the done chip's summary. The session owns two tabs, so the REPL
    // needs the tab id the first turn returned.
    for pass in 0..4 {
        browse_script.push(tool(
            "browser_repl",
            json!({"tab_id":pricing_tab,"code":"await page.waitForTimeout(9000)"}),
            &format!("recheck-{pass}"),
        ));
        browse_script.push(text(rechecked));
    }
    restart_runtime(browse_pid, &mut browse_stream).await;

    let mut login_script = vec![
        tool(
            "browser_open_tab",
            json!({"url":format!("{origin}/login")}),
            "open-login",
        ),
        text("The sign-in page is open."),
    ];
    harness.replace_script(&json!({"responses":login_script}));
    let before = sidecars(&harness);
    submit(
        &harness,
        &login,
        "l1",
        "Sign in to Acme Cloud with my account.",
    )
    .await;
    let events = until_final(&mut login_stream, "The sign-in page is open.").await;
    let login_tab = ended_ok(&events, "open-login")["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let login_pid = *sidecars(&harness).difference(&before).next().unwrap();

    let mut checkout_script = vec![
        tool(
            "browser_open_tab",
            json!({"url":format!("{origin}/captcha")}),
            "open-captcha",
        ),
        text("The checkout asks for a human check."),
    ];
    harness.replace_script(&json!({"responses":checkout_script}));
    let before = sidecars(&harness);
    submit(
        &harness,
        &checkout,
        "c1",
        "Buy the Team plan for three seats.",
    )
    .await;
    let events = until_final(&mut checkout_stream, "The checkout asks for a human check.").await;
    let captcha_tab = ended_ok(&events, "open-captcha")["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let checkout_pid = *sidecars(&harness).difference(&before).next().unwrap();

    let mut setup_script = vec![
        tool(
            "browser_open_tab",
            json!({"url":format!("{origin}/setup")}),
            "open-setup",
        ),
        text("The two-step setup page is open."),
    ];
    harness.replace_script(&json!({"responses":setup_script}));
    let before = sidecars(&harness);
    submit(
        &harness,
        &setup,
        "s1",
        "Turn on two-step verification for my Acme account.",
    )
    .await;
    let events = until_final(&mut setup_stream, "The two-step setup page is open.").await;
    let setup_tab = ended_ok(&events, "open-setup")["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let setup_pid = *sidecars(&harness).difference(&before).next().unwrap();

    login_script.extend([
        tool(
            "browser_snapshot",
            json!({"tab_id":login_tab}),
            "login-fields",
        ),
        text("The sign-in page wants a texted code."),
    ]);
    restart_runtime(login_pid, &mut login_stream).await;
    harness.replace_script(&json!({"responses":login_script}));
    let before = sidecars(&harness);
    submit(&harness, &login, "l2", "Find the code field.").await;
    let events = until_final(&mut login_stream, "The sign-in page wants a texted code.").await;
    let code_ref = field(ended_ok(&events, "login-fields"), "Verification code");
    let login_pid = *sidecars(&harness).difference(&before).next().unwrap();

    let token = {
        let paired = harness
            .request(
                "POST",
                "/v1/devices/pair",
                Some(json!({"name":"screens","scopes":["read","steer","approve","human_step","browse","admin"]})),
            )
            .await;
        assert_eq!(paired.status, 201, "{}", paired.body);
        paired.body["token"].as_str().unwrap().to_owned()
    };
    let port = std::fs::read_to_string(harness.data_dir.join("opensided.port"))
        .unwrap()
        .trim()
        .to_owned();
    let ui_origin = format!("http://127.0.0.1:{port}");
    let (browser, _events) = Browser::launch(BrowserConfig::new(executable))
        .await
        .unwrap();
    let tab = browser
        .create_agent_tab(OWNER, &format!("{ui_origin}/"))
        .await
        .unwrap()
        .id;
    let emulation = Emulation::attach(&browser.endpoint().ws_url, &tab).await;
    let ui = Ui {
        browser,
        tab,
        origin: ui_origin,
        emulation: tokio::sync::Mutex::new(emulation),
    };
    ui.wait("token screen", "!!document.querySelector('.token-entry')")
        .await;
    ui.eval(&format!(
        "localStorage.setItem('openside.token', {}); localStorage.setItem('openside.scopes', {}); localStorage.removeItem('openside.pipPosition'); true",
        json!(token),
        json!(json!(["read", "steer", "approve", "human_step", "browse", "admin"]).to_string())
    ))
    .await;

    // The browse conversation reruns its slow check each pass; the card narrates it, then folds
    // when it is done.
    let mut recheck = 1;

    // Pass 1: the live browser card, before any ticket waits on the person.
    let card_screens = [
        "card-running",
        "card-cancel",
        "card-minimized",
        "browser-sheet",
        "browser-tabs",
        "browser-zoom",
        "card-done",
        "card-folded",
    ];
    for theme in ["light", "dark"] {
        ui.emulation.lock().await.scheme(theme).await;
        for viewport in &VIEWPORTS {
            if !pass_wanted(theme, viewport.name)
                || !card_screens.iter().any(|screen| wanted(screen))
            {
                continue;
            }
            ui.emulation.lock().await.viewport(viewport).await;
            let file = |screen: &str| out.join(format!("{theme}-{}-{screen}.png", viewport.name));
            ui.goto(&format!("/sessions/{browse}")).await;
            ui.wait("idle card", "!!document.querySelector('.browser-card img')?.naturalWidth && document.querySelector('.browser-card')?.dataset.tone === 'idle'").await;
            recheck += 1;
            // The restarted runtime reads the script when this first rerun starts it.
            harness.replace_script(&json!({"responses":browse_script}));
            submit(
                &harness,
                &browse,
                &format!("b{recheck}"),
                "Check the pricing page again.",
            )
            .await;
            ui.wait("running card", "document.querySelector('.browser-card')?.dataset.tone === 'running' && !!document.querySelector('.browser-card .browser-card-screen img')?.naturalWidth").await;
            if wanted("card-running") {
                ui.snap(&file("card-running")).await;
            }
            if wanted("card-cancel") {
                ui.click(".browser-card", "Cancel task").await;
                ui.wait("cancel dialog", "Array.from(document.querySelectorAll('.ui-dialog-title')).some((t) => t.textContent === 'Cancel this task?')").await;
                ui.snap(&file("card-cancel")).await;
                ui.click(".stop-task-dialog", "Keep running").await;
                ui.wait(
                    "dialog closed",
                    "!document.querySelector('.stop-task-dialog')",
                )
                .await;
            }
            if wanted("card-minimized") {
                ui.click(".browser-card", "Minimize").await;
                ui.wait("slim pill", "!!document.querySelector('.browser-card[data-minimized] .browser-card-thumb img')?.naturalWidth").await;
                ui.snap(&file("card-minimized")).await;
                ui.click(".browser-card", "Restore").await;
            }
            if wanted("browser-sheet") || wanted("browser-tabs") || wanted("browser-zoom") {
                ui.eval("document.querySelector('.browser-card-screen').click(); true")
                    .await;
                ui.wait("sheet frame", "!!document.querySelector('.browser-sheet .browser-sheet-screen img')?.naturalWidth && document.querySelectorAll('.browser-sheet-tabs li').length === 2").await;
                if wanted("browser-sheet") {
                    ui.snap(&file("browser-sheet")).await;
                }
                if wanted("browser-tabs") {
                    ui.eval("document.querySelector('.browser-sheet-tabs').scrollIntoView({ block: 'end' }); true").await;
                    ui.snap(&file("browser-tabs")).await;
                }
                if wanted("browser-zoom") {
                    ui.click(".browser-sheet", "Zoom browser preview").await;
                    ui.wait("zoom", "!!document.querySelector('[role=dialog][aria-label=\"Live browser preview\"] img')?.naturalWidth").await;
                    ui.snap(&file("browser-zoom")).await;
                    ui.click(
                        "[role=dialog][aria-label=\"Live browser preview\"]",
                        "Close",
                    )
                    .await;
                    ui.wait("zoom closed", "!document.querySelector('[role=dialog][aria-label=\"Live browser preview\"]')").await;
                }
                ui.click(".browser-sheet", "Close").await;
                ui.wait("card back", "!!document.querySelector('.browser-card')")
                    .await;
            }
            ui.wait(
                "done chip",
                "document.querySelector('.browser-card')?.dataset.tone === 'done'",
            )
            .await;
            if wanted("card-done") {
                ui.snap(&file("card-done")).await;
            }
            ui.wait(
                "folded",
                "!!document.querySelector('.browser-card[data-minimized]')",
            )
            .await;
            if wanted("card-folded") {
                ui.snap(&file("card-folded")).await;
            }
        }
    }

    // Phase 2: each session asks the person for something and waits.
    login_script.push(tool("request_otp", json!({"tab_id":login_tab,"field_ref":code_ref,"channel":"sms","summary":"Code from the Acme sign-in text"}), "otp"));
    checkout_script.push(tool("request_human_step", json!({"kind":"takeover","summary":"Tick the robot check on the Acme checkout","tab_id":captcha_tab}), "captcha"));
    setup_script.push(tool(
        "register_authenticator",
        json!({"tab_id":setup_tab}),
        "register",
    ));
    restart_runtime(login_pid, &mut login_stream).await;
    restart_runtime(checkout_pid, &mut checkout_stream).await;
    restart_runtime(setup_pid, &mut setup_stream).await;
    harness.replace_script(&json!({"responses":login_script}));
    submit(&harness, &login, "l3", "Go ahead and sign in.").await;
    let otp_ticket = opened(&mut login_stream).await;
    harness.replace_script(&json!({"responses":checkout_script}));
    submit(&harness, &checkout, "c2", "Continue the checkout.").await;
    opened(&mut checkout_stream).await;
    harness.replace_script(&json!({"responses":setup_script}));
    submit(&harness, &setup, "s2", "Save the authenticator.").await;
    opened(&mut setup_stream).await;

    // Pass 2: the tickets. A takeover admits one remote-screen connection, so only the first
    // pass opens it.
    let mut takeover_used = false;
    for theme in ["light", "dark"] {
        ui.emulation.lock().await.scheme(theme).await;
        for viewport in &VIEWPORTS {
            if !pass_wanted(theme, viewport.name) {
                continue;
            }
            ui.emulation.lock().await.viewport(viewport).await;
            let file = |screen: &str| out.join(format!("{theme}-{}-{screen}.png", viewport.name));
            let phone = viewport.mobile;
            if wanted("otp-inline") {
                ui.goto(&format!("/sessions/{login}")).await;
                ui.wait("inline code card", "!!document.querySelector('main .oc-card[data-kind=otp]') && !!document.querySelector('.card-dock .oc-card')").await;
                ui.eval("const card = document.querySelector('main .oc-card[data-kind=otp]'); card.querySelector('.oc-pill')?.click(); card.scrollIntoView({ block: 'center' }); document.activeElement?.blur(); true").await;
                ui.wait("inline code form", "!!document.querySelector('main .oc-card[data-kind=otp] input[autocomplete=one-time-code]')").await;
                ui.snap(&file("otp-inline")).await;
            }
            if wanted("card-action") {
                ui.goto(&format!("/sessions/{login}")).await;
                ui.wait("action card", "document.querySelector('.browser-card')?.dataset.tone === 'warn' && !!document.querySelector('.browser-card img')?.naturalWidth").await;
                ui.eval("document.activeElement?.blur(); document.querySelector('.conversation-scroll').scrollTo(0, 0); true").await;
                ui.snap(&file("card-action")).await;
            }
            if wanted("otp-sheet") {
                ui.goto(&format!("/sessions/{login}?ticket={otp_ticket}"))
                    .await;
                ui.wait(
                    "code sheet",
                    "!!document.querySelector('.card-sheet input[autocomplete=one-time-code]')",
                )
                .await;
                ui.snap(&file("otp-sheet")).await;
            }
            if wanted("otp-dock") {
                ui.goto(&format!("/sessions/{browse}")).await;
                ui.wait("dock over the transcript", "!!document.querySelector('.card-dock .oc-card') && !!document.querySelector('.bubble.assistant')").await;
                ui.snap(&file("otp-dock")).await;
                if phone {
                    ui.eval("document.querySelector('.card-dock .oc-pill').click(); true")
                        .await;
                    ui.wait(
                        "open dock",
                        "!!document.querySelector('.card-dock .oc-card[data-open=true] input')",
                    )
                    .await;
                    ui.snap(&file("otp-dock-open")).await;
                }
            }
            if wanted("otp-dock-settings") {
                ui.goto(&format!("/sessions/{browse}")).await;
                if phone {
                    ui.click("body", "Open menu").await;
                    ui.wait("drawer", "!!document.querySelector('.ui-drawer')")
                        .await;
                    ui.click(".ui-drawer", "Settings").await;
                } else {
                    ui.click(".shell-sidebar", "Settings").await;
                }
                ui.wait("settings and dock", "Array.from(document.querySelectorAll('.ui-dialog-title')).some((t) => t.textContent === 'Settings') && !!document.querySelector('.card-dock .oc-card')").await;
                ui.snap(&file("otp-dock-settings")).await;
                // A press inside the dock is outside the modal Settings sheet; it must not close it.
                // Opening the collapsed card must preserve the surrounding Settings dialog.
                let kept = ui
                    .eval(
                        "(async () => { const t = document.querySelector('.card-dock .oc-pill'); \
                         const el = t || document.querySelector('.card-dock input'); \
                         const at = el.getBoundingClientRect(); \
                         const init = { bubbles: true, cancelable: true, button: 0, pointerId: 1, pointerType: 'mouse', isPrimary: true, clientX: at.left + 4, clientY: at.top + 4 }; \
                         el.dispatchEvent(new PointerEvent('pointerdown', init)); el.dispatchEvent(new MouseEvent('mousedown', init)); \
                         el.dispatchEvent(new PointerEvent('pointerup', init)); el.dispatchEvent(new MouseEvent('mouseup', init)); el.click(); \
                         await new Promise((r) => setTimeout(r, 400)); \
                         return Array.from(document.querySelectorAll('.ui-dialog-title')).some((t) => t.textContent === 'Settings') })()",
                    )
                    .await;
                eprintln!(
                    "{theme}-{}: Settings still open after a press in the dock: {kept}",
                    viewport.name
                );
                if phone {
                    ui.snap(&file("otp-dock-settings-open")).await;
                }
            }
            if wanted("takeover-card") || wanted("takeover") || wanted("takeover-pip") {
                ui.goto(&format!("/sessions/{checkout}")).await;
                let card = ".oc-card[data-kind=takeover][data-status=pending]";
                ui.wait("takeover card", &format!("!!document.querySelector({}) && !!document.querySelector('.card-dock .oc-card')", json!(card))).await;
                ui.eval(&format!(
                    "document.querySelector({}).querySelector('.oc-pill')?.click(); true",
                    json!(card)
                ))
                .await;
                ui.eval(&format!(
                    "document.querySelector({}).scrollIntoView({{ block: 'center' }}); true",
                    json!(card)
                ))
                .await;
                if wanted("takeover-card") {
                    ui.snap(&file("takeover-card")).await;
                }
                if (wanted("takeover") || wanted("takeover-pip")) && takeover_used {
                    eprintln!("{theme}-{}: skipping the remote screen; a takeover admits one connection (rerun with OPENSIDE_SCREENS_PASSES)", viewport.name);
                } else if wanted("takeover") || wanted("takeover-pip") {
                    takeover_used = true;
                    ui.click(card, "Open remote screen").await;
                    ui.wait(
                        "takeover frame",
                        "!!document.querySelector('.takeover-screen img')?.naturalWidth",
                    )
                    .await;
                    if wanted("takeover") {
                        ui.snap(&file("takeover")).await;
                    }
                    if wanted("takeover-pip") {
                        ui.click(".takeover", "Minimize").await;
                        ui.wait(
                            "takeover pip",
                            "!!document.querySelector('.pip img')?.naturalWidth",
                        )
                        .await;
                        ui.snap(&file("takeover-pip")).await;
                        ui.click(".pip", "Close").await;
                    }
                }
            }
            if wanted("register-card") {
                ui.goto(&format!("/sessions/{setup}")).await;
                let card = ".oc-card[data-kind=confirmation][data-status=pending]";
                ui.wait("register card", &format!("!!document.querySelector({}) && !!document.querySelector('.card-dock .oc-card')", json!(card))).await;
                ui.eval(&format!(
                    "document.querySelector({}).querySelector('.oc-pill')?.click(); true",
                    json!(card)
                ))
                .await;
                ui.eval(&format!(
                    "document.querySelector({}).scrollIntoView({{ block: 'center' }}); true",
                    json!(card)
                ))
                .await;
                ui.snap(&file("register-card")).await;
            }
        }
    }
    ui.browser.shutdown().await.unwrap();
    harness.kill().await;
    drop(site);
}
