//! Persistent REPL contexts for external clients (`openside repl`).
//!
//! A [`Context`] owns one QuickJS runtime on a dedicated thread for its whole life, so top-level
//! `const`/`let`/`function` bindings of one execution are visible to the next. `page` starts
//! unset; `openTab(url)` opens an agent tab for the owner and attaches it, `getTabs()` returns a
//! page per attached tab, and a caller may attach an owned tab per execution. Every execution
//! re-authorizes every attached tab exactly like a single [`crate::run`]: each tab gets its own
//! driver, navigation guard and [`Guard`] for that execution, all tabs share one policy stop,
//! and nothing of a driver outlives its execution. Work a snippet left running when it ended is
//! stopped before the next execution starts.

use std::{
    cell::RefCell,
    collections::HashMap,
    future::Future,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use openside_browser::{Browser, NavigationGuard, OpenedBy};
use rquickjs::{
    prelude::Async, AsyncContext, AsyncRuntime, CatchResultExt, CaughtError, Function, Object,
    Persistent, Promise,
};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, watch};

use crate::{
    bounded,
    driver::{Console, Driver, Stops},
    interrupted, CancelOnDrop, Gate, Guard, Limits, Output, Stop, PRELUDE,
};

const GLOBALS: &str = include_str!("context.js");
const STOPPED: &str = "execution stopped";
const DETACHED: &str = "this tab is no longer attached to the REPL";
/// Most tabs one context keeps attached; a closed tab drops out and frees its place.
pub const MAX_TABS: usize = 8;

fn too_many_tabs() -> String {
    format!("the REPL already has {MAX_TABS} tabs attached; close one before attaching another")
}

/// Policy for the tabs of one execution of a [`Context`]; asked once per attached tab.
pub trait Guards: Send + Sync {
    /// The guard deciding every operation in `tab` during this execution.
    fn for_tab(&self, tab: &str) -> Arc<dyn Guard>;
}

/// One execution of a [`Context`].
pub struct Request {
    /// The snippet.
    pub code: String,
    /// An agent tab of the owner to attach and make `page` for this execution; the caller has
    /// already checked that the owner may use it.
    pub select: Option<String>,
    /// The attached tabs the caller still admits; every other attached tab is dropped before
    /// this execution, so a tab the caller never vetted is never touched.
    pub keep: Vec<String>,
    /// Resource bounds of this execution.
    pub limits: Limits,
}

/// A persistent REPL context. Executions run one at a time in submission order. Dropping the
/// context ends its thread once the execution in progress (already cancelled, since no
/// [`Context::run`] can outlive the context) has cleaned up; [`Context::shutdown`] ends it
/// while callers still hold it.
pub struct Context {
    jobs: mpsc::UnboundedSender<Job>,
    closing: watch::Sender<bool>,
    /// Its sender drops when the thread has released the runtime and the browser handle.
    ended: watch::Receiver<()>,
}

struct Job {
    request: Request,
    guards: Arc<dyn Guards>,
    cancel: Arc<AtomicBool>,
    reply: oneshot::Sender<Output>,
}

impl Context {
    /// Start a context whose tabs belong to `owner`.
    pub fn new(browser: Arc<Browser>, owner: String) -> Self {
        let (jobs, receiver) = mpsc::unbounded_channel();
        let (closing, close) = watch::channel(false);
        let (done, ended) = watch::channel(());
        // QuickJS is !Send, so the runtime lives on its own thread; a failed spawn drops the
        // receiver and every run reports the context as ended.
        let _ = std::thread::Builder::new()
            .name("openside-repl-context".into())
            .spawn(move || {
                serve(browser, owner, receiver, close);
                drop(done);
            });
        Self {
            jobs,
            closing,
            ended,
        }
    }

    /// Stop the execution in progress, fail every queued and later one without running it,
    /// and wait until the thread has released the runtime and the browser.
    pub async fn shutdown(&self) {
        self.closing.send_replace(true);
        let mut ended = self.ended.clone();
        while ended.changed().await.is_ok() {}
    }

    /// Whether the thread has ended.
    pub fn has_ended(&self) -> bool {
        self.ended.has_changed().is_err()
    }

