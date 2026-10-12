//! Frames, shadow DOM, XPath and strict-mode diagnostics in the REPL, against real Chromium.
//!
//! The top page is served from 127.0.0.1 and embeds a same-origin frame and a frame from
//! `localhost` on the same port: a different site, so Chromium runs it out of process.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use axum::{response::Html, routing::get, Router};
use openside_browser::{Browser, BrowserConfig};
use openside_repl::{run, Gate, Guard, Limits, Output, Verdict};
use serde_json::{json, Value};
use tokio::{sync::Mutex, task::JoinHandle};

const OWNER: &str = "repl-frames";
static SERIAL: Mutex<()> = Mutex::const_new(());

const INNER: &str = r#"<!doctype html><title>TITLE</title>
<button id="b" onclick="this.textContent = 'clicked'">Go</button>
<input id="i" aria-label="Name">
<span id="out"></span>
<script>
  document.getElementById('i').addEventListener('input', (event) => {
    document.getElementById('out').textContent = event.target.value;
  });
</script>"#;

struct Server {
    port: u16,
    task: JoinHandle<()>,
}

impl Server {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let top = format!(
            r#"<!doctype html><title>Top</title>
<button id="top" onclick="this.textContent = 'top clicked'">Go</button>
<ul><li>A</li><li>B</li></ul>
<div id="host"></div>
<script>
  const root = document.getElementById('host').attachShadow({{ mode: 'open' }});
  root.innerHTML = '<button class="sh" onclick="this.textContent = \'pierced\'">Shadow</button><span>deep text</span>';
  const closed = document.createElement('div');
  document.body.append(closed);
  closed.attachShadow({{ mode: 'closed' }}).innerHTML = '<span class="sealed">sealed</span>';
</script>
<iframe name="same" src="/same" style="display:block;width:400px;height:160px;border:0;margin:20px 0 0 30px"></iframe>
<iframe name="cross" src="http://localhost:{port}/cross" style="display:block;width:400px;height:160px;border:0;margin:20px 0 0 60px"></iframe>"#
        );
        let idle = format!(
            r#"<!doctype html><title>Idle</title>
<iframe name="late" src="http://localhost:{port}/idle-frame"></iframe>"#
        );
        let app = Router::new()
            .route("/", get(move || async move { Html(top) }))
            .route("/same", get(|| async { Html(INNER.replace("TITLE", "Same")) }))
            .route(
                "/cross",
                get(|| async { Html(INNER.replace("TITLE", "Cross")) }),
            )
            .route("/idle", get(move || async move { Html(idle) }))
            .route(
                "/idle-frame",
                get(|| async {
                    Html(
                        r#"<!doctype html><p id="done"></p>
<script>
  setTimeout(() => fetch('/slow').then(() => { document.getElementById('done').textContent = 'done'; }), 300);
</script>"#,
                    )
                }),
            )
            .route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                    "ok"
                }),
            );
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { port, task }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct AllowAll;

impl Guard for AllowAll {
    fn check(&self, _gate: &Gate<'_>) -> Verdict {
        Verdict::Allow
    }
}

/// Denies reading or acting in documents of the `localhost` origin, top or frame.
struct DenyLocalhost;

impl Guard for DenyLocalhost {
    fn check(&self, gate: &Gate<'_>) -> Verdict {
        let denied = |origin: &str| origin.contains("://localhost");
        match gate {
            Gate::Page { origin, frame, .. } | Gate::Script { origin, frame, .. }
                if denied(origin) || frame.is_some_and(|frame| denied(frame.origin)) =>
            {
                Verdict::Deny("localhost is not allowed".into())
            }
            _ => Verdict::Allow,
        }
    }
}

struct Fixture {
    browser: Arc<Browser>,
    tab: String,
    server: Server,
}

