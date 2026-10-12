//! Playwright compatibility suite for the openside REPL against a local mock site.
//!
//! The per-case table and pass rate published in `docs/specs/browser-repl.md` must equal what
//! this suite computes, and the controller must never write the DOM bypassing input events.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{collections::HashMap, sync::Arc, time::Duration};

use axum::{extract::Query, response::Html, routing::get, Router};
use openside_browser::{Browser, BrowserConfig};
use openside_repl::{run, Gate, Guard, Limits, Output, Stop, Verdict};
use serde_json::{json, Value};
use tokio::{sync::Mutex, task::JoinHandle};

const OWNER: &str = "repl-compat";
// Page mutations that bypass input events; the locator operations never need them.
const DOM_WRITE_METHODS: &[&str] = &["DOM.setAttributeValue", "DOM.setOuterHTML"];
const DOC: &str = include_str!("../../../docs/specs/browser-repl.md");
static SERIAL: Mutex<()> = Mutex::const_new(());

struct Server {
    url: String,
    task: JoinHandle<()>,
}

impl Server {
    async fn start() -> Self {
        fn escape(text: &str) -> String {
            text.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
        }
        let app = Router::new()
            .route("/", get(|| async { Html(include_str!("fixtures/index.html")) }))
            .route("/form", get(|| async { Html(include_str!("fixtures/form.html")) }))
            .route("/list", get(|| async { Html(include_str!("fixtures/list.html")) }))
            .route(
                "/submitted",
                get(|Query(query): Query<HashMap<String, String>>| async move {
                    let mut keys: Vec<_> = query.keys().collect();
                    keys.sort();
                    let items: String = keys
                        .into_iter()
                        .map(|key| {
                            format!("<li data-key=\"{}\">{}</li>", escape(key), escape(&query[key]))
                        })
                        .collect();
                    Html(format!(
                        "<!doctype html><html lang=\"en\"><head><title>Submitted</title></head><body><h1>Submitted</h1><ul>{items}</ul></body></html>"
                    ))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { url, task }
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

fn browser_config() -> Option<BrowserConfig> {
    match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(executable) => Some(BrowserConfig::new(executable)),
        Err(error) => {
            eprintln!(
                "skip: no chromium-compatible browser found (tried {})",
                error.tried.join(", ")
            );
            None
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
        let config = browser_config()?;
        let (browser, _events) = Browser::launch(config).await.expect("launch chromium");
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

    async fn run(&self, code: &str) -> Output {
        self.run_with(code, Arc::new(AllowAll), Limits::default())
            .await
    }

    async fn run_with(&self, code: &str, guard: Arc<dyn Guard>, limits: Limits) -> Output {
        let code = format!("const BASE = {:?};\n{code}", self.server.url);
        run(
            Arc::clone(&self.browser),
            OWNER.into(),
            self.tab.clone(),
            code,
            guard,
            limits,
        )
        .await
    }

    async fn stop(self) {
        drop(self.server);
        let browser = Arc::try_unwrap(self.browser)
            .ok()
            .expect("REPL runs released the browser");
        browser.shutdown().await.expect("shutdown chromium");
    }
}

enum Expect {
    /// JSON of the completion value; `{BASE}` is replaced by the mock site origin.
    Value(&'static str),
    /// Prefix of the reported error.
    Error(&'static str),
}

struct Case {
    id: &'static str,
    api: &'static str,
    code: &'static str,
    expect: Expect,
}

const CASES: &[Case] = &[
    Case { id: "goto-title", api: "page.goto · page.title", code: "await page.goto(BASE + '/'); await page.title()", expect: Expect::Value(r#""Openside REPL fixture""#) },
    Case { id: "url", api: "page.url", code: "page.url()", expect: Expect::Value(r#""{BASE}/""#) },
    Case { id: "role-heading", api: "getByRole(name) · textContent", code: "await page.getByRole('heading', { name: 'Openside fixture' }).textContent()", expect: Expect::Value(r#""Openside fixture""#) },
    Case { id: "text-substring", api: "getByText · count", code: "await page.getByText('Hello world').count()", expect: Expect::Value("1") },
    Case { id: "testid-innertext", api: "getByTestId · innerText", code: "await page.getByTestId('greeting').innerText()", expect: Expect::Value(r#""Hello world""#) },
    Case { id: "click-to-have-text", api: "click · expect.toHaveText", code: "await page.getByRole('button', { name: 'Increment' }).click(); await page.locator('#inc').click(); await expect(page.locator('#count')).toHaveText('2'); await page.locator('#count').textContent()", expect: Expect::Value(r#""2""#) },
    Case { id: "dblclick", api: "dblclick", code: "await page.locator('#dbl').dblclick(); await page.locator('#dbl').textContent()", expect: Expect::Value(r#""Double clicked""#) },
    Case { id: "hover", api: "hover", code: "await page.locator('#hover').hover(); await page.locator('#hover').textContent()", expect: Expect::Value(r#""Hovered""#) },
    Case { id: "visibility", api: "isVisible (display/visibility)", code: "[await page.locator('#hidden').isVisible(), await page.locator('#ghost').isVisible(), await page.locator('h1').isVisible()]", expect: Expect::Value("[false,false,true]") },
    Case { id: "auto-wait", api: "expect.toBeVisible auto-wait", code: "await page.getByRole('button', { name: 'Load later' }).click(); await expect(page.locator('#late-result')).toBeVisible(); await page.locator('#late-result').textContent()", expect: Expect::Value(r#""Arrived late""#) },
    Case { id: "reload-wait-for-selector", api: "page.reload · waitForSelector · last", code: "await page.reload(); await page.locator('#late').click(); await page.waitForSelector('#late-result'); await page.locator('p').last().textContent()", expect: Expect::Value(r#""Arrived late""#) },
    Case { id: "alt-title", api: "getByAltText · getByTitle", code: "[await page.getByAltText('Openside logo').count(), await page.getByTitle('Tooltip text').textContent()]", expect: Expect::Value(r#"[1,"hover me"]"#) },
    Case { id: "press-chord", api: "locator.press(Shift+A)", code: "await page.getByLabel('Key log').press('Shift+A'); await page.locator('#lastkey').textContent()", expect: Expect::Value(r#""A""#) },
    Case { id: "keyboard-type", api: "focus · page.keyboard.type · inputValue", code: "await page.getByLabel('Key log').fill(''); await page.getByLabel('Key log').focus(); await page.keyboard.type('hi'); await page.getByLabel('Key log').inputValue()", expect: Expect::Value(r#""hi""#) },
    Case { id: "wait-for-hidden", api: "locator.waitFor({state:'hidden'})", code: "await page.locator('#hidden').waitFor({ state: 'hidden' }); 'ok'", expect: Expect::Value(r#""ok""#) },
    Case { id: "link-navigation", api: "click link · expect(page).toHaveURL", code: "await page.getByRole('link', { name: 'Form page' }).click(); await expect(page).toHaveURL(BASE + '/form'); page.url()", expect: Expect::Value(r#""{BASE}/form""#) },
    Case { id: "label-fill", api: "getByLabel · fill · inputValue", code: "await page.getByLabel('Username').fill('ann'); await page.getByLabel('Username').inputValue()", expect: Expect::Value(r#""ann""#) },
    Case { id: "placeholder-to-have-value", api: "getByPlaceholder · expect.toHaveValue", code: "await page.getByPlaceholder('Email address').fill('ann@example.test'); await expect(page.getByPlaceholder('Email address')).toHaveValue('ann@example.test'); true", expect: Expect::Value("true") },
    Case { id: "fill-replaces", api: "fill (replace)", code: "await page.getByLabel('Username').fill('first'); await page.getByLabel('Username').fill('second'); await page.getByLabel('Username').inputValue()", expect: Expect::Value(r#""second""#) },
    Case { id: "clear", api: "clear", code: "await page.getByLabel('Username').clear(); await page.getByLabel('Username').inputValue()", expect: Expect::Value(r#""""#) },
    Case { id: "press-sequentially", api: "pressSequentially (textarea)", code: "await page.getByLabel('Bio').pressSequentially('Hi there'); await page.getByLabel('Bio').inputValue()", expect: Expect::Value(r#""Hi there""#) },
    Case { id: "check", api: "check · isChecked", code: "await page.getByLabel('Subscribe').check(); [await page.getByLabel('Subscribe').isChecked(), await page.getByRole('checkbox').isChecked()]", expect: Expect::Value("[true,true]") },
    Case { id: "uncheck-not-checked", api: "uncheck · expect.not.toBeChecked", code: "await page.getByLabel('Subscribe').uncheck(); await expect(page.getByLabel('Subscribe')).not.toBeChecked(); 'ok'", expect: Expect::Value(r#""ok""#) },
    Case { id: "radio", api: "getByRole(radio) · check · toBeChecked", code: "await page.getByRole('radio', { name: 'Pro' }).check(); await expect(page.getByRole('radio', { name: 'Pro' })).toBeChecked(); await page.getByRole('radio', { name: 'Free' }).isChecked()", expect: Expect::Value("false") },
    Case { id: "select-option", api: "selectOption · inputValue", code: "const picked = await page.getByLabel('Country').selectOption('us'); [picked, await page.getByLabel('Country').inputValue()]", expect: Expect::Value(r#"[["us"],"us"]"#) },
    Case { id: "enabled-editable", api: "toBeDisabled · toBeEnabled · isEditable", code: "await expect(page.getByRole('button', { name: 'Disabled action' })).toBeDisabled(); await expect(page.getByRole('button', { name: 'Sign up' })).toBeEnabled(); await page.getByLabel('Locked').isEditable()", expect: Expect::Value("false") },
    Case { id: "attributes", api: "getAttribute · expect.toHaveAttribute", code: "await expect(page.getByLabel('Locked')).toHaveAttribute('readonly'); await page.getByLabel('Username').getAttribute('name')", expect: Expect::Value(r#""user""#) },
    Case { id: "enter-submits", api: "press(Enter) · waitForURL(glob)", code: "await page.getByLabel('Username').fill('ann'); await page.getByLabel('Username').press('Enter'); await page.waitForURL('**/submitted?**'); await page.locator('li[data-key=user]').textContent()", expect: Expect::Value(r#""ann""#) },
    Case { id: "history", api: "goBack · goForward", code: "await page.goBack(); const back = page.url(); await page.goForward(); [back, page.url().split('?')[0]]", expect: Expect::Value(r#"["{BASE}/form","{BASE}/submitted"]"#) },
    Case { id: "count-nth-last", api: "count · nth · last · chained locator", code: "await page.goto(BASE + '/list'); [await page.locator('li.fruit').count(), await page.locator('li.fruit').nth(1).locator('span').textContent(), await page.locator('li.fruit').last().locator('span').textContent()]", expect: Expect::Value(r#"[3,"Banana","Cherry"]"#) },
    Case { id: "filter-has-text", api: "filter({hasText}) · toHaveCount · allInnerTexts", code: "await page.locator('li').filter({ hasText: 'Banana' }).getByRole('button').click(); await expect(page.locator('li.fruit')).toHaveCount(2); await page.locator('li.fruit').allInnerTexts()", expect: Expect::Value(r#"["Apple Delete","Cherry Delete"]"#) },
    Case { id: "text-content-whitespace", api: "allTextContents (whitespace-only text nodes)", code: "await page.locator('li.fruit').allTextContents()", expect: Expect::Value(r#"["Apple Delete","Cherry Delete"]"#) },
    Case { id: "filter-has", api: "filter({has}) · getByRole(disabled)", code: "await page.locator('li.fruit').filter({ has: page.getByRole('button', { disabled: true }) }).locator('span').textContent()", expect: Expect::Value(r#""Cherry""#) },
    Case { id: "filter-has-not-text", api: "filter({hasNotText})", code: "await page.locator('li.fruit').filter({ hasNotText: 'Cherry' }).locator('span').textContent()", expect: Expect::Value(r#""Apple""#) },
    Case { id: "strict-mode", api: "strict mode violation", code: "await page.locator('li.fruit').click()", expect: Expect::Error("Error: strict mode violation") },
    Case { id: "action-timeout", api: "click({timeout}) TimeoutError", code: "await page.locator('#missing').click({ timeout: 300 })", expect: Expect::Error("TimeoutError: Timeout 300ms exceeded") },
    Case { id: "chain-text-engine", api: "'ul >> text=Apple'", code: "await page.locator('ul >> text=Apple').count()", expect: Expect::Value("1") },
    Case { id: "role-selector", api: "'role=button[name=\"Delete\"]'", code: "await page.locator('role=button[name=\"Delete\"]').count()", expect: Expect::Value("2") },
    Case { id: "css-has-text", api: "'li:has-text(\"Cherry\")'", code: "await page.locator('li:has-text(\"Cherry\")').count()", expect: Expect::Value("1") },
    Case { id: "columnheader", api: "getByRole(columnheader) · allInnerTexts", code: "[await page.getByRole('columnheader', { name: 'Qty' }).textContent(), await page.locator('th').allInnerTexts()]", expect: Expect::Value(r#"["Qty",["Name","Qty"]]"#) },
    Case { id: "contain-text", api: "expect.toContainText · not.toHaveText", code: "await expect(page.locator('table')).toContainText('Qty'); await expect(page.locator('h1')).not.toHaveText('anything', { timeout: 200 }); 'ok'", expect: Expect::Value(r#""ok""#) },
    Case { id: "aria-ref", api: "page.snapshot ref → 'aria-ref=eN'", code: "const snap = await page.snapshot(); const ref = /\\[(e\\d+)\\] link \"Home\"/.exec(snap)[1]; await page.locator('aria-ref=' + ref).getAttribute('href')", expect: Expect::Value(r#""/""#) },
    Case { id: "generic-expect", api: "expect(value).toBe/toEqual/toContain", code: "expect(1 + 1).toBe(2); expect([1, { a: 2 }]).toEqual([1, { a: 2 }]); expect('openside').toContain('side'); expect(3).not.toBeGreaterThan(4); 'ok'", expect: Expect::Value(r#""ok""#) },
    Case { id: "completion-value", api: "top-level await · waitForTimeout", code: "await page.waitForTimeout(50); const n = await page.locator('li.fruit').count(); n * 10", expect: Expect::Value("20") },
    Case { id: "content", api: "page.content", code: "(await page.content()).includes('<ul id=\"fruits\">')", expect: Expect::Value("true") },
    Case { id: "load-state-title", api: "waitForLoadState · expect(page).toHaveTitle(regex)", code: "await page.reload(); await page.waitForLoadState('domcontentloaded'); await expect(page).toHaveTitle(/List/); await page.title()", expect: Expect::Value(r#""List page""#) },
    Case { id: "page-shorthands", api: "page.fill · page.inputValue · page.click", code: "await page.goto(BASE + '/form'); await page.fill('#user', 'short'); await page.click('text=Subscribe'); [await page.inputValue('#user'), await page.isChecked('input[name=subscribe]')]", expect: Expect::Value(r#"["short",true]"#) },
    Case { id: "role-regex-name", api: "getByRole({name: RegExp})", code: "await page.getByRole('button', { name: /sign/i }).textContent()", expect: Expect::Value(r#""Sign up""#) },
    Case { id: "locator-all", api: "locator.all · first", code: "const radios = await page.getByRole('radio').all(); [radios.length, await page.getByRole('radio').first().getAttribute('value')]", expect: Expect::Value(r#"[2,"free"]"#) },
    Case { id: "evaluate-arg", api: "page.evaluate(fn, arg)", code: "await page.evaluate(([a, b]) => document.title + ' ' + (a + b), [1, 2])", expect: Expect::Value(r#""Form page 3""#) },
    Case { id: "eval-selectors", api: "page.$eval · page.$$eval", code: "[await page.$eval('#user', (e) => e.value), await page.$$eval('option', (options) => options.map((o) => o.value))]", expect: Expect::Value(r#"["short",["kr","jp","us"]]"#) },
    Case { id: "locator-evaluate", api: "locator.evaluate · evaluateAll", code: "[await page.getByLabel('Locked').evaluate((e) => e.readOnly), await page.getByRole('radio').evaluateAll((radios) => radios.map((r) => r.value))]", expect: Expect::Value(r#"[true,["free","pro"]]"#) },
];

fn expected(case: &Case, base: &str) -> Result<Value, String> {
    match case.expect {
        Expect::Value(json) => Ok(serde_json::from_str(&json.replace("{BASE}", base)).unwrap()),
        Expect::Error(prefix) => Err(prefix.to_owned()),
    }
}

fn passed(output: &Output, expected: &Result<Value, String>) -> bool {
    match (expected, &output.error) {
        (Ok(value), None) => output.return_value == *value,
        (Err(prefix), Some(error)) => error.starts_with(prefix.as_str()),
        _ => false,
    }
}

fn table(results: &[(&Case, bool)]) -> String {
    let mut table = String::from("| # | 케이스 | Playwright API | 결과 |\n|---|---|---|---|\n");
    for (index, (case, ok)) in results.iter().enumerate() {
        table.push_str(&format!(
            "| {} | `{}` | {} | {} |\n",
            index + 1,
            case.id,
            case.api.replace('|', "\\|"),
            if *ok { "pass" } else { "fail" }
        ));
    }
    let pass = results.iter().filter(|(_, ok)| *ok).count();
    table.push_str(&format!(
        "\n통과율: {pass}/{} ({:.1}%)\n",
        results.len(),
        pass as f64 * 100.0 / results.len() as f64
    ));
    table
}

fn published_table() -> &'static str {
    let start = DOC
        .find("<!-- compat:begin -->\n")
        .expect("compat table start marker")
        + "<!-- compat:begin -->\n".len();
    let end = DOC
        .find("<!-- compat:end -->")
        .expect("compat table end marker");
    &DOC[start..end]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn playwright_compat_suite_matches_published_table() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    assert!(
        CASES.len() >= 30,
        "compat suite must keep at least 30 cases"
    );
    fixture.browser.record_protocol(true);
    let mut results = Vec::new();
    for case in CASES {
        let output = fixture.run(case.code).await;
        let wanted = expected(case, &fixture.server.url);
        let ok = passed(&output, &wanted);
        if !ok {
            eprintln!(
                "case {} failed: expected {wanted:?}, got value {} error {:?} console {:?}",
                case.id, output.return_value, output.error, output.console
            );
        }
        results.push((case, ok));
    }
    let log = fixture.browser.take_protocol_log();
    fixture.browser.record_protocol(false);
    let mut methods: Vec<_> = log.iter().map(|(method, _)| method.as_str()).collect();
    methods.sort_unstable();
    methods.dedup();
    eprintln!("CDP methods issued during the suite: {methods:?}");
    fixture.stop().await;

    let computed = table(&results);
    eprintln!("computed compat table:\n{computed}");
    assert!(!log.is_empty(), "protocol recording captured the suite");
    for forbidden in DOM_WRITE_METHODS {
        assert!(
            !methods.contains(forbidden),
            "the REPL issued DOM write command {forbidden}"
        );
    }
    assert_eq!(
        published_table(),
        computed,
        "docs/specs/browser-repl.md compat table differs from the computed results"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sandbox_limits_errors_and_policy_stops() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    fixture.browser.record_protocol(true);

    let sandbox = fixture
        .run("[typeof require, typeof process, typeof fetch, typeof std, typeof os, typeof __openside_op, typeof __openside_log, typeof Deno, typeof XMLHttpRequest]")
        .await;
    assert_eq!(
        sandbox.return_value,
        json!(vec!["undefined"; 9]),
        "{sandbox:?}"
    );

    let console = fixture
        .run("console.log('hello', { a: 1 }, [2]); console.error('bad'); 7")
        .await;
    assert_eq!(console.return_value, json!(7));
    assert_eq!(console.console, vec!["hello {\"a\":1} [2]", "[error] bad"]);

    let unsupported = fixture.run("await page.exposeFunction('f', () => 1)").await;
    assert!(
        unsupported
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("NotSupported: page.exposeFunction is not supported by the openside REPL (page code cannot call back into the REPL)")),
        "{unsupported:?}"
    );
    for code in [
        "await page.pdf()",
        "page.touchscreen.tap(1, 1)",
        "await page.route('**', r => r.abort())",
    ] {
        let output = fixture.run(code).await;
        assert!(
            output
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with("NotSupported: ")),
            "{code}: {output:?}"
        );
    }

    let caught = fixture
        .run("try { await page.locator('#nope').click({ timeout: 100 }) } catch (e) { e.name }")
        .await;
    assert_eq!(caught.return_value, json!("TimeoutError"), "{caught:?}");

    let short = Limits {
        timeout: Duration::from_millis(500),
        ..Limits::default()
    };
    let started = std::time::Instant::now();
    let spin = fixture
        .run_with("for (;;) {}", Arc::new(AllowAll), short.clone())
        .await;
    assert!(spin.error.is_some(), "{spin:?}");
    let hang = fixture
        .run_with("await new Promise(() => {})", Arc::new(AllowAll), short)
        .await;
    assert!(
        hang.error
            .as_deref()
            .is_some_and(|error| error.starts_with("TimeoutError: REPL execution exceeded 500ms")),
        "{hang:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));

    let memory = fixture
        .run("const chunks = []; for (;;) chunks.push('x'.repeat(1 << 20) + chunks.length)")
        .await;
    assert!(
        memory
            .error
            .as_deref()
            .is_some_and(|error| error.contains("out of memory")
                || error.contains("RangeError")
                || error.contains("InternalError")),
        "{memory:?}"
    );

    let big = fixture.run("'x'.repeat(100000)").await;
    assert_eq!(
        big.return_value["truncated"],
        json!(true),
        "{:?}",
        big.error
    );

    // A guard asking for approval stops the script before the click reaches the page, and
    // the script cannot swallow the stop.
    struct ApproveBuy;
    impl Guard for ApproveBuy {
        fn check(&self, gate: &Gate<'_>) -> Verdict {
            match gate {
                Gate::Page {
                    target: Some(target),
                    ..
                } if target.name.contains("Buy") => {
                    Verdict::Approval(format!("{} {}", target.action, target.name))
                }
                _ => Verdict::Allow,
            }
        }
    }
    fixture.run("await page.goto(BASE + '/')").await;
    let stopped = fixture
        .run_with(
            "try { await page.getByRole('button', { name: 'Buy now' }).click() } catch (e) {} console.log('after'); await page.locator('#count').click(); 'finished'",
            Arc::new(ApproveBuy),
            Limits::default(),
        )
        .await;
    assert_eq!(
        stopped.stop,
        Some(Stop::Approval("click Buy now".into())),
        "{stopped:?}"
    );
    assert_eq!(stopped.return_value, Value::Null);
    let bought = fixture
        .run("await page.locator('#bought').textContent()")
        .await;
    assert_eq!(bought.return_value, json!("not bought"));

    struct DenyNavigation;
    impl Guard for DenyNavigation {
        fn check(&self, gate: &Gate<'_>) -> Verdict {
            match gate {
                Gate::Navigate { url } if url.contains("/list") => {
                    Verdict::Deny("blocked origin".into())
                }
                _ => Verdict::Allow,
            }
        }
    }
    let denied = fixture
        .run_with(
            "await page.goto(BASE + '/list')",
            Arc::new(DenyNavigation),
            Limits::default(),
        )
        .await;
    assert_eq!(denied.stop, Some(Stop::Denied("blocked origin".into())));
    assert_eq!(
        fixture.run("page.url()").await.return_value,
        json!(format!("{}/", fixture.server.url))
    );

    // Another session cannot drive this tab.
    let foreign = run(
        Arc::clone(&fixture.browser),
        "someone-else".into(),
        fixture.tab.clone(),
        "1".into(),
        Arc::new(AllowAll),
        Limits::default(),
    )
    .await;
    assert!(
        foreign
            .error
            .as_deref()
            .is_some_and(|error| error.contains("not owned")),
        "{foreign:?}"
    );

    let log = fixture.browser.take_protocol_log();
    for (method, _) in &log {
        assert!(
            !DOM_WRITE_METHODS.contains(&method.as_str()),
            "issued {method}"
        );
    }
    fixture.stop().await;
}
