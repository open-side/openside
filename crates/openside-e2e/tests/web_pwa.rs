//! F-7: the installed web app behaves like a native one, checked in headless Chromium emulating
//! a 390x844 touch phone against a real daemon serving the production bundle.
//!
//! - The phone's share sheet POSTs a title, text, URL and an image to `/share-target`; the
//!   service worker (`web/public/sw.js`) answers the POST, the app opens a new chat with the
//!   shared text in the composer and the image attached, and sending it creates the conversation
//!   and delivers the attachment. A photo bigger than the daemon's 2 MiB attachment budget is
//!   shrunk to fit.
//! - A message typed while the daemon is unreachable is queued in IndexedDB and delivered once
//!   the connection is back, creating the conversation exactly once.
//! - Manifest shortcuts and the install banner render on the phone viewport.
//!
//! `OPENSIDE_PWA_DIST` overrides the bundle directory (default `web/dist`) so a run can use a
//! private build. Captures go to `OPENSIDE_PWA_CAPTURES` (default
//! `/tmp/openside-qa/polish/PwaNative/after`).

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use openside_browser::{Browser, BrowserConfig};
use openside_e2e::Harness;
use serde_json::{json, Value};
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};

const OWNER: &str = "web-pwa";
const DAEMON: &str = env!("CARGO_BIN_EXE_e2e-opensided");
const MAX_ATTACHMENT_BYTES: u64 = 2 * 1024 * 1024;

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// One DevTools session on the page; it holds the phone emulation for as long as it lives.
struct Cdp {
    socket: Socket,
    session: String,
    next: u64,
}

impl Cdp {
    async fn attach(ws_url: &str, tab: &str) -> Self {
        let (socket, _) = tokio_tungstenite::connect_async(ws_url).await.unwrap();
        let mut cdp = Self {
            socket,
            session: String::new(),
            next: 0,
        };
        let attached = cdp
            .call(
                None,
                "Target.attachToTarget",
                json!({"targetId":tab,"flatten":true}),
            )
            .await;
        cdp.session = attached["sessionId"].as_str().unwrap().to_owned();
        cdp
    }

    async fn call(&mut self, session: Option<&str>, method: &str, params: Value) -> Value {
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

    async fn page(&mut self, method: &str, params: Value) -> Value {
        let session = self.session.clone();
        self.call(Some(&session), method, params).await
    }

    async fn emulate_phone(&mut self) {
        self.page(
            "Emulation.setDeviceMetricsOverride",
            json!({"width":390,"height":844,"deviceScaleFactor":3,"mobile":true}),
        )
        .await;
        self.page(
            "Emulation.setTouchEmulationEnabled",
            json!({"enabled":true,"maxTouchPoints":5}),
        )
        .await;
        self.page("Network.enable", json!({})).await;
    }

    async fn offline(&mut self, offline: bool) {
        self.page(
            "Network.emulateNetworkConditions",
            json!({"offline":offline,"latency":0,"downloadThroughput":-1,"uploadThroughput":-1}),
        )
        .await;
    }
}

struct Ui {
    browser: Browser,
    tab: String,
    origin: String,
    captures: PathBuf,
}

impl Ui {
    async fn eval(&self, expression: &str) -> Value {
        self.browser
            .evaluate(&self.tab, expression)
            .await
            .unwrap_or_else(|e| panic!("evaluate {expression}: {e:?}"))
    }