impl Fixture {
    async fn start() -> Option<Self> {
        let executable = match openside_config::discover_browser(&openside_config::Config::default())
        {
            Ok(executable) => executable,
            Err(error) => {
                eprintln!(
                    "skip: no chromium-compatible browser found (tried {})",
                    error.tried.join(", ")
                );
                return None;
            }
        };
        let (browser, _events) = Browser::launch(BrowserConfig::new(executable))
            .await
            .expect("launch chromium");
        let browser = Arc::new(browser);
        let tab = browser
            .create_agent_tab(OWNER, "about:blank")
            .await
            .expect("agent tab")
            .id;
        Some(Self {
            browser,
            tab,
            server: Server::start().await,
        })
    }

    async fn run_with(&self, code: &str, guard: Arc<dyn Guard>) -> Output {
        let code = format!(
            "const BASE = 'http://127.0.0.1:{}';\n{code}",
            self.server.port
        );
        run(
            Arc::clone(&self.browser),
            OWNER.into(),
            self.tab.clone(),
            code,
            guard,
            Limits::default(),
        )
        .await
    }

    async fn value(&self, code: &str) -> Value {
        let output = self.run_with(code, Arc::new(AllowAll)).await;
        assert_eq!(output.error, None, "{code}: {output:?}");
        output.return_value
    }

    async fn error(&self, code: &str, guard: Arc<dyn Guard>) -> String {
        let output = self.run_with(code, guard).await;
        match output.error {
            Some(error) => error,
            None => panic!("{code}: expected an error, got {:?}", output.return_value),
        }
    }

    async fn open(&self) {
        // The frames load after the top document; wait until both have their button.
        self.value(
            "await page.goto(BASE + '/'); \
             await page.frameLocator('iframe[name=same]').locator('#b').waitFor(); \
             await page.frameLocator('iframe[name=cross]').locator('#b').waitFor(); null",
        )
        .await;
    }

