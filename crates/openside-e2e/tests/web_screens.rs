//! Screenshot harness for the web UI (visual QA, not an acceptance suite; ignored by default).
//! Boots the hermetic daemon on a scripted model and seeds one conversation per state the
//! conversation surface has to design for: a long thread (tool activity, Markdown, tables,
//! code, SVG/HTML visuals, an image attachment, a subagent's pending approval and two queued
//! follow-ups), a short thread, a turn still streaming with a running subagent, an
//! interrupted run with a failed tool, every pending card kind (question with options, free
//! answer, secret, inline verification code, desktop step, device approval), an archived
//! conversation and the daemon going away. Each is captured in headless Chromium at 390x844
//! and 1440x900, light and dark.
//!
//! Run through `tools/qa/web-screens.sh [out-dir]`, which rebuilds `web/dist` first. Env:
//! - `OPENSIDE_SCREENS_DIR`: output directory (default `/tmp/openside-qa/redesign/core`).
//! - `OPENSIDE_SCREENS_ROUTES`: extra comma-separated app paths to capture, such as
//!   `/skills,/settings`; each is saved as `route-<path>`.
//! - `OPENSIDE_SCREENS_ONLY`: comma-separated screen names to limit a run to: new-chat,
//!   model-picker, conversation, long-thread-top, tool-activity, code-response, ticket, agents,
//!   visuals, files, short-thread, streaming, stop-confirm, working, failed, ask-options, ask-text, secret, otp,
//!   takeover, device, queue-menu, queue-edit, session-menu, drawer (phone only), archived,
//!   search, search-empty, settings, appearance, theme-preset (Solarized light / Nord dark
//!   palettes), offline (captured last: the daemon is stopped for it).
//! - `OPENSIDE_SCREENS_SKIP_LIVE=1`: skip seeding the otp and secret states, which drive a live
//!   agent tab (and so also the cross-conversation code dock on every other screen).
//!
//! Files are named `<theme>-<phone|desktop>-<screen>.png`.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{path::PathBuf, time::Duration};

use futures_util::{SinkExt, StreamExt};
use openside_browser::{Browser, BrowserConfig};
use openside_e2e::{Harness, SseReader};
use openside_proto::{Event, EventKind};
use serde_json::{json, Value};
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};

const OWNER: &str = "web-screens";
const DAEMON: &str = env!("CARGO_BIN_EXE_e2e-opensided");

fn text(text: &str) -> Value {
    json!({"parts":[{"type":"text","text":text}]})
}
fn tool(name: &str, args: Value, id: &str) -> Value {
    json!({"parts":[{"type":"tool_call","name":name,"args":args,"id":id}]})
}

const SUMMARY: &str = r#"## What this project does

**openside** runs a local agent daemon and serves this web app from it. The pieces that matter:

- `opensided` owns sessions, the event log and the approval queue.
- The Python sidecar runs the model loop and calls tools.
- The web app reads everything over `/v1` and the live event stream.

| Layer | Language | Entry point |
| --- | --- | --- |
| Daemon | Rust | `crates/opensided` |
| Sidecar | Python | `agent/openside_agent` |
| Web | TypeScript | `web/src/main.tsx` |

A session starts like this:

```rust
let session = client
    .create_session(CreateSession { title: "Release notes".into(), mode: Mode::Guard })
    .await?;
client.submit(&session.id, "Summarize this week's changes").await?;
```

Approvals stop the run until you answer them, so nothing outside the workspace changes silently."#;

const VISUALS: &str = r##"

```svg visual
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 320 120"><rect width="320" height="120" fill="#f4f6fb"/><rect x="20" y="70" width="40" height="40" fill="#2f6fed"/><rect x="80" y="40" width="40" height="70" fill="#2f6fed"/><rect x="140" y="20" width="40" height="90" fill="#2f6fed"/><text x="200" y="64" font-family="sans-serif" font-size="16" fill="#333">Weekly merges</text></svg>
```

```html visual
<!doctype html><html><body style="margin:0;font-family:sans-serif;background:#fff;color:#222;padding:16px"><h2 style="margin:0 0 8px">Release 0.4</h2><p style="margin:0">12 fixes, 3 features, 1 breaking change.</p></body></html>
```"##;

/// A `width`x`height` RGB gradient PNG (stored deflate blocks), so the attachment thumbnail
/// and zoom viewer have something visible to show.
fn gradient_png(width: u32, height: u32) -> Vec<u8> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }
    let mut raw = Vec::new();
    for y in 0..height {
        raw.push(0);
        for x in 0..width {
            raw.extend_from_slice(&[
                (40 + x * 180 / width) as u8,
                (110 + y * 100 / height) as u8,
                230,
            ]);
        }
    }
    let mut zlib = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (index, block) in blocks.iter().enumerate() {
        zlib.push(u8::from(index + 1 == blocks.len()));
        let len = u16::try_from(block.len()).unwrap();
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut png, b"IHDR", &header);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    png
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