    /// Run one snippet. [`Output::page`] is the tab of `page` afterwards and [`Output::tabs`]
    /// every attached tab. Resolving `cancellation` stops the snippet and waits for its cleanup.
    pub async fn run(
        &self,
        request: Request,
        guards: Arc<dyn Guards>,
        cancellation: impl Future<Output = ()>,
    ) -> Output {
        let cancel = Arc::new(AtomicBool::new(false));
        let (reply, receiver) = oneshot::channel();
        let job = Job {
            request,
            guards,
            cancel: Arc::clone(&cancel),
            reply,
        };
        if self.jobs.send(job).is_err() {
            return Output::failed("Error: REPL context ended");
        }
        let _cancel = CancelOnDrop(Arc::clone(&cancel));
        tokio::pin!(receiver);
        tokio::select! {
            biased;
            () = cancellation => cancel.store(true, Ordering::Release),
            result = &mut receiver => {
                return result.unwrap_or_else(|_| Output::failed("Error: REPL context ended unexpectedly"));
            }
        }
        receiver
            .await
            .unwrap_or_else(|_| Output::failed("Error: REPL context ended unexpectedly"))
    }
}

fn serve(
    browser: Arc<Browser>,
    owner: String,
    mut jobs: mpsc::UnboundedReceiver<Job>,
    mut closing: watch::Receiver<bool>,
) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
    else {
        return;
    };
    runtime.block_on(async move {
        let state = State::new(browser, owner).await;
        loop {
            let job = tokio::select! {
                biased;
                _ = closing.wait_for(|closing| *closing) => break,
                job = jobs.recv() => match job {
                    Some(job) => job,
                    None => break,
                },
            };
            let state = match &state {
                Ok(state) => state,
                Err(message) => {
                    let _ = job.reply.send(Output::failed(message.clone()));
                    continue;
                }
            };
            let (cancel, mut stop) = (Arc::clone(&job.cancel), closing.clone());
            // Closing cancels the execution, which still runs its cleanup before returning.
            let closed = async move {
                let _ = stop.wait_for(|closing| *closing).await;
                cancel.store(true, Ordering::Release);
                std::future::pending::<Output>().await
            };
            let output = tokio::select! {
                output = state.execute(job.request, job.guards, job.cancel) => output,
                output = closed => output,
            };
            let _ = job.reply.send(output);
        }
        jobs.close();
        while let Ok(job) = jobs.try_recv() {
            let _ = job.reply.send(Output::failed("Error: REPL context closed"));
        }
        drop(state);
    });
}

/// Host side of the context, reached from JavaScript through `op` and `log`.
struct Host {
    browser: Arc<Browser>,
    owner: String,
    /// Attached tabs in attachment order.
    attached: RefCell<Vec<String>>,
    run: RefCell<Option<Run>>,
}

/// Per-execution state; `None` between executions, so leftover work reaches no tab.
struct Run {
    drivers: HashMap<String, Rc<Driver>>,
    navigation: Vec<NavigationGuard>,
    guards: Arc<dyn Guards>,
    limits: Limits,
    deadline: Instant,
    cancel: Arc<AtomicBool>,
    stops: Stops,
    console: Console,
}

impl Run {
    fn stopped(&self) -> bool {
        self.stops.current().is_some()
            || self.stops.aborted().load(Ordering::Acquire)
            || self.cancel.load(Ordering::Acquire)
            || Instant::now() >= self.deadline
    }
}

fn reply(result: Result<Value, String>) -> String {
    match result {
        Ok(value) => json!({ "value": value }).to_string(),
        Err(error) => json!({ "error": error, "name": "Error" }).to_string(),
    }
}

impl Host {
    fn log(&self, line: String) {
        if let Some(run) = self.run.borrow_mut().as_mut() {
            run.console.push(line, run.limits.console_bytes);
        }
    }

    async fn op(&self, tab: String, name: String, payload: String) -> String {
        if tab.is_empty() {
            return reply(match name.as_str() {
                "open_tab" => self.open_tab().await,
                "tabs" => self.tabs().await,
                _ => Err(format!(
                    "{name} needs a page; open one with openTab(url) or pick one from getTabs()"
                )),
            });
        }
        let driver = self
            .run
            .borrow()
            .as_ref()
            .and_then(|run| run.drivers.get(&tab).cloned());
        match driver {
            Some(driver) => driver.dispatch(&name, &payload).await,
            None => reply(Err(DETACHED.into())),
        }
    }

