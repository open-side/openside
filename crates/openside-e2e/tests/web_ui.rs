//! Web UI acceptance for the session screen: the built PWA served by the real daemon,
//! driven in headless Chromium against the real Python sidecar on a scripted model.
//! Covers approval cards resuming the same tool call (desktop and a 390x844 phone
//! viewport), the subagent view, secret_input through the UI, the takeover remote screen
//! and rebuilding the screen from the event log after a reload.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{path::PathBuf, time::Duration};

use axum::{response::Html, routing::get, Router};
use futures_util::{SinkExt, StreamExt};
use openside_browser::{Browser, BrowserConfig};
use openside_e2e::{Harness, SseReader};
use openside_proto::{Event, EventKind};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

const OWNER: &str = "web-ui-e2e";
const DAEMON: &str = env!("CARGO_BIN_EXE_e2e-opensided");

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

fn text(text: &str) -> Value {
    json!({"parts":[{"type":"text","text":text}]})
}
fn tool(name: &str, args: Value, id: &str) -> Value {
    json!({"parts":[{"type":"tool_call","name":name,"args":args,"id":id}]})
}

// React tracks input values through the native setter; assigning `.value` directly would be
// ignored, so test input goes through the prototype setter.
const HELPERS: &str = r#"(() => {
  window.__violations = [];
  document.addEventListener('securitypolicyviolation', (e) => window.__violations.push({directive:e.violatedDirective,blocked:e.blockedURI,source:e.sourceFile,line:e.lineNumber}));
  window.__os = {
    fill(el, value) {
      const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype
        : el instanceof HTMLSelectElement ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, value);
      el.dispatchEvent(new Event(el instanceof HTMLSelectElement ? 'change' : 'input', { bubbles: true }));
    },
    button(scope, text) {
      return scope && Array.from(scope.querySelectorAll('button')).find((b) => b.textContent.trim() === text);
    },
    card(selector) {
      const card = document.querySelector(selector);
      card?.querySelector('.oc-pill')?.click();
      return card;
    },
    send(text) {
      const f = document.querySelector('.composer');
      __os.fill(f.querySelector('textarea'), text);
      __os.submit(f).click();
      return true;
    },
    // The composer's submit button reads Send when idle and Queue during a run.
    submit(form) {
      return form.querySelector('button[type=submit]:not(:disabled)');
    },
    finalText(text) {
      return Array.from(document.querySelectorAll('.bubble.assistant[data-final=true]')).some((b) => b.textContent === text)
        && document.querySelector('main.session')?.dataset.status === 'idle';
    },
  };
  return true;
})()"#;

struct Ui {
    browser: Browser,
    tab: String,
    origin: String,
}

impl Ui {
    /// Open the daemon-served UI with `token` already saved, on `path`.
    async fn open(executable: PathBuf, harness: &Harness, token: &str, path: &str) -> Self {
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
        let ui = Self {
            browser,
            tab,
            origin,
        };
        ui.wait("token screen", "!!document.querySelector('.token-entry')")
            .await;
        ui.eval(&format!(
            "localStorage.setItem('openside.token', {}); true",
            json!(token)
        ))
        .await;
        ui.goto(path).await;
        ui
    }

    async fn goto(&self, path: &str) {
        self.browser
            .navigate(OWNER, &self.tab, &format!("{}{path}", self.origin))
            .await
            .unwrap();
        self.wait(
            "document",
            "document.readyState === 'complete' && !!document.querySelector('.shell')",
        )
        .await;
        self.eval(HELPERS).await;
    }

    async fn eval(&self, expression: &str) -> Value {
        self.browser
            .evaluate(&self.tab, expression)
            .await
            .unwrap_or_else(|e| panic!("evaluate {expression}: {e:?}"))
    }

    /// Poll a page predicate; on timeout, fail with the visible page text.
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

    async fn close(self) {
        assert_eq!(
            self.eval("window.__violations || []").await,
            json!([]),
            "CSP violations in the UI"
        );
        self.browser.shutdown().await.unwrap();
    }

