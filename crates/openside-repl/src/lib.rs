//! Sandboxed Playwright-syntax JavaScript REPL over an owned openside browser tab.
//!
//! Code runs in a fresh QuickJS context (rquickjs, ADR-021) on a dedicated thread. The only
//! globals beyond the ECMAScript standard library are `page`, `expect`, and `console`; there is
//! no module loader, `require`, process, filesystem, network, or Vault API. The Playwright
//! prelude is JavaScript inside QuickJS whose operations are Rust host functions: locators are
//! resolved from CDP DOM and accessibility trees in Rust, and actions are CDP Input/DOM/Page
//! commands from [`openside_browser::PAGE_COMMANDS`]. Nothing is injected into or evaluated in
//! the page. `page.locator('aria-ref=eN')` resolves the same node as snapshot reference `eN`.
//!
//! Every page operation first authorizes the current top-frame document with the caller's
//! [`Guard`] and re-checks that the document did not change before returning page data.
//! Interaction targets are authorized as the node that will actually receive the input, re-read
//! right before every input event: the element under the click point (hit-tested, top frame
//! only, with its current role and name), the node holding keyboard focus, the form an Enter
//! key would submit, and every option a `<select>` passes through. DOM and accessibility reads
//! are bounded in host memory. While the script runs, every top-frame document request of the
//! tab, including redirects and page-initiated navigations, is paused and authorized as
//! [`Gate::Navigate`]
//! ([`openside_browser::Browser::guard_navigation`]). Page operations of one execution run one at
//! a time, and none starts or sends a further CDP command after a stop. A denial or a required
//! approval stops the script uncatchably and is reported in [`Output::stop`]. Unsupported
//! Playwright APIs throw `NotSupported`.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod dom;
mod driver;
mod keys;

use std::{
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use openside_browser::Browser;
use rquickjs::{
    prelude::Async, AsyncContext, AsyncRuntime, CatchResultExt, CaughtError, Function, Object,
    Promise,
};
use serde::Serialize;
use serde_json::{json, Value};

use driver::Driver;

const PRELUDE: &str = include_str!("prelude.js");

/// Resource bounds for one execution.
#[derive(Clone, Debug)]
pub struct Limits {
    /// Wall-clock budget for the whole snippet, including browser waits.
    pub timeout: Duration,
    /// Default auto-wait for locator actions, assertions, and navigation.
    pub action_timeout: Duration,
    /// QuickJS heap limit.
    pub memory_bytes: usize,
    /// Total captured `console` text; later lines are replaced by one truncation notice.
    pub console_bytes: usize,
    /// Serialized return value; larger values are replaced by a truncated preview.
    pub return_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            action_timeout: Duration::from_secs(5),
            memory_bytes: 32 * 1024 * 1024,
            console_bytes: 8 * 1024,
            return_bytes: 16 * 1024,
        }
    }
}

/// The node an interaction is about to reach, by its accessibility role and name.
#[derive(Debug, Clone, Copy)]
pub struct Target<'a> {
    /// What happens to it: `click`, `fill`, `press keys in`, `submit form`, `select option`, ...
    pub action: &'a str,
    /// Accessibility role (`form` for a form submission, `option` for select options).
    pub role: &'a str,
    /// Accessible name; for submissions and selections, every relevant label joined by ` / `.
    pub name: &'a str,
}

/// An operation the caller must authorize before the REPL performs it.
#[derive(Debug)]
pub enum Gate<'a> {
    /// A top-frame document request to this URL: `page.goto`, `reload`, `goBack`/`goForward`,
    /// and any link, form, script, or redirect navigation the page starts.
    Navigate {
        /// Absolute destination URL.
        url: &'a str,
    },
    /// Reading or acting on the current document.
    Page {
        /// Current top-frame security origin (`null` when opaque).
        origin: &'a str,
        /// Interaction target; `None` for reads.
        target: Option<Target<'a>>,
    },
}

/// A [`Guard`] decision.
#[derive(Debug)]
pub enum Verdict {
    /// Proceed.
    Allow,
    /// Stop the script; the reason is reported to the caller.
    Deny(String),
    /// Stop the script before the operation; a human must approve this description first.
    Approval(String),
}

/// Policy hook consulted before navigation and every page access. Navigation checks run on the
/// browser's event task while the script waits, so implementations must be cheap and must not
/// block.
pub trait Guard: Send + Sync {
    /// Decide one operation.
    fn check(&self, gate: &Gate<'_>) -> Verdict;
}

/// Why a script was stopped by its [`Guard`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    /// Policy denied an operation.
    Denied(String),
    /// An operation needs human approval; nothing past it ran.
    Approval(String),
}