    /// Polls `predicate`; an evaluation that fails because the page is mid-navigation counts as
    /// "not yet".
    async fn wait(&self, what: &str, predicate: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        loop {
            if matches!(
                self.browser.evaluate(&self.tab, predicate).await,
                Ok(Value::Bool(true))
            ) {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {what}; page: {}",
                self.browser
                    .evaluate(
                        &self.tab,
                        "location.href + ' :: ' + document.body.innerText.slice(0, 400)"
                    )
                    .await
                    .map(|v| v.to_string())
                    .unwrap_or_default()
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn goto(&self, path: &str) {
        self.browser
            .navigate(OWNER, &self.tab, &format!("{}{path}", self.origin))
            .await
            .unwrap();
        self.wait(
            "the app shell",
            "document.readyState === 'complete' && !!document.querySelector('.shell')",
        )
        .await;
    }

    /// Screenshot through the emulated-phone DevTools session; `Browser::screenshot` ignores the
    /// device override and returns the desktop viewport.
    async fn capture(&self, cdp: &mut Cdp, name: &str) {
        use base64::Engine as _;
        self.eval(
            "document.fonts.ready.then(() => new Promise((r) => setTimeout(() => r(true), 350)))",
        )
        .await;
        let shot = cdp
            .page(
                "Page.captureScreenshot",
                json!({"format":"png","clip":{"x":0,"y":0,"width":390,"height":844,"scale":1}}),
            )
            .await;
        let png = base64::engine::general_purpose::STANDARD
            .decode(shot["data"].as_str().unwrap())
            .unwrap();
        std::fs::create_dir_all(&self.captures).unwrap();
        std::fs::write(self.captures.join(format!("{name}.png")), png).unwrap();
    }

    /// What the phone's share sheet does: a multipart POST to the manifest's `share_target`.
    async fn share(&self, fields: Value, image: &str) {
        let script = format!(
            r#"(async () => {{
              const fields = {fields};
              const form = document.createElement('form');
              form.method = 'POST'; form.action = '/share-target'; form.enctype = 'multipart/form-data';
              for (const [name, value] of Object.entries(fields)) {{
                const input = document.createElement('input');
                input.type = 'hidden'; input.name = name; input.value = value; form.append(input);
              }}
              const files = new DataTransfer();
              {image}
              const picker = document.createElement('input');
              picker.type = 'file'; picker.name = 'files'; picker.files = files.files; form.append(picker);
              document.body.append(form);
              form.submit();
              return true;
            }})()"#
        );
        self.eval(&script).await;
    }
}

/// A 1x1 PNG.
const TINY_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

fn tiny_image() -> String {
    format!(
        "files.items.add(new File([Uint8Array.from(atob('{TINY_PNG}'), (c) => c.charCodeAt(0))], 'shot.png', {{ type: 'image/png' }}));"
    )
}

/// Random-noise PNG far above the attachment budget, like a phone photo.
const BIG_IMAGE: &str = r"
const canvas = document.createElement('canvas');
canvas.width = 1500; canvas.height = 1500;
const context = canvas.getContext('2d');
const pixels = context.createImageData(1500, 1500);
crypto.getRandomValues(pixels.data.subarray(0, 65536));
for (let i = 65536; i < pixels.data.length; i += 65536) pixels.data.copyWithin(i, 0, Math.min(65536, pixels.data.length - i));
for (let i = 3; i < pixels.data.length; i += 4) pixels.data[i] = 255;
context.putImageData(pixels, 0, 0);
const photo = await new Promise((resolve) => canvas.toBlob(resolve, 'image/png'));
files.items.add(new File([photo], 'IMG_2041.png', { type: 'image/png' }));
";

fn bundle() -> Option<PathBuf> {
    let dist = std::env::var_os("OPENSIDE_PWA_DIST").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist"),
        PathBuf::from,
    );
    dist.join("index.html")
        .is_file()
        .then(|| dist.canonicalize().unwrap())
}