    /// RGB of the screenshot pixel at (x, y) CSS px inside the `index`th visual frame. The
    /// frame is scrolled into view, the tab captured over DevTools, and the PNG decoded by
    /// the page itself (no access to the sandboxed document is needed or possible).
    async fn frame_pixel(&self, index: usize, x: i64, y: i64) -> Vec<i64> {
        use base64::Engine;
        let rect = self
            .eval(&format!(
                "(() => {{ const f = document.querySelectorAll('.visuals iframe')[{index}]; f.scrollIntoView({{ block: 'center' }}); \
                 const r = f.getBoundingClientRect(); return [r.left, r.top, r.width, r.height] }})()"
            ))
            .await;
        let (left, top) = (rect[0].as_f64().unwrap(), rect[1].as_f64().unwrap());
        assert!(
            (x as f64) < rect[2].as_f64().unwrap() && (y as f64) < rect[3].as_f64().unwrap(),
            "point outside frame {rect}"
        );
        // Let the scroll paint before capturing.
        self.eval(
            "new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(() => r(true))))",
        )
        .await;
        let png = self.browser.screenshot(&self.tab).await.unwrap();
        let data = base64::engine::general_purpose::STANDARD.encode(png);
        let pixel = self
            .eval(&format!(
                "(async () => {{ const img = new Image(); img.src = 'data:image/png;base64,{data}'; await img.decode(); \
                 const c = document.createElement('canvas'); c.width = img.width; c.height = img.height; \
                 const g = c.getContext('2d'); g.drawImage(img, 0, 0); const s = img.width / innerWidth; \
                 return Array.from(g.getImageData(Math.round(({left} + {x}) * s), Math.round(({top} + {y}) * s), 1, 1).data).slice(0, 3) }})()"
            ))
            .await;
        pixel
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_i64().unwrap())
            .collect()
    }
}

async fn pair(harness: &Harness, scopes: &[&str]) -> String {
    let paired = harness
        .request(
            "POST",
            "/v1/devices/pair",
            Some(json!({"name":"web ui e2e","scopes":scopes})),
        )
        .await;
    assert_eq!(paired.status, 201, "{}", paired.body);
    paired.body["token"].as_str().unwrap().to_owned()
}

async fn create(harness: &Harness, mode: &str) -> String {
    let response = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"title":"Web UI","mode":mode,"workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(response.status, 201, "{}", response.body);
    response.body["id"].as_str().unwrap().to_owned()
}

async fn prepared(
    script: &Value,
    browser_for_agent: Option<&PathBuf>,
) -> Option<(Harness, PathBuf)> {
    let dist = bundle()?;
    let executable = chromium()?;
    let mut harness = Harness::prepare(DAEMON, script)?;
    harness.append_config(&format!("[ui]\ndir = {}\n", json!(dist)));
    // Keep skill discovery inside the test root.
    let library = harness.root().join("skill-library");
    harness.append_config(&format!("[skills]\ndir = {}\n", json!(library)));
    if let Some(agent_browser) = browser_for_agent {
        harness.append_config(&format!(
            "[browser]\nexecutable = {}\n",
            json!(agent_browser)
        ));
    }
    harness.launch().await;
    Some((harness, executable))
}

