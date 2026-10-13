//! Real mobile PWA journeys over a disposable encrypted relay or an explicitly opted-in
//! Cloudflare Quick Tunnel. QR decoding uses desktop screenshot pixels, not the link's DOM.
//! Chromium's iPhone UA is not WebKit; real installation, OS badges and GCM/APNs remain external.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod ui_support;

use ui_support::{send, silent};

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

use axum::{response::Html, routing::get, Router};
use base64::Engine;
use futures_util::{FutureExt, SinkExt, StreamExt};
use openside_browser::{Browser, BrowserConfig};
use openside_e2e::Harness;
use openside_proto::EventKind;
use openside_push::fake_push::FakePushService;
use openside_relay::server::{self, ServerConfig};
use serde_json::{json, Value};
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};

const OWNER: &str = "mobile-remote-e2e";
const DAEMON: &str = env!("CARGO_BIN_EXE_e2e-opensided");
const REGISTRATION: &str = "private-mobile-e2e-relay-registration";
const IMAGE: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
const APPROVE: &str = "(() => { const card = document.querySelector('.oc-card[data-kind=confirmation][data-status=pending]'); card?.querySelector('.oc-pill')?.click(); return card && Array.from(card.querySelectorAll('button')).find(b => b.textContent.trim() === 'Approve'); })()";
const PROBE: &str = r#"(() => {
    window.__mobile = {badges: [], streaming: []};
    for (const [name, count] of [['setAppBadge', null], ['clearAppBadge', 0]]) {
        const native = navigator[name];
        if (typeof native === 'function') Object.defineProperty(navigator, name, {value: function(value) {
            window.__mobile.badges.push(count === null ? value : count);
            return native.apply(this, arguments);
        }});
    }
    new MutationObserver(() => {
        const text = document.querySelector('.bubble.assistant.streaming')?.textContent;
        if (text && window.__mobile.streaming.at(-1) !== text) window.__mobile.streaming.push(text);
    }).observe(document.body, {subtree: true, childList: true, characterData: true});
})()"#;