/// Result of one execution.
#[derive(Clone, Debug, Serialize)]
pub struct Output {
    /// JSON value of the last expression statement (`null` for `undefined`).
    pub return_value: Value,
    /// Captured `console.*` lines in order.
    pub console: Vec<String>,
    /// `Name: message` of an uncaught error, timeout, or policy stop.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Policy stop, if any; not part of the serialized model result.
    #[serde(skip)]
    pub stop: Option<Stop>,
}

impl Output {
    fn failed(message: impl Into<String>) -> Self {
        Self {
            return_value: Value::Null,
            console: Vec::new(),
            error: Some(message.into()),
            stop: None,
        }
    }
}

struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// Run `code` against tab `tab_id`, which `owner` must own as an agent tab.
///
/// Dropping the returned future cancels the script at its next interrupt check or operation.
pub async fn run(
    browser: Arc<Browser>,
    owner: String,
    tab_id: String,
    code: String,
    guard: Arc<dyn Guard>,
    limits: Limits,
) -> Output {
    let cancel = Arc::new(AtomicBool::new(false));
    let thread_cancel = Arc::clone(&cancel);
    let (sender, receiver) = tokio::sync::oneshot::channel();
    // The non-parallel QuickJS runtime is !Send; a dedicated thread also keeps CPU-bound
    // scripts off the caller's executor.
    let spawned = std::thread::Builder::new()
        .name("openside-repl".into())
        .spawn(move || {
            let output = match tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
            {
                Ok(runtime) => runtime.block_on(execute(
                    browser,
                    owner,
                    tab_id,
                    code,
                    guard,
                    limits,
                    thread_cancel,
                )),
                Err(_) => Output::failed("Error: REPL runtime unavailable"),
            };
            let _ = sender.send(output);
        });
    if spawned.is_err() {
        return Output::failed("Error: REPL thread unavailable");
    }
    let _cancel = CancelOnDrop(cancel);
    receiver
        .await
        .unwrap_or_else(|_| Output::failed("Error: REPL thread ended unexpectedly"))
}

