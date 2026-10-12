//! Host operations behind the JavaScript prelude use `openside_browser::PAGE_COMMANDS`.
//! DOM/AX selector engines run in Rust; fixed page-side readers use named isolated worlds.
//!
//! `Driver` is one struct whose operations are spread over these modules, each with its own
//! `impl Driver` and an `*_op` dispatcher that [`Driver::op`] consults:
//!
//! - `error`: `OpError`, `OpResult`, mapping of browser errors.
//! - `nav`: goto/reload/history/url, navigation timeouts, waiting for a commit.
//! - `locate`: selector evaluation, refs, strict single-match, DOM/AX reads, page reads.
//! - `actionability`: visibility, enabled and checked state.
//! - `act`: `locator.<action>()` dispatch over pointer, keyboard, fill, and select.
//! - `pointer`: the per-event node gate and the box math behind pointer targets.
//! - `reach`: reaching a pointer target (scroll, stability, hit target) and click/hover/drag.
//! - `mouse`: the pointer device, `page.mouse`, trusted mouse and HTML5 drag events.
//! - `scroll`: bringing an element into view with trusted wheel events.
//! - `keyboard`: focus, held keys, chords, text insertion, IME composition.
//! - `fill`: which elements accept `fill`.
//! - `select`: `selectOption`.
//! - `wait`: element/load-state waits and renderer metrics.
//! - `events`: popups, dialogs, downloads, file choosers.
//! - `script`: page script (`evaluate`, handles, init scripts) and raw `page.cdp`.
//!
//! This file keeps the shared core: state, CDP access, policy gates, and the operation envelope.

mod act;
mod actionability;
mod error;
mod events;
mod fill;
mod keyboard;
mod locate;
mod mouse;
mod nav;
mod pointer;
mod reach;
mod script;
mod scroll;
mod select;
mod wait;

use std::{
    cell::{Cell, RefCell},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use openside_browser::Browser;
use serde_json::{json, Value};

use self::error::{browser_error, OpError, OpResult};
use crate::{dom::TOO_LARGE, Gate, Guard, Limits, Stop, Target, Verdict};

const STOPPED: &str = "execution stopped";
const CHANGED: &str = "the page navigated to another document during the operation";

/// Fixed internal readers and their protocol contracts, distinct from caller page scripts.
pub struct InternalPrimitive {
    /// Stable audit key; changing it requires updating protocol evidence.
    pub name: &'static str,
    /// The same source must not be admitted through a different execution command.
    pub method: &'static str,
    /// Main-world execution is never a trusted internal reader.
    pub world: &'static str,
    /// Audits compare verbatim, not prefixes or code patterns.
    pub source: &'static str,
    /// False is reserved for XPath's element handles, which require group cleanup.
    pub return_by_value: bool,
    /// Stability must await its animation frames before input can proceed.
    pub await_promise: bool,
    /// Handle-returning readers must declare the group their driver releases.
    pub object_group: Option<&'static str>,
}

/// Shares one setup reader between daemon secrecy checks and the protocol audit.
pub const SETUP_SCREENING: InternalPrimitive = InternalPrimitive {
    name: "setup_screening",
    method: "Runtime.evaluate",
    world: "openside-fill",
    source: PROTECT_SETUP_SOURCES,
    return_by_value: true,
    await_promise: false,
    object_group: None,
};

/// Only these fixed readers are trusted by basic-step audits; caller scripts are separate.
pub const INTERNAL_PRIMITIVES: &[InternalPrimitive] = &[
    SETUP_SCREENING,
    InternalPrimitive {
        name: "scroll_plan",
        method: "Runtime.callFunctionOn",
        world: "openside-scroll",
        source: scroll::PLAN,
        return_by_value: true,
        await_promise: false,
        object_group: None,
    },
    InternalPrimitive {
        name: "stability",
        method: "Runtime.callFunctionOn",
        world: "openside-pointer",
        source: reach::STABLE_BOX,
        return_by_value: true,
        await_promise: true,
        object_group: Some("openside-pointer"),
    },
    InternalPrimitive {
        name: "fill_read_back",
        method: "Runtime.callFunctionOn",
        world: "openside-fill",
        source: fill::READ_BACK,
        return_by_value: true,
        await_promise: false,
        object_group: None,
    },
    InternalPrimitive {
        name: "xpath",
        method: "Runtime.callFunctionOn",
        world: "openside",
        source: locate::XPATH,
        return_by_value: false,
        await_promise: false,
        object_group: Some("openside-locate"),
    },
];

const PROTECT_SETUP_SOURCES: &str = r"(() => {
    // Protect the same visible-control values the registration reader accepts.
    const values = [...document.querySelectorAll('input,textarea')].filter((el) => {
        const r = el.getBoundingClientRect(), s = getComputedStyle(el);
        return el.type !== 'password' && el.type !== 'hidden' && r.width > 1 && r.height > 1
            && s.display !== 'none' && s.visibility !== 'hidden' && Number(s.opacity) > 0;
    }).map((el) => String(el.value || '').slice(0, 4096));
    const text = [...values, document.body ? document.body.innerText : ''].join('\n').slice(0, 50000);
    const setup = /authenticator|2-?step|two-?factor|2fa|setup key|secret key|scan (the |this )?(qr )?code|인증 ?앱|OTP 앱|보안 ?키/i.test(text);
    const uris = new Set();
    const add = (s) => {
        if (typeof s !== 'string' || s.length > 4096) return;
        for (const m of s.matchAll(/otpauth:\/\/totp\/[^\s<>]+/gi)) uris.add(m[0]);
    };
    for (const el of document.querySelectorAll('*')) {
        for (const a of el.attributes) add(a.value);
    }
    add(text);
    const qr = (setup || uris.size > 0) && [...document.querySelectorAll('img,canvas,svg')].some((el) => {
        const r = el.getBoundingClientRect();
        const style = getComputedStyle(el);
        return style.display !== 'none' && style.visibility !== 'hidden'
            && r.width >= 96 && Math.abs(r.width - r.height) <= 8;
    });
    return {uris: [...uris].slice(0, 16), text: setup || uris.size > 0 ? text : '', qr};
})()";