struct Task(tokio::task::JoinHandle<()>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct Cdp {
    socket: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    session: String,
    next: u64,
    events: Vec<Value>,
}
impl Cdp {
    async fn attach(browser: &Browser, tab: &str) -> Self {
        let (socket, _) = tokio_tungstenite::connect_async(browser.endpoint().ws_url)
            .await
            .unwrap();
        let mut cdp = Self {
            socket,
            session: String::new(),
            next: 0,
            events: Vec::new(),
        };
        cdp.session = cdp
            .root(
                "Target.attachToTarget",
                json!({"targetId":tab,"flatten":true}),
            )
            .await["sessionId"]
            .as_str()
            .unwrap()
            .into();
        cdp
    }
    async fn command(&mut self, session: Option<&str>, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        let mut request = json!({"id":id,"method":method,"params":params});
        if let Some(session) = session {
            request["sessionId"] = json!(session);
        }
        self.socket
            .send(Message::Text(request.to_string().into()))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let frame = self.socket.next().await.expect("CDP connected").unwrap();
                if let Message::Text(text) = frame {
                    let reply: Value = serde_json::from_str(&text).unwrap();
                    if reply["id"] == id {
                        assert!(reply["error"].is_null(), "{method}: {reply}");
                        return reply["result"].clone();
                    }
                    self.events.push(reply);
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{method} deadline"))
    }
    async fn root(&mut self, method: &str, params: Value) -> Value {
        self.command(None, method, params).await
    }
    async fn call(&mut self, method: &str, params: Value) -> Value {
        let session = self.session.clone();
        self.command(Some(&session), method, params).await
    }
    async fn eval(&mut self, expression: &str) -> Value {
        let result = self.call("Runtime.evaluate", json!({"expression":expression,"returnByValue":true,"awaitPromise":true,"userGesture":true})).await;
        assert!(
            result["exceptionDetails"].is_null(),
            "{expression}: {result}"
        );
        result["result"]["value"].clone()
    }
    async fn wait(&mut self, description: &str, expression: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            if self.eval(expression).await == true {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!(
                    "{description}: {}",
                    self.eval("document.body.innerText").await
                );
            }
            tokio::time::sleep(Duration::from_millis(75)).await;
        }
    }
    async fn click(&mut self, selector: &str) {
        self.wait(
            "click target",
            &format!("!!({selector}) && !({selector}).disabled"),
        )
        .await;
        assert!(
            self.tap(selector).await,
            "click target vanished: {selector}"
        );
    }
    async fn tap(&mut self, selector: &str) -> bool {
        let position = self.eval(&format!("(() => {{ const e = {selector}; if (!e || e.disabled) return null; e.scrollIntoView({{block:'center',behavior:'instant'}}); const r = e.getBoundingClientRect(); return {{x:r.left+r.width/2,y:r.top+r.height/2}}; }})()")).await;
        if position.is_null() {
            return false;
        }
        let point = json!({"x":position["x"],"y":position["y"]});
        self.call(
            "Input.dispatchTouchEvent",
            json!({"type":"touchStart","touchPoints":[point]}),
        )
        .await;
        self.call(
            "Input.dispatchTouchEvent",
            json!({"type":"touchEnd","touchPoints":[]}),
        )
        .await;
        true
    }
    /// CDP touch input can silently drop while the compositor settles on a loaded host; the
    /// hit test succeeds yet no event reaches the page. Re-click after a grace window while
    /// `until` stays false. Only for targets a duplicate tap cannot hurt — never toggles or
    /// submits.
    async fn click_until(&mut self, selector: &str, until: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            if self.eval(until).await == true {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "clicks on {selector} never produced {until}"
            );
            if self.tap(selector).await {
                // The last tap may have landed; only re-tap once the grace window passed
                // without the effect, and a gone target counts as landed.
                tokio::time::sleep(Duration::from_millis(750)).await;
            } else {
                tokio::time::sleep(Duration::from_millis(75)).await;
            }
        }
    }
    async fn fill(&mut self, selector: &str, text: &str) {
        self.eval(&format!("(() => {{const e = document.querySelector({}); Object.getOwnPropertyDescriptor(e instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype, 'value').set.call(e, {}); e.dispatchEvent(new Event('input', {{bubbles:true}})); return true;}})()", json!(selector), json!(text))).await;
    }
    async fn capture(&mut self, path: &Path) {
        self.eval("document.fonts.ready.then(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(() => r(true)))))").await;
        let shot = self
            .call(
                "Page.captureScreenshot",
                json!({"format":"png","captureBeyondViewport":false}),
            )
            .await;
        std::fs::write(
            path,
            base64::engine::general_purpose::STANDARD
                .decode(shot["data"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
    }
    async fn require_event_poll(&mut self) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(70);
        loop {
            let returned = self
                .events
                .iter()
                .filter(|event| event["method"] == "Network.responseReceived")
                .filter(|event| event["params"]["response"]["status"].as_f64() == Some(200.0))
                .filter_map(|event| event["params"]["response"]["url"].as_str())
                .filter_map(|url| url::Url::parse(url).ok())
                .any(|url| url.path().ends_with("/events/poll"));
            if returned {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "Quick event fallback never returned HTTP 200"
            );
            self.eval("document.querySelector('main.session')?.dataset.status")
                .await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    async fn registration(&mut self, origin: &str) -> Value {
        self.call("ServiceWorker.enable", json!({})).await;
        let scope = format!("{origin}/");
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Some(registration) = self
                    .events
                    .iter()
                    .filter(|event| event["method"] == "ServiceWorker.workerRegistrationUpdated")
                    .filter_map(|event| event["params"]["registrations"].as_array())
                    .flatten()
                    .find(|registration| {
                        registration["scopeURL"] == scope && registration["isDeleted"] == false
                    })
                {
                    return registration["registrationId"].clone();
                }
                let frame = self.socket.next().await.unwrap().unwrap();
                if let Message::Text(text) = frame {
                    self.events.push(serde_json::from_str(&text).unwrap());
                }
            }
        })
        .await
        .expect("service worker registration event")
    }
}

#[derive(Clone, Copy)]
enum Transport {
    Relay,
    Cloudflare,
}
impl Transport {
    fn name(self) -> &'static str {
        match self {
            Self::Relay => "relay",
            Self::Cloudflare => "cloudflare",
        }
    }
}

struct Remote {
    transport: Transport,
    origin: String,
    blue_preview: String,
}