    async fn open_tab(&self) -> Result<Value, String> {
        let cancel = match self.run.borrow().as_ref() {
            Some(run) if !run.stopped() => Arc::clone(&run.cancel),
            _ => return Err(STOPPED.into()),
        };
        if self.attached.borrow().len() >= MAX_TABS {
            return Err(too_many_tabs());
        }
        let cancelled = async {
            while !cancel.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        };
        let tab = self
            .browser
            .create_agent_tab_cancellable(&self.owner, "about:blank", cancelled)
            .await
            .map_err(|error| format!("could not open a tab: {error}"))?;
        self.attached.borrow_mut().push(tab.id.clone());
        let url = self.prepare(&tab.id).await.inspect_err(|_| {
            self.attached.borrow_mut().retain(|known| *known != tab.id);
        })?;
        Ok(json!({ "tab_id": tab.id, "url": url }))
    }

    /// The attached tabs that still exist, dropping the others.
    async fn tabs(&self) -> Result<Value, String> {
        let live = self.live().await;
        let drivers = self
            .run
            .borrow()
            .as_ref()
            .map(|run| run.drivers.clone())
            .unwrap_or_default();
        Ok(Value::Array(
            live.into_iter()
                .filter_map(|(tab, url)| {
                    let driver = drivers.get(&tab)?;
                    let known = driver.url.borrow().clone();
                    Some(
                        json!({ "tab_id": tab, "url": if known.is_empty() { url } else { known } }),
                    )
                })
                .collect(),
        ))
    }

    async fn live(&self) -> Vec<(String, String)> {
        let tabs = self.browser.tabs().await;
        let mut live = Vec::new();
        self.attached.borrow_mut().retain(|id| {
            let found = tabs.iter().find(|tab| {
                tab.id == *id
                    && tab.opened_by == OpenedBy::Agent
                    && tab.owner_session_id.as_deref() == Some(self.owner.as_str())
            });
            if let Some(tab) = found {
                live.push((tab.id.clone(), tab.url.clone()));
            }
            found.is_some()
        });
        live
    }

    /// Give `tab` a driver and navigation guard for the current execution; returns its URL.
    async fn prepare(&self, tab: &str) -> Result<String, String> {
        let (guard, limits, deadline, cancel, stops) = match self.run.borrow().as_ref() {
            Some(run) => (
                run.guards.for_tab(tab),
                run.limits.clone(),
                run.deadline,
                Arc::clone(&run.cancel),
                run.stops.clone(),
            ),
            None => return Err(STOPPED.into()),
        };
        let driver = Rc::new(Driver::new(
            Arc::clone(&self.browser),
            self.owner.clone(),
            tab.to_owned(),
            Arc::clone(&guard),
            limits,
            deadline,
            cancel,
            stops.clone(),
        ));
        let url = driver
            .refresh_url()
            .await
            .map_err(|_| "tab is unavailable or not owned by this session".to_owned())?;
        // Without interception a page could navigate anywhere unchecked, so the tab stays out.
        let navigation = self
            .browser
            .guard_navigation(
                &self.owner,
                tab,
                Arc::new(move |url: &str| stops.allow(guard.check(&Gate::Navigate { url }))),
            )
            .await
            .map_err(|_| "navigation guard unavailable for this tab".to_owned())?;
        // Background agent tabs need emulated focus for focus/keyboard semantics.
        driver
            .cleanup_cmd(
                "Emulation.setFocusEmulationEnabled",
                json!({"enabled": true}),
            )
            .await;
        match self.run.borrow_mut().as_mut() {
            Some(run) => {
                run.drivers.insert(tab.to_owned(), driver);
                run.navigation.push(navigation);
                Ok(url)
            }
            None => Err(STOPPED.into()),
        }
    }
}

// Fields drop in order: the saved JavaScript object must go before the runtime that owns it.
struct State {
    api: Persistent<Object<'static>>,
    context: AsyncContext,
    runtime: AsyncRuntime,
    host: Rc<Host>,
    /// Tab of `page` after the last execution that reported it.
    page: RefCell<Option<String>>,
    /// The snippet of the last execution if a policy stop ended it; see [`State::execute`].
    stopped_code: RefCell<Option<String>>,
}