/// Tool-return parts Pydantic AI committed for `call`. The history snapshot is written as the
/// run settles, which can trail the final answer on screen, so wait for it to show the call.
async fn tool_returns(harness: &Harness, session: &str, call: &str) -> Vec<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let returns: Vec<Value> = harness.snapshot(session)["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|message| message["parts"].as_array().into_iter().flatten())
            .filter(|part| part["part_kind"] == "tool-return" && part["tool_call_id"] == call)
            .cloned()
            .collect();
        if !returns.is_empty() || tokio::time::Instant::now() >= deadline {
            return returns;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
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

/// The pending approval card's Approve button, once the card is on screen.
const APPROVE: &str =
    "__os.button(__os.card('.oc-card[data-kind=\"confirmation\"][data-status=\"pending\"]'), 'Approve')";

const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100%" height="100%" viewBox="0 0 40 20" preserveAspectRatio="none"><rect width="40" height="20" fill="#2f6fed"/></svg>"##;
const HTML: &str = r##"<!doctype html><html><body style="margin:0;height:100vh;background:#d23c3c"><h1 style="margin:0;color:#fff">Quarterly summary</h1></body></html>"##;
const CHART: &str = r##"<!doctype html><html><body style="margin:0;background:#fff">
<div style="position:absolute;left:20px;top:100px;width:40px;height:50px;background:#1f9d55"></div>
<div style="position:absolute;left:80px;top:30px;width:40px;height:120px;background:#1f9d55"></div>
</body></html>"##;
const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

#[tokio::test]
async fn web_ui_subagent_view_and_approval_resume_same_call_and_reload_rebuilds_screen() {
    let Some((mut harness, executable)) = prepared(&json!({"responses":[]}), None).await else {
        return;
    };
    let outside = harness.root().join("outside.txt");
    harness.replace_script(&json!({
        "responses": [
            tool("delegate", json!({"task":"Write the report outside the workspace","tools":["write"]}), "delegate-call"),
            text(&format!("parent done\n```svg visual\n{SVG}\n```\n```html visual\n{HTML}\n```\n```html visual\n{CHART}\n```")),
        ],
        "subagents": {
            "delegate-call": [
                tool("write", json!({"path":outside,"content":"from child"}), "child-write"),
                text("child wrote the report"),
            ]
        }
    }));
    let session = create(&harness, "guard").await;
    let token = pair(&harness, &["read", "steer", "approve"]).await;
    let ui = Ui::open(
        executable,
        &harness,
        &token,
        &format!("/sessions/{session}"),
    )
    .await;
    ui.wait("session screen", "!!document.querySelector('.composer')")
        .await;
    // PP-14: attach an image through the composer's file input.
    ui.eval(&format!(
        "(() => {{ const bytes = Uint8Array.from(atob({}), (c) => c.charCodeAt(0)); const dt = new DataTransfer(); \
         dt.items.add(new File([bytes], 'dot.png', {{ type: 'image/png' }})); \
         const input = document.querySelector('.composer input[type=file]'); input.files = dt.files; \
         input.dispatchEvent(new Event('change', {{ bubbles: true }})); return true }})()",
        json!(PNG_1X1)
    ))
    .await;
    ui.wait(
        "attachment chip",
        "!!document.querySelector('.composer .attachment-chip img')",
    )
    .await;
    // A-6: `/` in the composer offers the daemon's skills; the chosen one becomes a chip and
    // its mention loads the skill before the model runs.
    let created = harness
        .request(
            "POST",
            "/v1/skills",
            Some(json!({"name":"report-style","description":"House style for reports","body":"Use short sections."})),
        )
        .await;
    assert_eq!(created.status, 201, "{}", created.body);
    ui.eval(
        "__os.fill(document.querySelector('.composer textarea'), 'Delegate the report /rep'); true",
    )
    .await;
    ui.wait(
        "skill picker",
        "Array.from(document.querySelectorAll('#skill-mentions [role=option]'), (o) => o.querySelector('strong').textContent).join() === '/report-style'",
    )
    .await;
    ui.eval("document.querySelector('#skill-mentions [role=option]').dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true })); true")
        .await;
    ui.wait(
        "skill chip",
        "document.querySelector('.composer textarea').value === 'Delegate the report /report-style ' \
         && Array.from(document.querySelectorAll('.composer .skill-chip'), (c) => c.textContent.replace('×', '')).join() === '/report-style'",
    )
    .await;
    ui.eval("(() => { const f = document.querySelector('.composer'); __os.button(f, 'Send').click(); return true })()")
        .await;
    ui.wait(
        "attached image in the transcript",
        "(() => { const i = document.querySelector('.bubble.user img.attachment-image'); return !!i && i.complete && i.naturalWidth === 1 })()",
    )
    .await;

    // The child shows up in the Agents sheet (opened from the composer pill), waiting on the
    // approval it raised.
    ui.wait(
        "subagent waiting on approval",
        &format!("!!document.querySelector('.agents-pill') && !!{APPROVE}"),
    )
    .await;
    ui.eval("document.querySelector('.agents-pill').click(); true")
        .await;
    ui.wait(
        "agents sheet with the waiting child",
        "!!document.querySelector('.subagent-list li.subagent-waiting_human')",
    )
    .await;
    let child = ui
        .eval("document.querySelector('.subagent-list li[data-subagent]').dataset.subagent")
        .await;
    assert_eq!(
        ui.eval("document.querySelector('.subagent-list .subagent-task').textContent")
            .await,
        "Write the report outside the workspace"
    );
    // The approval names the child that raised it: its id in data-subagent-id, its task as text.
    assert_eq!(
        ui.eval("(() => { const chip = document.querySelector('.oc-card[data-ticket][data-status=pending] .oc-asker'); return [chip.dataset.subagentId, chip.textContent] })()")
            .await,
        json!([child.as_str().unwrap(), "Write the report outside the workspace asked"])
    );
    // Live progress: the child's event stream is listed under it.
    ui.eval("document.querySelector('.subagent-list details').open = true; true")
        .await;
    ui.wait(
        "subagent progress events",
        "document.querySelectorAll('.subagent-list .subagent-events li').length > 0",
    )
    .await;
    assert!(!outside.exists(), "the write must wait for approval");
    ui.eval("document.querySelector('.tray-sheet button[aria-label=\"Close\"]').click(); true")
        .await;
    ui.wait(
        "agents sheet closed",
        "!document.querySelector('.tray-sheet')",
    )
    .await;

    ui.eval(&format!("{APPROVE}.click()")).await;
    ui.wait(
        "parent settles after the child",
        "__os.finalText('parent done')",
    )
    .await;
    let bubbles = ui
        .eval(
            "Array.from(document.querySelectorAll('.bubble.assistant')).map((b) => b.textContent)",
        )
        .await;
    // turn.ended arrives before assistant.final; the answer still renders once.
    assert_eq!(bubbles, json!(["parent done"]));
    let mut log = harness.events(&session, 0).await;
    let first = log.settled().await;
    let queued = first.iter().find_map(|e| match &e.kind {
        EventKind::MessageQueued { text, .. } => Some(text.clone()),
        _ => None,
    });
    assert_eq!(queued.as_deref(), Some("Delegate the report /report-style"));
    let skill = first.iter().find_map(|e| match &e.kind {
        EventKind::ToolStarted { name, args, .. } if name == "skill" => Some(args.clone()),
        _ => None,
    });
    assert_eq!(
        skill,
        Some(json!({"name":"report-style"})),
        "the mention loaded the skill"
    );
    // A-15: each visual renders in a script-free sandboxed iframe from its signed preview
    // URL. Rendering is checked from outside the frame, on screenshot pixels: a full-bleed
    // SVG, an HTML page and a static HTML bar chart whose bars have different heights.
    ui.wait(
        "visual frames",
        "document.querySelectorAll('.visuals iframe.sandbox-frame').length === 3",
    )
    .await;
    assert_eq!(
        ui.eval("Array.from(document.querySelectorAll('.visuals iframe'), (f) => f.getAttribute('sandbox'))")
            .await,
        json!(["", "", ""])
    );
    const BLUE: [i64; 3] = [0x2f, 0x6f, 0xed];
    const RED: [i64; 3] = [0xd2, 0x3c, 0x3c];
    const GREEN: [i64; 3] = [0x1f, 0x9d, 0x55];
    const WHITE: [i64; 3] = [0xff, 0xff, 0xff];
    for (frame, x, y, color) in [
        (0, 20, 20, BLUE),
        (0, 300, 200, BLUE),
        (1, 300, 300, RED),
        (2, 40, 140, GREEN),
        (2, 40, 60, WHITE),
        (2, 100, 60, GREEN),
        (2, 100, 140, GREEN),
        (2, 160, 140, WHITE),
    ] {
        // Frames load independently; a sample may precede its frame's first paint.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let got = ui.frame_pixel(frame, x, y).await;
            if got.iter().zip(color).all(|(a, b)| (a - b).abs() <= 8) {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "visual {frame} at ({x},{y}): {got:?}, expected {color:?}"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    ui.eval("__os.button(document.querySelector('.visuals figure'), 'Source').click()")
        .await;
    ui.wait(
        "visual source",
        &format!(
            "document.querySelector('.visual-source')?.textContent === {}",
            json!(SVG)
        ),
    )
    .await;
    ui.eval("__os.button(document.querySelector('.visuals figure'), 'Preview').click()")
        .await;
    ui.eval("document.querySelector('.agents-pill').click(); true")
        .await;
    ui.wait(
        "child completed in the subagent view",
        "!!document.querySelector('.subagent-list li.subagent-completed')",
    )
    .await;
    ui.eval("document.querySelector('.tray-sheet button[aria-label=\"Close\"]').click(); true")
        .await;
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "from child");
    let tickets = harness
        .request("GET", "/v1/tickets?status=resolved", None)
        .await;
    let ticket = tickets.body["tickets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|ticket| ticket["session_id"] == session.as_str())
        .expect("resolved ticket");
    assert_eq!(ticket["tool_call_id"], "child-write");
    assert_eq!(ticket["resolution"]["approved"], true);
    let children = harness
        .request("GET", &format!("/v1/sessions/{session}/subagents"), None)
        .await;
    assert_eq!(children.body["subagents"][0]["status"], "completed");
    assert_eq!(
        tool_returns(&harness, &session, "delegate-call")
            .await
            .len(),
        1
    );

    // PP-6: a reload rebuilds the same screen from the event log alone. Disclosure state is
    // view-local, so the progress list opened above is closed before comparing.
    ui.eval("document.querySelectorAll('main.session .oc-card[data-open=true] .oc-close').forEach(b=>b.click()); true").await;
    ui.wait(
        "view-local Card disclosures collapsed",
        "!document.querySelector('main.session .oc-card[data-open=true]')",
    )
    .await;
    ui.wait(
        "terminal Card announcement delivered",
        "Array.from(document.querySelectorAll('main.session .oc-sr')).every(e=>!e.textContent) ",
    )
    .await;
    const SCREEN: &str = "(() => { document.querySelectorAll('main.session details').forEach((d) => { d.open = false }); \
        const main = document.querySelector('main.session'); return main ? [main.dataset.lastSeq, main.innerText] : null })()";
    let before = ui.eval(SCREEN).await;
    ui.goto(&format!("/sessions/{session}")).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let after = ui.eval(SCREEN).await;
        if after == before {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "reloaded screen differs:\nbefore {before}\nafter  {after}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    ui.close().await;
    harness.kill().await;
}

/// Attach a second DevTools session to the UI tab and emulate a 390x844 phone. The
/// override lives as long as the returned socket.
async fn emulate_phone(
    ws_url: &str,
    tab: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let (mut socket, _) = tokio_tungstenite::connect_async(ws_url).await.unwrap();
    async fn call(
        socket: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        request: Value,
    ) -> Value {
        let id = request["id"].clone();
        socket
            .send(Message::Text(request.to_string().into()))
            .await
            .unwrap();
        loop {
            let next = tokio::time::timeout(Duration::from_secs(10), socket.next())
                .await
                .expect("CDP reply")
                .unwrap()
                .unwrap();
            if let Message::Text(body) = next {
                let reply: Value = serde_json::from_str(body.as_str()).unwrap();
                if reply["id"] == id {
                    assert!(reply["error"].is_null(), "{reply}");
                    return reply["result"].clone();
                }
            }
        }
    }
    let attached = call(
        &mut socket,
        json!({"id":1,"method":"Target.attachToTarget","params":{"targetId":tab,"flatten":true}}),
    )
    .await;
    let session = attached["sessionId"].as_str().unwrap().to_owned();
    call(
        &mut socket,
        json!({"id":2,"sessionId":session,"method":"Emulation.setDeviceMetricsOverride",
        "params":{"width":390,"height":844,"deviceScaleFactor":3,"mobile":true}}),
    )
    .await;
    call(
        &mut socket,
        json!({"id":3,"sessionId":session,"method":"Emulation.setTouchEmulationEnabled",
        "params":{"enabled":true,"maxTouchPoints":5}}),
    )
    .await;
    socket
}

#[tokio::test]
async fn pp22_phone_viewport_approval_resumes_the_same_task() {
    let Some((mut harness, executable)) = prepared(&json!({"responses":[]}), None).await else {
        return;
    };
    let outside = harness.root().join("from-phone.txt");
    harness.replace_script(&json!({"responses":[
        tool("write", json!({"path":outside,"content":"approved on the phone"}), "phone-write"),
        text("written after approval"),
    ]}));
    let session = create(&harness, "guard").await;
    let token = pair(&harness, &["read", "steer", "approve"]).await;
    let ui = Ui::open(executable, &harness, &token, "/").await;
    let _phone = emulate_phone(&ui.browser.endpoint().ws_url, &ui.tab).await;
    ui.goto(&format!("/sessions/{session}")).await;
    assert_eq!(
        ui.eval("[innerWidth, innerHeight]").await,
        json!([390, 844])
    );
    ui.wait("session screen", "!!document.querySelector('.composer')")
        .await;
    ui.eval("__os.send('Save the note')").await;
    ui.wait("approval card", &format!("!!{APPROVE}")).await;
    // The card appears when the ticket is recorded; the session reports `waiting_human`
    // once the deferred run has gone idle.
    tokio::time::timeout(Duration::from_secs(10), async {
        while harness
            .request("GET", &format!("/v1/sessions/{session}"), None)
            .await
            .body["status"]
            != "waiting_human"
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("session reports waiting_human");
    // The card and its buttons fit the phone screen without horizontal scrolling.
    assert_eq!(
        ui.eval(&format!(
            "(() => {{ const r = {APPROVE}.getBoundingClientRect(); const c = document.querySelector('.oc-card[data-kind=\"confirmation\"][data-status=\"pending\"]').getBoundingClientRect(); \
             return r.right <= innerWidth && c.right <= innerWidth && document.documentElement.scrollWidth <= innerWidth }})()"
        ))
        .await,
        true
    );
    assert!(!outside.exists());
    ui.eval(&format!("{APPROVE}.click()")).await;
    ui.wait(
        "task resumed and settled",
        "__os.finalText('written after approval')",
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(&outside).unwrap(),
        "approved on the phone"
    );
    let returns = tool_returns(&harness, &session, "phone-write").await;
    assert_eq!(returns.len(), 1, "approved call resumed exactly once");
    assert!(
        !returns[0]["content"].to_string().contains("denied"),
        "{returns:?}"
    );
    ui.close().await;
    harness.kill().await;
}

const SECRET_PAGE: &str = r#"<!doctype html><title>Verification</title>
<label>Code <input aria-label="Code" autocomplete="off"></label>"#;
const CAPTCHA_PAGE: &str = r#"<!doctype html><html><head><title>Captcha fixture</title><style>
body{margin:0} #go{position:fixed;left:0;top:0;width:400px;height:200px}
#name{position:fixed;left:0;top:300px;width:400px;height:60px}
</style></head><body><button id="go" onclick="document.title='Clicked by human'">I am not a robot</button>
<input id="name" aria-label="Name" autocomplete="off"></body></html>"#;
const CANARY: &str = "web-ui-secret-canary-58213";

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

async fn submit(harness: &Harness, session: &str, id: &str) {
    let response = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            Some(json!({"message_id":id,"text":id,"mode":"now"})),
        )
        .await;
    assert_eq!(response.status, 202, "{}", response.body);
}

/// Events up to the settle that follows a final assistant message `text`.
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

#[tokio::test]
async fn web_ui_secret_input_and_takeover_screen_resume_the_agent() {
    let Some(agent_browser) = chromium() else {
        return;
    };
    let app = Router::new()
        .route("/secret", get(|| async { Html(SECRET_PAGE) }))
        .route("/captcha", get(|| async { Html(CAPTCHA_PAGE) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let site = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let Some((mut harness, executable)) =
        prepared(&json!({"responses":[]}), Some(&agent_browser)).await
    else {
        server.abort();
        return;
    };
    let session = create(&harness, "full").await;
    let mut stream = harness.events(&session, 0).await;
    let mut responses = vec![
        tool(
            "browser_open_tab",
            json!({"url":format!("{site}/secret")}),
            "open-secret",
        ),
        tool(
            "browser_open_tab",
            json!({"url":format!("{site}/captcha")}),
            "open-captcha",
        ),
        text("opened"),
    ];
    harness.replace_script(&json!({"responses":responses}));
    submit(&harness, &session, "open-message").await;
    let events = until_final(&mut stream, "opened").await;
    let secret_tab = ended_ok(&events, "open-secret")["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let captcha_tab = ended_ok(&events, "open-captcha")["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    responses.extend([
        tool(
            "browser_snapshot",
            json!({"tab_id":secret_tab}),
            "secret-snapshot",
        ),
        text("found the code field"),
    ]);
    harness.replace_script(&json!({"responses":responses}));
    harness.kill_sidecar();
    stream.settled().await;
    submit(&harness, &session, "snapshot-message").await;
    let events = until_final(&mut stream, "found the code field").await;
    let code_ref = field(ended_ok(&events, "secret-snapshot"), "Code");
    responses.extend([
        tool("request_human_step", json!({"kind":"secret_input","summary":"Enter the verification code","tab_id":secret_tab,"origin":site,"field_ref":code_ref}), "code-step"),
        tool("request_human_step", json!({"kind":"takeover","summary":"Solve the CAPTCHA","tab_id":captcha_tab}), "captcha"),
        tool("browser_snapshot", json!({"tab_id":captcha_tab}), "after-takeover"),
        text("all done"),
    ]);
    harness.replace_script(&json!({"responses":responses}));
    harness.kill_sidecar();
    stream.settled().await;

    let token = pair(&harness, &["read", "steer", "approve", "human_step"]).await;
    let ui = Ui::open(
        executable,
        &harness,
        &token,
        &format!("/sessions/{session}"),
    )
    .await;
    ui.wait("session screen", "!!document.querySelector('.composer')")
        .await;
    ui.eval("__os.send('Continue')").await;

    // (b) The secret goes from a password field straight to the daemon.
    let secret_card = "__os.card('.oc-card[data-kind=\"secret_input\"][data-status=\"pending\"]')";
    ui.wait(
        "secret card",
        &format!("!!{secret_card}?.querySelector('input[type=password]')"),
    )
    .await;
    ui.eval(&format!(
        "(() => {{ const i = {secret_card}.querySelector('input[type=password]'); __os.fill(i, {}); i.form.requestSubmit(); return true }})()",
        json!(CANARY)
    ))
    .await;
    ui.wait(
        "secret submitted",
        "!!document.querySelector('.oc-card[data-kind=\"secret_input\"][data-status=\"done\"]')",
    )
    .await;

    // (d) The takeover card opens the remote screen, which receives frames of the tab.
    let takeover_card = "__os.card('.oc-card[data-kind=\"takeover\"][data-status=\"pending\"]')";
    ui.wait(
        "takeover card",
        &format!("!!__os.button({takeover_card}, 'Open remote screen')"),
    )
    .await;
    ui.eval(&format!(
        "__os.button({takeover_card}, 'Open remote screen').click()"
    ))
    .await;
    ui.wait(
        "first frame",
        "(() => { const i = document.querySelector('.takeover-screen img'); return !!i && i.complete && i.naturalWidth > 0 })()",
    )
    .await;
    assert_eq!(
        ui.eval("document.querySelector('.takeover-screen').dataset.frameWidth")
            .await,
        "1440"
    );
    assert_eq!(
        ui.eval("!!document.querySelector('.takeover button[aria-label=\"Close\"]')")
            .await,
        true
    );
    // A click on the button and typing into the field, through the UI's own handlers.
    ui.eval(
        r#"(() => {
          const screen = document.querySelector('.takeover-screen');
          const r = screen.querySelector('img').getBoundingClientRect();
          const at = (x, y) => ({ clientX: r.left + x * r.width / 1440, clientY: r.top + y * r.height / 900, button: 0, bubbles: true });
          for (const [x, y] of [[100, 100], [100, 330]]) {
            screen.dispatchEvent(new PointerEvent('pointerdown', at(x, y)));
            screen.dispatchEvent(new PointerEvent('pointerup', at(x, y)));
          }
          for (const key of 'ui-typed') {
            screen.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }));
            screen.dispatchEvent(new KeyboardEvent('keyup', { key, bubbles: true, cancelable: true }));
          }
          return true;
        })()"#,
    )
    .await;
    // Input is forwarded as it arrives; give Chromium a moment before ending the takeover.
    tokio::time::sleep(Duration::from_secs(1)).await;
    ui.eval("__os.button(document.querySelector('.takeover'), 'Done').click()")
        .await;
    ui.wait(
        "takeover completed and dismissed",
        "document.querySelector('.takeover') === null && !!document.querySelector('.oc-card[data-kind=\"takeover\"][data-status=\"done\"]')",
    )
    .await;
    ui.wait("agent resumed and settled", "__os.finalText('all done')")
        .await;

    let events = until_final(&mut stream, "all done").await;
    let code = tool_returns(&harness, &session, "code-step").await;
    assert_eq!(code.len(), 1);
    assert_eq!(code[0]["content"]["status"], "filled");
    let captcha = tool_returns(&harness, &session, "captcha").await;
    assert_eq!(captcha.len(), 1, "takeover call resumed exactly once");
    assert!(
        captcha[0]["content"]
            .to_string()
            .contains("Clicked by human"),
        "{captcha:?}"
    );
    let after = ended_ok(&events, "after-takeover").to_string();
    assert!(
        after.contains("ui-typed"),
        "typed text reached the page: {after}"
    );
    // The secret never reaches events, the transcript or the page.
    harness.assert_no_canaries(&[CANARY]);
    assert_eq!(
        ui.eval(&format!(
            "document.documentElement.outerHTML.includes({})",
            json!(CANARY)
        ))
        .await,
        false
    );
    ui.close().await;
    harness.kill().await;
    server.abort();
}

/// An emailed-code page that, like many real ones, submits itself once the code is complete.
const OTP_PAGE: &str = r#"<!doctype html><title>Check your email</title>
<form method="post" action="/login"><label>Email code <input name="code" aria-label="Email code" autocomplete="one-time-code"
 oninput="if (this.value.length === 6) this.form.submit()"></label></form>"#;
const OTP_CODE: &str = "Q7K2XM";

#[tokio::test]
async fn web_ui_inline_code_form_finishes_a_login_without_leaving_the_conversation() {
    let Some(agent_browser) = chromium() else {
        return;
    };
    let received = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    let sink = received.clone();
    let app = Router::new().route(
        "/login",
        get(|| async { Html(OTP_PAGE) }).post(
            move |axum::extract::Form(form): axum::extract::Form<
                std::collections::HashMap<String, String>,
            >| async move {
                let code = form.get("code").cloned().unwrap_or_default();
                let ok = code == OTP_CODE;
                sink.lock().push(code);
                Html(if ok {
                    "<!doctype html><title>Signed in</title><h1>Welcome back</h1>"
                } else {
                    "<!doctype html><title>Wrong code</title>"
                })
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let site = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let Some((mut harness, executable)) =
        prepared(&json!({"responses":[]}), Some(&agent_browser)).await
    else {
        server.abort();
        return;
    };
    let session = create(&harness, "full").await;
    let mut stream = harness.events(&session, 0).await;
    let mut responses = vec![
        tool(
            "browser_open_tab",
            json!({"url":format!("{site}/login")}),
            "open",
        ),
        text("opened"),
    ];
    harness.replace_script(&json!({"responses":responses}));
    submit(&harness, &session, "open-message").await;
    let events = until_final(&mut stream, "opened").await;
    let tab = ended_ok(&events, "open")["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    responses.extend([
        tool("browser_snapshot", json!({"tab_id":tab}), "fields"),
        text("found the code field"),
    ]);
    harness.replace_script(&json!({"responses":responses}));
    harness.kill_sidecar();
    stream.settled().await;
    submit(&harness, &session, "snapshot-message").await;
    let events = until_final(&mut stream, "found the code field").await;
    let code_ref = field(ended_ok(&events, "fields"), "Email code");
    responses.extend([
        tool("request_otp", json!({"tab_id":tab,"field_ref":code_ref,"channel":"email","summary":"Code from the sign-in email"}), "otp"),
        text("signed in"),
    ]);
    harness.replace_script(&json!({"responses":responses}));
    harness.kill_sidecar();
    stream.settled().await;

    let token = pair(&harness, &["read", "steer", "approve", "human_step"]).await;
    let path = format!("/sessions/{session}");
    let ui = Ui::open(executable, &harness, &token, &path).await;
    ui.wait("session screen", "!!document.querySelector('.composer')")
        .await;
    let history = ui.eval("history.length").await;
    ui.eval("__os.send('Sign in')").await;

    // The code form appears in the conversation itself: no new route, no dialog, no takeover.
    let form =
        "__os.card('.oc-card[data-kind=\"otp\"][data-status=\"pending\"]')?.querySelector('form')";
    ui.wait(
        "inline code form",
        &format!("!!{form}?.querySelector('input[autocomplete=one-time-code]')"),
    )
    .await;
    assert_eq!(
        ui.eval("location.pathname + location.search").await,
        json!(path)
    );
    assert_eq!(ui.eval("history.length").await, history);
    assert_eq!(
        ui.eval("!!document.querySelector('[role=dialog], .takeover, .card-sheet')")
            .await,
        false
    );
    assert_eq!(
        ui.eval(&format!(
            "{form}.textContent.includes('Enter the code sent by email.')"
        ))
        .await,
        true
    );

    ui.eval(&format!(
        "(() => {{ const i = {form}.querySelector('input[autocomplete=one-time-code]'); __os.fill(i, {}); i.form.requestSubmit(); return true }})()",
        json!(OTP_CODE)
    ))
    .await;
    ui.wait("agent resumed and settled", "__os.finalText('signed in')")
        .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while received.lock().is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the site never got the code"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(*received.lock(), [OTP_CODE]);
    let otp = tool_returns(&harness, &session, "otp").await;
    assert_eq!(otp.len(), 1);
    assert_eq!(otp[0]["content"], json!({"status":"filled"}));
    assert_eq!(
        ui.eval("location.pathname + location.search").await,
        json!(path)
    );
    assert_eq!(
        ui.eval(&format!(
            "document.documentElement.outerHTML.includes({})",
            json!(OTP_CODE)
        ))
        .await,
        false
    );
    let events = until_final(&mut stream, "signed in").await;
    // The site's own POST body sits in the agent browser's session history, as for any form
    // a person submits, so only the agent's surfaces are checked here.
    let transcript = harness.snapshot(&session).to_string();
    let events = serde_json::to_string(&events).unwrap();
    assert!(!transcript.contains(OTP_CODE) && !events.contains(OTP_CODE));
    ui.close().await;
    harness.kill().await;
    server.abort();
}
