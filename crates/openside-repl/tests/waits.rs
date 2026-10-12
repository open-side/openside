//! Real-Chromium tests for navigation results, load-state waits, network idle, network waits,
//! URL waits, and navigation error classes. The fixture server makes every timing explicit:
//! a slow image delays `load` but not `DOMContentLoaded`, a fetch chain keeps the network busy
//! after `load`, and a never-answered request keeps it busy forever.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{sync::Arc, time::Duration};

use axum::{
    extract::Query,
    http::{header, HeaderMap, StatusCode},
    response::{Html, Redirect},
    routing::get,
    Router,
};
use openside_browser::{Browser, BrowserConfig};
use openside_repl::{run, Gate, Guard, Limits, Output, Verdict};
use serde_json::{json, Value};
use tokio::{sync::Mutex, task::JoinHandle};

const OWNER: &str = "repl-waits";
static SERIAL: Mutex<()> = Mutex::const_new(());

struct AllowAll;

impl Guard for AllowAll {
    fn check(&self, _gate: &Gate<'_>) -> Verdict {
        Verdict::Allow
    }
}

/// 1x1 transparent PNG.
const PIXEL: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xff, 0xff, 0x3f,
    0x00, 0x05, 0xfe, 0x02, 0xfe, 0xdc, 0xcc, 0x59, 0xe7, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

fn delay(query: &std::collections::HashMap<String, String>) -> Duration {
    Duration::from_millis(query.get("ms").and_then(|ms| ms.parse().ok()).unwrap_or(0))
}

fn routes() -> Router {
    type Q = Query<std::collections::HashMap<String, String>>;
    Router::new()
        .route(
            "/ok",
            get(|| async {
                (
                    [("x-fixture", "Yes")],
                    Html("<title>Ok</title><h1>ok</h1>"),
                )
            }),
        )
        .route("/missing", get(|| async { StatusCode::NOT_FOUND }))
        .route(
            "/missing-page",
            get(|| async {
                (
                    StatusCode::NOT_FOUND,
                    Html("<title>Gone</title><h1>Not here</h1>"),
                )
            }),
        )
        .route("/boom", get(|| async { StatusCode::INTERNAL_SERVER_ERROR }))
        .route(
            "/boom-page",
            get(|| async {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Html("<title>Broken</title><h1>Oops</h1>"),
                )
            }),
        )
        .route("/redirect", get(|| async { Redirect::temporary("/ok") }))
        .route(
            "/slow.png",
            get(|query: Q| async move {
                tokio::time::sleep(delay(&query)).await;
                ([(header::CONTENT_TYPE, "image/png")], PIXEL)
            }),
        )
        .route(
            "/slow-image",
            get(|| async {
                Html(
                    "<title>Slow</title><h1>slow</h1><img src=\"/slow.png?ms=900\">\
                     <script>document.addEventListener('DOMContentLoaded',\
                     () => document.title = 'dom');\
                     addEventListener('load', () => document.title = 'loaded');</script>",
                )
            }),
        )
        .route(
            "/api",
            get(|query: Q| async move {
                tokio::time::sleep(delay(&query)).await;
                (
                    [("content-type", "application/json"), ("x-test", "yes")],
                    json!({"tag": query.get("tag"), "ok": true}).to_string(),
                )
            })
            .post(|headers: HeaderMap| async move {
                json!({"method": "POST", "length": headers.get(header::CONTENT_LENGTH).map(|v| v.to_str().unwrap().to_owned())}).to_string()
            }),
        )
        .route(
            "/chain",
            get(|| async {
                Html(
                    "<title>chain</title><h1>chain</h1><script>addEventListener('load', () =>\
                     setTimeout(() => fetch('/api?ms=200').then(() => fetch('/api?ms=200'))\
                     .then(() => { document.title = 'chain done'; }), 50));</script>",
                )
            }),
        )
        .route(
            "/hang-fetch",
            get(|| async {
                Html(
                    "<title>hang</title><h1>hang</h1><script>addEventListener('load', () =>\
                     fetch('/never').catch(() => {}));</script>",
                )
            }),
        )
        .route(
            "/never",
            get(|| async {
                tokio::time::sleep(Duration::from_secs(600)).await;
                Html("<title>never</title>")
            }),
        )
        .route(
            "/buttons",
            get(|| async {
                Html(
                    "<title>Buttons</title>\
                     <button id=load onclick=\"setTimeout(() => fetch('/api?ms=50&tag=clicked'), 100)\">Load</button>\
                     <button id=post onclick=\"setTimeout(() => fetch('/api', {method: 'POST', body: 'x'}), 100)\">Post</button>\
                     <button id=push onclick=\"setTimeout(() => history.pushState(null, '', '/pushed'), 100)\">Push</button>\
                     <button id=blink onclick=\"location.hash = '#a'; setTimeout(() => location.hash = '#b', 5)\">Blink</button>\
                     <button id=leave onclick=\"setTimeout(() => location.href = '/ok', 300)\">Leave</button>",
                )
            }),
        )
        .route(
            "/flag",
            get(|| async {
                Html("<title>Flag</title><script>window.flag = 'off'; window.count = 0;\
                      setTimeout(() => { window.flag = 'on'; }, 400);\
                      setInterval(() => { window.count++; }, 50);</script>")
            }),
        )
        .route(
            "/delayed-nav",
            get(|| async {
                Html("<title>Delayed</title><script>setTimeout(() => location.href = '/ok?from=delayed', 300);</script>")
            }),
        )
}

