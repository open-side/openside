//! Policy enforcement of the openside REPL against mock pages that try to slip an operation
//! past the guard: page-initiated and redirected navigation to a disallowed origin, overlays,
//! implicit form submission, sensitive select options, focus moved by the page or into a
//! frame, concurrent operations after a stop, and host resource budgets.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use axum::{
    response::{Html, Redirect},
    routing::get,
    Router,
};
use openside_browser::{Browser, BrowserConfig};
use openside_repl::{run, Gate, Guard, Limits, Output, Stop, Verdict};
use serde_json::{json, Value};
use tokio::{sync::Mutex, task::JoinHandle};

const OWNER: &str = "repl-policy";
static SERIAL: Mutex<()> = Mutex::const_new(());

/// Allows only the `127.0.0.1` origin and asks before any target whose name holds a
/// payment or deletion keyword, the way the daemon guard does.
struct SiteGuard {
    allowed: String,
}

impl Guard for SiteGuard {
    fn check(&self, gate: &Gate<'_>) -> Verdict {
        let origin_ok = |url: &str| url == "about:blank" || url.starts_with(&self.allowed);
        match gate {
            Gate::Navigate { url } if origin_ok(url) => Verdict::Allow,
            Gate::Navigate { url } => Verdict::Deny(format!("navigation to {url}")),
            Gate::Page { origin, .. } if *origin != "null" && !origin_ok(origin) => {
                Verdict::Deny(format!("page on {origin}"))
            }
            Gate::Page {
                target: Some(target),
                ..
            } => {
                let name = target.name.to_lowercase();
                if ["pay", "delete"].iter().any(|word| name.contains(word)) {
                    Verdict::Approval(format!("{} {} {}", target.action, target.role, target.name))
                } else {
                    Verdict::Allow
                }
            }
            Gate::Page { .. } => Verdict::Allow,
            Gate::Script { .. } => Verdict::Allow,
            Gate::Upload { .. } | Gate::Download { .. } => Verdict::Deny("file transfer".into()),
        }
    }
}

struct Server {
    allowed: String,
    other: String,
    hits: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}