    async fn stop(self) {
        drop(self.server);
        let browser = Arc::try_unwrap(self.browser)
            .ok()
            .expect("REPL runs released the browser");
        browser.shutdown().await.expect("shutdown chromium");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn frame_locators_act_inside_same_and_cross_origin_frames() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    fixture.open().await;

    // Page locators stay in the top document; frame locators enter one frame's document.
    assert_eq!(
        fixture
            .value("[await page.locator('#b').count(), await page.frameLocator('iframe[name=same]').locator('#b').count()]")
            .await,
        json!([0, 1])
    );

    // Clicks and typing reach elements of the same-process and the out-of-process frame.
    assert_eq!(
        fixture
            .value(
                "const same = page.frameLocator('iframe[name=same]'); \
                 const cross = page.locator('iframe[name=cross]').contentFrame(); \
                 await same.getByRole('button', { name: 'Go' }).click(); \
                 await cross.locator('#b').click(); \
                 await cross.getByLabel('Name').fill('kim'); \
                 await same.locator('#i').pressSequentially('lee'); \
                 [await same.locator('#b').textContent(), await cross.locator('#b').textContent(), \
                  await cross.locator('#out').textContent(), await same.locator('#out').textContent(), \
                  await page.locator('#top').textContent()]"
            )
            .await,
        json!(["clicked", "clicked", "kim", "lee", "Go"])
    );

    // Page frame listing and lookup by name or URL.
    assert_eq!(
        fixture
            .value(
                "const frames = await page.frames(); \
                 const cross = page.frame({ url: /\\/cross$/ }); \
                 [frames.length, frames.map((frame) => frame.name()), frames[2].url().endsWith('/cross'), \
                  frames[1].parentFrame() === frames[0], frames[0].childFrames().length, \
                  await page.frame('same').title(), await cross.title(), \
                  await frames[2].locator('#b').textContent(), page.mainFrame().url() === page.url(), \
                  await page.mainFrame().locator('#top').count()]"
            )
            .await,
        json!([3, ["", "same", "cross"], true, true, 2, "Same", "Cross", "clicked", true, 1])
    );

    // Frame content is part of the snapshot, and its refs act in the frame.
    let snapshot = fixture.value("await page.snapshot()").await;
    let snapshot = snapshot.as_str().unwrap();
    assert!(snapshot.contains("\"Same\""), "{snapshot}");
    assert!(snapshot.contains("\"Cross\""), "{snapshot}");
    assert_eq!(
        fixture
            .value(
                "const snap = await page.snapshot(); \
                 const line = snap.split('\\n').findIndex((text) => text.includes('\"Cross\"')); \
                 const ref = /\\[(e\\d+)\\] textbox \"Name\"/.exec(snap.split('\\n').slice(line).join('\\n'))[1]; \
                 await page.locator('aria-ref=' + ref).fill('ref'); \
                 await page.frameLocator('iframe[name=cross]').locator('#out').textContent()"
            )
            .await,
        json!("ref")
    );
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shadow_dom_xpath_and_strict_mode_candidates() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    fixture.open().await;

    // Open shadow roots are pierced by CSS, text and role; closed ones are not.
    assert_eq!(
        fixture
            .value(
                "await page.getByRole('button', { name: 'Shadow' }).click(); \
                 [await page.locator('.sh').textContent(), await page.locator('#host').locator('.sh').count(), \
                  await page.getByText('deep text').count(), await page.locator('.sealed').count()]"
            )
            .await,
        json!(["pierced", 1, 1, 0])
    );

    // XPath, with the page or a frame document as the context.
    assert_eq!(
        fixture
            .value(
                "[await page.locator('//li').count(), await page.locator('xpath=//li[2]').textContent(), \
                  await page.locator('ul').locator('xpath=./li[1]').textContent(), \
                  await page.frameLocator('iframe[name=cross]').locator('//button').count()]"
            )
            .await,
        json!([2, "B", "A", 1])
    );

    // An ambiguous locator lists its candidates, including the frame they are in.
    let error = fixture
        .error("await page.locator('li').click()", Arc::new(AllowAll))
        .await;
    assert!(error.contains("resolved to 2 elements"), "{error}");
    assert!(error.contains("1) <li>A</li>"), "{error}");
    assert!(error.contains("2) <li>B</li>"), "{error}");
    let error = fixture
        .error(
            "await page.frame('same').locator('button, input').click()",
            Arc::new(AllowAll),
        )
        .await;
    assert!(
        error.contains("<button id=\"b\">Go</button> in frame \"same\""),
        "{error}"
    );
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn frame_origins_are_authorized_on_their_own() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    fixture.open().await;

    // Acting in a denied frame stops the script before any input reaches it.
    let error = fixture
        .error(
            "await page.frameLocator('iframe[name=cross]').locator('#b').click()",
            Arc::new(DenyLocalhost),
        )
        .await;
    assert!(error.contains("localhost is not allowed"), "{error}");
    assert_eq!(
        fixture
            .value("await page.frameLocator('iframe[name=cross]').locator('#b').textContent()")
            .await,
        json!("Go")
    );

    // The allowed frame stays usable, and the snapshot leaves the denied frame's content out.
    let output = fixture
        .run_with(
            "await page.frameLocator('iframe[name=same]').locator('#b').click(); await page.snapshot()",
            Arc::new(DenyLocalhost),
        )
        .await;
    assert_eq!(output.error, None, "{output:?}");
    let snapshot = output.return_value.as_str().unwrap().to_owned();
    assert!(snapshot.contains("\"Same\""), "{snapshot}");
    assert!(!snapshot.contains("\"Cross\""), "{snapshot}");
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_idle_counts_requests_of_out_of_process_frames() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    // The frame's slow request starts after the top document has loaded; network idle must
    // wait for it, so its result is there as soon as the navigation returns.
    assert_eq!(
        fixture
            .value(
                "await page.goto(BASE + '/idle', { waitUntil: 'networkidle' }); \
                 await page.frame('late').locator('#done').textContent()"
            )
            .await,
        json!("done")
    );
    fixture.stop().await;
}