async fn open_public_shell(browser: &Browser, tab: &str, origin: &str) {
    // Quick Tunnel DNS can become visible a minute after cloudflared publishes its URL.
    // Resolve and load the public shell before starting the pairing code's three-minute clock.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    let url = format!("{origin}/");
    loop {
        let loaded = browser.navigate(OWNER, tab, &url).await;
        if loaded.is_ok()
            && browser
                .evaluate(tab, "!!document.querySelector('.token-entry')")
                .await
                .is_ok_and(|value| value == true)
        {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "public shell did not become reachable: {loaded:?}"
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn ready(harness: &Harness, transport: Transport) -> String {
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let response = harness.request("GET", "/v1/remote/status", None).await;
            assert_eq!(response.status, 200, "{}", response.body);
            let state = &response.body[transport.name()];
            if state["state"] == "connected" || state["state"] == "running" {
                if let Some(url) = state["url"].as_str() {
                    return url.trim_end_matches('/').into();
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("remote transport public URL")
}

async fn desktop_offer(desktop: &mut Cdp, transport: Transport, name: &str, out: &Path) -> String {
    desktop
        .wait(
            "remote screen",
            "!!document.querySelector('[data-section=remote]')",
        )
        .await;
    desktop.eval("Array.from(document.querySelectorAll('.remote-result button')).find(b => b.textContent.trim() === 'Connect another phone')?.click(); true").await;
    // The code is issued on arrival with the default name; the name and permissions sit behind a toggle.
    desktop
        .wait(
            "first desktop pairing QR",
            "!!document.querySelector('.remote-qr svg.qr-code')",
        )
        .await;
    desktop
        .eval("document.querySelector('.pair-options-toggle').click(); true")
        .await;
    desktop
        .wait(
            "phone pairing form",
            "!!document.querySelector('.remote-form input[maxlength=\"60\"]')",
        )
        .await;
    desktop
        .fill(".remote-form input[maxlength=\"60\"]", name)
        .await;
    // Exercise permission selection without pinning the product's initial checkbox defaults.
    desktop.eval("document.querySelectorAll('.remote-scopes label').forEach(label => {const input=label.querySelector('input'); const wanted=label.dataset.risk !== 'high'; if(!input.disabled && input.checked !== wanted) input.click();}); true").await;
    desktop
        .eval("document.querySelector('.remote-form button[type=submit]').click(); true")
        .await;
    // The code made with the default name is replaced by one for this phone.
    desktop
        .wait(
            "desktop pairing QR for this phone",
            &format!(
                "document.querySelector('.remote-qr svg.qr-code')?.getAttribute('aria-label') === {}",
                json!(format!("QR code to connect {name}"))
            ),
        )
        .await;
    desktop.eval("document.querySelector('.remote-qr').scrollIntoView({block:'center',behavior:'instant'}); true").await;
    let path = out.join(format!("desktop-{name}-qr.png"));
    desktop.capture(&path).await;
    let decoded = tokio::process::Command::new("zbarimg")
        .args(["--quiet", "--raw"])
        .arg(&path)
        .output()
        .await
        .expect("install zbar-tools for QR decoding");
    assert!(
        decoded.status.success(),
        "QR screenshot could not be decoded: {}",
        String::from_utf8_lossy(&decoded.stderr)
    );
    let links = String::from_utf8(decoded.stdout).unwrap();
    let link = links
        .lines()
        .next()
        .expect("decoded pairing URL")
        .to_owned();
    assert!(link.contains("/pair#"), "decoded non-pairing QR");
    if matches!(transport, Transport::Relay) {
        assert!(link.contains("t=relay") && link.contains("&k="));
    }
    link
}

async fn phone(
    harness: &Harness,
    executable: &Path,
    desktop: &mut Cdp,
    remote: &Remote,
    profile: &str,
    out: &Path,
    push: &FakePushService,
) {
    let transport = remote.transport;
    let origin = remote.origin.as_str();
    let mut config = BrowserConfig::new(executable);
    config.profile_dir = Some(harness.root().join(format!("phone-{profile}")));
    let (browser, _) = Browser::launch(config).await.unwrap();
    let outcome = std::panic::AssertUnwindSafe(async {
        let tab = browser.create_agent_tab(OWNER, "about:blank").await.unwrap().id;
        let mut page = Cdp::attach(&browser, &tab).await;
        page.call("Emulation.setDeviceMetricsOverride", json!({"width":390,"height":844,"deviceScaleFactor":3,"mobile":true})).await;
        page.call("Emulation.setTouchEmulationEnabled", json!({"enabled":true,"maxTouchPoints":5})).await;
        page.call("Emulation.setFocusEmulationEnabled", json!({"enabled":true})).await;
        page.call("Network.enable", json!({})).await;
        let ua = if profile == "iphone" { "Mozilla/5.0 (iPhone; CPU iPhone OS 17_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.6 Mobile/15E148 Safari/604.1" } else { "Mozilla/5.0 (Linux; Android 14; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Mobile Safari/537.36" };
        page.call("Emulation.setUserAgentOverride", json!({"userAgent":ua,"platform":if profile == "iphone" {"iPhone"} else {"Android"}})).await;
        page.call("Emulation.setEmulatedMedia", json!({"features":[{"name":"prefers-color-scheme","value":"light"},{"name":"prefers-reduced-motion","value":"reduce"}]})).await;
        page.root("Browser.grantPermissions", json!({"permissions":["notifications"],"origin":origin})).await;
        open_public_shell(&browser, &tab, origin).await;
        assert_eq!(page.eval("localStorage.getItem('openside.token')").await, Value::Null);
        page.capture(&out.join(format!("{profile}-before-pair.png"))).await;
        let link = desktop_offer(desktop, transport, profile, out).await;
        assert!(link.starts_with(&format!("{origin}/pair#")));
        browser.navigate(OWNER, &tab, &link).await.unwrap();
        page.wait("paired mobile PWA", "!!document.querySelector('.shell') && location.hash === ''").await;
        page.eval(PROBE).await;
        desktop.wait("desktop notices the paired phone", &format!("document.querySelector('.remote-result-title')?.textContent.includes({}) === true", json!(format!("Connected as {profile}")))).await;
        desktop.capture(&out.join(format!("desktop-{profile}-connected.png"))).await;
        assert_eq!(page.eval("[innerWidth,innerHeight,devicePixelRatio,navigator.maxTouchPoints]").await, json!([390,844,3,5]));
        assert_eq!(page.eval("navigator.userAgent").await, ua);
        page.wait("service worker activated", "navigator.serviceWorker.getRegistration('/').then(r => !!r?.active)").await;
        let manifest = page.eval("fetch('/manifest.webmanifest').then(r => r.json())").await;
        assert_eq!(manifest["display"], "standalone");
        assert!(manifest["icons"].as_array().is_some_and(|icons| icons.len() >= 2));
        if matches!(transport, Transport::Relay) {
            assert_eq!(page.eval("JSON.parse(localStorage.getItem('openside.transport')).kind").await, "relay");
        } else {
            assert_eq!(page.eval("localStorage.getItem('openside.transport')").await, Value::Null);
        }
        if profile == "iphone" {
            page.wait("iPhone install instructions", "!!document.querySelector('[aria-label=\"Install openside\"]')").await;
            page.capture(&out.join(format!("{profile}-install.png"))).await;
            let install = page.eval("({text:document.querySelector('.install-banner').textContent,native:Array.from(document.querySelectorAll('.install-banner button')).some(b=>b.textContent.trim()==='Install')})").await;
            // UA emulation cannot stop Blink from emitting its real beforeinstallprompt event.
            assert!(install["text"].as_str().unwrap().contains("Add to Home Screen") || install["native"] == true, "{install}");
            if install["native"] == true {
                eprintln!("MOBILE_REMOTE {} {profile} NOT_RUN iOS_install_hint: Blink offered its native install prompt despite the iPhone UA", transport.name());
            }
            page.click_until(
                "document.querySelector('.install-banner button[aria-label=Dismiss]')",
                "!document.querySelector('.install-banner')",
            ).await;
        }
        let token = page.eval("localStorage.getItem('openside.token')").await.as_str().unwrap().to_owned();
        let devices = harness.request("GET", "/v1/devices", None).await;
        let device = devices.body["devices"].as_array().unwrap().iter().find(|device| device["name"] == profile).unwrap();
        assert_eq!(device["kind"], "person_device");
        let scopes = device["scopes"].as_array().unwrap();
        assert_eq!(scopes.len(), 5);
        for scope in ["read", "steer", "approve", "human_step", "browse"] {
            assert!(scopes.contains(&json!(scope)));
        }
        let badge_supported = page.eval("typeof navigator.setAppBadge === 'function' && typeof navigator.clearAppBadge === 'function'").await == true;
        let subscriber = push.subscribe();
        let subscribed = harness.tcp_request("POST", "/v1/push/subscriptions", Some(&token), Some(subscriber.to_json())).await;
        assert_eq!(subscribed.status, 201, "{}", subscribed.body);
        let registration = page.registration(origin).await;
        let chief = harness.request("GET", "/v1/agents/chief", None).await;
        let session = chief.body["home_session_id"].as_str().unwrap().to_owned();
        page.wait("team menu toggle", "!!document.querySelector('button[aria-label=\"Open menu\"]')").await;
        page.click("document.querySelector('button[aria-label=\"Open menu\"]')").await;
        page.click_until(
            "document.querySelector('.ui-drawer a[href=\"/history\"]')",
            "location.pathname === '/history'",
        ).await;
        page.wait("existing conversation history", "!!document.querySelector('.history-list .session-title') && document.body.innerText.includes('Existing conversation')").await;
        page.capture(&out.join(format!("{profile}-conversations.png"))).await;
        page.click("document.querySelector('button[aria-label=\"Open menu\"]')").await;
        page.click_until(
            "document.querySelector('.ui-drawer [data-agent=\"chief\"] .person-link')",
            "location.pathname === '/agents/chief'",
        ).await;
        page.wait("chief thread closes the phone drawer", "location.pathname === '/agents/chief' && document.querySelector('.ui-drawer') === null && !!document.querySelector('main.session .composer textarea')").await;
        page.capture(&out.join(format!("{profile}-chief-thread.png"))).await;
        let message = format!("Show the remote {profile} phone browser and request my approval.");
        page.fill(".composer textarea", &message).await;
        page.click("document.querySelector('.composer button[type=submit]')").await;
        let loads = page.eval("performance.timeOrigin").await;
        page.wait("approval streamed to phone", &format!("!!({APPROVE})")).await;
        page.wait("delegated task attribution", &format!("Array.from(document.querySelectorAll('.oc-asker')).some(e=>e.textContent.includes({}))", json!(format!("Prepare {profile} phone preview asked")))).await;
        let pending = harness.request("GET", "/v1/tickets?status=pending", None).await;
        let ticket = pending.body["tickets"].as_array().unwrap().iter().find(|ticket| ticket["session_id"] == session).unwrap();
        let task = ticket["origin"]["subagent_id"].as_str().expect("delegated approval task").to_owned();
        let ticket = ticket["id"].as_str().unwrap().to_owned();
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let tasks = harness.request("GET", &format!("/v1/sessions/{session}/subagents"), None).await;
                assert_eq!(tasks.status, 200, "{}", tasks.body);
                if tasks.body["subagents"].as_array().unwrap().iter().any(|child| child["id"] == task && child["status"] == "waiting_human") { break; }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }).await.expect("delegated task waits for the phone's approval");
        let local_denied = harness.request("POST", &format!("/v1/tickets/{ticket}/resolve"), Some(json!({"value":true}))).await;
        assert_eq!(local_denied.status, 403);
        assert_eq!(local_denied.body["code"], "person_approval_required");
        assert_eq!(harness.request("GET", &format!("/v1/tickets/{ticket}"), None).await.body["status"], "pending");
        page.eval("document.dispatchEvent(new Event('visibilitychange')); true").await;
        if badge_supported {
            page.wait("foreground badge path", "window.__mobile.badges.includes(1)").await;
        } else {
            eprintln!("MOBILE_REMOTE {} {profile} NOT_RUN badge: native Badging API unavailable in this Chromium build", transport.name());
        }
        page.wait("browser preview JPEG over remote WebSocket", "!!document.querySelector('.browser-card img')?.naturalWidth").await;
        let color = page.eval("(() => { const i=document.querySelector('.browser-card img'); const c=document.createElement('canvas'); c.width=i.naturalWidth; c.height=i.naturalHeight; const x=c.getContext('2d'); x.drawImage(i,0,0); return Array.from(x.getImageData(300,200,1,1).data).slice(0,3); })()").await;
        assert!(color[0].as_u64().unwrap() > 180 && color[2].as_u64().unwrap() < 30, "red preview pixels: {color}");
        assert_eq!(page.eval("document.documentElement.scrollWidth <= innerWidth").await, true);
        page.capture(&out.join(format!("{profile}-approval-pip.png"))).await;
        let delivery = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let deliveries = push.deliveries();
                if let Some(delivery) = deliveries.into_iter().find(|delivery| delivery.subscriber == subscriber.id && delivery.json()["ticket_id"] == ticket) {
                    break delivery;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }).await.expect("VAPID/RFC8291 ticket delivery");
        let payload = delivery.json();
        assert_eq!(payload["url"], format!("/sessions/{session}?ticket={ticket}"));
        page.call("ServiceWorker.deliverPushMessage", json!({"origin":origin,"registrationId":registration,"data":String::from_utf8(delivery.plaintext.clone()).unwrap()})).await;
        page.wait("real service worker notification", &format!("navigator.serviceWorker.ready.then(r => r.getNotifications()).then(all => all.some(n => n.data?.ticket_id === {} && n.data.url === {}))", json!(ticket), payload["url"])).await;
        page.capture(&out.join(format!("{profile}-notification.png"))).await;
        if matches!(transport, Transport::Cloudflare) {
            // A large initial SSE batch can flush; the human pause also exercises the silence watchdog.
            page.require_event_poll().await;
        }
        page.click_until(
            APPROVE,
            "!document.querySelector('.oc-card[data-kind=confirmation][data-status=pending]')",
        ).await;
        page.wait("same chief task resumed on phone", &format!("document.body.innerText.includes({}) && document.querySelector('main.session')?.dataset.status === 'idle'", json!(format!("Phone {profile} approval resumed the same task.")))).await;
        assert_eq!(page.eval("performance.timeOrigin").await, loads, "SSE must update without reload");
        page.eval("document.dispatchEvent(new Event('visibilitychange')); true").await;
        if badge_supported {
            page.wait("badge cleared after approval", "window.__mobile.badges.at(-1) === 0").await;
        }
        page.wait("settled ticket notification dismissed", "navigator.serviceWorker.ready.then(r => r.getNotifications()).then(all => !all.some(n => n.data?.kind === 'ticket'))").await;
        let store = openside_sessions::Store::open(harness.data_dir.join("events.db")).unwrap();
        let snapshot = store.events_after(&openside_proto::SessionId::new(&session), 0, usize::MAX).unwrap();
        assert!(snapshot.iter().any(|event| matches!(event.kind, EventKind::AssistantFinal {..})), "complete assistant messages required");
        assert!(!snapshot.iter().any(|event| matches!(event.kind, EventKind::AssistantDelta {..})), "chief must not stream message fragments");
        assert!(snapshot.iter().any(|event| matches!(&event.kind, EventKind::MessageQueued { text, .. } if text == &message)), "the phone's relay message must reach the chief home session");
        let poll_requests = page.events.iter().filter(|event| event["method"] == "Network.requestWillBeSent")
            .filter_map(|event| event["params"]["request"]["url"].as_str())
            .filter_map(|url| url::Url::parse(url).ok())
            .filter(|url| url.path().ends_with("/events/poll")).count();
        if matches!(transport, Transport::Cloudflare) {
            assert!(poll_requests > 0, "real Quick Tunnel must exercise the sequence-resumable event fallback");
        }
        let mut observations = page.eval("({streaming_mutations:window.__mobile.streaming.length,badge_calls:window.__mobile.badges,emulation:[innerWidth,innerHeight,devicePixelRatio,navigator.maxTouchPoints]})").await;
        observations["event_poll_requests"] = json!(poll_requests);
        std::fs::write(out.join(format!("{profile}-observations.json")), serde_json::to_vec_pretty(&observations).unwrap()).unwrap();
        // Closing the completed Task's tab and reporting its result wake the chief separately.
        let expected_cursor = if profile == "iphone" { 4 } else { 9 };
        tokio::time::timeout(Duration::from_secs(30), async {
            while harness.snapshot(&session)["script_cursor"] != expected_cursor {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }).await.expect("task report consumed before the next phone message");
        page.eval(&format!("(() => {{const data = new DataTransfer(); data.items.add(new File([Uint8Array.from(atob({}), c=>c.charCodeAt(0))], 'phone-photo.png', {{type:'image/png'}})); const input=document.querySelector('.composer input[type=file]'); input.files=data.files; input.dispatchEvent(new Event('change',{{bubbles:true}})); return true;}})()", json!(IMAGE))).await;
        page.wait("image attachment selected", "!!document.querySelector('.composer .attachment-chip img')").await;
        page.fill(".composer textarea", "Read the attached phone photo and change the preview to blue.").await;
        page.capture(&out.join(format!("{profile}-attachment.png"))).await;
        page.click("document.querySelector('.composer button[type=submit]')").await;
        let digest: String = ring::digest::digest(&ring::digest::SHA256, &base64::engine::general_purpose::STANDARD.decode(IMAGE).unwrap()).as_ref().iter().map(|byte| format!("{byte:02x}")).collect();
        page.wait("photo reaches scripted model", &format!("document.body.innerText.includes({}) && document.querySelector('main.session')?.dataset.status === 'idle'", json!(format!("media image/png:{digest}")))).await;
        // A person-owned preview survives Task completion; terminal Tasks intentionally close their tabs.
        let preview = harness.request(
            "POST", &format!("/v1/sessions/{session}/tools/browser_open_tab"),
            Some(json!({"args":{"url":remote.blue_preview}})),
        ).await;
        assert_eq!(preview.status, 200, "{}", preview.body);
        page.wait("blue browser preview pixels", "(() => { const i=document.querySelector('.browser-card img'); if(!i?.naturalWidth)return false; const c=document.createElement('canvas'); c.width=i.naturalWidth;c.height=i.naturalHeight; const x=c.getContext('2d');x.drawImage(i,0,0);const p=x.getImageData(300,200,1,1).data;return p[2]>180 && p[0]<30;})()").await;
        page.capture(&out.join(format!("{profile}-completed.png"))).await;
        page.call("Emulation.setEmulatedMedia", json!({"features":[{"name":"prefers-color-scheme","value":"dark"},{"name":"prefers-reduced-motion","value":"reduce"}]})).await;
        page.capture(&out.join(format!("{profile}-completed-dark.png"))).await;
        let events = store.events_after(&openside_proto::SessionId::new(&session), 0, usize::MAX).unwrap();
        assert!(events.iter().any(|event| matches!(&event.kind, EventKind::MessageQueued { attachments, .. } if attachments.iter().any(|attachment| attachment.name.as_deref() == Some("phone-photo.png") && attachment.sha256 == digest))));
        let badge_evidence = if badge_supported {"native_badge_calls"} else {"badge_unavailable"};
        eprintln!("MOBILE_REMOTE {} {profile} PASS qr_pixels pair person_device history chief_thread live_events approval pip_pixels image_model {badge_evidence} service_worker_notification", transport.name());
        eprintln!("MOBILE_REMOTE {} {profile} NOT_RUN real_install OS_badge camera_scan WebKit GCM_APNs: headless Chromium and decrypted fake-push/CDP delivery only", transport.name());
    }).catch_unwind().await;
    let shutdown = browser.shutdown().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
    shutdown.unwrap();
}

async fn journey(transport: Transport) {
    std::env::set_var(
        "DBUS_SESSION_BUS_ADDRESS",
        "unix:path=/nonexistent/openside-mobile-e2e-bus",
    );
    let dist = std::env::var_os("OPENSIDE_MOBILE_DIST")
        .map_or_else(
            || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web/dist"),
            PathBuf::from,
        )
        .canonicalize()
        .expect("build the production web bundle");
    assert!(dist.join("index.html").is_file());
    let out = std::env::var_os("OPENSIDE_MOBILE_CAPTURES")
        .map_or_else(
            || PathBuf::from("/tmp/openside-qa/round2/MobileE2E"),
            PathBuf::from,
        )
        .join(transport.name());
    std::fs::create_dir_all(&out).unwrap();
    let executable = openside_config::discover_browser(&openside_config::Config::default())
        .expect("mobile E2E needs Chromium");
    let site_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let site_origin = format!(
        "http://localhost:{}",
        site_listener.local_addr().unwrap().port()
    );
    let _site = Task(tokio::spawn(async move {
        axum::serve(site_listener, Router::new()
        .route("/red",get(||async{Html("<!doctype html><title>Phone red preview</title><body style='margin:0;background:rgb(224,0,0)'>Red phone preview")}))
        .route("/blue",get(||async{Html("<!doctype html><title>Phone blue preview</title><body style='margin:0;background:rgb(0,0,224)'>Blue phone preview")}))).await.unwrap();
    }));
    let mut responses = Vec::new();
    let mut subagents = serde_json::Map::new();
    for profile in ["iphone", "android"] {
        responses.extend([
            json!({"parts":[{"type":"tool_call","name":"create_task","id":format!("{profile}-preview"),"args":{"goal":format!("Prepare {profile} phone preview")}}]}),
            send(&format!("Phone {profile} preview task started.")),
            send(&format!("Phone {profile} approval resumed the same task.")),
            silent(),
            send("{{media_digest}}"),
        ]);
        subagents.insert(format!("{profile}-preview"), json!([
            {"parts":[{"type":"tool_call","name":"browser_open_tab","id":format!("{profile}-open"),"args":{"url":format!("{site_origin}/red")}}]},
            {"parts":[{"type":"tool_call","name":"request_human_step","id":format!("{profile}-confirm"),"args":{"kind":"confirm","summary":"Approve this remote phone task"}}]},
            {"parts":[{"type":"text","text":"The person approved the phone preview."}]}
        ]));
    }
    let script = json!({"delay_ms":400,"responses":responses,"subagents":subagents});
    let mut harness = Harness::prepare(DAEMON, &script).expect("uv sync --project agent required");
    let push = FakePushService::spawn().await.unwrap();
    harness.append_config(&format!(
        "[daemon]\nlisten = \"127.0.0.1:0\"\n[ui]\ndir = {}\n[browser]\nexecutable = {}\n[network]\nallow = [\"http://{}\"]\n",
        json!(dist),
        json!(executable),
        push.addr()
    ));
    let _relay = if matches!(transport, Transport::Relay) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let token = harness.root().join("relay-token");
        std::fs::write(&token, REGISTRATION).unwrap();
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
        harness.append_config(&format!(
            "[remote]\nmode = \"relay\"\n[remote.relay]\nurl = {}\ntoken_file = {}\n",
            json!(origin),
            json!(token)
        ));
        Some(Task(tokio::spawn(async move {
            axum::serve(
                listener,
                server::router(ServerConfig {
                    daemon_tokens: vec![REGISTRATION.into()],
                    web_dir: Some(dist),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        })))
    } else {
        // A pre-existing named tunnel must not hijack the disposable Quick Tunnel's ingress.
        let cloudflared = harness.root().join("home/.cloudflared");
        std::fs::create_dir_all(&cloudflared).unwrap();
        std::fs::write(
            cloudflared.join("config.yml"),
            "ingress:\n  - service: http_status:404\n",
        )
        .unwrap();
        harness.append_config(
            "[remote]\nmode = \"cloudflare\"\n[remote.cloudflare]\ntunnel = \"quick\"\n",
        );
        None
    };
    harness.launch().await;
    let origin = ready(&harness, transport).await;
    assert_eq!(
        harness
            .request("PUT", "/v1/onboarding", Some(json!({"state":"skipped"})))
            .await
            .status,
        200
    );
    assert_eq!(harness.request("POST","/v1/sessions",Some(json!({"title":"Existing conversation","mode":"full","workspace":harness.workspace()}))).await.status,201);
    let bootstrap = harness
        .request("GET", "/v1/remote/local-access", None)
        .await;
    assert_eq!(bootstrap.status, 200, "{}", bootstrap.body);
    let mut config = BrowserConfig::new(&executable);
    config.profile_dir = Some(harness.root().join("desktop-profile"));
    let (browser, _) = Browser::launch(config).await.unwrap();
    let outcome = std::panic::AssertUnwindSafe(async {
        let tab = browser
            .create_agent_tab(OWNER, bootstrap.body["url"].as_str().unwrap())
            .await
            .unwrap()
            .id;
        let mut desktop = Cdp::attach(&browser, &tab).await;
        desktop
            .call(
                "Emulation.setDeviceMetricsOverride",
                json!({"width":1440,"height":900,"deviceScaleFactor":1,"mobile":false}),
            )
            .await;
        desktop
            .wait(
                "desktop local bootstrap",
                "!!document.querySelector('.shell') && location.hash === ''",
            )
            .await;
        let port = std::fs::read_to_string(harness.data_dir.join("opensided.port")).unwrap();
        browser
            .navigate(
                OWNER,
                &tab,
                &format!("http://127.0.0.1:{}/remote", port.trim()),
            )
            .await
            .unwrap();
        let remote = Remote { transport, origin, blue_preview: format!("{site_origin}/blue") };
        for profile in ["iphone", "android"] {
            phone(
                &harness,
                &executable,
                &mut desktop,
                &remote,
                profile,
                &out,
                &push,
            )
            .await;
        }
        desktop.call("Emulation.setEmulatedMedia", json!({"features":[{"name":"prefers-color-scheme","value":"dark"},{"name":"prefers-reduced-motion","value":"reduce"}]})).await;
        desktop.capture(&out.join("desktop-connected-dark.png")).await;
    })
    .catch_unwind()
    .await;
    let shutdown = browser.shutdown().await;
    harness.kill().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
    shutdown.unwrap();
}

#[tokio::test]
#[ignore = "requires production web bundle, Chromium, Python sidecar and zbarimg; gate runs explicitly"]
async fn relay_phone_qr_chat_approval_preview_attachment_and_notification() {
    journey(Transport::Relay).await;
}

#[tokio::test]
#[ignore = "requires cloudflared and explicit public-network opt-in; never runs in the hermetic gate"]
async fn cloudflare_quick_phone_qr_chat_approval_preview_attachment_and_notification() {
    assert_eq!(
        std::env::var("OPENSIDE_MOBILE_CLOUDFLARE").as_deref(),
        Ok("1"),
        "set OPENSIDE_MOBILE_CLOUDFLARE=1 to authorize a disposable public Quick Tunnel"
    );
    journey(Transport::Cloudflare).await;
}
