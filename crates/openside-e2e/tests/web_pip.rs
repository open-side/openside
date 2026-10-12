//! JPEG pixels, not merely WebSocket wiring, prove agent navigation reaches the read-only live
//! browser card.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::time::Duration;

use axum::{response::Html, routing::get, Router};
use openside_browser::{Browser, BrowserConfig};
use openside_e2e::Harness;
use serde_json::{json, Value};

const OWNER: &str = "preview-ui-test";
const DAEMON: &str = env!("CARGO_BIN_EXE_e2e-opensided");

struct Site(tokio::task::JoinHandle<()>);
impl Drop for Site {
    fn drop(&mut self) {
        self.0.abort();
    }
}
struct BrowserCleanup(u32);
impl Drop for BrowserCleanup {
    fn drop(&mut self) {
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(i32::try_from(self.0).unwrap()),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
}
async fn wait(browser: &Browser, tab: &str, predicate: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if browser.evaluate(tab, predicate).await.unwrap() == true {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out: {predicate}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
async fn submit(harness: &Harness, session: &str, message: &str) {
    let response = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            Some(json!({"message_id":message,"text":message,"mode":"queue"})),
        )
        .await;
    assert_eq!(response.status, 202, "{}", response.body);
    harness.events(session, 0).await.settled().await;
}

#[tokio::test]
async fn web_pip_updates_jpeg_pixels_when_the_agent_navigates_without_a_takeover_ticket() {
    let dist = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web/dist");
    if !dist.join("index.html").is_file() {
        eprintln!("SKIP preview UI: run pnpm -C web build");
        return;
    }
    let executable = match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(browser) => browser,
        Err(reason) => {
            eprintln!("SKIP preview UI requires Chromium: {reason}");
            return;
        }
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let site_origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let _site = Site(tokio::spawn(async move {
        axum::serve(listener,Router::new().route("/red",get(|| async { Html("<!doctype html><title>Red</title><body style='margin:0;background:rgb(224,0,0)'>Red") })).route("/blue",get(|| async { Html("<!doctype html><title>Blue</title><body style='margin:0;background:rgb(0,0,224)'>Blue") }))).await.unwrap();
    }));
    let script = json!({"responses":[
        {"parts":[{"type":"tool_call","name":"browser_open_tab","args":{"url":format!("{site_origin}/red")},"id":"red"}]},
        {"parts":[{"type":"text","text":"Red opened"}]},
        {"parts":[{"type":"tool_call","name":"browser_repl","args":{"code":format!("await page.goto({})",json!(format!("{site_origin}/blue")))},"id":"blue"}]},
        {"parts":[{"type":"text","text":"Blue opened"}]}
    ]});
    let Some(mut harness) = Harness::prepare(DAEMON, &script) else {
        return;
    };
    harness.append_config(&format!(
        "[ui]\ndir = {}\n[browser]\nexecutable = {}\n",
        json!(dist.canonicalize().unwrap()),
        json!(executable)
    ));
    harness.launch().await;
    let response = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"title":"PiP","mode":"full","workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(response.status, 201, "{}", response.body);
    let session = response.body["id"].as_str().unwrap().to_owned();
    submit(&harness, &session, "open red").await;
    let response = harness
        .request(
            "POST",
            "/v1/devices/pair",
            Some(json!({"name":"preview viewer","scopes":["read","browse"]})),
        )
        .await;
    assert_eq!(response.status, 201);
    let token = response.body["token"].as_str().unwrap();
    let port = std::fs::read_to_string(harness.data_dir.join("opensided.port")).unwrap();
    let origin = format!("http://127.0.0.1:{}", port.trim());
    let (browser, _) = Browser::launch(BrowserConfig::new(executable))
        .await
        .unwrap();
    let _cleanup = BrowserCleanup(browser.process_id());
    let ui = browser;
    let tab = ui.create_agent_tab(OWNER, &origin).await.unwrap().id;
    wait(&ui, &tab, "!!document.querySelector('.token-entry')").await;
    ui.evaluate(
        &tab,
        &format!(
            "localStorage.setItem('openside.token',{}); true",
            json!(token)
        ),
    )
    .await
    .unwrap();
    ui.navigate(OWNER, &tab, &format!("{origin}/sessions/{session}"))
        .await
        .unwrap();
    // The live browser card docks by itself once the conversation has used the browser.
    wait(
        &ui,
        &tab,
        "!!document.querySelector('.browser-card img')?.naturalWidth",
    )
    .await;
    const PIXEL: &str = "(()=>{const i=document.querySelector('.browser-card img');if(!i?.naturalWidth)return null;const c=document.createElement('canvas');c.width=i.naturalWidth;c.height=i.naturalHeight;const x=c.getContext('2d');x.drawImage(i,0,0);return Array.from(x.getImageData(500,300,1,1).data).slice(0,3)})()";
    let red = ui.evaluate(&tab, PIXEL).await.unwrap();
    assert!(
        red[0].as_u64().unwrap() > 180 && red[2].as_u64().unwrap() < 30,
        "{red}"
    );
    ui.evaluate(
        &tab,
        "window.__previewBefore=document.querySelector('.browser-card img').src; true",
    )
    .await
    .unwrap();
    submit(&harness, &session, "navigate blue").await;
    wait(&ui,&tab,"document.querySelector('.browser-card img')?.src!==window.__previewBefore && document.querySelector('.browser-card img')?.complete && !!document.querySelector('.browser-card img')?.naturalWidth").await;
    let blue = ui.evaluate(&tab, PIXEL).await.unwrap();
    assert!(
        blue[2].as_u64().unwrap() > 180 && blue[0].as_u64().unwrap() < 30,
        "{blue}"
    );
    let size: Value = ui.evaluate(&tab,"[document.querySelector('.browser-card img').naturalWidth,document.querySelector('.browser-card img').naturalHeight]").await.unwrap();
    assert_eq!(size, json!([720, 450]));
    // The card is named after the live tab from the tab ledger. `ledger` fetches
    // GET /v1/browser/tabs with this device's token and returns the title of the entry for the
    // blue page, so each check compares the UI with the API's current answer. The card may have
    // folded to its slim pill after the run; both forms carry the expand button.
    let blue_url = format!("{site_origin}/blue");
    let ledger = format!(
        "fetch('/v1/browser/tabs',{{headers:{{Authorization:'Bearer '+localStorage.getItem('openside.token')}}}}).then(r=>r.json()).then(b=>b.tabs.find(t=>t.url==={})?.title)",
        json!(blue_url)
    );
    const EXPAND: &str = "document.querySelector('.browser-card button[aria-label^=\"Expand \"]')";
    wait(&ui, &tab, &format!("{ledger}.then(t => t !== undefined && {EXPAND}?.getAttribute('aria-label') === 'Expand ' + t)")).await;
    // The ledger follows Target.targetInfoChanged, so it converges to the document title.
    wait(&ui, &tab, &format!("{ledger}.then(t => t === 'Blue')")).await;
    wait(
        &ui,
        &tab,
        &format!("{EXPAND}?.getAttribute('aria-label') === 'Expand Blue'"),
    )
    .await;
    ui.evaluate(&tab, &format!("{EXPAND}.click(); true"))
        .await
        .unwrap();
    // Expanded: the browser sheet shows the ledger's title and sanitized URL with the live frames.
    wait(
        &ui,
        &tab,
        &format!(
            "{ledger}.then(t => {{ const s = document.querySelector('.browser-sheet'); const i = s?.querySelector('.browser-sheet-screen img'); return t === 'Blue' && !document.querySelector('.browser-card') && s?.querySelector('.ui-dialog-title')?.textContent === t && s?.querySelector('.browser-sheet-url')?.textContent === {} && s?.querySelector('a.browser-sheet-open')?.getAttribute('href') === {} && !!i?.complete && i.naturalWidth === 720 }})",
            json!(blue_url),
            json!(blue_url)
        ),
    )
    .await;
    ui.evaluate(&tab,"document.querySelector('.browser-sheet button[aria-label=\"Zoom browser preview\"]').click(); true").await.unwrap();
    wait(
        &ui,
        &tab,
        "(() => { const i = document.querySelector('[role=dialog][aria-label=\"Live browser preview\"] img'); return !!i?.complete && i.naturalWidth === 720 })()",
    )
    .await;
    ui.evaluate(&tab,"document.querySelector('[role=dialog][aria-label=\"Live browser preview\"] button[aria-label=\"Close\"]').click(); true").await.unwrap();
    wait(
        &ui,
        &tab,
        "!document.querySelector('[role=dialog][aria-label=\"Live browser preview\"]')",
    )
    .await;
    ui.evaluate(
        &tab,
        "document.querySelector('.browser-sheet button[aria-label=\"Close\"]').click(); true",
    )
    .await
    .unwrap();
    wait(
        &ui,
        &tab,
        "!!document.querySelector('.browser-card img')?.naturalWidth",
    )
    .await;
    // Minimize unless the finished run already folded the card.
    ui.evaluate(
        &tab,
        "document.querySelector('.browser-card button[aria-label=\"Minimize\"]')?.click(); true",
    )
    .await
    .unwrap();
    wait(
        &ui,
        &tab,
        "!!document.querySelector('.browser-card[data-minimized] .browser-card-thumb img')",
    )
    .await;
    // Idle, × hides the card rather than cancelling anything.
    ui.evaluate(
        &tab,
        "document.querySelector('.browser-card button[aria-label=\"Close\"]').click(); true",
    )
    .await
    .unwrap();
    wait(&ui, &tab, "!document.querySelector('.browser-card')").await;
    ui.shutdown().await.unwrap();
    harness.kill().await;
}