/// Bounded console capture shared with the JavaScript `console` object.
#[derive(Default)]
pub(crate) struct Console {
    pub(crate) lines: Vec<String>,
    bytes: usize,
    truncated: bool,
}

// Each stored line costs host memory even when empty, so it is charged against the budget too.
const LINE_OVERHEAD: usize = 32;

impl Console {
    pub(crate) fn push(&mut self, line: String, limit: usize) {
        if self.truncated {
            return;
        }
        let cost = line.len() + LINE_OVERHEAD;
        if self.bytes + cost > limit {
            self.truncated = true;
            self.lines
                .push("[console output truncated; log less to see the rest]".into());
            return;
        }
        self.bytes += cost;
        self.lines.push(line);
    }
}

/// The policy stop of one execution, shared with the navigation interceptor, which records
/// stops from the browser's event task.
#[derive(Clone)]
pub(crate) struct Stops {
    stop: Arc<Mutex<Option<Stop>>>,
    abort: Arc<AtomicBool>,
}

impl Stops {
    // Policy stops are uncatchable: the interrupt handler ends the script at its next check, and
    // every later CDP command is refused.
    pub(crate) fn halt(&self, stop: Stop) {
        self.stop
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_or_insert(stop);
        self.abort.store(true, Ordering::Release);
    }

    /// Record a stop for a non-`Allow` verdict; returns whether the operation may proceed.
    pub(crate) fn allow(&self, verdict: Verdict) -> bool {
        match verdict {
            Verdict::Allow => true,
            Verdict::Deny(reason) => {
                self.halt(Stop::Denied(reason));
                false
            }
            Verdict::Approval(action) => {
                self.halt(Stop::Approval(action));
                false
            }
        }
    }

