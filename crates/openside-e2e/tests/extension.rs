//! Stock Chromium MV3 fallback: real daemon, scripted sidecar, disposable profile.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use futures_util::{SinkExt, StreamExt};
use openside_e2e::Harness;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_tungstenite::tungstenite::Message;

struct Cdp {
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    next: u64,
}
impl Cdp {
    async fn call(&mut self, session: Option<&str>, method: &str, params: Value) -> Value {
        let message = self.send(session, method, params).await;
        assert!(message.get("error").is_none(), "{method}: {message}");
        message["result"].clone()
    }
    /// The raw reply, error included.
    async fn send(&mut self, session: Option<&str>, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        let mut request = json!({"id":id,"method":method,"params":params});
        if let Some(session) = session {
            request["sessionId"] = json!(session);
        }
        self.socket
            .send(Message::text(request.to_string()))
            .await
            .unwrap();
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(20), self.socket.next())
                .await
                .expect("CDP timeout")
                .unwrap()
                .unwrap();
            let Message::Text(text) = frame else { continue };
            let message: Value = serde_json::from_str(&text).unwrap();
            if message["id"] == id {
                return message;
            }
        }
    }
    async fn attach(&mut self, target: &str) -> String {
        self.call(
            None,
            "Target.attachToTarget",
            json!({"targetId":target,"flatten":true}),
        )
        .await["sessionId"]
            .as_str()
            .unwrap()
            .into()
    }
    async fn evaluate(&mut self, session: &str, expression: &str) -> Value {
        let result = self
            .call(
                Some(session),
                "Runtime.evaluate",
                evaluate_params(expression),
            )
            .await;
        assert!(
            result.get("exceptionDetails").is_none(),
            "JS exception: {result}"
        );
        result["result"]["value"].clone()
    }
    /// `None` while the page has no JavaScript context to evaluate in: a freshly attached or
    /// navigating target briefly has none, and a pending evaluation dies with its context.
    async fn evaluate_when_ready(&mut self, session: &str, expression: &str) -> Option<Value> {
        let message = self
            .send(
                Some(session),
                "Runtime.evaluate",
                evaluate_params(expression),
            )
            .await;
        if let Some(error) = message.get("error") {
            let between_contexts = error["message"].as_str().is_some_and(|text| {
                [
                    "Cannot find default execution context",
                    "Execution context was destroyed",
                    "Inspected target navigated or closed",
                ]
                .iter()
                .any(|transient| text.starts_with(transient))
            });
            assert!(between_contexts, "Runtime.evaluate: {message}");
            return None;
        }
        let result = &message["result"];
        assert!(
            result.get("exceptionDetails").is_none(),
            "JS exception: {result}"
        );
        Some(result["result"]["value"].clone())
    }
    async fn click(&mut self, session: &str, selector: &str) {
        // Native viewport geometry can lag a device-metrics change by one rendered frame.
        let position = self.evaluate(session, &format!("(async () => {{ await new Promise(resolve => requestAnimationFrame(resolve)); const element = document.querySelector({}); element.scrollIntoView({{block:'center',inline:'center',behavior:'instant'}}); await new Promise(resolve => requestAnimationFrame(resolve)); const r = element.getBoundingClientRect(); const x = r.x+r.width/2; const y = r.y+r.height/2; return {{x,y,hit:element.contains(document.elementFromPoint(x,y))}}; }})()", json!(selector))).await;
        assert_eq!(
            position["hit"],
            json!(true),
            "fixture click target is not visible: {selector}: {position}"
        );
        for kind in ["mousePressed", "mouseReleased"] {
            self.call(Some(session), "Input.dispatchMouseEvent", json!({"type":kind,"x":position["x"],"y":position["y"],"button":"left","clickCount":1})).await;
        }
    }
    async fn page(&mut self, url: &str) -> String {
        let target = self
            .call(None, "Target.createTarget", json!({"url":url}))
            .await["targetId"]
            .as_str()
            .unwrap()
            .to_owned();
        let session = self.attach(&target).await;
        self.wait_for(
            &session,
            &format!(
                "location.href === {} && document.readyState === 'complete'",
                json!(url)
            ),
        )
        .await;
        session
    }
    async fn wait_for(&mut self, session: &str, expression: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            if self.evaluate_when_ready(session, expression).await == Some(json!(true)) {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                let visible = self.evaluate(session, "document.body.innerText").await;
                panic!(
                    "browser condition did not become true: {expression}; visible text: {visible}"
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

fn evaluate_params(expression: &str) -> Value {
    json!({"expression":expression,"awaitPromise":true,"returnByValue":true,"userGesture":true})
}

async fn debug_endpoint(profile: &Path) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(text) = std::fs::read_to_string(profile.join("DevToolsActivePort")) {
            let mut lines = text.lines();
            if let (Some(port), Some(path)) = (lines.next(), lines.next()) {
                return format!("ws://127.0.0.1:{port}{path}");
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Chromium did not publish its debugging endpoint"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn gmail_and_slack_handle_with_create_context_sessions_and_ask_streams_reply() {
    let executable = match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(path) => path,
        Err(reason) => {
            eprintln!("SKIP: extension E2E requires stock Chromium with MV3 support: {reason}");
            return;
        }
    };
    // Unbreakable fixtures: the Ask panel must wrap or truncate them, never scroll sideways.
    let long_url = format!("https://example.test/{}", "a".repeat(200));
    let long_korean = "가나다라마바사아자차카타파하".repeat(20);
    let reply = format!(
        "Message received by openside. See {long_url} and {long_korean}.\n\n```\n{}\n```",
        "x".repeat(300)
    );
    let Some(mut harness) = Harness::prepare(
        env!("CARGO_BIN_EXE_e2e-opensided"),
        &json!({"responses":[{"parts":[{"type":"text","text":reply}]}]}),
    ) else {
        return;
    };
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let build_dir = harness.root().join("extension");
    let build = tokio::process::Command::new("pnpm")
        .args(["-C", "extension", "build"])
        .current_dir(&repo)
        .env("OPENSIDE_EXTENSION_OUTDIR", &build_dir)
        .env("OPENSIDE_EXTENSION_TEST_MATCHES", "1")
        .output()
        .await
        .expect("pnpm must be installed for extension E2E");
    assert!(
        build.status.success(),
        "extension build: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(build_dir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["manifest_version"], 3);
    let ui = harness.root().join("mock-pages");
    std::fs::create_dir_all(&ui).unwrap();
    for site in ["gmail", "slack"] {
        std::fs::copy(
            repo.join(format!("extension/tests/fixtures/{site}.html")),
            ui.join(format!("{site}.html")),
        )
        .unwrap();
    }
    std::fs::write(
        ui.join("index.html"),
        "<!doctype html><title>Extension fixtures</title>",
    )
    .unwrap();
    harness.append_config(&format!("[ui]\ndir = {}\n", json!(ui)));
    harness.append_config(&format!(
        "[browser]\nexecutable = {}\nextensions = [{}]\ngateway = \"off\"\n",
        json!(executable),
        json!(build_dir)
    ));
    harness.launch().await;
    let port = std::fs::read_to_string(harness.data_dir.join("opensided.port"))
        .unwrap()
        .trim()
        .to_owned();
    let origin = format!("http://127.0.0.1:{port}");
    let paired = harness
        .request(
            "POST",
            "/v1/devices/pair",
            Some(json!({"name":"extension-e2e","scopes":["read","steer","approve"]})),
        )
        .await;
    assert_eq!(paired.status, 201);
    let token = paired.body["token"].as_str().unwrap();
    let initial = harness
        .tcp_request("GET", "/v1/browser/tabs", Some(token), None)
        .await;
    assert_eq!(initial.status, 200);
    assert_eq!(initial.body["tabs"], json!([]));
    assert!(
        !harness.data_dir.join("browser-profile").exists(),
        "reading the ledger must not launch a browser"
    );
    let owner = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"title":"Browser owner","mode":"full"})),
        )
        .await;
    assert_eq!(owner.status, 201);
    let owner_id = owner.body["id"].as_str().unwrap();
    let opened = harness.request("POST", &format!("/v1/sessions/{owner_id}/repl"), Some(json!({"code": format!("await page.goto({}); page.url()", json!(format!("{origin}/gmail.html")))}))).await;
    assert_eq!(opened.status, 200);
    assert_eq!(opened.body["status"], "done", "{}", opened.body);
    let owned_target = opened.body["result"]["tab_id"].as_str().unwrap().to_owned();
    let profile = harness.data_dir.join("browser-profile");
    let (socket, _) = tokio_tungstenite::connect_async(debug_endpoint(&profile).await)
        .await
        .unwrap();
    let mut cdp = Cdp { socket, next: 0 };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    // Recent Chromium runs component extensions with their own `background.js` worker
    // (Contextual Tasks in 154), so the worker is ours only if it reports our manifest.
    let mut foreign = std::collections::HashSet::new();
    let (worker, worker_session) = 'found: loop {
        let targets = cdp.call(None, "Target.getTargets", json!({})).await;
        let candidates: Vec<Value> = targets["targetInfos"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|target| {
                target["type"] == "service_worker"
                    && target["url"]
                        .as_str()
                        .is_some_and(|url| url.ends_with("/background.js"))
                    && !foreign.contains(target["targetId"].as_str().unwrap())
            })
            .cloned()
            .collect();
        for candidate in candidates {
            let target = candidate["targetId"].as_str().unwrap().to_owned();
            let session = cdp.attach(&target).await;
            match cdp
                .evaluate_when_ready(&session, "chrome.runtime.getManifest().name")
                .await
            {
                Some(name) if name == manifest["name"] => break 'found (candidate, session),
                Some(_) => {
                    foreign.insert(target);
                }
                None => {}
            }
            cdp.call(
                None,
                "Target.detachFromTarget",
                json!({"sessionId":session}),
            )
            .await;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "stock Chromium did not load the unpacked MV3 extension"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let extension_origin = worker["url"]
        .as_str()
        .unwrap()
        .trim_end_matches("background.js");
    let options = cdp.page(&format!("{extension_origin}options.html")).await;
    // The test build grants loopback host access; the headless permission dialog is not exercised.
    cdp.evaluate(&options, &format!("document.querySelector('#url').value = {}; document.querySelector('#token').value = {}; document.querySelector('#pairing').requestSubmit(); true", json!(origin), json!(token))).await;
    cdp.wait_for(
        &options,
        "document.querySelector('#status').textContent === 'Paired for this browser session.'",
    )
    .await;
    assert_eq!(
        cdp.evaluate(&options, "document.querySelector('#token').value")
            .await,
        json!("")
    );
    assert_eq!(
        cdp.evaluate(
            &options,
            "document.querySelector('#pairing-state').textContent"
        )
        .await,
        json!(format!("Paired with {origin}"))
    );
    cdp.call(
        Some(&options),
        "Emulation.setDeviceMetricsOverride",
        json!({"width":320,"height":720,"deviceScaleFactor":1,"mobile":false}),
    )
    .await;
    cdp.wait_for(
        &options,
        "document.documentElement.scrollWidth <= document.documentElement.clientWidth",
    )
    .await;
    cdp.call(
        Some(&options),
        "Emulation.clearDeviceMetricsOverride",
        json!({}),
    )
    .await;
    cdp.click(&options, "#agent-tabs").await;
    cdp.wait_for(
        &options,
        "document.querySelector('#agent-tabs').textContent === 'Disable Agent Tabs'",
    )
    .await;
    let grouping = format!(
        "chrome.debugger.getTargets().then(async targets => {{ const target = targets.find(t => t.id === {}); if (!target || target.tabId === undefined) return false; const tab = await chrome.tabs.get(target.tabId); if (tab.groupId < 0) return false; return (await chrome.tabGroups.get(tab.groupId)).title === 'Agent Tabs'; }})",
        json!(owned_target)
    );
    cdp.wait_for(&worker_session, &grouping).await;
    cdp.click(&options, "#agent-tabs").await;
    cdp.wait_for(
        &options,
        "document.querySelector('#agent-tabs').textContent === 'Enable Agent Tabs'",
    )
    .await;
    let ungrouped = format!(
        "chrome.debugger.getTargets().then(async targets => {{ const target = targets.find(t => t.id === {}); return target && (await chrome.tabs.get(target.tabId)).groupId === -1; }})",
        json!(owned_target)
    );
    cdp.wait_for(&worker_session, &ungrouped).await;
    cdp.click(&options, "#agent-tabs").await;
    cdp.wait_for(
        &options,
        "document.querySelector('#agent-tabs').textContent === 'Disable Agent Tabs'",
    )
    .await;
    cdp.wait_for(&worker_session, &grouping).await;
    let mut expected = Vec::new();
    for (site, text, selector) in [
        ("gmail", "Older email, not selected.", ".a3s"),
        ("slack", "Older Slack message.", "[data-qa=message-text]"),
    ] {
        let url = format!("{origin}/{site}.html");
        let page = cdp.page(&url).await;
        cdp.wait_for(&page, "!!document.querySelector('#openside-handle-host')")
            .await;
        let count = harness.request("GET", "/v1/sessions", None).await.body["sessions"]
            .as_array()
            .unwrap()
            .len();
        assert_eq!(cdp.evaluate(&page, "document.querySelector('#openside-handle-host').click(); document.querySelector('#openside-handle-host').dispatchEvent(new MouseEvent('click', {bubbles:true})); document.querySelector('#openside-handle-host').shadowRoot === null").await, json!(true));
        let quiet_until = tokio::time::Instant::now() + Duration::from_millis(500);
        loop {
            assert_eq!(
                harness.request("GET", "/v1/sessions", None).await.body["sessions"]
                    .as_array()
                    .unwrap()
                    .len(),
                count,
                "synthetic page clicks must not create sessions"
            );
            if tokio::time::Instant::now() >= quiet_until {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        cdp.evaluate(&page, &format!("(() => {{ const range = document.createRange(); range.selectNodeContents(document.querySelector({})); const selection = getSelection(); selection.removeAllRanges(); selection.addRange(range); return selection.toString(); }})()", json!(selector))).await;
        cdp.click(&page, "#openside-handle-host").await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            if harness.request("GET", "/v1/sessions", None).await.body["sessions"]
                .as_array()
                .unwrap()
                .len()
                == count + 1
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "trusted Handle with click did not create a session"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let target = cdp
            .call(Some(&page), "Target.getTargetInfo", json!({}))
            .await["targetInfo"]["targetId"]
            .clone();
        assert_eq!(cdp.evaluate(&worker_session, &format!("chrome.debugger.getTargets().then(async targets => {{ const decoy = targets.find(t => t.id === {}); return decoy && (await chrome.tabs.get(decoy.tabId)).groupId === -1; }})", target)).await, json!(true), "a user-created same-URL tab must stay ungrouped");
        expected.push((url, text));
    }
    let listed = harness.request("GET", "/v1/sessions", None).await;
    let sessions = listed.body["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 4);
    assert_eq!(
        sessions
            .iter()
            .filter(|session| session["agent_id"] == "chief")
            .count(),
        1
    );
    let mut queued = Vec::new();
    for session in sessions
        .iter()
        .filter(|session| session["id"] != owner_id && session["agent_id"].is_null())
    {
        let id = session["id"].as_str().unwrap();
        let events = harness.events(id, 0).await.settled().await;
        queued.extend(events.into_iter().filter_map(|event| match event.kind {
            openside_proto::EventKind::MessageQueued { text, .. } => Some(text),
            _ => None,
        }));
    }
    for (url, message) in expected {
        assert!(
            queued.iter().any(|text| text.contains(message)
                && text.contains(&url)
                && text.contains("untrusted page context")),
            "message context was not delivered"
        );
    }
    // The side-panel document is also drivable as an extension page in headless Chromium.
    let panel = cdp.page(&format!("{extension_origin}panel.html")).await;
    cdp.evaluate(&panel, &format!("document.querySelector('#message').value = {}; document.querySelector('#compose').requestSubmit(); true", json!(format!("Question from Ask {long_url} {long_korean}")))).await;
    cdp.wait_for(&panel, "document.querySelector('#transcript').textContent.includes('Message received by openside.')").await;
    let no_sideways_scroll = "(() => { const root = document.documentElement; return root.scrollWidth <= root.clientWidth && [...document.querySelectorAll('.scroll, .bar, .bubble, .oc-card, .composer')].every(e => e.scrollWidth <= e.clientWidth); })()";
    for width in [320, 400, 480] {
        cdp.call(
            Some(&panel),
            "Emulation.setDeviceMetricsOverride",
            json!({"width":width,"height":720,"deviceScaleFactor":1,"mobile":false}),
        )
        .await;
        cdp.wait_for(&panel, no_sideways_scroll).await;
    }
    assert_eq!(
        harness.request("GET", "/v1/sessions", None).await.body["sessions"]
            .as_array()
            .unwrap()
            .len(),
        5
    );
    let ledger = harness
        .tcp_request("GET", "/v1/browser/tabs", Some(token), None)
        .await;
    assert_eq!(ledger.status, 200);
    assert!(ledger.body["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tab| tab["tab_id"] == owned_target
            && tab["opened_by"] == "agent"
            && tab["owner_session_id"] == owner_id));
    assert!(ledger.body["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .all(|tab| tab["opened_by"] == "agent" && tab["owner_session_id"].is_string()));
    let no_token = harness
        .tcp_request("GET", "/v1/browser/tabs", None, None)
        .await;
    assert_eq!(no_token.status, 401);
    let private = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"title":"Private browser","mode":"full","incognito":true})),
        )
        .await;
    let private_id = private.body["id"].as_str().unwrap();
    let private_tab = harness.request("POST", &format!("/v1/sessions/{private_id}/repl"), Some(json!({"code":format!("await page.goto({}); page.url()", json!(format!("{origin}/slack.html")))}))).await;
    assert_eq!(private_tab.body["status"], "done", "{}", private_tab.body);
    let private_target = private_tab.body["result"]["tab_id"].as_str().unwrap();
    let listed = harness
        .tcp_request("GET", "/v1/browser/tabs", Some(token), None)
        .await;
    assert!(!listed.body["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tab| tab["tab_id"] == private_target));
    cdp.next += 1;
    cdp.socket
        .send(Message::text(
            json!({"id":cdp.next,"method":"Browser.close","params":{}}).to_string(),
        ))
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let stale = harness
            .tcp_request("GET", "/v1/browser/tabs", Some(token), None)
            .await;
        if stale.status == 503 {
            assert_eq!(stale.body["code"], "browser_disconnected");
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "closed browser must return 503 rather than stale metadata: {}",
            stale.body
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    drop(cdp);
    harness.kill().await;
}
