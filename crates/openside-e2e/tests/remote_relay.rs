//! F-8 acceptance with disposable daemons, a local relay and headless Chromium.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    os::unix::fs::PermissionsExt,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use axum::{response::Html, routing::get, Router};
use futures_util::{FutureExt, SinkExt, StreamExt};
use openside_browser::{Browser, BrowserConfig};
use openside_e2e::Harness;
use openside_relay::server::{self, ServerConfig};
use serde_json::{json, Value};

const DAEMON: &str = env!("CARGO_BIN_EXE_e2e-opensided");
const REGISTRATION: &str = "private-f8-relay-registration-token";
const DIRECT_SHELL_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:; connect-src 'self'; manifest-src 'self'; worker-src 'self'; font-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'; frame-src 'self' blob:; frame-ancestors 'none'";
const RELAY_SHELL_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'; manifest-src 'self'; worker-src 'self'; font-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'; frame-src 'self' blob:; frame-ancestors 'none'";

type DebugSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn debug_command(
    socket: &mut DebugSocket,
    id: u64,
    session: Option<&str>,
    method: &str,
    params: Value,
) -> Value {
    let mut message = json!({"id":id,"method":method,"params":params});
    if let Some(session) = session {
        message["sessionId"] = json!(session);
    }
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            message.to_string().into(),
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let message = socket
                .next()
                .await
                .expect("debugger remains connected")
                .unwrap();
            if let tokio_tungstenite::tungstenite::Message::Text(text) = message {
                let response: Value = serde_json::from_str(&text).unwrap();
                if response["id"] == id {
                    assert!(response.get("error").is_none(), "{response}");
                    return response["result"].clone();
                }
            }
        }
    })
    .await
    .expect("debugger response")
}

struct Page<'a> {
    browser: &'a Browser,
    tab: String,
}

