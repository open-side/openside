//! Page script in the REPL (ADR-022) against real Chromium: `evaluate`, handles, `$eval`/`$$eval`,
//! `locator.evaluate*`, `addInitScript`, raw `page.cdp`, their policy gate, recovery from a
//! script that never returns, and one call that batches driver actions with page script.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{response::Html, routing::get, Router};
use openside_browser::{Browser, BrowserConfig};
use openside_repl::{run, Gate, Guard, Limits, Output, Stop, Verdict};
use serde_json::{json, Value};
use tokio::{sync::Mutex, task::JoinHandle};

const OWNER: &str = "repl-script";
static SERIAL: Mutex<()> = Mutex::const_new(());

const DATA: &str = r#"<!doctype html><html lang="en"><head><title>Data</title></head><body>
<h1 id="title">Hello</h1>
<ul><li class="item" data-p="3">Apple</li><li class="item" data-p="1">Banana</li><li class="item" data-p="2">Cherry</li></ul>
<script>window.appState = { count: 2, tags: ['a', 'b'] };</script>
</body></html>"#;

const SHOP: &str = r#"<!doctype html><html lang="en"><head><title>Shop</title></head><body>
<h1>Shop</h1>
<label>Search <input id="q" name="q"></label>
<button id="add" onclick="setTimeout(() => { const li = document.createElement('li'); li.className = 'row'; li.dataset.price = '4'; li.textContent = document.getElementById('q').value; document.getElementById('cart').append(li); document.getElementById('status').hidden = false; }, 200)">Add to cart</button>
<p id="status" hidden>Added</p>
<ul id="cart"><li class="row" data-price="3">Pear</li></ul>
</body></html>"#;

/// Allows everything and records every [`Gate::Script`] it is asked about.
#[derive(Default)]
struct Recorder {
    scripts: parking_lot::Mutex<Vec<(String, String)>>,
    frames: parking_lot::Mutex<Vec<(String, String)>>,
}

impl Guard for Recorder {
    fn check(&self, gate: &Gate<'_>) -> Verdict {
        if let Gate::Script { origin, frame, api } = gate {
            self.scripts
                .lock()
                .push(((*origin).to_owned(), (*api).to_owned()));
            if let Some(frame) = frame {
                self.frames
                    .lock()
                    .push((frame.origin.to_owned(), (*api).to_owned()));
            }
        }
        Verdict::Allow
    }
}

struct Fixture {
    browser: Arc<Browser>,
    tab: String,
    url: String,
    server: JoinHandle<()>,
}

