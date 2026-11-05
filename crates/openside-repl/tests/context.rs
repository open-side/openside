//! Real-Chromium tests for persistent REPL contexts: `page` starts unset, `openTab`/`getTabs`
//! attach tabs, bindings and tabs carry over between executions, approved reruns do not
//! redeclare, and work left running by one execution does not run in the next.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use axum::{response::Html, routing::get, Router};
use openside_browser::{Browser, BrowserConfig};
use openside_repl::{Context, Gate, Guard, Guards, Limits, Output, Request, Verdict};
use serde_json::json;
use tokio::{sync::Mutex, task::JoinHandle};

const OWNER: &str = "repl-context";
static SERIAL: Mutex<()> = Mutex::const_new(());

/// Asks for approval of the first click on "Buy", then allows everything.
#[derive(Default)]
struct Policy {
    approved: AtomicBool,
}

struct TabGuard(Arc<Policy>);

impl Guard for TabGuard {
    fn check(&self, gate: &Gate<'_>) -> Verdict {
        match gate {
            Gate::Page {
                target: Some(target),
                ..
            } if target.name == "Buy" && !self.0.approved.load(Ordering::Acquire) => {
                Verdict::Approval("click button \"Buy\"".into())
            }
            _ => Verdict::Allow,
        }
    }
}

struct TabGuards(Arc<Policy>);

impl Guards for TabGuards {
    fn for_tab(&self, _tab: &str) -> Arc<dyn Guard> {
        Arc::new(TabGuard(Arc::clone(&self.0)))
    }
}

