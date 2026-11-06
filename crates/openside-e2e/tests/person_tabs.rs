//! The person's own browser: a headless Chromium (the patched openside browser when
//! `OPENSIDE_PATCHED_BROWSER` names it) with the openside extension, paired to a real daemon.
//! The agent lists the person's tabs, attaches one, reads it and clicks in it.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::result_large_err)]

use futures_util::{SinkExt, StreamExt};
use openside_e2e::Harness;
use openside_proto::EventKind;
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

const EXTENSION_ORIGIN: &str = "chrome-extension://jneddlbpgmlheplolfonafkbkbkjhcfj";

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Open the bridge the way the extension does, with any token and Origin.
async fn bridge(
    port: &str,
    token: &str,
    origin: &str,
) -> Result<Socket, tokio_tungstenite::tungstenite::Error> {
    let mut request = format!("ws://127.0.0.1:{port}/v1/browser/person/bridge")
        .into_client_request()
        .unwrap();
    let headers = request.headers_mut();
    headers.insert(
        "sec-websocket-protocol",
        format!("openside.v1, openside.bearer.{token}")
            .parse()
            .unwrap(),
    );
    headers.insert("origin", origin.parse().unwrap());
    tokio_tungstenite::connect_async(request)
        .await
        .map(|(socket, _)| socket)
}

fn refusal_status(result: Result<Socket, tokio_tungstenite::tungstenite::Error>) -> u16 {
    match result {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => response.status().as_u16(),
        Err(error) => panic!("unexpected bridge error: {error}"),
        Ok(_) => panic!("the bridge accepted a connection it must refuse"),
    }
}

async fn hello(socket: &mut Socket, run: &str) {
    let frame = json!({"kind":"hello","protocol":"openside.person-browser","version":1,"run":run});
    socket.send(Message::text(frame.to_string())).await.unwrap();
}

const PAGE: &str = "<!doctype html><title>Order 42</title><h1>Order 42</h1><p>Ready to ship.</p>\
<button onclick=\"document.title='Marked done'\">Mark done</button>";

struct Cdp {
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    next: u64,
}

impl Cdp {
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

    async fn call(&mut self, session: Option<&str>, method: &str, params: Value) -> Value {
        let message = self.send(session, method, params).await;
        assert!(message.get("error").is_none(), "{method}: {message}");
        message["result"].clone()
    }

    /// `None` while the target has no JavaScript context yet.
    async fn evaluate(&mut self, session: &str, expression: &str) -> Option<Value> {
        let message = self
            .send(
                Some(session),
                "Runtime.evaluate",
                json!({"expression":expression,"awaitPromise":true,"returnByValue":true}),
            )
            .await;
        let result = message.get("result")?;
        if result.get("exceptionDetails").is_some() {
            return None;
        }
        Some(result["result"]["value"].clone())
    }
}

/// Serves the fixture page on its own loopback port: a site other than the daemon's.
async fn fixture_site() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{PAGE}",
                    PAGE.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    format!("http://{address}/order")
}

/// One finished tool call: runtime id, whether it failed, and its redacted result.
type Ended = (String, bool, String);

/// Send a message to the scripted agent and return the tool calls of that run.
async fn turn(harness: &Harness, session: &str, after: &mut u64, message_id: &str) -> Vec<Ended> {
    let mut stream = harness.events(session, *after).await;
    let submitted = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            Some(json!({"message_id":message_id,"text":"Use my open tab","mode":"now"})),
        )
        .await;
    assert_eq!(submitted.status, 202, "{}", submitted.body);
    let events = stream.settled().await;
    if let Some(last) = events.last() {
        *after = last.seq;
    }
    events
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::ToolEnded {
                tool_call_id,
                result,
                is_error,
                ..
            } => Some((tool_call_id, is_error, result.to_string())),
            _ => None,
        })
        .collect()
}

fn ended<'a>(calls: &'a [Ended], id: &str) -> &'a Ended {
    calls
        .iter()
        .find(|(call, _, _)| call.ends_with(id))
        .unwrap_or_else(|| panic!("no tool call {id} in {calls:?}"))
}

fn call(name: &str, args: Value, id: &str) -> Value {
    json!({"parts":[{"type":"tool_call","name":name,"args":args,"id":id}]})
}