    pub(crate) fn current(&self) -> Option<Stop> {
        self.stop
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

pub(crate) struct Driver {
    browser: Arc<Browser>,
    owner: String,
    tab: String,
    guard: Arc<dyn Guard>,
    limits: Limits,
    deadline: Instant,
    cancel: Arc<AtomicBool>,
    stops: Stops,
    pub(crate) abort: Arc<AtomicBool>,
    pub(crate) console: RefCell<Console>,
    pub(crate) url: RefCell<String>,
    metrics: Cell<bool>,
    // Promise.all over page operations must not interleave focus, input, and policy checks.
    serial: tokio::sync::Mutex<()>,
    scripts: script::Scripts,
    events: events::Events,
    // Held keys and the pointer device persist across operations of one execution.
    held: RefCell<crate::keys::Held>,
    pointer: RefCell<mouse::Pointer>,
}

impl Driver {
    pub(crate) fn new(
        browser: Arc<Browser>,
        owner: String,
        tab: String,
        guard: Arc<dyn Guard>,
        limits: Limits,
        deadline: Instant,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        let abort = Arc::new(AtomicBool::new(false));
        let events = events::Events::new(&browser);
        Self {
            browser,
            owner,
            tab,
            guard,
            limits,
            deadline,
            cancel,
            stops: Stops {
                stop: Arc::new(Mutex::new(None)),
                abort: Arc::clone(&abort),
            },
            abort,
            console: RefCell::default(),
            url: RefCell::new(String::new()),
            metrics: Cell::new(false),
            serial: tokio::sync::Mutex::new(()),
            scripts: script::Scripts::default(),
            events,
            held: RefCell::default(),
            pointer: RefCell::default(),
        }
    }

    pub(crate) fn stops(&self) -> Stops {
        self.stops.clone()
    }

    fn stopped(&self) -> bool {
        self.abort.load(Ordering::Acquire)
            || self.cancel.load(Ordering::Acquire)
            || Instant::now() >= self.deadline
    }

    pub(crate) fn log(&self, line: String) {
        self.console
            .borrow_mut()
            .push(line, self.limits.console_bytes);
    }

    pub(crate) async fn cmd(&self, method: &str, params: Value) -> OpResult<Value> {
        if self.stopped() {
            return Err(OpError::new(STOPPED));
        }
        self.dialog_blocks(method)?;
        self.browser
            .page_command(&self.owner, &self.tab, method, params)
            .await
            .map_err(browser_error)
    }

    /// Restore tab state after the script, even when it was stopped.
    pub(crate) async fn cleanup_cmd(&self, method: &str, params: Value) {
        let _ = tokio::time::timeout(
            Duration::from_millis(250),
            self.browser
                .page_command(&self.owner, &self.tab, method, params),
        )
        .await;
    }

    async fn frame(&self) -> OpResult<Value> {
        let mut tree = self.cmd("Page.getFrameTree", json!({})).await?;
        let frame = tree["frameTree"]["frame"].take();
        if let Some(url) = frame["url"].as_str() {
            // `url` of a frame excludes the fragment, which `page.url()` includes.
            let fragment = frame["urlFragment"].as_str().unwrap_or_default();
            *self.url.borrow_mut() = format!("{url}{fragment}");
        }
        Ok(frame)
    }

    pub(crate) async fn refresh_url(&self) -> OpResult<String> {
        self.frame().await?;
        Ok(self.url.borrow().clone())
    }

    /// Run one prelude operation; the JSON reply always carries the latest known top-frame URL.
    pub(crate) async fn dispatch(&self, name: &str, payload: &str) -> String {
        // Operations that must run next to a pending one (an event handler answering the dialog
        // a click is waiting on) skip the queue.
        let _serial = if events::runs_beside_others(name) {
            None
        } else {
            Some(self.serial.lock().await)
        };
        let result = if self.stops.current().is_some() || self.stopped() {
            Err(OpError::new(STOPPED))
        } else {
            match serde_json::from_str::<Value>(payload) {
                Ok(args) => self.op(name, args).await,
                Err(_) => Err(OpError::new("invalid operation payload")),
            }
        };
        let url = self.url.borrow().clone();
        let reply = match result {
            Ok(value) => json!({"value": value, "url": url}),
            Err(error) => json!({"error": error.message, "name": error.name, "url": url}),
        };
        let mut out = Bounded(Vec::new());
        match serde_json::to_writer(&mut out, &reply) {
            Ok(()) => String::from_utf8(out.0).unwrap_or_default(),
            Err(_) => json!({"error": TOO_LARGE, "name": "Error", "url": url}).to_string(),
        }
    }

    /// Run one operation: each module claims the names it owns, so adding an operation touches
    /// only the module it belongs to.
    async fn op(&self, name: &str, args: Value) -> OpResult<Value> {
        if let Some(result) = self.nav_op(name, &args).await {
            return result;
        }
        if let Some(result) = self.locate_op(name, &args).await {
            return result;
        }
        if let Some(result) = self.act_op(name, &args).await {
            return result;
        }
        if let Some(result) = self.keyboard_op(name, &args).await {
            return result;
        }
        if let Some(result) = self.wait_op(name, &args).await {
            return result;
        }
        if let Some(result) = self.events_op(name, &args).await {
            return result;
        }
        if let Some(result) = self.script_op(name, &args).await {
            return result;
        }
        Err(OpError {
            name: "NotSupported",
            message: format!("{name} is not supported by the openside REPL"),
        })
    }

    async fn gate(&self, gate: Gate<'_>) -> OpResult<()> {
        let verdict = self.guard.check(&gate);
        let message = match &verdict {
            Verdict::Allow => return Ok(()),
            Verdict::Deny(reason) => format!("policy denied: {reason}"),
            Verdict::Approval(action) => format!("human approval required: {action}"),
        };
        self.stops.allow(verdict);
        Err(OpError {
            name: "PolicyError",
            message,
        })
    }

    /// Authorize the current top-frame document and, if given, the interaction target in it;
    /// returns the authorized document's loader id for [`Self::same_document`].
    async fn gate_page(&self, target: Option<Target<'_>>) -> OpResult<String> {
        let frame = self.frame().await?;
        let origin = frame["securityOrigin"].as_str().unwrap_or("null");
        self.gate(Gate::Page {
            origin,
            frame: None,
            target,
        })
        .await?;
        Ok(frame["loaderId"].as_str().unwrap_or_default().to_owned())
    }

    /// Page data may only leave an operation if it came from the document that was authorized.
    async fn same_document(&self, loader: &str) -> OpResult<()> {
        if self.frame().await?["loaderId"] == loader {
            Ok(())
        } else {
            Err(OpError::new(CHANGED))
        }
    }
}

fn timeout_of(args: &Value, default: Duration) -> Duration {
    args["timeout"]
        .as_u64()
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis)
        .unwrap_or(default)
}

/// Largest operation reply handed to the script; it is copied into the QuickJS heap next.
const MAX_REPLY_BYTES: usize = 4 * 1024 * 1024;

/// Serialization sink that fails instead of growing past [`MAX_REPLY_BYTES`].
struct Bounded(Vec<u8>);

impl std::io::Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len() + bytes.len() > MAX_REPLY_BYTES {
            return Err(std::io::ErrorKind::OutOfMemory.into());
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