struct Fixture {
    browser: Arc<Browser>,
    context: Option<Context>,
    policy: Arc<Policy>,
    /// The tabs the last execution left attached; the next one keeps them, like the daemon.
    kept: std::sync::Mutex<Vec<String>>,
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
                    eprintln!("SKIP: REPL context tests require Chromium: {reason}");
                    return None;
                }
            },
        };
        let (browser, _events) = Browser::launch(BrowserConfig::new(executable))
            .await
            .expect("launch chromium");
        let browser = Arc::new(browser);
        let app = Router::new()
            .route(
                "/shop",
                get(|| async {
                    Html("<title>Shop</title><button onclick=\"document.title='Bought'\">Buy</button>")
                }),
            )
            .route("/ok", get(|| async { Html("<title>Ok</title><h1>ok</h1>") }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Some(Self {
            context: Some(Context::new(Arc::clone(&browser), OWNER.into())),
            browser,
            policy: Arc::default(),
            kept: Default::default(),
            url,
            server,
        })
    }

    async fn run(&self, code: &str, select: Option<&str>) -> Output {
        let keep = self.kept.lock().unwrap().clone();
        self.run_keeping(code, select, keep).await
    }

    async fn run_keeping(&self, code: &str, select: Option<&str>, keep: Vec<String>) -> Output {
        let output = self
            .context
            .as_ref()
            .unwrap()
            .run(
                Request {
                    code: code.replace("BASE", &format!("{:?}", self.url)),
                    select: select.map(str::to_owned),
                    keep,
                    limits: Limits::default(),
                },
                Arc::new(TabGuards(Arc::clone(&self.policy))),
                std::future::pending(),
            )
            .await;
        self.kept.lock().unwrap().clone_from(&output.tabs);
        eprintln!(
            "{code}\n   -> {} {:?} page={:?} tabs={:?}",
            output.return_value, output.error, output.page, output.tabs
        );
        output
    }

    async fn stop(mut self) {
        self.server.abort();
        let context = self.context.take().unwrap();
        context.shutdown().await;
        assert!(context.has_ended());
        drop(context);
        let browser = Arc::try_unwrap(self.browser)
            .ok()
            .expect("a closed context has released the browser");
        browser.shutdown().await.expect("shutdown chromium");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bindings_and_opened_tabs_persist_across_executions() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    let neutral = fixture
        .run(
            "const answer = 41; [typeof page, (await getTabs()).length]",
            None,
        )
        .await;
    assert_eq!(neutral.error, None, "{neutral:?}");
    assert_eq!(neutral.return_value, json!(["undefined", 0]));
    assert_eq!((neutral.page.as_deref(), neutral.tabs.len()), (None, 0));

    let opened = fixture
        .run(
            "const first = await openTab(BASE + '/ok'); console.log(await first.title()); answer + 1",
            None,
        )
        .await;
    assert_eq!(opened.error, None, "{opened:?}");
    assert_eq!(opened.return_value, json!(42));
    assert_eq!(opened.console, vec!["Ok".to_owned()]);
    let tab = opened.page.clone().expect("openTab sets page");
    assert_eq!(opened.tabs, vec![tab.clone()]);

    // The page object, its tab, and the binding all carry over.
    let again = fixture
        .run(
            "await expect(page).toHaveTitle('Ok'); [first === page, page.url().endsWith('/ok'), (await getTabs()).length]",
            None,
        )
        .await;
    assert_eq!(again.error, None, "{again:?}");
    assert_eq!(again.return_value, json!([true, true, 1]));
    assert_eq!(again.page.as_deref(), Some(tab.as_str()));

    let redeclared = fixture.run("const answer = 1; answer", None).await;
    assert!(
        redeclared
            .error
            .as_deref()
            .is_some_and(|error| error.contains("redeclaration of 'answer'")),
        "{redeclared:?}"
    );

    // A second tab becomes page; both stay attached and reachable.
    let second = fixture
        .run(
            "const second = await openTab(BASE + '/shop'); const all = await getTabs(); [all.length, await all[0].title(), await page.title()]",
            None,
        )
        .await;
    assert_eq!(second.error, None, "{second:?}");
    assert_eq!(second.return_value, json!([2, "Ok", "Shop"]));
    assert_eq!(second.tabs.len(), 2);
    assert_ne!(second.page.as_deref(), Some(tab.as_str()));

    // Selecting a tab makes it page for this execution.
    let selected = fixture.run("await page.title()", Some(&tab)).await;
    assert_eq!(selected.return_value, json!("Ok"), "{selected:?}");
    assert_eq!(selected.page.as_deref(), Some(tab.as_str()));

    // A tab closed outside the REPL drops out of the context, and page was that tab.
    fixture.browser.close_agent_tab(OWNER, &tab).await.unwrap();
    let closed = fixture
        .run("[(await getTabs()).length, typeof page]", None)
        .await;
    assert_eq!(closed.return_value, json!([1, "undefined"]), "{closed:?}");
    assert_eq!(closed.tabs.len(), 1, "{closed:?}");
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_approved_rerun_does_not_redeclare_and_leftover_work_stops() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    let opened = fixture.run("await openTab(BASE + '/shop'); 1", None).await;
    assert_eq!(opened.error, None, "{opened:?}");
    let buy = "const before = await page.title(); await page.getByRole('button', {name: 'Buy'}).click(); [before, await page.title()]";
    let stopped = fixture.run(buy, None).await;
    assert!(stopped.stop.is_some(), "{stopped:?}");
    fixture.policy.approved.store(true, Ordering::Release);
    let rerun = fixture.run(buy, None).await;
    assert_eq!(rerun.error, None, "{rerun:?}");
    assert_eq!(rerun.return_value, json!(["Shop", "Bought"]));

    // A loop the snippet did not await must not keep driving the page afterwards.
    let started = fixture
        .run(
            "globalThis.hits = 0; (async () => { for (;;) { await page.title(); hits++; } })(); 'started'",
            None,
        )
        .await;
    assert_eq!(started.return_value, json!("started"), "{started:?}");
    let later = fixture
        .run(
            "const seen = hits; await page.waitForTimeout(300); hits - seen",
            None,
        )
        .await;
    assert_eq!(later.return_value, json!(0), "{later:?}");
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_kept_tabs_stay_attached_and_attachments_are_capped() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    let opened = fixture.run("await openTab(BASE + '/ok'); 1", None).await;
    assert_eq!(opened.error, None, "{opened:?}");
    let tab = opened.page.clone().unwrap();
    // A tab the caller no longer admits leaves the context, and page with it.
    let dropped = fixture
        .run_keeping("[(await getTabs()).length, typeof page]", None, Vec::new())
        .await;
    assert_eq!(dropped.return_value, json!([0, "undefined"]), "{dropped:?}");
    assert!(dropped.tabs.is_empty());
    // Reattaching it explicitly works; the old binding still reaches no tab until then.
    let back = fixture.run("await page.title()", Some(&tab)).await;
    assert_eq!(back.return_value, json!("Ok"), "{back:?}");

    let filled = fixture
        .run(
            &format!(
                "for (let i = 1; i < {}; i++) await openTab(); (await getTabs()).length",
                openside_repl::MAX_TABS
            ),
            None,
        )
        .await;
    assert_eq!(
        filled.return_value,
        json!(openside_repl::MAX_TABS),
        "{filled:?}"
    );
    let refused = fixture.run("await openTab()", None).await;
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|error| error.contains("tabs attached")),
        "{refused:?}"
    );
    assert_eq!(refused.tabs.len(), openside_repl::MAX_TABS);
    for tab in &refused.tabs {
        fixture.browser.close_agent_tab(OWNER, tab).await.unwrap();
    }
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_stops_the_running_execution_and_fails_queued_ones() {
    let _serial = SERIAL.lock().await;
    let Some(fixture) = Fixture::start().await else {
        return;
    };
    let opened = fixture.run("await openTab(BASE + '/ok'); 1", None).await;
    assert_eq!(opened.error, None, "{opened:?}");
    let started = std::time::Instant::now();
    let pause = |ms| tokio::time::sleep(std::time::Duration::from_millis(ms));
    let (running, queued, ()) = tokio::join!(
        fixture.run("await page.waitForTimeout(20000); 'finished'", None),
        async {
            pause(200).await;
            fixture.run("'ran'", None).await
        },
        async {
            pause(600).await;
            fixture.context.as_ref().unwrap().shutdown().await;
        },
    );
    assert!(
        running.error.is_some() && running.return_value != json!("finished"),
        "{running:?}"
    );
    assert_eq!(
        queued.error.as_deref(),
        Some("Error: REPL context closed"),
        "{queued:?}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    assert!(fixture.context.as_ref().unwrap().has_ended());
    let later = fixture.run("1", None).await;
    assert_eq!(later.error.as_deref(), Some("Error: REPL context ended"));
    fixture.stop().await;
}