impl State {
    async fn new(browser: Arc<Browser>, owner: String) -> Result<Self, String> {
        let unavailable = |_| "Error: JavaScript runtime unavailable".to_owned();
        let runtime = AsyncRuntime::new().map_err(unavailable)?;
        runtime
            .set_memory_limit(Limits::default().memory_bytes)
            .await;
        runtime.set_max_stack_size(512 * 1024).await;
        let context = AsyncContext::full(&runtime).await.map_err(unavailable)?;
        let host = Rc::new(Host {
            browser,
            owner,
            attached: RefCell::default(),
            run: RefCell::default(),
        });
        let shared = Rc::clone(&host);
        let api = context
            .async_with(
                async move |ctx| -> Result<Persistent<Object<'static>>, String> {
                    let failed = |error: String| format!("Error: REPL prelude failed: {error}");
                    let operations = Rc::clone(&shared);
                    let op =
                        Function::new(
                            ctx.clone(),
                            Async(move |tab: String, name: String, payload: String| {
                                let host = Rc::clone(&operations);
                                async move {
                                    Ok::<_, rquickjs::Error>(host.op(tab, name, payload).await)
                                }
                            }),
                        )
                        .map_err(|error| failed(error.to_string()))?;
                    let log = Function::new(ctx.clone(), move |line: String| shared.log(line))
                        .map_err(|error| failed(error.to_string()))?;
                    let factory: Function = ctx
                        .eval(PRELUDE)
                        .catch(&ctx)
                        .map_err(|error| failed(error.to_string()))?;
                    let globals: Function = ctx
                        .eval(GLOBALS)
                        .catch(&ctx)
                        .map_err(|error| failed(error.to_string()))?;
                    let api: Object = globals
                        .call((factory, op, log))
                        .catch(&ctx)
                        .map_err(|error| failed(error.to_string()))?;
                    Ok(Persistent::save(&ctx, api))
                },
            )
            .await?;
        Ok(Self {
            runtime,
            context,
            host,
            api,
            page: RefCell::default(),
            stopped_code: RefCell::default(),
        })
    }

    async fn execute(
        &self,
        request: Request,
        guards: Arc<dyn Guards>,
        cancel: Arc<AtomicBool>,
    ) -> Output {
        let Request {
            code,
            select,
            keep,
            limits,
        } = request;
        let deadline = Instant::now() + limits.timeout;
        let stops = Stops::new();
        *self.host.run.borrow_mut() = Some(Run {
            drivers: HashMap::new(),
            navigation: Vec::new(),
            guards,
            limits: limits.clone(),
            deadline,
            cancel: Arc::clone(&cancel),
            stops: stops.clone(),
            console: Console::default(),
        });
        self.runtime.set_memory_limit(limits.memory_bytes).await;
        let (abort, flag) = (stops.aborted(), Arc::clone(&cancel));
        self.runtime
            .set_interrupt_handler(Some(Box::new(move || {
                abort.load(Ordering::Acquire)
                    || flag.load(Ordering::Acquire)
                    || Instant::now() >= deadline
            })))
            .await;
        self.host
            .attached
            .borrow_mut()
            .retain(|tab| keep.contains(tab));
        let outcome = match self.attach(select.as_deref()).await {
            Ok(tabs) => {
                // A snippet stopped for approval already declared its top-level bindings, so
                // the approved rerun of the same code runs as a block instead of redeclaring.
                let code = if self.stopped_code.borrow().as_deref() == Some(code.as_str()) {
                    format!("{{\n{code}\n}}")
                } else {
                    code.clone()
                };
                let evaluation = self.evaluate(code, tabs, select);
                let watch = async {
                    while !cancel.load(Ordering::Acquire) {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                };
                tokio::select! {
                    result = tokio::time::timeout_at(deadline.into(), evaluation) => match result {
                        Ok(result) => result,
                        Err(_) => Err(format!("TimeoutError: REPL execution exceeded {}ms", limits.timeout.as_millis())),
                    },
                    () = watch => Err("Error: REPL execution cancelled".into()),
                }
            }
            Err(message) => Err(format!("Error: {message}")),
        };
        let console = self.finish_run(&cancel).await;
        let stop = stops.current();
        let (return_value, mut error) = match outcome {
            Ok((Ok(value), page)) => {
                *self.page.borrow_mut() = page;
                (bounded(value, limits.return_bytes), None)
            }
            Ok((Err(error), page)) => {
                *self.page.borrow_mut() = page;
                (Value::Null, Some(error))
            }
            Err(error) => (Value::Null, Some(error)),
        };
        if let Some(stop) = &stop {
            error = Some(match stop {
                Stop::Denied(reason) => format!("PolicyError: policy denied: {reason}"),
                Stop::Approval(action) => format!("PolicyError: human approval required: {action}"),
            });
        }
        *self.stopped_code.borrow_mut() = stop.is_some().then_some(code);
        let tabs = self.host.attached.borrow().clone();
        let page = self
            .page
            .borrow()
            .clone()
            .filter(|page| tabs.contains(page));
        Output {
            return_value,
            console,
            error,
            stop,
            page,
            tabs,
        }
    }

    /// Prepare every attached tab, attaching `select` first if needed. Tabs that are gone or
    /// cannot be guarded are detached; failing to prepare `select` fails the execution.
    async fn attach(&self, select: Option<&str>) -> Result<Vec<Value>, String> {
        if let Some(tab) = select {
            let mut attached = self.host.attached.borrow_mut();
            if !attached.iter().any(|known| known == tab) {
                if attached.len() >= MAX_TABS {
                    return Err(too_many_tabs());
                }
                attached.push(tab.to_owned());
            }
        }
        let mut tabs = Vec::new();
        let attached = self.host.attached.borrow().clone();
        for tab in attached {
            match self.host.prepare(&tab).await {
                Ok(url) => tabs.push(json!({ "tab_id": tab, "url": url })),
                Err(message) if select == Some(tab.as_str()) => {
                    self.host
                        .attached
                        .borrow_mut()
                        .retain(|known| *known != tab);
                    return Err(message);
                }
                Err(_) => self
                    .host
                    .attached
                    .borrow_mut()
                    .retain(|known| *known != tab),
            }
        }
        Ok(tabs)
    }

    /// Restore every tab, end the execution, and stop work the snippet left running.
    async fn finish_run(&self, cancel: &AtomicBool) -> Vec<String> {
        let drivers: Vec<Rc<Driver>> = self
            .host
            .run
            .borrow()
            .as_ref()
            .map(|run| run.drivers.values().cloned().collect())
            .unwrap_or_default();
        let _ = tokio::time::timeout(
            Duration::from_secs(3),
            futures_util::future::join_all(drivers.iter().map(|driver| async move {
                driver.cleanup_scripts().await;
                driver.end_events().await;
                driver.release_input().await;
                driver
                    .cleanup_cmd(
                        "Emulation.setFocusEmulationEnabled",
                        json!({"enabled": false}),
                    )
                    .await;
            })),
        )
        .await;
        drop(drivers);
        // Every operation of this execution now fails, and the interrupt handler ends any
        // JavaScript still queued, so nothing carries into the next execution.
        cancel.store(true, Ordering::Release);
        let run = self.host.run.borrow_mut().take();
        let console = run.map(|run| run.console.lines).unwrap_or_default();
        let _ = tokio::time::timeout(Duration::from_secs(1), self.runtime.idle()).await;
        console
    }

    /// `Ok((result, page))`, where `page` is the tab of `page` when the snippet ended.
    async fn evaluate(
        &self,
        code: String,
        tabs: Vec<Value>,
        select: Option<String>,
    ) -> Result<(Result<Value, String>, Option<String>), String> {
        let api = self.api.clone();
        let page = self.page.borrow().clone();
        self.context
            .async_with(
                async move |ctx| -> Result<(Result<Value, String>, Option<String>), String> {
                    let failed = |_| "Error: REPL context unavailable".to_owned();
                    let api = api.restore(&ctx).map_err(failed)?;
                    let start: Function = api.get("start").map_err(failed)?;
                    let finish: Function = api.get("finish").map_err(failed)?;
                    let describe: Function = api.get("describe").map_err(failed)?;
                    let tabs = ctx
                        .json_parse(Value::Array(tabs).to_string())
                        .map_err(failed)?;
                    let () = start
                        .call((tabs, select))
                        .catch(&ctx)
                        .map_err(|error| format!("Error: REPL context failed: {error}"))?;
                    let promise = match ctx.eval_promise(code).catch(&ctx) {
                        Ok(promise) => promise,
                        Err(CaughtError::Exception(exception)) => {
                            let message: String = describe
                                .call((exception,))
                                .unwrap_or_else(|_| "SyntaxError".into());
                            return Ok((Err(message), page));
                        }
                        Err(error) => return Ok((Err(format!("Error: {error}")), page)),
                    };
                    let finished: Promise = finish.call((promise,)).map_err(failed)?;
                    let reply: String = match finished.into_future::<String>().await.catch(&ctx) {
                        Ok(reply) => reply,
                        Err(error) => return Err(interrupted(&error.to_string())),
                    };
                    let reply: Value = serde_json::from_str(&reply)
                        .map_err(|_| "Error: invalid REPL result".to_owned())?;
                    let page = reply["page"].as_str().map(str::to_owned);
                    Ok(if reply["ok"] == true {
                        (Ok(reply["value"].clone()), page)
                    } else {
                        (
                            Err(reply["error"]
                                .as_str()
                                .map(interrupted)
                                .unwrap_or_else(|| "Error: unknown failure".into())),
                            page,
                        )
                    })
                },
            )
            .await
    }
}