async fn execute(
    browser: Arc<Browser>,
    owner: String,
    tab_id: String,
    code: String,
    guard: Arc<dyn Guard>,
    limits: Limits,
    cancel: Arc<AtomicBool>,
) -> Output {
    let deadline = Instant::now() + limits.timeout;
    let driver = Rc::new(Driver::new(
        Arc::clone(&browser),
        owner.clone(),
        tab_id.clone(),
        Arc::clone(&guard),
        limits.clone(),
        deadline,
        Arc::clone(&cancel),
    ));
    if driver.refresh_url().await.is_err() {
        return Output::failed("Error: tab is unavailable or not owned by this session");
    }
    let stops = driver.stops();
    let navigation = browser
        .guard_navigation(
            &owner,
            &tab_id,
            Arc::new(move |url: &str| stops.allow(guard.check(&Gate::Navigate { url }))),
        )
        .await;
    // Without interception a page could navigate anywhere unchecked, so do not run at all.
    let Ok(_navigation) = navigation else {
        return Output::failed("Error: navigation guard unavailable for this tab");
    };
    // Background agent tabs need emulated focus for focus/keyboard semantics.
    driver
        .cleanup_cmd(
            "Emulation.setFocusEmulationEnabled",
            json!({"enabled": true}),
        )
        .await;
    let evaluation = evaluate(
        Rc::clone(&driver),
        code,
        limits.clone(),
        deadline,
        cancel.clone(),
    );
    let watch = async {
        while !cancel.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    let outcome = tokio::select! {
        result = tokio::time::timeout(limits.timeout, evaluation) => match result {
            Ok(result) => result,
            Err(_) => Err(format!("TimeoutError: REPL execution exceeded {}ms", limits.timeout.as_millis())),
        },
        () = watch => Err("Error: REPL execution cancelled".into()),
    };
    driver
        .cleanup_cmd(
            "Emulation.setFocusEmulationEnabled",
            json!({"enabled": false}),
        )
        .await;
    let stop = driver.stops().current();
    let console = std::mem::take(&mut driver.console.borrow_mut().lines);
    let (return_value, mut error) = match outcome {
        Ok(Ok(value)) => (bounded(value, limits.return_bytes), None),
        Ok(Err(error)) | Err(error) => (Value::Null, Some(error)),
    };
    if let Some(stop) = &stop {
        error = Some(match stop {
            Stop::Denied(reason) => format!("PolicyError: policy denied: {reason}"),
            Stop::Approval(action) => format!("PolicyError: human approval required: {action}"),
        });
    }
    Output {
        return_value,
        console,
        error,
        stop,
    }
}

fn bounded(value: Value, limit: usize) -> Value {
    let serialized = value.to_string();
    if serialized.len() <= limit {
        return value;
    }
    let mut end = limit.min(serialized.len());
    while !serialized.is_char_boundary(end) {
        end -= 1;
    }
    json!({"truncated": true, "preview": &serialized[..end], "bytes": serialized.len()})
}

/// `Ok(Ok(value))` on completion, `Ok(Err(message))` for a script error.
async fn evaluate(
    driver: Rc<Driver>,
    code: String,
    limits: Limits,
    deadline: Instant,
    cancel: Arc<AtomicBool>,
) -> Result<Result<Value, String>, String> {
    let runtime = AsyncRuntime::new().map_err(|_| "Error: JavaScript runtime unavailable")?;
    runtime.set_memory_limit(limits.memory_bytes).await;
    runtime.set_max_stack_size(512 * 1024).await;
    let abort = Arc::clone(&driver.abort);
    runtime
        .set_interrupt_handler(Some(Box::new(move || {
            abort.load(Ordering::Acquire)
                || cancel.load(Ordering::Acquire)
                || Instant::now() >= deadline
        })))
        .await;
    let context = AsyncContext::full(&runtime)
        .await
        .map_err(|_| "Error: JavaScript context unavailable")?;
    let result = context
        .async_with(async move |ctx| -> Result<Result<Value, String>, String> {
            let globals = ctx.globals();
            let host = Rc::clone(&driver);
            let op = Function::new(
                ctx.clone(),
                Async(move |name: String, payload: String| {
                    let host = Rc::clone(&host);
                    async move { Ok::<_, rquickjs::Error>(host.dispatch(&name, &payload).await) }
                }),
            )
            .map_err(|_| "Error: REPL host unavailable")?;
            let logger = Rc::clone(&driver);
            let log = Function::new(ctx.clone(), move |line: String| logger.log(line))
                .map_err(|_| "Error: REPL host unavailable")?;
            let initial = driver.url.borrow().clone();
            for (name, value) in [("__openside_op", op), ("__openside_log", log)] {
                globals
                    .set(name, value)
                    .map_err(|_| "Error: REPL host unavailable")?;
            }
            globals
                .set("__openside_url", initial)
                .map_err(|_| "Error: REPL host unavailable")?;
            let prelude: Object = ctx
                .eval(PRELUDE)
                .catch(&ctx)
                .map_err(|error| format!("Error: REPL prelude failed: {error}"))?;
            let finish: Function = prelude
                .get("finish")
                .map_err(|_| "Error: REPL prelude failed")?;
            let describe: Function = prelude
                .get("describe")
                .map_err(|_| "Error: REPL prelude failed")?;
            let promise = match ctx.eval_promise(code).catch(&ctx) {
                Ok(promise) => promise,
                Err(CaughtError::Exception(exception)) => {
                    let message: String = describe
                        .call((exception,))
                        .unwrap_or_else(|_| "SyntaxError".into());
                    return Ok(Err(message));
                }
                Err(error) => return Ok(Err(format!("Error: {error}"))),
            };
            let finished: Promise = finish
                .call((promise,))
                .map_err(|_| "Error: REPL prelude failed")?;
            let reply: String = match finished.into_future::<String>().await.catch(&ctx) {
                Ok(reply) => reply,
                Err(error) => return Ok(Err(interrupted(&error.to_string()))),
            };
            let reply: Value =
                serde_json::from_str(&reply).map_err(|_| "Error: invalid REPL result")?;
            Ok(if reply["ok"] == true {
                Ok(reply["value"].clone())
            } else {
                Err(reply["error"]
                    .as_str()
                    .map(interrupted)
                    .unwrap_or_else(|| "Error: unknown failure".into()))
            })
        })
        .await;
    result
}

fn interrupted(message: &str) -> String {
    if message.contains("interrupted") {
        "Error: execution interrupted (time limit, cancellation, or policy stop)".into()
    } else if message.contains("out of memory") {
        "RangeError: out of memory (REPL heap limit)".into()
    } else {
        message.to_owned()
    }
}