impl Fixture {
    async fn start() -> Option<Self> {
        let executable = match openside_config::discover_browser(&openside_config::Config::default())
        {
            Ok(path) => path,
            Err(reason) => {
                eprintln!("SKIP: page script tests require Chromium: {reason}");
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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}");
        // `localhost` is another site than 127.0.0.1, so that iframe runs out of process.
        let frames = format!(
            r#"<!doctype html><html><head><title>Frames</title></head><body>
<iframe name="inner" src="/data"></iframe>
<iframe name="cross" src="http://localhost:{port}/data"></iframe>
</body></html>"#
        );
        let app = Router::new()
            .route("/data", get(|| async { Html(DATA) }))
            .route("/shop", get(|| async { Html(SHOP) }))
            .route(
                "/frames",
                get(move || {
                    let frames = frames.clone();
                    async move { Html(frames) }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Some(Self {
            browser,
            tab,
            url,
            server,
        })
    }

    async fn run_with(&self, code: &str, guard: Arc<dyn Guard>, limits: Limits) -> Output {
        let started = Instant::now();
        let output = run(
            Arc::clone(&self.browser),
            OWNER.into(),
            self.tab.clone(),
            format!("const BASE = {:?};\n{code}", self.url),
            guard,
            limits,
        )
        .await;
        eprintln!(
            "[{:>5}ms] {code}\n   -> {} {:?}",
            started.elapsed().as_millis(),
            output.return_value,
            output.error
        );
        output
    }

    async fn run(&self, code: &str) -> Output {
        self.run_with(code, Arc::new(Recorder::default()), Limits::default())
            .await
    }

    async fn value(&self, code: &str) -> Value {
        let output = self.run(code).await;
        assert_eq!(output.error, None, "{code}: {output:?}");
        output.return_value
    }

    async fn error(&self, code: &str) -> String {
        let output = self.run(code).await;
        match output.error {
            Some(error) => error,
            None => panic!("{code}: expected an error, got {:?}", output.return_value),
        }
    }

    async fn stop(self) {
        self.server.abort();
        let browser = Arc::try_unwrap(self.browser)
            .ok()
            .expect("REPL runs released the browser");
        browser.shutdown().await.expect("shutdown chromium");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn evaluate_reads_page_data_and_round_trips_values() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    fixture.run("await page.goto(BASE + '/data')").await;

    assert_eq!(
        fixture
            .value("await page.evaluate(() => document.querySelector('h1').textContent + ' ' + window.appState.count)")
            .await,
        json!("Hello 2")
    );
    assert_eq!(
        fixture
            .value("await page.evaluate(([a, b]) => a + b, [2, 3])")
            .await,
        json!(5)
    );
    assert_eq!(
        fixture
            .value("await page.evaluate('window.appState.tags.length + 1')")
            .await,
        json!(3)
    );
    assert_eq!(
        fixture
            .value("await page.evaluate(async () => { await new Promise((r) => setTimeout(r, 20)); return location.pathname; })")
            .await,
        json!("/data")
    );
    // Values JSON cannot carry survive both directions; DOM nodes come back as a reference
    // marker and shared or cyclic objects stay shared.
    assert_eq!(
        fixture
            .value(
                "const sent = { u: undefined, n: NaN, z: -0, big: 10n, d: new Date(0), re: /x/g };
                 const back = await page.evaluate((v) => ({ ...v, kinds: [typeof v.u, typeof v.big, v.d instanceof Date, v.re instanceof RegExp, Object.is(v.z, -0)], node: document.body }), sent);
                 const cyc = await page.evaluate(() => { const a = { name: 'a' }; a.self = a; return [a, a]; });
                 ['u' in back && back.u === undefined, Number.isNaN(back.n), Object.is(back.z, -0), back.big === 10n,
                  back.d.toISOString(), String(back.re), back.kinds, back.node, cyc[0].self === cyc[0], cyc[0] === cyc[1]]"
            )
            .await,
        json!([true, true, true, true, "1970-01-01T00:00:00.000Z", "/x/g", ["undefined", "bigint", true, true, true], "ref: <Node>", true, true])
    );
    assert!(fixture
        .error("await page.evaluate(() => { throw new TypeError('boom') })")
        .await
        .starts_with("Error: page.evaluate: TypeError: boom"),);

    // $eval takes the first match without waiting; $$eval gets every match, possibly none.
    assert_eq!(
        fixture
            .value("[await page.$eval('li.item', (e, suffix) => e.textContent + suffix, '!'), await page.$$eval('li.item', (items) => items.map((i) => Number(i.dataset.p))), await page.$$eval('.none', (items) => items.length)]")
            .await,
        json!(["Apple!", [3, 1, 2], 0])
    );
    assert!(fixture
        .error("await page.$eval('#missing', (e) => e.id)")
        .await
        .contains("page.$eval: failed to find element matching"));

    // locator.evaluate is strict and auto-waits like actions; evaluateAll is not strict.
    assert_eq!(
        fixture
            .value("[await page.getByRole('heading').evaluate((e) => e.tagName), await page.locator('li').evaluateAll((items, min) => items.filter((i) => Number(i.dataset.p) >= min).map((i) => i.textContent), 2)]")
            .await,
        json!(["H1", ["Apple", "Cherry"]])
    );
    assert!(fixture
        .error("await page.locator('li').evaluate((e) => e.id)")
        .await
        .contains("strict mode violation"));
    assert!(fixture
        .error("await page.locator('#late').evaluate((e) => e.id, undefined, { timeout: 200 })")
        .await
        .starts_with("TimeoutError: Timeout 200ms exceeded"));
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handles_can_be_used_passed_and_disposed() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    fixture.run("await page.goto(BASE + '/data')").await;
    assert_eq!(
        fixture
            .value(
                "const state = await page.evaluateHandle(() => window.appState);
                 const count = await state.evaluate((s) => s.count);
                 const json = await state.jsonValue();
                 const prop = await (await state.getProperty('count')).jsonValue();
                 const keys = [...(await state.getProperties()).keys()];
                 const passed = await page.evaluate(([s, key]) => s[key] * 10, [state, 'count']);
                 const label = String(state);
                 await state.dispose();
                 let after;
                 try { await state.evaluate((s) => s); } catch (e) { after = e.message; }
                 const primitive = await page.evaluateHandle(() => 42);
                 const heading = await page.locator('h1').evaluateHandle((e) => e);
                 [count, json, prop, keys, passed, label, after, await primitive.jsonValue(), await heading.evaluate((e) => e.id), heading.asElement() !== null, primitive.asElement()]"
            )
            .await,
        json!([2, {"count": 2, "tags": ["a", "b"]}, 2, ["count", "tags"], 20, "JSHandle@Object", "jsHandle.evaluate: JSHandle is disposed", 42, "title", true, null])
    );
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn init_scripts_raw_cdp_and_the_script_gate() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    fixture.run("await page.goto(BASE + '/data')").await;
    let recorder = Arc::new(Recorder::default());
    let output = fixture
        .run_with(
            "await page.addInitScript((seed) => { window.__seed = seed; }, 'seeded');
             await page.addInitScript({ content: 'window.__content = 1;' });
             await page.reload();
             const seen = await page.evaluate(() => [window.__seed, window.__content]);
             const raw = await page.cdp('Runtime.evaluate', { expression: '6 * 7', returnByValue: true });
             const handle = await page.evaluateHandle(() => document);
             let reserved;
             try { await page.cdp('Fetch.disable'); } catch (e) { reserved = e.message; }
             [seen, raw.result.value, reserved]",
            Arc::clone(&recorder) as Arc<dyn Guard>,
            Limits::default(),
        )
        .await;
    assert_eq!(output.error, None, "{output:?}");
    assert_eq!(output.return_value[0], json!(["seeded", 1]));
    assert_eq!(output.return_value[1], json!(42));
    assert!(
        output.return_value[2]
            .as_str()
            .is_some_and(|message| message.contains("reserved by the host")),
        "{output:?}"
    );
    let origin = fixture.url.clone();
    let seen = recorder.scripts.lock().clone();
    for api in [
        "addInitScript",
        "evaluate",
        "evaluateHandle",
        "cdp:Runtime.evaluate",
        "cdp:Fetch.disable",
    ] {
        assert!(
            seen.contains(&(origin.clone(), api.to_owned())),
            "{api} was not gated on {origin}: {seen:?}"
        );
    }
    // Init scripts end with the execution that added them.
    assert_eq!(
        fixture
            .value("await page.reload(); await page.evaluate(() => typeof window.__seed)")
            .await,
        json!("undefined")
    );

    struct DenyScript;
    impl Guard for DenyScript {
        fn check(&self, gate: &Gate<'_>) -> Verdict {
            match gate {
                Gate::Script { api, .. } => Verdict::Deny(format!("no {api}")),
                _ => Verdict::Allow,
            }
        }
    }
    for (code, reason) in [
        (
            "await page.evaluate(() => document.title = 'changed')",
            "no evaluate",
        ),
        (
            "await page.$$eval('li', (items) => items.length)",
            "no evaluate",
        ),
        (
            "await page.cdp('Runtime.evaluate', { expression: '1' })",
            "no cdp:Runtime.evaluate",
        ),
        ("await page.addInitScript('1')", "no addInitScript"),
    ] {
        let denied = fixture
            .run_with(code, Arc::new(DenyScript), Limits::default())
            .await;
        assert_eq!(
            denied.stop,
            Some(Stop::Denied(reason.into())),
            "{code}: {denied:?}"
        );
        assert_eq!(denied.return_value, Value::Null);
    }
    assert_eq!(fixture.value("await page.title()").await, json!("Data"));
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn page_script_runs_in_same_and_cross_process_frames() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    fixture
        .run("await page.goto(BASE + '/frames'); await page.frame('cross').locator('h1').waitFor()")
        .await;
    let recorder = Arc::new(Recorder::default());
    let output = fixture
        .run_with(
            "const inner = page.frame('inner');
             const cross = page.frame('cross');
             const same = await inner.evaluate(() => [document.title, location.pathname, window.appState.count]);
             const items = await inner.$$eval('li.item', (items) => items.map((item) => item.textContent));
             const first = await cross.$eval('li.item', (item) => item.dataset.p);
             const host = await cross.evaluate(() => location.hostname);
             const heading = await page.frameLocator('iframe[name=cross]').locator('h1').evaluate((h) => h.textContent);
             const state = await cross.evaluateHandle(() => window.appState);
             const tags = await state.evaluate((s) => s.tags.length);
             let mixed;
             try { await page.evaluate((s) => s, state); } catch (e) { mixed = e.message; }
             await state.dispose();
             [same, items, first, host, heading, tags, mixed, await page.evaluate(() => document.title)]",
            Arc::clone(&recorder) as Arc<dyn Guard>,
            Limits::default(),
        )
        .await;
    assert_eq!(output.error, None, "{output:?}");
    let value = &output.return_value;
    assert_eq!(
        value.as_array().unwrap()[..6],
        json!([
            ["Data", "/data", 2],
            ["Apple", "Banana", "Cherry"],
            "3",
            "localhost",
            "Hello",
            2
        ])
        .as_array()
        .unwrap()[..]
    );
    assert!(
        value[6]
            .as_str()
            .is_some_and(|message| message.contains("another frame")),
        "{output:?}"
    );
    assert_eq!(value[7], json!("Frames"));
    let port = fixture.url.rsplit(':').next().unwrap();
    let frames = recorder.frames.lock().clone();
    for origin in [fixture.url.clone(), format!("http://localhost:{port}")] {
        assert!(
            frames
                .iter()
                .any(|(seen, api)| *seen == origin && api == "evaluate"),
            "no frame gate on {origin}: {frames:?}"
        );
    }
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_script_that_never_returns_does_not_wedge_the_tab() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    fixture.run("await page.goto(BASE + '/data')").await;
    let short = Limits {
        timeout: Duration::from_millis(1500),
        ..Limits::default()
    };
    for code in [
        "await page.evaluate(() => { for (;;) {} })",
        "await page.evaluate(() => new Promise(() => {}))",
    ] {
        let hung = fixture
            .run_with(code, Arc::new(Recorder::default()), short.clone())
            .await;
        assert!(
            hung.error
                .as_deref()
                .is_some_and(|error| error.starts_with("TimeoutError: REPL execution exceeded")),
            "{code}: {hung:?}"
        );
        let started = Instant::now();
        assert_eq!(
            fixture
                .value("await page.evaluate(() => document.title)")
                .await,
            json!("Data"),
            "after {code}"
        );
        assert!(started.elapsed() < Duration::from_secs(5), "after {code}");
    }
    fixture.stop().await;
}

/// B-15 ⑦: one `browser_repl` execution navigates, snapshots, fills, clicks, waits, and
/// extracts with page script.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_call_batches_driver_actions_and_page_script() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    fixture.browser.record_protocol(true);
    let output = fixture
        .run(
            "await page.goto(BASE + '/shop');
             const snap = await page.snapshot();
             await page.getByLabel('Search').fill('Plum');
             await page.getByRole('button', { name: 'Add to cart' }).click();
             await expect(page.getByText('Added')).toBeVisible();
             await page.waitForSelector('li.row >> nth=1');
             const rows = await page.$$eval('li.row', (items) => items.map((item) => ({ name: item.textContent, price: Number(item.dataset.price) })));
             ({ snapshot: snap.includes('Add to cart'), rows, url: page.url() })",
        )
        .await;
    let log = fixture.browser.take_protocol_log();
    fixture.browser.record_protocol(false);
    assert_eq!(output.error, None, "{output:?}");
    assert_eq!(
        output.return_value,
        json!({
            "snapshot": true,
            "rows": [{"name": "Pear", "price": 3}, {"name": "Plum", "price": 4}],
            "url": format!("{}/shop", fixture.url),
        })
    );
    let methods: Vec<_> = log.iter().map(|(method, _)| method.as_str()).collect();
    for method in [
        "Page.navigate",
        "Input.dispatchMouseEvent",
        "Runtime.callFunctionOn",
    ] {
        assert!(
            methods.contains(&method),
            "{method} missing from {methods:?}"
        );
    }
    fixture.stop().await;
}