impl Page<'_> {
    async fn eval(&self, expression: &str) -> Value {
        self.browser
            .evaluate(&self.tab, expression)
            .await
            .unwrap_or_else(|error| panic!("evaluate {expression}: {error:?}"))
    }

    async fn wait(&self, description: &str, predicate: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(40);
        loop {
            if self.eval(predicate).await == true {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "waiting for {description}: {}",
                self.eval("document.body.innerText").await
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn preview_pixel(&self, selector: &str) -> Value {
        use base64::Engine;
        self.eval(&format!(
            "document.querySelector({}).scrollIntoView({{block:'center',behavior:'instant'}}); true",
            json!(selector)
        )).await;
        // Two animation frames do not wait out the files sheet's opacity/transform transitions.
        self.wait("preview surface animations settled", &format!(
            "(() => {{ const frame = document.querySelector({}); if (!frame) return false; for (let el = frame; el; el = el.parentElement) {{ if (el.hasAttribute('data-starting-style') || el.getAnimations().some(a => a.playState === 'running' || a.pending)) return false; }} return true; }})()",
            json!(selector)
        )).await;
        self.eval("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve(true))))").await;
        let rect = self.eval(&format!(
            "(() => {{ const f = document.querySelector({}); const r = f.getBoundingClientRect(); const x = r.left + 30, y = r.top + 30; return {{x,y,visible:document.elementFromPoint(x,y) === f}}; }})()",
            json!(selector)
        )).await;
        assert_eq!(
            rect["visible"], true,
            "preview pixel must be visible: {selector} {rect}"
        );
        let png = self.browser.screenshot(&self.tab).await.unwrap();
        let image = base64::engine::general_purpose::STANDARD.encode(png);
        self.eval(&format!(
            "(async () => {{ const image = new Image(); image.src = 'data:image/png;base64,{image}'; await image.decode(); const canvas = document.createElement('canvas'); canvas.width=image.width; canvas.height=image.height; const ctx=canvas.getContext('2d'); ctx.drawImage(image,0,0); const scale=image.width/innerWidth; return Array.from(ctx.getImageData(Math.round({}*scale),Math.round({}*scale),1,1).data).slice(0,3); }})()",
            rect["x"], rect["y"]
        )).await
    }

    async fn frame_color(&self, selector: &str) -> Value {
        let source = self
            .eval(&format!("document.querySelector({}).src", json!(selector)))
            .await;
        let (mut socket, _) = tokio_tungstenite::connect_async(self.browser.endpoint().ws_url)
            .await
            .unwrap();
        let targets = debug_command(&mut socket, 1, None, "Target.getTargets", json!({})).await;
        let iframe = targets["targetInfos"]
            .as_array()
            .unwrap()
            .iter()
            .find(|target| target["type"] == "iframe" && target["url"] == source);
        let target = iframe.map_or(self.tab.as_str(), |target| {
            target["targetId"].as_str().unwrap()
        });
        let attached = debug_command(
            &mut socket,
            2,
            None,
            "Target.attachToTarget",
            json!({"targetId":target,"flatten":true}),
        )
        .await;
        let session = attached["sessionId"].as_str().unwrap();
        let mut params = json!({
            "expression":"document.querySelector('div') ? getComputedStyle(document.querySelector('div')).backgroundColor : document.body?.innerText",
            "returnByValue":true
        });
        if iframe.is_none() {
            let tree = debug_command(
                &mut socket,
                3,
                Some(session),
                "Page.getFrameTree",
                json!({}),
            )
            .await;
            let violations = self.eval("window.previewViolations ?? []").await;
            let frame = tree["frameTree"]["childFrames"].as_array().expect("preview child frame").iter()
                .find(|frame| frame["frame"]["url"] == source)
                .unwrap_or_else(|| panic!("preview frame URL {source}; tree {tree}; targets {targets}; CSP {violations}"));
            let world = debug_command(
                &mut socket,
                4,
                Some(session),
                "Page.createIsolatedWorld",
                json!({"frameId":frame["frame"]["id"],"worldName":"preview-style-proof"}),
            )
            .await;
            params["contextId"] = world["executionContextId"].clone();
        }
        let evaluated =
            debug_command(&mut socket, 5, Some(session), "Runtime.evaluate", params).await;
        assert!(evaluated.get("exceptionDetails").is_none(), "{evaluated}");
        socket.close(None).await.unwrap();
        evaluated["result"]["value"].clone()
    }
}

async fn wait_status(harness: &Harness, transport: &str, state: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let response = harness.request("GET", "/v1/remote/status", None).await;
            assert_eq!(response.status, 200, "{}", response.body);
            if response.body[transport]["state"] == state {
                return response.body;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("remote transport ready")
}

#[tokio::test]
async fn pwa_pairs_lists_streams_and_takes_over_through_the_encrypted_relay() {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../web/dist")
        .canonicalize()
        .expect("run pnpm -C web build before relay acceptance");
    assert!(dist.join("index.html").is_file());
    let executable = openside_config::discover_browser(&openside_config::Config::default())
        .expect("headless Chromium is required for relay acceptance");
    let mut harness = Harness::prepare(DAEMON, &json!({"responses":[]}))
        .expect("scripted Python sidecar is required for relay acceptance");
    harness.append_config(&format!("[ui]\ndir = {}\n", json!(dist)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_origin = format!("http://{}", listener.local_addr().unwrap());
    let relay = tokio::spawn(async move {
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
    });
    let token_file = harness.root().join("relay-token");
    std::fs::write(&token_file, REGISTRATION).unwrap();
    std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    harness.append_config(&format!(
        "[browser]\nexecutable = {}\n[remote]\nmode = [\"relay\"]\n[remote.relay]\nurl = {}\ntoken_file = {}\n",
        json!(executable), json!(relay_origin), json!(token_file)
    ));
    harness.launch().await;
    wait_status(&harness, "relay", "connected").await;
    let port = std::fs::read_to_string(harness.data_dir.join("opensided.port")).unwrap();
    let daemon_shell = reqwest::get(format!("http://127.0.0.1:{}/", port.trim()))
        .await
        .unwrap();
    assert_eq!(
        daemon_shell.headers()["content-security-policy"],
        DIRECT_SHELL_CSP
    );
    let relay_shell = reqwest::get(&relay_origin).await.unwrap();
    assert_eq!(
        relay_shell.headers()["content-security-policy"],
        RELAY_SHELL_CSP
    );
    assert_eq!(
        harness
            .request("PUT", "/v1/onboarding", Some(json!({"state":"skipped"})))
            .await
            .status,
        200
    );

    let probes = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let site_origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let probe_count = Arc::clone(&probes);
    let site = tokio::spawn(async move {
        axum::serve(listener, Router::new()
            .route("/", get(|| async { Html("<!doctype html><title>Relay screen</title><h1>Relay screencast fixture</h1><input aria-label='Human input'>") }))
            .route("/probe", get(move || {
                let count = Arc::clone(&probe_count);
                async move { count.fetch_add(1, Ordering::SeqCst); "unexpected preview request" }
            }))).await.unwrap();
    });
    let created = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({
                "title":"Relay acceptance", "mode":"full", "workspace":harness.workspace()
            })),
        )
        .await;
    assert_eq!(created.status, 201, "{}", created.body);
    let session = created.body["id"].as_str().unwrap().to_owned();
    let opened = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/tools/browser_open_tab"),
            Some(json!({"args":{"url":format!("{site_origin}/")}})),
        )
        .await;
    assert_eq!(opened.status, 200, "{}", opened.body);
    assert_eq!(opened.body["status"], "done", "{}", opened.body);
    let controlled = opened.body["result"]["tab_id"].as_str().unwrap();
    let hostile = format!(
        "<div style='width:300px;height:100px;background:#2f6fed'>Relay preview</div><script>parent.document.body.dataset.compromised='yes';fetch('{site_origin}/probe')</script><img src='{site_origin}/probe'><meta http-equiv='refresh' content='0;url={site_origin}/probe'>"
    );
    let answer = format!("Relay stream completed.\n```html visual\n{hostile}\n```");
    harness.replace_script(&json!({"responses":[
        {"parts":[{"type":"tool_call","name":"write","id":"relay-artifact","args":{"path":"report.html","content":hostile}}]},
        {"parts":[{"type":"tool_call","name":"request_human_step","id":"relay-screen","args":{"kind":"takeover","summary":"Complete the relay screen","tab_id":controlled}}]},
        {"parts":[{"type":"text","text":answer}]}
    ]}));

    let mut ui_config = BrowserConfig::new(executable);
    // Keeping the UI profile inside the harness lets its subreaper identify adopted renderers.
    ui_config.profile_dir = Some(harness.root().join("relay-ui-profile"));
    let (browser, _events) = Browser::launch(ui_config).await.unwrap();
    // Assertion failures must still await the standalone UI browser's shutdown; Drop kills
    // its process group but cannot synchronously reap the Tokio child.
    let outcome = std::panic::AssertUnwindSafe(async {
    let tab = browser.create_agent_tab("relay-pwa-acceptance", &format!("{relay_origin}/")).await.unwrap();
    let page = Page { browser: &browser, tab: tab.id };
    page.wait("relay pairing instructions", "document.body.innerText.includes('openside pair --via relay')").await;
    assert_eq!(page.eval("document.querySelector('input[type=password]') === null").await, true);
    let offer = harness.request("POST", "/v1/pairing", Some(json!({
        "name":"Relay PWA", "scopes":["read","steer","approve","human_step"], "via":"relay"
    }))).await;
    assert_eq!(offer.status, 201, "{}", offer.body);
    let link = offer.body["url"].as_str().unwrap();
    assert!(link.starts_with(&format!("{relay_origin}/pair#c=")));
    browser.navigate("relay-pwa-acceptance", &page.tab, link).await.unwrap();
    page.wait("paired chief shell", "!!document.querySelector('.shell') && !!document.querySelector('a[href=\"/history\"]')").await;
    page.eval("document.querySelector('a[href=\"/history\"]').click(); true").await;
    page.wait("paired session history", "!!document.querySelector('.history-list') && document.body.innerText.includes('Relay acceptance')").await;
    assert_eq!(page.eval("location.hash === '' && JSON.parse(localStorage.getItem('openside.transport')).kind === 'relay'").await, true);
    assert_eq!(harness.request("POST", "/v1/pairing/exchange", Some(json!({"code":offer.body["code"]}))).await.status, 404);
    page.eval(&format!("Array.from(document.querySelectorAll('a')).find(a => a.getAttribute('href') === {}).click()", json!(format!("/sessions/{session}")))).await;
    page.wait("relay session SSE", "!!document.querySelector('.composer textarea')").await;
    page.eval(r#"(() => {
        const form = document.querySelector('.composer');
        const input = form.querySelector('textarea');
        Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(input, 'Show the relay screen');
        input.dispatchEvent(new Event('input', {bubbles:true}));
        return true;
    })()"#).await;
    page.wait("enabled send", "!!document.querySelector('.composer button[type=submit]:not(:disabled)')").await;
    page.eval("document.querySelector('.composer button[type=submit]').click()").await;
    page.wait("takeover Card streamed", "!!document.querySelector('.oc-card[data-kind=takeover][data-status=pending] .oc-pill')").await;
    page.eval("document.querySelector('.oc-card[data-kind=takeover][data-status=pending] .oc-pill').click(); true").await;
    page.wait("ticket over SSE", "Array.from(document.querySelectorAll('button')).some(b => b.textContent.trim() === 'Open remote screen')").await;
    page.eval("Array.from(document.querySelectorAll('button')).find(b => b.textContent.trim() === 'Open remote screen').click()").await;
    page.wait("relayed takeover screencast", "(() => { const img = document.querySelector('.takeover-screen img'); return !!img && img.complete && img.naturalWidth > 0; })()").await;
    assert_eq!(page.eval("document.querySelector('.takeover-screen').dataset.frameWidth").await, "1440");
    assert_eq!(page.eval("!!document.querySelector('.takeover button[aria-label=\"Close\"]')").await, true);
    page.eval("Array.from(document.querySelectorAll('.takeover button')).find(b => b.textContent.trim() === 'Done').click()").await;
    page.wait("same run resumed over SSE", "document.body.innerText.includes('Relay stream completed.') && document.querySelector('main.session')?.dataset.status === 'idle'").await;
    page.wait("completed takeover receipt and dismissed screen", "document.querySelector('.takeover') === null && !!document.querySelector('.oc-card[data-kind=takeover][data-status=done]')").await;
    page.wait("visual preview via blob", "!!document.querySelector('figure.visual iframe[src^=\"blob:\"]')").await;
    assert_eq!(page.eval("document.querySelector('figure.visual iframe').getAttribute('sandbox')").await, "");
    assert_eq!(page.eval("document.querySelector('figure.visual iframe').contentDocument === null").await, true);
    page.wait("files pill", "!!document.querySelector('.files-pill')").await;
    page.eval("document.querySelector('.files-pill').click()").await;
    page.wait("artifact list", "Array.from(document.querySelectorAll('.files-open')).some(b => b.textContent.includes('report.html'))").await;
    page.eval("Array.from(document.querySelectorAll('.files-open')).find(b => b.textContent.includes('report.html')).click()").await;
    page.wait("artifact preview via blob", "!!document.querySelector('.artifact-preview iframe[src^=\"blob:\"]')").await;
    assert_eq!(page.eval("document.querySelector('.artifact-preview iframe').getAttribute('sandbox')").await, "");
    // Waiting for both frame loads makes the network/script assertions exercise rendering, not just URL creation.
    page.eval("window.__previewLoaded = 0; document.querySelectorAll('iframe.sandbox-frame').forEach(f => f.addEventListener('load', () => window.__previewLoaded++)); true").await;
    page.eval("document.querySelectorAll('iframe.sandbox-frame').forEach(f => f.src = f.src); true").await;
    page.wait("sandbox frame loads", "window.__previewLoaded >= 2").await;
    assert_eq!(page.frame_color(".artifact-preview iframe").await, "rgb(47, 111, 237)", "artifact preview styles must survive the relay");
    assert_eq!(page.preview_pixel(".artifact-preview iframe").await, json!([47, 111, 237]), "artifact preview must render, not merely load a blob URL");
    page.eval("document.querySelector('.tray-sheet button[aria-label=\"Close\"]').click()").await;
    page.wait("files sheet dismissed", "document.querySelector('.tray-sheet') === null").await;
    assert_eq!(page.frame_color("figure.visual iframe").await, "rgb(47, 111, 237)", "inline preview styles must survive the relay");
    assert_eq!(page.preview_pixel("figure.visual iframe").await, json!([47, 111, 237]), "visual preview must render, not merely load a blob URL");
    assert_eq!(page.eval("!document.body.dataset.compromised && !!localStorage.getItem('openside.token')").await, true);
    assert_eq!(probes.load(Ordering::SeqCst), 0, "preview escaped its CSP/sandbox");
    }).catch_unwind().await;
    let browser_shutdown = browser.shutdown().await;
    harness.kill().await;
    site.abort();
    relay.abort();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
    browser_shutdown.unwrap();
}

fn fake_binary(harness: &Harness, body: &str) -> std::path::PathBuf {
    let path = harness.root().join("fake-cloudflared");
    std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path
}

#[tokio::test]
async fn cloudflared_quick_url_restart_backoff_and_shutdown_are_supervised() {
    let mut harness =
        Harness::prepare(DAEMON, &json!({"responses":[]})).expect("scripted sidecar environment");
    let root = harness.root().display().to_string();
    let bin = fake_binary(
        &harness,
        &format!(
            "printf '%s\\n' \"$*\" >> '{root}/cloudflare-args'\n\
         date +%s%N >> '{root}/cloudflare-times'\n\
         echo $$ > '{root}/cloudflare-pid'\n\
         n=$(wc -l < '{root}/cloudflare-times')\n\
         if [ \"$n\" -le 2 ]; then exit 3; fi\n\
         echo 'INF https://f8-quick.trycloudflare.com' >&2\nexec sleep 60\n"
        ),
    );
    harness.append_config(&format!("[daemon]\nlisten = \"127.0.0.1:0\"\n[remote]\nmode = \"cloudflare\"\n[remote.cloudflare]\nbin = {}\n", json!(bin)));
    harness.launch().await;
    let status = wait_status(&harness, "cloudflare", "running").await;
    assert_eq!(
        status["cloudflare"]["url"],
        "https://f8-quick.trycloudflare.com"
    );
    assert_eq!(status["cloudflare"]["restarts"], 2);
    let times: Vec<u128> = std::fs::read_to_string(harness.root().join("cloudflare-times"))
        .unwrap()
        .lines()
        .map(|line| line.parse().unwrap())
        .collect();
    assert_eq!(times.len(), 3);
    assert!(
        times[1] - times[0] >= 900_000_000,
        "first backoff less than 1 second"
    );
    assert!(
        times[2] - times[1] >= 1_900_000_000,
        "second backoff less than 2 seconds"
    );
    let offer = harness
        .request(
            "POST",
            "/v1/pairing",
            Some(json!({"name":"quick phone","scopes":["read"],"via":"cloudflare"})),
        )
        .await;
    assert_eq!(offer.status, 201);
    assert!(offer.body["url"]
        .as_str()
        .unwrap()
        .starts_with("https://f8-quick.trycloudflare.com/pair#"));
    let args = std::fs::read_to_string(harness.root().join("cloudflare-args")).unwrap();
    assert!(args
        .lines()
        .all(|line| line.starts_with("tunnel --no-autoupdate --url http://127.0.0.1:")));
    let pid: i32 = std::fs::read_to_string(harness.root().join("cloudflare-pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    harness.kill().await;
    assert!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err(),
        "supervised child survived shutdown"
    );
}

#[tokio::test]
async fn cloudflared_named_token_stays_out_of_argv_and_child_exits_on_shutdown() {
    let mut harness =
        Harness::prepare(DAEMON, &json!({"responses":[]})).expect("scripted sidecar environment");
    let root = harness.root().display().to_string();
    let bin = fake_binary(
        &harness,
        &format!(
            "printf '%s\\n' \"$*\" > '{root}/cloudflare-args'\n\
         printf '%s' \"$TUNNEL_TOKEN\" > '{root}/cloudflare-env'\n\
         echo $$ > '{root}/cloudflare-pid'\n\
         echo 'INF Registered tunnel connection connIndex=0' >&2\nexec sleep 60\n"
        ),
    );
    let token = "F8-NAMED-TUNNEL-SECRET-CANARY";
    let token_file = harness.root().join("named-token");
    std::fs::write(&token_file, token).unwrap();
    std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    harness.append_config(&format!(
        "[daemon]\nlisten = \"127.0.0.1:0\"\n[remote]\nmode = \"cloudflare\"\n[remote.cloudflare]\ntunnel = \"named\"\nhostname = \"desk.example.test\"\ntoken_file = {}\nbin = {}\n", json!(token_file), json!(bin)
    ));
    harness.launch().await;
    let status = wait_status(&harness, "cloudflare", "running").await;
    assert_eq!(status["cloudflare"]["url"], "https://desk.example.test");
    assert_eq!(
        std::fs::read_to_string(harness.root().join("cloudflare-args"))
            .unwrap()
            .trim(),
        "tunnel --no-autoupdate run"
    );
    assert_eq!(
        std::fs::read_to_string(harness.root().join("cloudflare-env")).unwrap(),
        token
    );
    let pid: i32 = std::fs::read_to_string(harness.root().join("cloudflare-pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    #[cfg(target_os = "linux")]
    let argv = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap();
    #[cfg(not(target_os = "linux"))]
    let argv = {
        let output = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "command="])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "could not inspect the owned child process"
        );
        output.stdout
    };
    assert!(!String::from_utf8_lossy(&argv).contains(token));
    assert!(!serde_json::to_string(&status).unwrap().contains(token));
    assert!(
        !std::fs::read_to_string(harness.root().join("daemon.stderr"))
            .unwrap()
            .contains(token)
    );
    harness.kill().await;
    assert!(nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err());
}