#[tokio::test]
async fn phone_share_target_offline_outbox_and_install_surfaces() {
    let Some(dist) = bundle() else {
        eprintln!("SKIP: web UI bundle is missing; run pnpm -C web build");
        return;
    };
    let executable: PathBuf =
        match openside_config::discover_browser(&openside_config::Config::default()) {
            Ok(path) => path,
            Err(reason) => {
                eprintln!("SKIP: requires Chromium: {reason}");
                return;
            }
        };
    let reply = |text: &str| json!({"parts":[{"type":"text","text":text}]});
    let Some(mut harness) =
        Harness::prepare(DAEMON, &json!({"responses":[reply("Got it, thanks.")]}))
    else {
        eprintln!("SKIP: hermetic daemon prerequisites are missing");
        return;
    };
    harness.append_config(&format!("[ui]\ndir = {}\n", json!(dist)));
    harness.launch().await;
    let onboarding = harness
        .request("PUT", "/v1/onboarding", Some(json!({"state":"skipped"})))
        .await;
    assert_eq!(onboarding.status, 200, "{}", onboarding.body);

    let paired = harness
        .request(
            "POST",
            "/v1/devices/pair",
            Some(json!({"name":"pwa","scopes":["read","steer"]})),
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
    let captures = std::env::var_os("OPENSIDE_PWA_CAPTURES").map_or_else(
        || PathBuf::from("/tmp/openside-qa/polish/PwaNative/after"),
        PathBuf::from,
    );
    let ui = Ui {
        browser,
        tab: tab.clone(),
        origin: origin.clone(),
        captures,
    };
    let mut cdp = Cdp::attach(&ui.browser.endpoint().ws_url, &tab).await;
    cdp.emulate_phone().await;

    ui.wait("token screen", "!!document.querySelector('.token-entry')")
        .await;
    ui.eval(&format!(
        "localStorage.setItem('openside.token', {}); true",
        json!(token)
    ))
    .await;
    ui.goto("/").await;

    // The production bundle registers the worker; share and offline both depend on it controlling the page.
    ui.wait(
        "the service worker to control the page",
        "navigator.serviceWorker.ready.then(() => !!navigator.serviceWorker.controller)",
    )
    .await;

    // The manifest advertises everything the worker and the app handle.
    let manifest = ui
        .eval("fetch('/manifest.webmanifest').then((r) => r.json())")
        .await;
    assert_eq!(
        manifest["display_override"],
        json!(["window-controls-overlay", "standalone"])
    );
    assert_eq!(manifest["share_target"]["action"], "/share-target");
    assert_eq!(manifest["share_target"]["method"], "POST");
    let shortcuts: Vec<&str> = manifest["shortcuts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(shortcuts, ["New chat", "Needs you"]);
    let theme_colors = ui
        .eval("Array.from(document.querySelectorAll('meta[name=theme-color]')).map((m) => m.media)")
        .await;
    assert_eq!(
        theme_colors,
        json!([
            "(prefers-color-scheme: light)",
            "(prefers-color-scheme: dark)"
        ])
    );

    // 1. Share a page link with an image: title, text and URL become the draft, the image an attachment.
    ui.share(
        json!({"title":"Cool article","text":"Worth a read","url":"https://example.com/cool"}),
        &tiny_image(),
    )
    .await;
    ui.wait(
        "the shared draft in the new-chat composer",
        "location.pathname === '/' && location.search === '' && (() => { const t = document.querySelector('.composer textarea'); \
           return !!t && t.value === 'Cool article\\nWorth a read\\nhttps://example.com/cool' \
             && !!document.querySelector('.attachment-chip .attachment-thumb'); })()",
    )
    .await;
    ui.capture(&mut cdp, "share-draft-390x844").await;
    ui.eval("document.querySelector('.composer-send').click(); true")
        .await;
    ui.wait(
        "the shared message in a new conversation",
        "location.pathname.startsWith('/sessions/') && document.body.innerText.includes('https://example.com/cool') \
           && document.body.innerText.includes('Got it, thanks.')",
    )
    .await;
    ui.capture(&mut cdp, "share-sent-390x844").await;
    let sessions = harness.request("GET", "/v1/sessions", None).await;
    let shared = sessions.body["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == "Cool article")
        .unwrap_or_else(|| {
            panic!(
                "no conversation named after the shared title: {}",
                sessions.body
            )
        })
        .clone();
    let artifacts = harness
        .request(
            "GET",
            &format!("/v1/sessions/{}/artifacts", shared["id"].as_str().unwrap()),
            None,
        )
        .await;
    assert_eq!(
        artifacts.body["artifacts"][0]["source"], "attachment",
        "{}",
        artifacts.body
    );

    // 2. A photo over the 2 MiB budget is re-encoded to fit and still sends.
    ui.goto("/").await;
    ui.share(json!({"title":"","text":"","url":""}), BIG_IMAGE)
        .await;
    ui.wait(
        "the shrunk photo in the composer",
        "location.search === '' && !!document.querySelector('.attachment-chip .attachment-thumb') \
           && document.body.innerText.includes('IMG_2041.jpg')",
    )
    .await;
    ui.eval("document.querySelector('.composer-send').click(); true")
        .await;
    ui.wait("the photo conversation", "location.pathname.startsWith('/sessions/') && document.body.innerText.includes('Got it, thanks.')")
        .await;
    let sessions = harness.request("GET", "/v1/sessions", None).await;
    let photo = sessions.body["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == "IMG_2041.jpg")
        .unwrap_or_else(|| panic!("no photo conversation: {}", sessions.body))
        .clone();
    let artifacts = harness
        .request(
            "GET",
            &format!("/v1/sessions/{}/artifacts", photo["id"].as_str().unwrap()),
            None,
        )
        .await;
    let size = artifacts.body["artifacts"][0]["size"].as_u64().unwrap();
    assert!(
        size > 10_000 && size <= MAX_ATTACHMENT_BYTES,
        "attachment is {size} bytes: {}",
        artifacts.body
    );

    // 3. Offline: the message is queued in IndexedDB; the conversation exists only once the daemon is back.
    ui.goto("/").await;
    let before = harness.request("GET", "/v1/sessions", None).await.body["sessions"]
        .as_array()
        .unwrap()
        .len();
    cdp.offline(true).await;
    ui.eval(
        "(() => { const el = document.querySelector('.composer textarea'); \
           Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(el, 'Written in a tunnel'); \
           el.dispatchEvent(new Event('input', { bubbles: true })); return true })()",
    )
    .await;
    ui.eval("document.querySelector('.composer-send').click(); true")
        .await;
    ui.wait("the queued-message notice", "!!document.querySelector('.outbox-notice') && document.querySelector('.composer textarea').value === ''")
        .await;
    let queued = ui
        .eval(
            "new Promise((resolve) => { const open = indexedDB.open('openside-pwa'); \
               open.onsuccess = () => { const all = open.result.transaction('outbox').objectStore('outbox').getAll(); \
                 all.onsuccess = () => resolve(all.result.map((e) => ({ id: e.message_id, text: e.text, session: e.session_id }))); }; })",
        )
        .await;
    assert_eq!(queued.as_array().unwrap().len(), 1, "{queued}");
    assert_eq!(queued[0]["text"], "Written in a tunnel");
    assert!(queued[0]["session"].is_null());
    ui.capture(&mut cdp, "outbox-queued-390x844").await;
    assert_eq!(
        harness.request("GET", "/v1/sessions", None).await.body["sessions"]
            .as_array()
            .unwrap()
            .len(),
        before,
        "nothing reached the daemon while offline"
    );

    cdp.offline(false).await;
    ui.wait(
        "the queued message to be delivered and its chat opened",
        "location.pathname.startsWith('/sessions/') && document.body.innerText.includes('Written in a tunnel') \
           && document.body.innerText.includes('Got it, thanks.') && !document.querySelector('.outbox-notice')",
    )
    .await;
    ui.capture(&mut cdp, "outbox-delivered-390x844").await;
    let sessions = harness.request("GET", "/v1/sessions", None).await;
    let tunnel: Vec<_> = sessions.body["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["title"] == "Written in a tunnel")
        .collect();
    assert_eq!(
        tunnel.len(),
        1,
        "exactly one conversation: {}",
        sessions.body
    );

    // 4. The install banner appears on a phone once the browser offers installation.
    ui.goto("/").await;
    ui.eval(
        "(() => { const e = new Event('beforeinstallprompt', { cancelable: true }); \
           e.prompt = async () => undefined; e.userChoice = Promise.resolve({ outcome: 'dismissed' }); \
           window.dispatchEvent(e); return true })()",
    )
    .await;
    ui.wait(
        "the install banner",
        "!!document.querySelector('.install-banner')",
    )
    .await;
    ui.capture(&mut cdp, "install-banner-390x844").await;

    // 5. Swipe from the left edge opens the conversation drawer.
    ui.eval("document.querySelector('.install-banner button[aria-label=Dismiss]').click(); true")
        .await;
    cdp.page(
        "Input.dispatchTouchEvent",
        json!({"type":"touchStart","touchPoints":[{"x":6,"y":400}]}),
    )
    .await;
    for x in [30, 60, 100] {
        cdp.page(
            "Input.dispatchTouchEvent",
            json!({"type":"touchMove","touchPoints":[{"x":x,"y":402}]}),
        )
        .await;
    }
    cdp.page(
        "Input.dispatchTouchEvent",
        json!({"type":"touchEnd","touchPoints":[]}),
    )
    .await;
    ui.wait(
        "the drawer opened by the edge swipe",
        "!!document.querySelector('.ui-drawer')",
    )
    .await;
    ui.capture(&mut cdp, "edge-swipe-drawer-390x844").await;

    // 6. A notch and home bar are respected: header, composer dock and drawer keep clear of the insets.
    cdp.page(
        "Emulation.setSafeAreaInsetsOverride",
        json!({"insets":{"top":47,"bottom":34,"left":44,"right":44}}),
    )
    .await;
    let padding = ui
        .eval(
            "(() => { const px = (s, side) => parseFloat(getComputedStyle(document.querySelector(s))['padding' + side]); \
               return { drawerLeft: px('.ui-drawer', 'Left') } })()",
        )
        .await;
    assert!(
        padding["drawerLeft"].as_f64().unwrap() >= 44.0,
        "drawer ignores the left inset: {padding}"
    );
    ui.eval("document.querySelector('button[aria-label=\"Close conversations\"]').click(); true")
        .await;
    ui.wait(
        "the drawer to close",
        "!document.querySelector('.ui-drawer')",
    )
    .await;
    let padding = ui
        .eval(
            "(() => { const px = (s, side) => parseFloat(getComputedStyle(document.querySelector(s))['padding' + side]); \
               return { headerTop: px('.app-header', 'Top'), headerLeft: px('.app-header', 'Left'), \
                        headerRight: px('.app-header', 'Right'), dockLeft: px('.composer-dock', 'Left'), \
                        dockRight: px('.composer-dock', 'Right'), dockBottom: px('.composer-dock', 'Bottom') } })()",
        )
        .await;
    let at_least = |key: &str, floor: f64| {
        assert!(
            padding[key].as_f64().unwrap() >= floor,
            "{key} below {floor}: {padding}"
        )
    };
    at_least("headerTop", 47.0);
    at_least("headerLeft", 44.0);
    at_least("headerRight", 44.0);
    at_least("dockLeft", 44.0);
    at_least("dockRight", 44.0);
    at_least("dockBottom", 34.0);
    ui.capture(&mut cdp, "safe-area-390x844").await;

    drop(cdp);
    ui.browser.shutdown().await.unwrap();
    harness.kill().await;
}