impl Server {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let allowed = format!("http://127.0.0.1:{port}");
        // Same server under another host name: a different origin for the guard.
        let other = format!("http://localhost:{port}");
        let hits = Arc::new(AtomicUsize::new(0));
        let target = format!("{other}/target");
        let app = Router::new()
            .route(
                "/nav",
                get({
                    let target = target.clone();
                    move || async move {
                        Html(format!(
                            r#"<!doctype html><title>Nav</title>
<a id="away" href="{target}">Elsewhere</a>
<a id="redirect" href="/redirect">Redirected</a>
<button id="script" onclick="location.href='{target}'">Scripted</button>"#
                        ))
                    }
                }),
            )
            .route("/redirect", get(move || async move { Redirect::temporary(&target) }))
            .route(
                "/target",
                get({
                    let hits = Arc::clone(&hits);
                    move || async move {
                        hits.fetch_add(1, Ordering::SeqCst);
                        Html("<!doctype html><title>Target</title><p id=secret>secret</p>")
                    }
                }),
            )
            .route(
                "/overlay",
                get(|| async {
                    Html(
                        r#"<!doctype html><title>Overlay</title>
<button id="plain" onclick="document.querySelector('#log').textContent='button'">Continue</button>
<p id="log">none</p>
<div id="cover" onclick="document.querySelector('#log').textContent='overlay'"
  style="position:fixed;inset:0;background:rgba(0,0,0,.1)"></div>"#,
                    )
                }),
            )
            .route(
                "/pay",
                get(|| async {
                    Html(
                        r#"<!doctype html><title>Checkout</title>
<form action="/paid"><label>Note <input id="note" name="note"></label><button>Pay now</button></form>"#,
                    )
                }),
            )
            .route(
                "/steal",
                get(|| async {
                    Html(
                        r#"<!doctype html><title>Steal</title>
<label>Search <input id="search" oninput="document.querySelector('#danger').focus()"></label>
<button id="danger" onclick="document.querySelector('#log').textContent='deleted'">Delete everything</button>
<p id="log">kept</p>"#,
                    )
                }),
            )
            .route(
                "/paid",
                get(|| async { Html("<!doctype html><title>Paid</title><p>paid</p>") }),
            )
            .route(
                "/select",
                get(|| async {
                    Html(
                        r#"<!doctype html><title>Select</title>
<label>Account action <select id="action"><option>Keep</option><option>Rename</option><option>Delete account</option></select></label>"#,
                    )
                }),
            )
            .route(
                "/frame",
                get(|| async {
                    Html(
                        r#"<!doctype html><title>Frame</title>
<label>Top <input id="top"></label>
<iframe srcdoc="<input id=inner>" onload="this.contentDocument.querySelector('input').focus()"></iframe>"#,
                    )
                }),
            )
            .route(
                "/huge",
                get(|| async {
                    Html(format!(
                        r#"<!doctype html><title>Huge</title><p data-x="{}">big</p>"#,
                        "x".repeat(5 * 1024 * 1024)
                    ))
                }),
            )
            .route(
                "/many",
                get(|| async {
                    Html(
                        r#"<!doctype html><title>Many</title><body>
<script>for (let i = 0; i < 210000; i++) document.body.appendChild(document.createElement('i'))</script></body>"#,
                    )
                }),
            )
            .route(
                "/relabel",
                get(|| async {
                    Html(
                        r#"<!doctype html><title>Relabel</title>
<button id="hover" onmousemove="this.textContent='Delete account'"
  onclick="document.querySelector('#log').textContent='hover'">Continue</button>
<button id="key" onkeydown="this.textContent='Delete all'"
  onclick="document.querySelector('#log').textContent='key'">Next</button>
<p id="log">none</p>
<button id="blank" style="width:40px;height:20px"
  onclick="document.querySelector('#log').textContent='blank'"></button>
<button id="multi" onclick="const n = document.querySelector('#n'); n.textContent = String(Number(n.textContent) + 1)">More</button>
<p id="n">0</p>"#,
                    )
                }),
            )
            .route(
                "/shop",
                get(|| async {
                    Html(
                        r#"<!doctype html><title>Shop</title>
<button onclick="document.querySelector('#paid').textContent='paid'">Pay</button>
<button id="count" onclick="this.textContent=String(Number(this.textContent)+1)">0</button>
<p id="paid">unpaid</p>"#,
                    )
                }),
            );
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            allowed,
            other,
            hits,
            task,
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
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

    async fn run_limited(&self, code: &str, limits: Limits) -> Output {
        let code = format!("const BASE = {:?};\n{code}", self.server.allowed);
        let guard = Arc::new(SiteGuard {
            allowed: self.server.allowed.clone(),
        });
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

    async fn run(&self, code: &str) -> Output {
        let limits = Limits {
            action_timeout: Duration::from_secs(2),
            ..Limits::default()
        };
        self.run_limited(code, limits).await
    }

    async fn value(&self, code: &str) -> Value {
        let output = self.run(code).await;
        assert_eq!(output.stop, None, "{code}: {output:?}");
        assert_eq!(output.error, None, "{code}: {output:?}");
        output.return_value
    }

    async fn stop(self) {
        drop(self.server);
        let browser = Arc::try_unwrap(self.browser)
            .ok()
            .expect("REPL runs released the browser");
        browser.shutdown().await.expect("shutdown chromium");
    }
}

fn denied(output: &Output) -> bool {
    matches!(&output.stop, Some(Stop::Denied(_)))
}

fn approval(output: &Output, contains: &str) -> bool {
    matches!(&output.stop, Some(Stop::Approval(action)) if action.contains(contains))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn page_initiated_and_redirected_navigation_to_a_disallowed_origin_is_blocked() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    let nav = format!("{}/nav", fixture.server.allowed);
    for (step, code) in [
        ("link", "await page.locator('#away').click(); await page.waitForTimeout(500); await page.title()"),
        ("redirect link", "await page.locator('#redirect').click(); await page.waitForTimeout(500); await page.title()"),
        ("script", "await page.locator('#script').click(); await page.waitForTimeout(500); await page.title()"),
        ("redirected goto", "await page.goto(BASE + '/redirect'); await page.title()"),
    ] {
        fixture.value("await page.goto(BASE + '/nav')").await;
        let output = fixture.run(code).await;
        assert!(denied(&output), "{step}: {output:?}");
        assert_eq!(output.return_value, Value::Null, "{step}: {output:?}");
        assert_eq!(
            fixture.server.hits.load(Ordering::SeqCst),
            0,
            "{step}: the disallowed document was requested"
        );
        assert_eq!(fixture.value("page.url()").await, json!(nav), "{step}");
    }
    // The other origin really is reachable; only the guard kept the page away from it.
    assert!(fixture.server.other.starts_with("http://localhost:"));
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn input_reaches_only_the_authorized_target() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };

    // An overlay above the button would take the click: the click times out and nothing is hit.
    fixture.value("await page.goto(BASE + '/overlay')").await;
    let covered = fixture
        .run("await page.getByRole('button', { name: 'Continue' }).click({ timeout: 800 })")
        .await;
    assert!(
        covered
            .error
            .as_deref()
            .is_some_and(|error| error.contains("receive pointer events")),
        "{covered:?}"
    );
    assert_eq!(
        fixture
            .value("await page.locator('#log').textContent()")
            .await,
        json!("none")
    );

    // Enter in a neutral field submits the payment form, so it is authorized as that submission.
    for code in [
        "await page.locator('#note').press('Enter')",
        "await page.locator('#note').type('gift\\n')",
        "await page.locator('#note').focus(); await page.keyboard.press('Enter')",
    ] {
        fixture.value("await page.goto(BASE + '/pay')").await;
        let output = fixture.run(code).await;
        assert!(approval(&output, "submit form"), "{code}: {output:?}");
        assert!(approval(&output, "Pay now"), "{code}: {output:?}");
        assert_eq!(fixture.value("await page.title()").await, json!("Checkout"));
    }

    // Choosing a sensitive option is authorized by the option, not the neutral select label.
    fixture.value("await page.goto(BASE + '/select')").await;
    assert_eq!(
        fixture
            .value("await page.locator('#action').selectOption('Rename')")
            .await,
        json!(["Rename"])
    );
    let output = fixture
        .run("await page.getByLabel('Account action').selectOption('Delete account')")
        .await;
    assert!(approval(&output, "Delete account"), "{output:?}");
    assert_eq!(
        fixture
            .value("await page.locator('#action').inputValue()")
            .await,
        json!("Rename"),
        "no key was sent once the walk would pass a sensitive option"
    );

    // The page moves focus to a sensitive button mid-typing; the next key is re-authorized.
    fixture.value("await page.goto(BASE + '/steal')").await;
    let output = fixture
        .run("await page.locator('#search').type('a\\n')")
        .await;
    assert!(approval(&output, "Delete everything"), "{output:?}");
    assert_eq!(
        fixture
            .value("await page.locator('#log').textContent()")
            .await,
        json!("kept")
    );

    // A neutral target relabelled by the page between input events is re-authorized before
    // the next event: the press after a hover, and the key-up after a key-down.
    for (code, label) in [
        ("await page.locator('#hover').click()", "Delete account"),
        (
            "await page.locator('#key').focus(); await page.keyboard.press('Space')",
            "Delete all",
        ),
    ] {
        fixture.value("await page.goto(BASE + '/relabel')").await;
        let output = fixture.run(code).await;
        assert!(approval(&output, label), "{code}: {output:?}");
        assert_eq!(
            fixture
                .value("await page.locator('#log').textContent()")
                .await,
            json!("none"),
            "{code}"
        );
    }