struct Fixture {
    browser: Arc<Browser>,
    tab: String,
    url: String,
    server: JoinHandle<()>,
}

fn executable() -> Option<std::path::PathBuf> {
    match std::env::var_os("OPENSIDE_FLOW_BROWSER") {
        Some(path) => Some(path.into()),
        None => match openside_config::discover_browser(&openside_config::Config::default()) {
            Ok(path) => Some(path),
            Err(reason) => {
                eprintln!("SKIP: REPL waits require Chromium: {reason}");
                None
            }
        },
    }
}

impl Fixture {
    async fn start() -> Option<Self> {
        Self::start_with(BrowserConfig::new(executable()?)).await
    }

    async fn start_with(config: BrowserConfig) -> Option<Self> {
        let (browser, _events) = Browser::launch(config).await.expect("launch chromium");
        let browser = Arc::new(browser);
        let tab = browser
            .create_agent_tab(OWNER, "about:blank")
            .await
            .expect("agent tab")
            .id;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, routes()).await.unwrap();
        });
        Some(Self {
            browser,
            tab,
            url,
            server,
        })
    }

    /// Run `code` and return its completion value; a script error fails the test.
    async fn value(&self, code: &str) -> Value {
        let output = self.run(code).await;
        assert_eq!(output.error, None, "{code}\n{output:?}");
        output.return_value
    }

    async fn run(&self, code: &str) -> Output {
        let started = std::time::Instant::now();
        let output = run(
            Arc::clone(&self.browser),
            OWNER.into(),
            self.tab.clone(),
            format!("const BASE = {:?};\n{code}", self.url),
            Arc::new(AllowAll),
            Limits::default(),
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

    async fn stop(self) {
        self.server.abort();
        let browser = Arc::try_unwrap(self.browser)
            .ok()
            .expect("REPL runs released the browser");
        browser.shutdown().await.expect("shutdown chromium");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn goto_resolves_with_the_response_whatever_the_status() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    let value = fixture
        .value(
            "const summarize = (r) => ({ status: r.status(), ok: r.ok(), url: r.url().replace(BASE, '') });\
             const out = {};\
             const ok = await page.goto(BASE + '/ok');\
             out.ok = summarize(ok);\
             out.header = ok.headers()['x-fixture'];\
             out.body = await ok.text();\
             out.missing = summarize(await page.goto(BASE + '/missing'));\
             out.missingPage = summarize(await page.goto(BASE + '/missing-page'));\
             out.missingPageTitle = await page.title();\
             out.boom = summarize(await page.goto(BASE + '/boom'));\
             out.boomPage = summarize(await page.goto(BASE + '/boom-page'));\
             out.redirected = summarize(await page.goto(BASE + '/redirect'));\
             out.fragment = await page.goto(BASE + '/ok#frag');\
             out.urlAfter = page.url().replace(BASE, '');\
             out",
        )
        .await;
    assert_eq!(
        value["ok"],
        json!({"status": 200, "ok": true, "url": "/ok"})
    );
    assert_eq!(value["header"], "Yes");
    assert!(value["body"].as_str().unwrap().contains("<h1>ok</h1>"));
    assert_eq!(
        value["missing"],
        json!({"status": 404, "ok": false, "url": "/missing"})
    );
    assert_eq!(value["missingPage"]["status"], 404);
    assert_eq!(value["missingPageTitle"], "Gone");
    assert_eq!(value["boom"]["status"], 500);
    assert_eq!(value["boomPage"]["status"], 500);
    assert_eq!(
        value["redirected"],
        json!({"status": 200, "ok": true, "url": "/ok"})
    );
    assert_eq!(
        value["fragment"],
        Value::Null,
        "a fragment change loads nothing"
    );
    assert_eq!(value["urlAfter"], "/ok#frag");
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reload_and_history_moves_resolve_with_the_new_document() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    let value = fixture
        .value(
            "const out = {};\
             await page.goto(BASE + '/ok');\
             const reloaded = await page.reload();\
             out.reload = [reloaded.status(), reloaded.url().replace(BASE, '')];\
             await page.goto(BASE + '/missing-page');\
             const back = await page.goBack();\
             out.backTitle = await page.title();\
             out.back = back === null ? null : back.status();\
             const forward = await page.goForward();\
             out.forwardTitle = await page.title();\
             out.forward = forward === null ? null : forward.status();\
             out",
        )
        .await;
    assert_eq!(value["reload"], json!([200, "/ok"]));
    assert_eq!(value["backTitle"], "Ok");
    // A page restored from the back/forward cache has no response of its own.
    assert!(value["back"] == 200 || value["back"].is_null(), "{value}");
    assert_eq!(value["forwardTitle"], "Gone");
    assert!(
        value["forward"] == 404 || value["forward"].is_null(),
        "{value}"
    );
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_until_stops_at_the_requested_lifecycle_point() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    let value = fixture
        .value(
            "const out = {};\
             let t0 = Date.now();\
             const dom = await page.goto(BASE + '/slow-image', { waitUntil: 'domcontentloaded' });\
             out.domMs = Date.now() - t0;\
             out.domStatus = dom.status();\
             out.domTitle = await page.title();\
             await page.waitForLoadState('load');\
             out.loadMs = Date.now() - t0;\
             out.loadTitle = await page.title();\
             t0 = Date.now();\
             const commit = await page.goto(BASE + '/slow-image?again', { waitUntil: 'commit' });\
             out.commitMs = Date.now() - t0;\
             out.commitStatus = commit.status();\
             await page.waitForLoadState('domcontentloaded');\
             out.commitDomTitle = await page.title();\
             t0 = Date.now();\
             await page.waitForLoadState('load');\
             out.alreadyLoadedMs = Date.now() - t0;\
             out",
        )
        .await;
    assert!(value["domMs"].as_u64().unwrap() < 750, "{value}");
    assert_eq!(value["domStatus"], 200);
    assert!(
        ["dom", "loaded"].contains(&value["domTitle"].as_str().unwrap()),
        "{value}"
    );
    assert!(value["loadMs"].as_u64().unwrap() >= 850, "{value}");
    assert_eq!(value["loadTitle"], "loaded");
    assert!(value["commitMs"].as_u64().unwrap() < 750, "{value}");
    assert_eq!(value["commitStatus"], 200);
    assert!(value["alreadyLoadedMs"].as_u64().unwrap() < 1500, "{value}");
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_idle_waits_for_the_request_chain_and_a_quiet_period() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    let value = fixture
        .value(
            "const out = {};\
             await page.goto(BASE + '/chain');\
             out.titleAtLoad = await page.title();\
             let t0 = Date.now();\
             await page.waitForLoadState('networkidle');\
             out.idleMs = Date.now() - t0;\
             out.titleAtIdle = await page.title();\
             t0 = Date.now();\
             await page.goto(BASE + '/chain?again', { waitUntil: 'networkidle' });\
             out.gotoIdleMs = Date.now() - t0;\
             out.gotoIdleTitle = await page.title();\
             await page.goto(BASE + '/ok');\
             t0 = Date.now();\
             await page.waitForLoadState('networkidle');\
             out.quietMs = Date.now() - t0;\
             out",
        )
        .await;
    assert_eq!(
        value["titleAtLoad"], "chain",
        "load must not wait for the chain: {value}"
    );
    assert_eq!(value["titleAtIdle"], "chain done", "{value}");
    assert!(value["idleMs"].as_u64().unwrap() >= 450, "{value}");
    assert_eq!(value["gotoIdleTitle"], "chain done", "{value}");
    assert!(value["gotoIdleMs"].as_u64().unwrap() >= 900, "{value}");
    // The quiet period is a window, not a fixed sleep on top of the work.
    let quiet = value["quietMs"].as_u64().unwrap();
    assert!((400..1500).contains(&quiet), "{value}");
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_that_never_goes_idle_times_out_naming_the_pending_request() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    let value = fixture
        .value(
            "const out = {};\
             try {\
               await page.goto(BASE + '/hang-fetch', { waitUntil: 'networkidle', timeout: 1800 });\
             } catch (e) { out.goto = { name: e.name, transient: e.transient, message: e.message.split(BASE).join('') }; }\
             try {\
               await page.waitForLoadState('networkidle', { timeout: 700 });\
             } catch (e) { out.wait = { name: e.name, transient: e.transient, message: e.message.replace(BASE, '') }; }\
             out.title = await page.title();\
             out",
        )
        .await;
    let goto = &value["goto"];
    assert_eq!(goto["name"], "TimeoutError", "{value}");
    assert_eq!(goto["transient"], true);
    let message = goto["message"].as_str().unwrap();
    assert!(
        message.starts_with("navigation timeout of 1800ms exceeded waiting for \"networkidle\""),
        "{message}"
    );
    assert!(
        message.contains("1 request still in flight: GET /never"),
        "{message}"
    );
    let wait = &value["wait"];
    assert_eq!(wait["name"], "TimeoutError", "{value}");
    assert!(
        wait["message"].as_str().unwrap().contains("GET /never"),
        "{value}"
    );
    // The tab stays usable: the page itself loaded.
    assert_eq!(value["title"], "hang");
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waiting_for_a_response_or_request_sees_only_what_happens_after_the_call() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    let value = fixture
        .value(
            "const out = {};\
             await page.goto(BASE + '/buttons');\
             const [response] = await Promise.all([\
               page.waitForResponse('**/api?*tag=clicked'),\
               page.click('#load'),\
             ]);\
             out.status = response.status();\
             out.ok = response.ok();\
             out.header = response.headers()['x-test'];\
             out.json = await response.json();\
             out.requestUrl = response.request().url().replace(BASE, '');\
             const [, byPredicate] = await Promise.all([\
               page.click('#load'),\
               page.waitForResponse(async (r) => r.url().includes('/api') && r.status() === 200),\
             ]);\
             out.predicate = byPredicate.status();\
             const [request] = await Promise.all([\
               page.waitForRequest((r) => r.method() === 'POST'),\
               page.click('#post'),\
             ]);\
             out.post = [request.method(), request.url().replace(BASE, ''), request.resourceType()];\
             try {\
               await page.waitForResponse('**/api?*tag=clicked', { timeout: 700 });\
             } catch (e) { out.stale = { name: e.name, transient: e.transient, message: e.message }; }\
             out",
        )
        .await;
    assert_eq!(value["status"], 200);
    assert_eq!(value["ok"], true);
    assert_eq!(value["header"], "yes");
    assert_eq!(value["json"], json!({"tag": "clicked", "ok": true}));
    assert_eq!(value["requestUrl"], "/api?ms=50&tag=clicked");
    assert_eq!(value["predicate"], 200);
    assert_eq!(value["post"], json!(["POST", "/api", "fetch"]));
    assert_eq!(
        value["stale"]["name"], "TimeoutError",
        "an older response must not match: {value}"
    );
    assert_eq!(value["stale"]["transient"], true);
    assert!(value["stale"]["message"]
        .as_str()
        .unwrap()
        .contains("page.waitForResponse"));
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_for_url_matches_globs_regexps_predicates_and_short_lived_urls() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    let value = fixture
        .value(
            "const out = {};\
             await page.goto(BASE + '/delayed-nav');\
             await page.waitForURL('**/ok?from=*');\
             out.glob = [page.url().replace(BASE, ''), await page.title()];\
             await page.goto(BASE + '/delayed-nav');\
             await page.waitForURL(/\\/ok\\?from=delayed$/);\
             out.regexp = page.url().replace(BASE, '');\
             await page.goto(BASE + '/delayed-nav');\
             await page.waitForURL((u) => u.pathname === '/ok' && u.searchParams.get('from') === 'delayed');\
             out.predicate = page.url().replace(BASE, '');\
             await page.goto(BASE + '/buttons');\
             await Promise.all([page.waitForURL('**/pushed'), page.click('#push')]);\
             out.push = page.url().replace(BASE, '');\
             await page.goto(BASE + '/buttons');\
             await Promise.all([page.waitForURL(/#a$/), page.click('#blink')]);\
             out.blink = true;\
             await page.goto(BASE + '/buttons');\
             await Promise.all([page.waitForURL('**/ok', { waitUntil: 'commit' }), page.click('#leave')]);\
             out.commit = page.url().replace(BASE, '');\
             try {\
               await page.waitForURL('**/nowhere', { timeout: 600 });\
             } catch (e) { out.miss = { name: e.name, transient: e.transient, message: e.message.replace(BASE, '') }; }\
             out",
        )
        .await;
    assert_eq!(value["glob"], json!(["/ok?from=delayed", "Ok"]));
    assert_eq!(value["regexp"], "/ok?from=delayed");
    assert_eq!(value["predicate"], "/ok?from=delayed");
    assert_eq!(value["push"], "/pushed");
    assert_eq!(value["blink"], true);
    assert_eq!(value["commit"], "/ok");
    assert_eq!(value["miss"]["name"], "TimeoutError");
    assert_eq!(value["miss"]["transient"], true);
    let message = value["miss"]["message"].as_str().unwrap();
    assert!(
        message.contains("Timeout 600ms exceeded waiting for \"**/nowhere\"; current URL /ok"),
        "{message}"
    );
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_for_function_polls_until_the_page_condition_holds() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    let value = fixture
        .value(
            "const out = {};\
             await page.goto(BASE + '/flag');\
             let t0 = Date.now();\
             const handle = await page.waitForFunction(() => window.flag === 'on');\
             out.waitedMs = Date.now() - t0;\
             out.truthy = await handle.jsonValue();\
             out.withArg = await (await page.waitForFunction((min) => window.count >= min && window.count, 3)).jsonValue() >= 3;\
             try {\
               await page.waitForFunction(() => window.flag === 'never', undefined, { timeout: 500, polling: 100 });\
             } catch (e) { out.timeout = { name: e.name, transient: e.transient, message: e.message }; }\
             try {\
               await page.waitForFunction(() => { throw new Error('boom'); });\
             } catch (e) { out.thrown = e.message; }\
             out",
        )
        .await;
    assert!(value["waitedMs"].as_u64().unwrap() >= 250, "{value}");
    assert_eq!(value["truthy"], true);
    assert_eq!(value["withArg"], true);
    assert_eq!(value["timeout"]["name"], "TimeoutError", "{value}");
    assert_eq!(value["timeout"]["transient"], true);
    assert!(
        value["thrown"].as_str().unwrap().contains("boom"),
        "{value}"
    );
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn navigation_errors_are_classified_as_transient_or_permanent() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    let closed = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let value = fixture
        .value(&format!(
            "const out = {{}};\
             const attempt = async (key, fn) => {{\
               try {{ await fn(); out[key] = 'resolved'; }}\
               catch (e) {{ out[key] = {{ name: e.name, transient: e.transient, message: e.message }}; }}\
             }};\
             await attempt('refused', () => page.goto({closed:?}));\
             await attempt('dns', () => page.goto('http://nonexistent.invalid/', {{ timeout: 8000 }}));\
             await attempt('malformed', () => page.goto('not a url'));\
             await attempt('waitUntil', () => page.goto(BASE + '/ok', {{ waitUntil: 'sometime' }}));\
             await attempt('state', () => page.waitForLoadState('whenever'));\
             await attempt('timeout', () => page.waitForURL('**/nowhere', {{ timeout: 300 }}));\
             out.alive = await page.goto(BASE + '/ok').then((r) => r.status());\
             out"
        ))
        .await;
    let refused = &value["refused"];
    assert_eq!(refused["name"], "NetworkError", "{value}");
    assert_eq!(refused["transient"], true);
    let message = refused["message"].as_str().unwrap();
    assert!(message.contains("net::ERR_CONNECTION_REFUSED"), "{message}");
    assert!(
        message.contains("transient: retrying can succeed"),
        "{message}"
    );
    let dns = &value["dns"];
    assert_eq!(dns["name"], "NavigationError", "{value}");
    assert_eq!(dns["transient"], false);
    assert!(dns["message"]
        .as_str()
        .unwrap()
        .contains("permanent: retrying will not help"));
    let malformed = &value["malformed"];
    assert_eq!(malformed["name"], "NavigationError", "{value}");
    assert_eq!(malformed["transient"], false);
    for key in ["waitUntil", "state"] {
        assert_eq!(value[key]["name"], "Error", "{value}");
        assert_eq!(value[key]["transient"], false);
        assert!(
            value[key]["message"].as_str().unwrap().contains("must be"),
            "{value}"
        );
    }
    assert_eq!(value["timeout"]["transient"], true);
    assert_eq!(value["alive"], 200);
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hung_navigation_does_not_stall_another_sessions_interactions() {
    let _serial = SERIAL.lock().await;
    let Some(executable) = executable() else {
        return;
    };
    let mut config = BrowserConfig::new(executable);
    // The hung navigation holds its command until this transport timeout; interactions of other
    // sessions must finish long before.
    config.command_timeout = Duration::from_secs(5);
    let fixture = Fixture::start_with(config).await.unwrap();
    let other = "repl-waits-other";
    let other_tab = fixture
        .browser
        .create_agent_tab(other, &format!("{}/buttons", fixture.url))
        .await
        .unwrap()
        .id;
    // Wait for the page to render its button.
    let reference = {
        let mut found = None;
        for _ in 0..50 {
            let snapshot = fixture.browser.snapshot(&other_tab).await.unwrap();
            found = snapshot
                .nodes
                .iter()
                .find(|node| node.role == "button")
                .and_then(|node| node.reference.clone());
            if found.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        found.expect("button reference")
    };
    let browser = Arc::clone(&fixture.browser);
    let (tab, url) = (fixture.tab.clone(), format!("{}/never", fixture.url));
    let hung = tokio::spawn(async move { browser.navigate(OWNER, &tab, &url).await });
    tokio::time::sleep(Duration::from_millis(400)).await;
    let started = std::time::Instant::now();
    fixture
        .browser
        .resolve_ref(other, &other_tab, &reference)
        .await
        .expect("interaction of the other session");
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "interaction waited {:?} behind another session's hung navigation",
        started.elapsed()
    );
    assert!(
        !hung.is_finished(),
        "the navigation was expected to still hang"
    );
    assert!(
        hung.await.unwrap().is_err(),
        "a navigation that never answers errors"
    );
    fixture.stop().await;
}
