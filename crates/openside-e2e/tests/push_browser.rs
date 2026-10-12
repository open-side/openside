//! F-2 on desktop Chromium: the push payload the daemon sends, decrypted by a 127.0.0.1
//! push service, is delivered to the real service worker (`web/public/sw.js`), which shows
//! a notification whose click target is the ticket's deep link.
//!
//! Chromium cannot subscribe to a real push service offline, so the push service leg is the
//! fake (VAPID-verified, RFC 8291-decrypted) and the browser leg is DevTools'
//! `ServiceWorker.deliverPushMessage` with exactly the decrypted bytes.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{path::Path, time::Duration};

use futures_util::{SinkExt, StreamExt};
use openside_browser::{Browser, BrowserConfig};
use openside_e2e::Harness;
use openside_push::fake_push::FakePushService;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Cdp {
    socket: Socket,
    next: u64,
    events: Vec<Value>,
}

impl Cdp {
    async fn call(&mut self, session: Option<&str>, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        let mut request = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            request["sessionId"] = json!(session);
        }
        self.socket
            .send(Message::text(request.to_string()))
            .await
            .unwrap();
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(15), self.socket.next())
                .await
                .unwrap_or_else(|_| panic!("{method} timed out"))
                .unwrap()
                .unwrap();
            let Message::Text(text) = frame else { continue };
            let message: Value = serde_json::from_str(&text).unwrap();
            if message["id"] == id {
                assert!(message.get("error").is_none(), "{method}: {message}");
                return message["result"].clone();
            }
            self.events.push(message);
        }
    }

    /// Read until an event satisfies `found`, keeping earlier ones.
    async fn event(&mut self, found: impl Fn(&Value) -> Option<Value>) -> Value {
        if let Some(hit) = self.events.iter().find_map(&found) {
            return hit;
        }
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(15), self.socket.next())
                .await
                .expect("CDP event timed out")
                .unwrap()
                .unwrap();
            let Message::Text(text) = frame else { continue };
            let message: Value = serde_json::from_str(&text).unwrap();
            if let Some(hit) = found(&message) {
                return hit;
            }
            self.events.push(message);
        }
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[tokio::test]
async fn chromium_service_worker_shows_the_pushed_ticket_with_its_deep_link() {
    let executable = match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(path) => path,
        Err(reason) => {
            eprintln!("SKIP: push delivery to a browser requires Chromium: {reason}");
            return;
        }
    };
    let Some(mut harness) = Harness::prepare(
        env!("CARGO_BIN_EXE_e2e-opensided"),
        &json!({"responses":[
            {"parts":[{"type":"tool_call","name":"ask_user","id":"ask-call",
                "args":{"question":"Ship it?","options":["yes","no"]}}]},
            {"parts":[{"type":"text","text":"shipped"}]}
        ]}),
    ) else {
        return;
    };
    // The real service worker and manifest, plus a page that only registers the worker
    // (the CSP forbids inline script, so it is a separate file).
    let ui = harness.root().join("ui");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/public"),
        &ui,
    );
    std::fs::write(
        ui.join("index.html"),
        "<!doctype html><title>push</title><script src=\"/register.js\"></script>\n",
    )
    .unwrap();
    std::fs::write(
        ui.join("register.js"),
        "window.__ready = navigator.serviceWorker.register('/sw.js').then(() => navigator.serviceWorker.ready);\n",
    )
    .unwrap();
    let push = FakePushService::spawn().await.unwrap();
    harness.append_config(&format!(
        "[ui]\ndir = {}\n[network]\nallow = [\"http://{}\"]\n",
        json!(ui),
        push.addr()
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
            Some(json!({"name":"browser","scopes":["read","approve"]})),
        )
        .await;
    let token = paired.body["token"].as_str().unwrap().to_owned();
    let created = harness
        .tcp_request(
            "POST",
            "/v1/push/subscriptions",
            Some(&token),
            Some(push.subscribe().to_json()),
        )
        .await;
    assert_eq!(created.status, 201, "{}", created.body);

    // A notification daemon on the user's session bus (mako, quickshell...) takes the notification
    // over from Chromium and the page can no longer list it, and the run would pop real notifications
    // on the developer's desktop. This binary holds one test, so setting the env var here is race-free.
    std::env::set_var(
        "DBUS_SESSION_BUS_ADDRESS",
        "unix:path=/nonexistent/openside-e2e-bus",
    );
    let (browser, _events) = Browser::launch(BrowserConfig::new(executable))
        .await
        .unwrap();
    let tab = browser
        .create_agent_tab("push-e2e", &format!("{origin}/"))
        .await
        .unwrap();
    let scope = browser
        .evaluate(
            &tab.id,
            "window.__ready.then((registration) => registration.scope)",
        )
        .await
        .unwrap();
    assert_eq!(scope, json!(format!("{origin}/")));

    let (socket, _) = tokio_tungstenite::connect_async(browser.endpoint().ws_url)
        .await
        .unwrap();
    let mut cdp = Cdp {
        socket,
        next: 0,
        events: Vec::new(),
    };
    let targets = cdp.call(None, "Target.getTargets", json!({})).await;
    let page = targets["targetInfos"]
        .as_array()
        .unwrap()
        .iter()
        .find(|target| {
            target["type"] == "page"
                && target["url"]
                    .as_str()
                    .is_some_and(|url| url.starts_with(&origin))
        })
        .cloned()
        .expect("page target");
    // Agent tabs live in the browser's default context; the permission check below proves
    // the grant reached the page.
    cdp.call(
        None,
        "Browser.grantPermissions",
        json!({"permissions":["notifications"],"origin":origin}),
    )
    .await;
    let attached = cdp
        .call(
            None,
            "Target.attachToTarget",
            json!({"targetId": page["targetId"], "flatten": true}),
        )
        .await;
    let session = attached["sessionId"].as_str().unwrap().to_owned();
    cdp.call(Some(&session), "ServiceWorker.enable", json!({}))
        .await;
    let scope_url = format!("{origin}/");
    let registration = cdp
        .event(|message| {
            (message["method"] == "ServiceWorker.workerRegistrationUpdated").then_some(())?;
            message["params"]["registrations"]
                .as_array()?
                .iter()
                .find(|r| r["scopeURL"] == scope_url.as_str() && r["isDeleted"] == false)
                .map(|r| r["registrationId"].clone())
        })
        .await;
    assert_eq!(
        browser
            .evaluate(&tab.id, "Notification.permission")
            .await
            .unwrap(),
        json!("granted")
    );

    let output = harness
        .cli(
            env!("CARGO_BIN_EXE_e2e-openside"),
            &["--json", "ship it"],
            None,
        )
        .await;
    assert!(output.status.success());
    let deliveries = push.wait_for(1, Duration::from_secs(15)).await;
    assert_eq!(deliveries.len(), 1, "no push: {:?}", push.rejections());
    let payload = String::from_utf8(deliveries[0].plaintext.clone()).unwrap();
    let link = deliveries[0].json()["url"].as_str().unwrap().to_owned();
    assert!(
        link.starts_with("/sessions/") && link.contains("?ticket="),
        "{link}"
    );
    cdp.call(
        Some(&session),
        "ServiceWorker.deliverPushMessage",
        json!({"origin": origin, "registrationId": registration, "data": payload}),
    )
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let shown = loop {
        let shown = browser
            .evaluate(
                &tab.id,
                "navigator.serviceWorker.ready.then((r) => r.getNotifications()).then((all) => all.map((n) => ({title: n.title, body: n.body, url: n.data && n.data.url})))",
            )
            .await
            .unwrap();
        if shown.as_array().is_some_and(|all| !all.is_empty())
            || tokio::time::Instant::now() > deadline
        {
            break shown;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(shown[0]["title"], "Question from the agent", "{shown}");
    assert_eq!(shown[0]["url"], json!(link));
    drop(cdp);
    browser.shutdown().await.unwrap();
    harness.kill().await;
}
