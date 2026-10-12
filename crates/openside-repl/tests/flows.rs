//! Real-Chromium regressions for REPL flows that the Playwright compatibility table does not
//! cover: pages taller than the viewport, completion values that are promises, navigations that
//! never finish.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{sync::Arc, time::Duration};

use axum::{response::Html, routing::get, Router};
use openside_browser::{Browser, BrowserConfig};
use openside_repl::{run, Gate, Guard, Limits, Output, Verdict};
use serde_json::json;
use tokio::{sync::Mutex, task::JoinHandle};

const OWNER: &str = "repl-flows";
static SERIAL: Mutex<()> = Mutex::const_new(());

struct AllowAll;

impl Guard for AllowAll {
    fn check(&self, _gate: &Gate<'_>) -> Verdict {
        Verdict::Allow
    }
}

fn long_page() -> String {
    let rows: String = (0..300).map(|n| format!("<li>Row {n}</li>")).collect();
    format!(
        "<!doctype html><title>Long</title><h1>Long</h1><ul>{rows}</ul>\
         <button onclick=\"document.title='Bottom clicked'\">Bottom button</button>\
         <div style=\"height:3000px\"></div>\
         <button onclick=\"document.title='Far clicked'\">Far button</button>"
    )
}

struct Fixture {
    browser: Arc<Browser>,
    tab: String,
    url: String,
    server: JoinHandle<()>,
}

impl Fixture {
    async fn start() -> Option<Self> {
        let executable = match std::env::var_os("OPENSIDE_FLOW_BROWSER") {
            Some(path) => path.into(),
            None => match openside_config::discover_browser(&openside_config::Config::default()) {
                Ok(path) => path,
                Err(reason) => {
                    eprintln!("SKIP: REPL flows require Chromium: {reason}");
                    return None;
                }
            },
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
        let app = Router::new()
            .route("/long", get(|| async { Html(long_page()) }))
            .route(
                "/never",
                get(|| async {
                    tokio::time::sleep(Duration::from_secs(600)).await;
                    Html("<title>never</title>")
                }),
            )
            .route(
                "/ok",
                get(|| async { Html("<title>Ok</title><h1>ok</h1>") }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
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
async fn pointer_actions_scroll_targets_below_the_fold_into_view() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    fixture.run("await page.goto(BASE + '/long'); 1").await;
    for (button, title) in [
        ("Bottom button", "Bottom clicked"),
        ("Far button", "Far clicked"),
    ] {
        let output = fixture
            .run(&format!(
                "await page.getByRole('button', {{name:'{button}'}}).click(); await page.title()"
            ))
            .await;
        assert_eq!(output.return_value, json!(title), "{output:?}");
    }
    // Back at the top the first button is above the fold again.
    let output = fixture
        .run("await page.keyboard.press('Control+Home'); await page.getByRole('button', {name:'Bottom button'}).hover(); await page.title()")
        .await;
    assert_eq!(output.error, None, "{output:?}");
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_promise_as_the_last_expression_reports_its_value() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    let output = fixture
        .run("await page.goto(BASE + '/ok'); page.title()")
        .await;
    assert_eq!(output.return_value, json!("Ok"), "{output:?}");
    let rejected = fixture
        .run("Promise.reject(new Error('late failure'))")
        .await;
    assert_eq!(
        rejected.error.as_deref(),
        Some("Error: late failure"),
        "{rejected:?}"
    );
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_navigation_that_never_commits_times_out_and_leaves_the_tab_usable() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    fixture.run("await page.goto(BASE + '/ok'); 1").await;
    let started = std::time::Instant::now();
    let hung = fixture
        .run("await page.goto(BASE + '/never', { timeout: 1500 }); 1")
        .await;
    assert!(
        hung.error
            .as_deref()
            .is_some_and(|error| error.starts_with("TimeoutError: navigation timeout of 1500ms")),
        "{hung:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(6),
        "{:?}",
        started.elapsed()
    );
    // The abandoned load is cancelled, so the tab keeps its page and answers right away.
    let after = fixture.run("[page.url(), await page.title()]").await;
    assert_eq!(after.error, None, "{after:?}");
    assert_eq!(after.return_value[1], json!("Ok"), "{after:?}");
    let next = fixture
        .run("await page.goto(BASE + '/long'); await page.title()")
        .await;
    assert_eq!(next.return_value, json!("Long"), "{next:?}");
    fixture.stop().await;
}