// React tracks input values through the native setter; Base UI menus open on pointer press.
const HELPERS: &str = r#"(() => {
  window.__qs = {
    press(el) {
      if (!el) return false;
      const at = el.getBoundingClientRect();
      const init = { bubbles: true, cancelable: true, button: 0, pointerId: 1, pointerType: 'mouse', isPrimary: true,
        clientX: at.left + at.width / 2, clientY: at.top + at.height / 2 };
      el.dispatchEvent(new PointerEvent('pointerdown', init));
      el.dispatchEvent(new MouseEvent('mousedown', init));
      el.dispatchEvent(new PointerEvent('pointerup', init));
      el.dispatchEvent(new MouseEvent('mouseup', init));
      el.dispatchEvent(new MouseEvent('click', init));
      return true;
    },
    labelled(label) {
      return Array.from(document.querySelectorAll('button')).find((b) => b.getAttribute('aria-label') === label && b.offsetParent !== null);
    },
    text(scope, text) {
      return Array.from((scope || document).querySelectorAll('a, button, [role=menuitem]')).find((b) => b.textContent.trim() === text);
    },
  };
  return true;
})()"#;

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// A second DevTools session on the UI tab for viewport and colour-scheme emulation; the
/// overrides last as long as the socket.
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

    async fn viewport(&mut self, viewport: &Viewport) {
        // OPENSIDE_SCREENS_DESKTOP_SCALE (e.g. 1.25) reproduces fractional desktop scaling.
        let scale = match viewport.mobile {
            false => std::env::var("OPENSIDE_SCREENS_DESKTOP_SCALE")
                .ok()
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(viewport.scale),
            true => viewport.scale,
        };
        self.call(
            "Emulation.setDeviceMetricsOverride",
            json!({"width":viewport.width,"height":viewport.height,
                "deviceScaleFactor":scale,"mobile":viewport.mobile}),
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

    /// Captured on this session: the viewport override is per DevTools session, so a capture
    /// through the browser's own session would come out at the unemulated window size.
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
                panic!("timed out waiting for {what}; page text: {text}");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn goto(&self, path: &str) {
        let url = format!("{}{path}", self.origin);
        // A navigation can be aborted by an in-flight one from the app (net::ERR_ABORTED), and
        // Page.navigate sometimes times out although the page did load; the readiness wait below
        // decides in that case.
        for attempt in 1..=3 {
            let error = match self.browser.navigate(OWNER, &self.tab, &url).await {
                Ok(_) => break,
                Err(error) => error,
            };
            let page = self.browser.evaluate(&self.tab, "location.href").await;
            if matches!(&page, Ok(Value::String(href)) if *href == url) {
                eprintln!("navigate {path}: {error:?}; the page is there, continuing");
                break;
            }
            if attempt == 3 {
                panic!("navigate {path}: {error:?}; page then: {page:?}");
            }
            eprintln!("navigate {path}: {error:?}; retrying");
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        self.wait(
            "document",
            "document.readyState === 'complete' && !!document.querySelector('.shell')",
        )
        .await;
        self.eval(HELPERS).await;
    }

    /// Fonts loaded, transitions settled, then a capture written to `path`.
    async fn snap(&self, path: &std::path::Path) {
        self.eval("document.fonts.ready.then(() => true)").await;
        tokio::time::sleep(Duration::from_millis(450)).await;
        let png = self.emulation.lock().await.capture().await;
        std::fs::write(path, png).unwrap();
        eprintln!("captured {}", path.display());
    }
}

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

async fn submit(harness: &Harness, session: &str, id: &str, text: &str, mode: &str) {
    submit_with(harness, session, id, text, mode, json!([])).await;
}

async fn submit_with(
    harness: &Harness,
    session: &str,
    id: &str,
    text: &str,
    mode: &str,
    attachments: Value,
) {
    let response = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            Some(json!({"message_id":id,"text":text,"mode":mode,"attachments":attachments})),
        )
        .await;
    assert_eq!(response.status, 202, "{}", response.body);
}

async fn create(harness: &Harness, title: &str) -> String {
    create_in(harness, title, "guard").await
}

async fn create_in(harness: &Harness, title: &str, mode: &str) -> String {
    let response = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"title":title,"mode":mode,"workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(response.status, 201, "{}", response.body);
    response.body["id"].as_str().unwrap().to_owned()
}

/// Events up to and including the first one `done` accepts.
async fn until(stream: &mut SseReader, what: &str, done: impl Fn(&Event) -> bool) -> Vec<Event> {
    tokio::time::timeout(Duration::from_secs(60), async {
        let mut events = Vec::new();
        loop {
            let event = stream.next().await;
            let stop = done(&event);
            events.push(event);
            if stop {
                return events;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

/// The pending ticket of `kind` in `session`. Policy may ask before a step on a new origin,
/// so any other pending approval of that session is granted on the way.
async fn pending_ticket(harness: &Harness, session: &str, kind: &str) -> Value {
    let found = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let pending = harness
                .request("GET", "/v1/tickets?status=pending", None)
                .await;
            for ticket in pending.body["tickets"].as_array().into_iter().flatten() {
                if ticket["session_id"] != session {
                    continue;
                }
                if ticket["kind"] == kind {
                    return ticket.clone();
                }
                if ticket["kind"] == "confirm" {
                    let id = ticket["id"].as_str().unwrap();
                    harness
                        .human_request(
                            "POST",
                            &format!("/v1/tickets/{id}/resolve"),
                            Some(json!({"value":true})),
                        )
                        .await;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    match found {
        Ok(ticket) => ticket,
        Err(_) => {
            let mut stream = harness.events(session, 0).await;
            let mut seen = Vec::new();
            while let Ok(event) =
                tokio::time::timeout(Duration::from_millis(500), stream.next()).await
            {
                seen.push(format!("{:?}", event.kind));
            }
            panic!(
                "no pending {kind} ticket in {session}; events:\n{}",
                seen.join("\n")
            )
        }
    }
}

fn ended_ok<'a>(events: &'a [Event], call: &str) -> &'a Value {
    events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::ToolEnded {
                tool_call_id,
                result,
                is_error: false,
            } if tool_call_id == call => Some(result),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing successful {call}: {events:?}"))
}

/// The ref of the textbox labelled `name` in a compact accessibility snapshot.
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

const OTP_PAGE: &str = r#"<!doctype html><title>Check your email</title>
<form method="post" action="/login"><label>Email code <input name="code" aria-label="Email code" autocomplete="one-time-code"></label></form>"#;
const DEPLOY_PAGE: &str = r#"<!doctype html><title>Deploy settings</title>
<form><label>Deploy key <input name="key" type="password" aria-label="Deploy key"></label></form>"#;

/// Agent (sidecar) processes started so far, live or not.
fn agent_pids(harness: &Harness) -> std::collections::HashSet<i32> {
    std::fs::read_dir(harness.root().join("stderr"))
        .unwrap()
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

/// Kills the live agent processes not in `others`, so one conversation reloads its script
/// while other conversations stay blocked on their tickets.
fn restart_agent(harness: &Harness, others: &std::collections::HashSet<i32>) {
    for pid in agent_pids(harness).difference(others) {
        if std::path::Path::new(&format!("/proc/{pid}")).exists() {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
        }
    }
}

/// A local sign-in page that asks for an emailed code, for the inline code card.
async fn otp_site() -> (String, tokio::task::JoinHandle<()>) {
    use axum::{response::Html, routing::get, Router};
    let app = Router::new()
        .route("/login", get(|| async { Html(OTP_PAGE) }))
        .route("/deploy", get(|| async { Html(DEPLOY_PAGE) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let site = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (site, server)
}

/// Every conversation besides the long one, each left in the state its screen shows.
struct Seeded {
    short: String,
    streaming: String,
    working: String,
    failed: String,
    ask_options: String,
    ask_text: String,
    secret: String,
    otp: String,
    takeover: String,
    device: String,
}

/// A conversation on its own script, its first message submitted.
async fn seed(harness: &Harness, title: &str, mode: &str, script: Value, message: &str) -> String {
    harness.replace_script(&script);
    let session = create_in(harness, title, mode).await;
    submit(harness, &session, "s1", message, "now").await;
    session
}

async fn seed_states(harness: &mut Harness) -> (Seeded, tokio::task::JoinHandle<()>) {
    let wait_forever =
        json!({"parts":[{"type":"wait_steer","timeout_ms":600_000},{"type":"text","text":"…"}]});

    // An interrupted run first: kill_sidecar needs it to be the only live agent process.
    harness.replace_script(&json!({"responses":[
        tool("read", json!({"path":"logs/checkout-flake.txt"}), "read-log"),
        wait_forever.clone(),
    ]}));
    let failed = create(harness, "Flaky checkout test triage").await;
    let mut stream = harness.events(&failed, 0).await;
    submit(
        harness,
        &failed,
        "f1",
        "Why does the checkout test fail on CI?",
        "now",
    )
    .await;
    until(&mut stream, "the failed read", |event| {
        matches!(&event.kind, EventKind::ToolEnded { tool_call_id, .. } if tool_call_id == "read-log")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    harness.kill_sidecar();
    until(&mut stream, "the interruption", |event| {
        matches!(&event.kind, EventKind::RuntimeError { .. })
    })
    .await;

    let (site, server) = otp_site().await;
    // OPENSIDE_SCREENS_SKIP_LIVE=1 leaves out the two states that drive a live agent tab (the code
    // and secret cards), for runs where those hang the capture renderer.
    let (otp, secret) = if std::env::var_os("OPENSIDE_SCREENS_SKIP_LIVE").is_some() {
        (String::new(), String::new())
    } else {
        live_states(harness, &site).await
    };

    let ask_options = seed(
        harness,
        "Pick a release channel",
        "guard",
        json!({"responses":[
            tool("ask_user", json!({"question":"Which channel should 0.4 ship to first?","options":["Stable","Beta","Nightly"]}), "ask-channel"),
            text("Shipping it."),
        ]}),
        "Get 0.4 ready to ship.",
    )
    .await;
    pending_ticket(harness, &ask_options, "info").await;

    let ask_text = seed(
        harness,
        "Name the release",
        "guard",
        json!({"responses":[
            tool("ask_user", json!({"question":"Should the notes go out with the tag?","options":["Yes","No"]}), "ask-notes"),
            tool("ask_user", json!({"question":"What should this release be called?"}), "ask-name"),
            text("Named."),
        ]}),
        "Tag the release once the notes are in.",
    )
    .await;
    let first = pending_ticket(harness, &ask_text, "info").await;
    let id = first["id"].as_str().unwrap();
    harness
        .request(
            "POST",
            &format!("/v1/tickets/{id}/resolve"),
            Some(json!({"value":"Yes"})),
        )
        .await;
    tokio::time::timeout(Duration::from_secs(60), async {
        while pending_ticket(harness, &ask_text, "info").await["id"] == first["id"] {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("second question");

    let takeover = seed(
        harness,
        "Book the team offsite venue",
        "guard",
        json!({"responses":[
            tool("request_human_step", json!({"kind":"takeover","summary":"Solve the CAPTCHA on the booking page","instructions":"The venue's site wants a person to confirm the booking."}), "captcha"),
            text("Booked."),
        ]}),
        "Book the venue we picked for the offsite.",
    )
    .await;
    pending_ticket(harness, &takeover, "takeover").await;

    let device = seed(
        harness,
        "Connect the analytics account",
        "guard",
        json!({"responses":[
            tool("request_human_step", json!({"kind":"device_approval","summary":"Approve the sign-in prompt on your phone","instructions":"Google sent a prompt to your phone. Tap Yes there, then mark this done.","resume_when":{"manual":true}}), "device"),
            text("Connected."),
        ]}),
        "Connect the analytics account to the dashboard.",
    )
    .await;
    pending_ticket(harness, &device, "device_approval").await;

    let short = seed(
        harness,
        "Trip planning: Lisbon",
        "guard",
        json!({"responses":[text("Three that hold up in the rain:\n\n1. **Gulbenkian Museum** for an unhurried afternoon.\n2. **LX Factory** for bookshops and lunch under one roof.\n3. **Time Out Market** when you want to try a bit of everything.")]}),
        "Three things to do in Lisbon on a rainy afternoon?",
    )
    .await;

    let streaming = seed(
        harness,
        "Weekly dependency updates",
        "guard",
        json!({
            "responses":[{"parts":[
                {"type":"text","text":"Seven crates have updates this week. I'm handing the lockfile review to a helper so the breaking changes get a careful read, and I'll summarize once it reports back."},
                {"type":"tool_call","name":"delegate","args":{"task":"Review the Cargo.lock diff for breaking changes","tools":["read"]},"id":"review"},
            ]}],
            "subagents":{"review":[wait_forever]},
        }),
        "Check this week's dependency updates.",
    )
    .await;
    let mut stream = harness.events(&streaming, 0).await;
    until(&mut stream, "the running subagent", |event| {
        matches!(&event.kind, EventKind::SubagentStarted { .. })
    })
    .await;

    // A run between steps: its calls returned and the model has not answered yet (status line).
    let working = seed(
        harness,
        "Release notes draft",
        "guard",
        json!({"responses":[
            tool("read", json!({"path":"README.md"}), "read-notes"),
            wait_forever.clone(),
        ]}),
        "Draft release notes from the README.",
    )
    .await;
    let mut stream = harness.events(&working, 0).await;
    until(&mut stream, "the returned read", |event| {
        matches!(&event.kind, EventKind::ToolEnded { tool_call_id, .. } if tool_call_id == "read-notes")
    })
    .await;

    // Settled conversations the short one waits on; it has no ticket to poll for.
    let mut stream = harness.events(&short, 0).await;
    until(&mut stream, "the short answer", |event| {
        matches!(&event.kind, EventKind::SessionSettled {})
    })
    .await;

    // Titles that must truncate rather than widen the sidebar: an unbroken URL and Korean.
    create(
        harness,
        &format!("https://example.com/{}", "a1b2c3d4e5".repeat(18)),
    )
    .await;
    create(
        harness,
        "다음 주 팀 워크숍 장소 예약과 참석자 일정 조율, 그리고 발표 자료 초안까지 한꺼번에 정리해 달라는 요청",
    )
    .await;

    let archived = create(harness, "Old onboarding notes").await;
    let archive = harness
        .request(
            "PATCH",
            &format!("/v1/sessions/{archived}"),
            Some(json!({"archived":true})),
        )
        .await;
    assert!(archive.status < 300, "{}", archive.body);

    (
        Seeded {
            short,
            streaming,
            working,
            failed,
            ask_options,
            ask_text,
            secret,
            otp,
            takeover,
            device,
        },
        server,
    )
}

/// The code and secret cards need a real tab: open it, find the field, then ask. Each step
/// extends the script and restarts the agent so the session reads the new turns.
async fn live_states(harness: &mut Harness, site: &str) -> (String, String) {
    let mut responses = vec![
        tool(
            "browser_open_tab",
            json!({"url":format!("{site}/login")}),
            "open",
        ),
        text("The sign-in page is open."),
    ];
    harness.replace_script(&json!({ "responses": responses }));
    let otp = create_in(harness, "Sign in to the vendor portal", "full").await;
    let mut stream = harness.events(&otp, 0).await;
    submit(
        harness,
        &otp,
        "o1",
        "Sign in to the vendor portal and download last month's invoice.",
        "now",
    )
    .await;
    let events = until_final(&mut stream, "The sign-in page is open.").await;
    let tab = ended_ok(&events, "open")["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    responses.extend([
        tool("browser_snapshot", json!({"tab_id":tab}), "fields"),
        text("It wants the code from the sign-in email."),
    ]);
    harness.replace_script(&json!({ "responses": responses }));
    harness.kill_sidecar();
    stream.settled().await;
    submit(harness, &otp, "o2", "Go ahead.", "now").await;
    let events = until_final(&mut stream, "It wants the code from the sign-in email.").await;
    let code = field(ended_ok(&events, "fields"), "Email code");
    responses.extend([
        tool("request_otp", json!({"tab_id":tab,"field_ref":code,"channel":"email","summary":"Code from the sign-in email"}), "otp"),
        text("Signed in."),
    ]);
    harness.replace_script(&json!({ "responses": responses }));
    harness.kill_sidecar();
    stream.settled().await;
    submit(harness, &otp, "o3", "The email just arrived.", "now").await;
    pending_ticket(harness, &otp, "secret_input").await;

    // A secret goes into a field of a real tab too. Other conversations' agents are live now,
    // so only this conversation's agent is restarted between steps.
    let others = agent_pids(harness);
    let mut responses = vec![
        tool(
            "browser_open_tab",
            json!({"url":format!("{site}/deploy")}),
            "open-deploy",
        ),
        text("The deploy settings are open."),
    ];
    harness.replace_script(&json!({ "responses": responses }));
    let secret = create_in(harness, "Rotate the staging deploy key", "full").await;
    let mut stream = harness.events(&secret, 0).await;
    submit(
        harness,
        &secret,
        "k1",
        "Rotate the staging deploy key.",
        "now",
    )
    .await;
    let events = until_final(&mut stream, "The deploy settings are open.").await;
    let tab = ended_ok(&events, "open-deploy")["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    responses.extend([
        tool("browser_snapshot", json!({"tab_id":tab}), "deploy-fields"),
        text("Found the key field."),
    ]);
    harness.replace_script(&json!({ "responses": responses }));
    restart_agent(harness, &others);
    stream.settled().await;
    submit(harness, &secret, "k2", "Go on.", "now").await;
    let events = until_final(&mut stream, "Found the key field.").await;
    let key = field(ended_ok(&events, "deploy-fields"), "Deploy key");
    responses.extend([
        json!({"parts":[
            {"type":"text","text":"The new key goes in here; I'll wait for you."},
            {"type":"tool_call","name":"request_human_step","args":{"kind":"secret_input","summary":"Paste the new deploy key for staging","instructions":"It goes straight into the Deploy key field; openside never sees or stores it.","tab_id":tab,"origin":site,"field_ref":key},"id":"secret"},
        ]}),
        text("Saved."),
    ]);
    harness.replace_script(&json!({ "responses": responses }));
    restart_agent(harness, &others);
    stream.settled().await;
    submit(harness, &secret, "k3", "Fill it in.", "now").await;
    pending_ticket(harness, &secret, "secret_input").await;
    (otp, secret)
}

#[tokio::test]
#[ignore = "visual QA harness; run tools/qa/web-screens.sh"]
async fn web_screens_capture() {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist");
    assert!(
        dist.join("index.html").is_file(),
        "web/dist is missing; run pnpm -C web build (tools/qa/web-screens.sh does)"
    );
    // OPENSIDE_PATCHED_BROWSER captures with the user's patched Chromium build instead.
    let executable = match std::env::var_os("OPENSIDE_PATCHED_BROWSER") {
        Some(path) => PathBuf::from(path),
        None => openside_config::discover_browser(&openside_config::Config::default())
            .expect("headless Chromium"),
    };
    let out = PathBuf::from(
        std::env::var("OPENSIDE_SCREENS_DIR")
            .unwrap_or_else(|_| "/tmp/openside-qa/redesign/core".into()),
    );
    std::fs::create_dir_all(&out).unwrap();
    let only: Vec<String> = std::env::var("OPENSIDE_SCREENS_ONLY")
        .map(|v| v.split(',').map(|s| s.trim().to_owned()).collect())
        .unwrap_or_default();
    let wanted = |screen: &str| only.is_empty() || only.iter().any(|s| s == screen);
    let routes: Vec<String> = std::env::var("OPENSIDE_SCREENS_ROUTES")
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let mut harness =
        Harness::prepare(DAEMON, &json!({"responses":[]})).expect("hermetic daemon prerequisites");
    harness.append_config(&format!(
        "[ui]\ndir = {}\n",
        json!(dist.canonicalize().unwrap())
    ));
    let library = harness.root().join("skill-library");
    harness.append_config(&format!("[skills]\ndir = {}\n", json!(library)));
    // The inline code card needs an agent tab to point at.
    harness.append_config(&format!("[browser]\nexecutable = {}\n", json!(executable)));
    harness.launch().await;
    let onboarding = harness
        .request("PUT", "/v1/onboarding", Some(json!({"state":"skipped"})))
        .await;
    assert_eq!(onboarding.status, 200, "{}", onboarding.body);
    let workspace = harness.workspace();
    std::fs::create_dir_all(workspace.join("src")).unwrap();
    std::fs::write(
        workspace.join("README.md"),
        "# openside\nLocal agent daemon with a web app.\n",
    )
    .unwrap();
    std::fs::write(
        workspace.join("src/lib.rs"),
        "pub fn version() -> &'static str { \"0.1\" }\n",
    )
    .unwrap();
    let (seeded, site) = seed_states(&mut harness).await;
    let report = harness.root().join("release-report.md");
    harness.replace_script(&json!({
        "responses": [
            tool("read", json!({"path":"README.md"}), "read-readme"),
            tool("read", json!({"path":"src/lib.rs"}), "read-lib"),
            text(&format!("{SUMMARY}{VISUALS}")),
            tool("delegate", json!({"task":"Draft the release report outside the workspace","tools":["write"]}), "delegate-call"),
            text("The report is ready for review."),
        ],
        "subagents": {
            "delegate-call": [
                tool("write", json!({"path":report,"content":"# Release report\n"}), "child-write"),
                text("child wrote the report"),
            ]
        }
    }));

    // One more conversation fills the sidebar, pinned.
    let pinned = create(&harness, "Release checklist").await;
    let pin = harness
        .request(
            "PATCH",
            &format!("/v1/sessions/{pinned}"),
            Some(json!({"pinned":true})),
        )
        .await;
    assert!(pin.status < 300, "{}", pin.body);

    let session = create(&harness, "Project summary").await;
    let mut stream = harness.events(&session, 0).await;
    let image = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(gradient_png(320, 200))
    };
    submit_with(
        &harness,
        &session,
        "m1",
        "Summarize this project for a new contributor.",
        "now",
        json!([{"name":"architecture.png","data_base64":image}]),
    )
    .await;
    until_final(&mut stream, &format!("{SUMMARY}{VISUALS}")).await;
    submit(
        &harness,
        &session,
        "m2",
        "Delegate the release report and save it next to the repo.",
        "now",
    )
    .await;
    // The child's write waits on an approval; follow-ups queue behind the blocked run.
    pending_ticket(&harness, &session, "confirm").await;
    submit(
        &harness,
        &session,
        "q1",
        "Also list the breaking changes at the top.",
        "queue",
    )
    .await;
    submit(&harness, &session, "q2", "Keep it under one page.", "queue").await;

    let token = {
        let paired = harness
            .request(
                "POST",
                "/v1/devices/pair",
                // `admin` so the Workspace sections (devices, vault, configuration) render data, not a scope error.
                Some(json!({"name":"screens","scopes":["read","steer","approve","admin"]})),
            )
            .await;
        assert_eq!(paired.status, 201, "{}", paired.body);
        paired.body["token"].as_str().unwrap().to_owned()
    };
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
    let ui = Ui {
        browser,
        tab,
        origin,
        emulation: tokio::sync::Mutex::new(emulation),
    };
    ui.wait("token screen", "!!document.querySelector('.token-entry')")
        .await;
    ui.eval(&format!(
        "localStorage.setItem('openside.token', {}); true",
        json!(token)
    ))
    .await;
    let conversation = format!("/sessions/{session}");
    let open_conversation = || async {
        ui.goto(&conversation).await;
        ui.wait(
            "seeded conversation",
            "document.querySelectorAll('.queue-item').length === 2 && !!document.querySelector('section.ticket-pending') \
             && !!document.querySelector('.subagents') && !!document.querySelector('.code-block')",
        )
        .await;
    };

    for theme in ["light", "dark"] {
        ui.emulation.lock().await.scheme(theme).await;
        for viewport in &VIEWPORTS {
            ui.emulation.lock().await.viewport(viewport).await;
            let file = |screen: &str| out.join(format!("{theme}-{}-{screen}.png", viewport.name));
            let phone = viewport.mobile;

            if wanted("new-chat") || wanted("model-picker") {
                ui.goto("/").await;
                ui.wait("new chat", "!!document.querySelector('.composer')")
                    .await;
                if wanted("new-chat") {
                    ui.snap(&file("new-chat")).await;
                }
                if wanted("model-picker") {
                    ui.eval("document.querySelector('.model-picker .tool-pill').click()")
                        .await;
                    ui.wait("model dialog", "!!document.querySelector('.ui-dialog')")
                        .await;
                    ui.snap(&file("model-picker")).await;
                }
            }
            // Composer at rest, one line, three lines and past its height cap.
            for (name, text) in [
                ("composer-empty", String::new()),
                ("composer-1-line", "Summarize the release notes".to_owned()),
                (
                    "composer-3-lines",
                    "First line\nSecond line\nThird line".to_owned(),
                ),
                (
                    "composer-overflow",
                    (1..=14)
                        .map(|n| format!("Line {n}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
            ] {
                if !wanted(name) {
                    continue;
                }
                ui.goto("/").await;
                ui.wait("composer", "!!document.querySelector('.composer textarea')")
                    .await;
                ui.eval(&format!(
                    "(() => {{ const el = document.querySelector('.composer textarea'); \
                       Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(el, {}); \
                       el.dispatchEvent(new Event('input', {{ bubbles: true }})); el.focus(); return true }})()",
                    json!(text)
                ))
                .await;
                tokio::time::sleep(Duration::from_millis(150)).await;
                let metrics = ui
                    .eval("(() => { const el = document.querySelector('.composer textarea'); const s = getComputedStyle(el); return [el.clientHeight, el.scrollHeight, s.overflowY, s.resize] })()")
                    .await;
                eprintln!("{name}: [clientHeight, scrollHeight, overflowY, resize] = {metrics}");
                ui.snap(&file(name)).await;
            }

            open_conversation().await;
            if wanted("conversation") {
                ui.snap(&file("conversation")).await;
            }
            for (screen, selector, block) in [
                ("tool-activity", ".activity", "start"),
                ("code-response", ".code-block", "center"),
                ("ticket", "section.ticket-pending", "center"),
                ("agents", ".subagents", "center"),
                ("visuals", ".visuals figure", "center"),
                ("files", ".artifacts", "center"),
            ] {
                if !wanted(screen) {
                    continue;
                }
                let found = ui
                    .eval(&format!(
                        "(() => {{ const el = document.querySelector({}); if (!el) return false; if (el.tagName === 'DETAILS') el.open = true; \
                         el.scrollIntoView({{ block: {} }}); return true }})()",
                        json!(selector),
                        json!(block)
                    ))
                    .await;
                if found == true {
                    ui.snap(&file(screen)).await;
                } else {
                    eprintln!("skipped {screen}: no {selector} on the page");
                }
            }
            if wanted("long-thread-top") {
                open_conversation().await;
                ui.eval("document.querySelector('.conversation-scroll').scrollTop = 0; true")
                    .await;
                ui.snap(&file("long-thread-top")).await;
            }
            // One conversation per state, captured at its latest turn.
            for (screen, session, ready) in [
                ("short-thread", &seeded.short, "document.querySelectorAll('.bubble.assistant[data-final=\"true\"]').length === 1"),
                ("streaming", &seeded.streaming, "document.body.innerText.includes('Seven crates') && !!document.querySelector('.subagent')"),
                ("working", &seeded.working, "!!document.querySelector('.status-line')"),
                ("failed", &seeded.failed, "!!document.querySelector('.runtime-error')"),
                ("ask-options", &seeded.ask_options, "!!document.querySelector('section.ticket-pending')"),
                ("ask-text", &seeded.ask_text, "!!document.querySelector('section.ticket-pending input')"),
                ("secret", &seeded.secret, "!!document.querySelector('section.ticket-pending input[type=password]')"),
                ("otp", &seeded.otp, "!!document.querySelector('section.ticket-otp')"),
                ("takeover", &seeded.takeover, "!!document.querySelector('section.ticket-pending')"),
                ("device", &seeded.device, "!!document.querySelector('section.ticket-pending')"),
            ] {
                if !wanted(screen) || session.is_empty() {
                    continue;
                }
                ui.goto(&format!("/sessions/{session}")).await;
                ui.wait(screen, ready).await;
                if matches!(screen, "failed" | "streaming") {
                    ui.eval("document.querySelectorAll('.activity:not(.status-line-group)').forEach((el) => { el.open = true }); true")
                        .await;
                }
                ui.snap(&file(screen)).await;
                if screen == "streaming" && wanted("stop-confirm") {
                    ui.eval("document.querySelector('.composer-stop').click(); true").await;
                    ui.wait("stop confirmation", "!!document.querySelector('.stop-task-dialog')")
                        .await;
                    ui.snap(&file("stop-confirm")).await;
                    ui.eval("__qs.text(document.querySelector('.stop-task-dialog'), 'Keep running').click()")
                        .await;
                }
            }
            if wanted("queue-menu") || wanted("queue-edit") {
                open_conversation().await;
                ui.eval("__qs.press(document.querySelector('.queue-item [aria-label=\"Message options\"]'))")
                    .await;
                ui.wait("queue menu", "!!document.querySelector('.ui-menu')")
                    .await;
                if wanted("queue-menu") {
                    ui.snap(&file("queue-menu")).await;
                }
                if wanted("queue-edit") {
                    ui.eval(
                        "__qs.text(document.querySelector('.ui-menu'), 'Edit message').click()",
                    )
                    .await;
                    ui.wait(
                        "queue editor",
                        "!!document.querySelector('.queue-edit textarea')",
                    )
                    .await;
                    ui.snap(&file("queue-edit")).await;
                }
            }
            if wanted("session-menu") {
                open_conversation().await;
                ui.eval("__qs.press(__qs.labelled('Session options'))")
                    .await;
                ui.wait("session menu", "!!document.querySelector('.ui-menu')")
                    .await;
                ui.snap(&file("session-menu")).await;
            }
            let open_drawer = || async {
                ui.eval("__qs.labelled('Open conversations').click()").await;
                ui.wait("drawer", "!!document.querySelector('.ui-drawer')")
                    .await;
            };
            if phone && wanted("drawer") {
                open_conversation().await;
                open_drawer().await;
                ui.snap(&file("drawer")).await;
            }
            // The sidebar's archived filter and search, results and none.
            for (screen, query) in [
                ("archived", None),
                ("search", Some("release")),
                ("search-empty", Some("quarterly tax filing")),
            ] {
                if !wanted(screen) {
                    continue;
                }
                open_conversation().await;
                let scope = if phone {
                    open_drawer().await;
                    ".ui-drawer"
                } else {
                    ".shell-sidebar"
                };
                match query {
                    None => {
                        ui.eval(&format!(
                            "(() => {{ const scope = document.querySelector({}); \
                               (scope.querySelector('[aria-label=\"Show archived\"]') || __qs.text(scope, 'Archived')).click(); return true }})()",
                            json!(scope)
                        ))
                        .await;
                        ui.wait(
                            "archived list",
                            "document.body.innerText.includes('Old onboarding notes')",
                        )
                        .await;
                    }
                    Some(query) => {
                        ui.eval(&format!(
                            "(() => {{ const el = document.querySelector({}).querySelector('input[type=search]'); \
                               Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(el, {}); \
                               el.dispatchEvent(new Event('input', {{ bubbles: true }})); return true }})()",
                            json!(scope),
                            json!(query)
                        ))
                        .await;
                        tokio::time::sleep(Duration::from_millis(600)).await;
                    }
                }
                ui.snap(&file(screen)).await;
            }
            if wanted("settings") || wanted("appearance") {
                open_conversation().await;
                let scope = if phone {
                    open_drawer().await;
                    ".ui-drawer"
                } else {
                    ".shell-sidebar"
                };
                ui.eval(&format!(
                    "__qs.text(document.querySelector({}), 'Settings').click()",
                    json!(scope)
                ))
                .await;
                ui.wait(
                    "settings dialog",
                    "Array.from(document.querySelectorAll('.ui-dialog-title')).some((t) => t.textContent === 'Settings')",
                )
                .await;
                if wanted("settings") {
                    ui.snap(&file("settings")).await;
                }
                let found = ui
                    .eval("(() => { const el = document.querySelector('.ui-dialog .appearance'); if (!el) return false; el.scrollIntoView({ block: 'start' }); return true })()")
                    .await;
                if wanted("appearance") && found == true {
                    ui.snap(&file("appearance")).await;
                } else if wanted("appearance") {
                    eprintln!("skipped appearance: the Settings dialog has no .appearance");
                }
            }
            // A preset palette per scheme, applied the way the Appearance settings store it.
            if wanted("theme-preset") {
                ui.eval(
                    "localStorage.setItem('openside.palette', JSON.stringify({ \
                       light: { id: 'solarized-light', background: '#fdf6e3', foreground: '#657b83', accent: '#b58900' }, \
                       dark: { id: 'nord-dark', background: '#2e3440', foreground: '#d8dee9', accent: '#88c0d0' } })); true",
                )
                .await;
                open_conversation().await;
                ui.snap(&file("theme-preset")).await;
                ui.eval("localStorage.removeItem('openside.palette'); true")
                    .await;
            }
            for route in &routes {
                ui.goto(route).await;
                let name = route.trim_matches('/').replace('/', "-");
                let name = if name.is_empty() { "root".into() } else { name };
                ui.snap(&file(&format!("route-{name}"))).await;
                if route == "/devices" {
                    ui.eval("Array.from(document.querySelectorAll('[role=radio]')).find(button => button.textContent === 'Connect an agent / CLI').click(); true").await;
                    ui.snap(&file("route-devices-agent")).await;
                }
            }
        }
    }
    // Last: the daemon goes away under an open conversation (nothing can be navigated after).
    if wanted("offline") {
        ui.goto(&conversation).await;
        ui.wait(
            "seeded conversation",
            "!!document.querySelector('section.ticket-pending')",
        )
        .await;
        harness.crash_daemon().await;
        ui.wait(
            "reconnecting",
            "!!document.querySelector('.connection-banner') || document.body.innerText.includes('Reconnecting')",
        )
        .await;
        for theme in ["light", "dark"] {
            ui.emulation.lock().await.scheme(theme).await;
            for viewport in &VIEWPORTS {
                ui.emulation.lock().await.viewport(viewport).await;
                ui.snap(&out.join(format!("{theme}-{}-offline.png", viewport.name)))
                    .await;
            }
        }
    }
    ui.browser.shutdown().await.unwrap();
    harness.kill().await;
    site.abort();
}