/// Polls `$check` (an `Option`-valued expression) until it yields a value.
macro_rules! until {
    ($what:expr, $check:expr) => {{
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(value) = $check {
                break value;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out: {}",
                $what
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }};
}

#[tokio::test]
async fn the_agent_lists_attaches_reads_and_acts_in_a_person_tab() {
    let executable = match std::env::var_os("OPENSIDE_PATCHED_BROWSER") {
        Some(path) => PathBuf::from(path),
        None => match openside_config::discover_browser(&openside_config::Config::default()) {
            Ok(path) => path,
            Err(reason) => {
                eprintln!("SKIP: person-tab E2E needs Chromium: {reason}");
                return;
            }
        },
    };
    let Some(mut harness) = Harness::prepare(
        env!("CARGO_BIN_EXE_e2e-opensided"),
        &json!({"responses":[]}),
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
            Some(json!({"name":"person-browser","scopes":["read","steer","approve"]})),
        )
        .await;
    assert_eq!(paired.status, 201, "{}", paired.body);
    let token = paired.body["token"].as_str().unwrap().to_owned();

    let page = fixture_site().await;
    let profile = harness.root().join("person-profile");
    let extension = build_dir.to_str().unwrap();
    let mut browser = tokio::process::Command::new(&executable)
        .args([
            "--headless=new".to_owned(),
            format!("--user-data-dir={}", profile.display()),
            "--remote-debugging-port=0".to_owned(),
            "--no-first-run".to_owned(),
            "--no-default-browser-check".to_owned(),
            format!("--load-extension={extension}"),
            format!("--disable-extensions-except={extension}"),
            page.clone(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("launch the person's browser");

    let endpoint = until!(
        "DevToolsActivePort",
        std::fs::read_to_string(profile.join("DevToolsActivePort"))
            .ok()
            .and_then(|text| {
                let mut lines = text.lines();
                Some(format!("ws://127.0.0.1:{}{}", lines.next()?, lines.next()?))
            })
    );
    let (socket, _) = tokio_tungstenite::connect_async(endpoint).await.unwrap();
    let mut cdp = Cdp { socket, next: 0 };

    // Pair the extension the way its options page does: the token lives in session storage.
    let mut checked = std::collections::HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let worker = 'found: loop {
        let targets = cdp.call(None, "Target.getTargets", json!({})).await;
        for target in targets["targetInfos"].as_array().unwrap() {
            let id = target["targetId"].as_str().unwrap().to_owned();
            let ours = target["type"] == "service_worker"
                && target["url"]
                    .as_str()
                    .is_some_and(|url| url.ends_with("/background.js"));
            if !ours || checked.contains(&id) {
                continue;
            }
            let session = cdp
                .call(
                    None,
                    "Target.attachToTarget",
                    json!({"targetId":id,"flatten":true}),
                )
                .await["sessionId"]
                .as_str()
                .unwrap()
                .to_owned();
            match cdp
                .evaluate(&session, "chrome.runtime.getManifest().name")
                .await
            {
                Some(name) if name == "openside (test build)" => break 'found session,
                Some(_) => {
                    checked.insert(id);
                }
                None => {}
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the browser did not load the openside extension"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let stored = cdp
        .evaluate(
            &worker,
            &format!(
                "chrome.storage.session.set({{pairing: {{url: {}, token: {}}}}}).then(() => true)",
                json!(origin),
                json!(token)
            ),
        )
        .await;
    assert_eq!(stored, Some(json!(true)));

    // The person's shell sees the live tab list once the extension connects.
    let tab_id = until!("the extension connects and reports the fixture tab", {
        let tabs = harness
            .request("GET", "/v1/browser/person/tabs", None)
            .await;
        assert_eq!(tabs.status, 200, "{}", tabs.body);
        tabs.body["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tab| tab["url"] == page && tab["title"] == "Order 42")
            .filter(|_| tabs.body["connected"] == true)
            .map(|tab| tab["tab_id"].as_i64().unwrap())
    });
    // An agent token never reaches the person's browser.
    let agent = harness
        .request(
            "POST",
            "/v1/devices/pair",
            Some(json!({"name":"agent","scopes":["read","steer"],"kind":"agent_client"})),
        )
        .await;
    let agent_token = agent.body["token"].as_str().unwrap();
    let refused = harness
        .tcp_request("GET", "/v1/browser/person/tabs", Some(agent_token), None)
        .await;
    assert_eq!(refused.status, 403, "{}", refused.body);

    // The agent's side, through the scripted model; the tab id is known only now, and the
    // sidecar reads its script when the chat's first message starts it.
    let target = json!({"tab_id": tab_id});
    harness.replace_script(&json!({"responses":[
        call("browser_user_tab_snapshot", target.clone(), "early"),
        call("browser_user_tabs", json!({}), "list"),
        call("browser_user_tab_attach", target.clone(), "attach"),
        call("browser_user_tab_snapshot", target.clone(), "read"),
        // The only control on the page is the first ref of its outline.
        call("browser_user_tab_act", json!({"tab_id": tab_id, "action": "click", "ref": "u1"}), "act"),
        {"parts":[{"type":"text","text":"Marked it done."}]},
        call("browser_user_tab_snapshot", target.clone(), "after"),
        {"parts":[{"type":"text","text":"That tab is no longer shared with me."}]},
    ]}));
    let created = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"mode":"full","workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(created.status, 201, "{}", created.body);
    let session = created.body["id"].as_str().unwrap().to_owned();
    let mut seen = 0;
    let calls = turn(&harness, &session, &mut seen, "first").await;

    // Unattached tabs cannot be read.
    assert!(ended(&calls, "early").1, "{calls:?}");
    let (_, failed, listed) = ended(&calls, "list");
    assert!(!failed && listed.contains("Order 42"), "{listed}");
    let (_, failed, attached) = ended(&calls, "attach");
    assert!(!failed && attached.contains("Order 42"), "{attached}");
    let chips = harness
        .request(
            "GET",
            &format!("/v1/sessions/{session}/attached-tabs"),
            None,
        )
        .await;
    assert_eq!(chips.status, 200);
    assert_eq!(chips.body["tabs"][0]["tab_id"], tab_id, "{}", chips.body);
    assert_eq!(chips.body["tabs"][0]["attached_by"], "agent");
    let (_, failed, snapshot) = ended(&calls, "read");
    assert!(!failed, "{snapshot}");
    for needle in [
        "Order 42",
        "Ready to ship.",
        "Mark done",
        "ref=u1",
        "untrusted",
    ] {
        assert!(
            snapshot.contains(needle),
            "{needle} missing from {snapshot}"
        );
    }
    let (_, failed, acted) = ended(&calls, "act");
    assert!(!failed && acted.contains("untrusted"), "{acted}");
    until!("the click reaches the person's page", {
        let tabs = harness
            .request("GET", "/v1/browser/person/tabs", None)
            .await;
        tabs.body["tabs"]
            .as_array()
            .is_some_and(|tabs| {
                tabs.iter()
                    .any(|tab| tab["tab_id"] == tab_id && tab["title"] == "Marked done")
            })
            .then_some(())
    });

    // The person removes the chip; the agent loses access.
    let removed = harness
        .request(
            "DELETE",
            &format!("/v1/sessions/{session}/attached-tabs/{tab_id}"),
            None,
        )
        .await;
    assert_eq!(removed.status, 204, "{}", removed.body);
    let calls = turn(&harness, &session, &mut seen, "second").await;
    assert!(ended(&calls, "after").1, "{calls:?}");

    // The bridge through the real auth middleware.
    let other = harness
        .request(
            "POST",
            "/v1/devices/pair",
            Some(json!({"name":"other-person-device","scopes":["read","steer","approve"]})),
        )
        .await;
    let other_token = other.body["token"].as_str().unwrap().to_owned();
    // A forged or missing extension Origin is refused before any upgrade.
    assert_eq!(
        refusal_status(bridge(&port, &other_token, "https://evil.example").await),
        403
    );
    // Agent tokens never reach the person's browser, whatever Origin they claim.
    assert_eq!(
        refusal_status(bridge(&port, agent_token, EXTENSION_ORIGIN).await),
        403
    );
    // Another paired device is refused while the extension's link is live.
    let mut intruder = bridge(&port, &other_token, EXTENSION_ORIGIN).await.unwrap();
    hello(&mut intruder, "intruder").await;
    let closed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match intruder.next().await {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(
        closed.is_ok(),
        "a second device took over the person's browser link"
    );
    let still = harness
        .request("GET", "/v1/browser/person/tabs", None)
        .await;
    assert!(
        still.body["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tab| tab["tab_id"] == tab_id),
        "{}",
        still.body
    );
    // The same device (a reconnecting extension) replaces its own link.
    let mut same = bridge(&port, &token, EXTENSION_ORIGIN).await.unwrap();
    hello(&mut same, "replacement-run").await;
    let answering = tokio::spawn(async move {
        while let Some(Ok(frame)) = same.next().await {
            let Message::Text(text) = frame else { continue };
            let request: Value = serde_json::from_str(&text).unwrap();
            if request["method"] == "tabs.list" {
                let reply = json!({"id": request["id"], "result": {"focused_window_id": null, "tabs": [], "groups": []}});
                same.send(Message::text(reply.to_string())).await.unwrap();
                return same;
            }
        }
        panic!("the replacement link never received a request");
    });
    until!("the same device's new link answers", {
        let tabs = harness
            .request("GET", "/v1/browser/person/tabs", None)
            .await;
        (tabs.body["connected"] == true && tabs.body["tabs"] == json!([])).then_some(())
    });
    drop(answering.await.unwrap());
    // The real extension reconnects on its own and takes the link back.
    until!("the extension reconnects", {
        let tabs = harness
            .request("GET", "/v1/browser/person/tabs", None)
            .await;
        tabs.body["tabs"]
            .as_array()
            .is_some_and(|tabs| tabs.iter().any(|tab| tab["tab_id"] == tab_id))
            .then_some(())
    });

    drop(cdp);
    browser.kill().await.unwrap();
}