    // A present accessibility node with an empty name is still decided on and acted upon (the
    // missing-node refusal is covered by the unit test `dom::tests::policy_needs_the_ax_node`:
    // Chromium reports a node, ignored or not, for every element).
    fixture.value("await page.goto(BASE + '/relabel')").await;
    fixture.value("await page.locator('#blank').click()").await;
    assert_eq!(
        fixture
            .value("await page.locator('#log').textContent()")
            .await,
        json!("blank")
    );

    // clickCount is bounded; an out-of-range count sends no event at all.
    for count in [0, 4, 1000] {
        let output = fixture
            .run(&format!(
                "await page.locator('#multi').click({{ clickCount: {count} }})"
            ))
            .await;
        assert!(
            output
                .error
                .as_deref()
                .is_some_and(|error| error.contains("clickCount must be between 1 and 3")),
            "{count}: {output:?}"
        );
    }
    fixture
        .value("await page.locator('#multi').click({ clickCount: 3 })")
        .await;
    assert_eq!(
        fixture
            .value("await page.locator('#n').textContent()")
            .await,
        json!("3")
    );

    // Keys aimed at another frame reach the element focused there, authorized in that frame.
    fixture
        .value("await page.goto(BASE + '/frame'); await page.waitForTimeout(300)")
        .await;
    fixture.value("await page.keyboard.type('x')").await;
    assert_eq!(
        fixture
            .value("[await page.locator('#top').inputValue(), await page.frameLocator('iframe').locator('#inner').inputValue()]")
            .await,
        json!(["", "x"])
    );
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stops_are_barriers_and_host_budgets_hold() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };

    // Concurrent operations: once the first stops for approval, the other sends nothing.
    fixture.value("await page.goto(BASE + '/shop')").await;
    fixture.browser.record_protocol(true);
    let output = fixture
        .run("await Promise.all([page.getByRole('button', { name: 'Pay' }).click(), page.locator('#count').click(), page.locator('#count').click()])")
        .await;
    let log = fixture.browser.take_protocol_log();
    fixture.browser.record_protocol(false);
    assert!(approval(&output, "Pay"), "{output:?}");
    assert!(
        !log.iter()
            .any(|(method, _)| method == "Input.dispatchMouseEvent"),
        "an input event was sent after the stop"
    );
    assert_eq!(
        fixture
            .value("[await page.locator('#paid').textContent(), await page.locator('#count').textContent()]")
            .await,
        json!(["unpaid", "0"])
    );

    // Empty console lines still cost budget.
    let output = fixture
        .run("for (let i = 0; i < 100000; i++) console.log(''); 'done'")
        .await;
    assert_eq!(output.return_value, json!("done"));
    assert!(output.console.len() < 300, "{} lines", output.console.len());
    assert!(output.console.last().unwrap().contains("truncated"));

    // Promise-job floods and catastrophic regular expressions end at the deadline.
    let short = Limits {
        timeout: Duration::from_millis(500),
        ..Limits::default()
    };
    for code in [
        "const loop = () => Promise.resolve().then(loop); loop(); await new Promise(() => {})",
        "/^(a+)+$/.test('a'.repeat(40) + 'b')",
    ] {
        let started = Instant::now();
        let output = fixture.run_limited(code, short.clone()).await;
        assert!(output.error.is_some(), "{code}: {output:?}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{code} took {:?}",
            started.elapsed()
        );
    }

    // Page data is bounded on the host: a reply larger than the reply budget and a document
    // past the node budget fail closed instead of being copied whole.
    fixture.value("await page.goto(BASE + '/huge')").await;
    let output = fixture.run("await page.content()").await;
    assert!(
        output
            .error
            .as_deref()
            .is_some_and(|error| error.contains("too large")),
        "{output:?}"
    );
    fixture.value("await page.goto(BASE + '/many')").await;
    for code in [
        "await page.locator('i').count()",
        "await page.content()",
        "await page.getByRole('button').count()",
    ] {
        let output = fixture.run(code).await;
        assert!(
            output
                .error
                .as_deref()
                .is_some_and(|error| error.contains("too large")),
            "{code}: {output:?}"
        );
    }
    fixture.stop().await;
}
