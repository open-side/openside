//! Waits that poll page state: element states, load state, and the network log the script's
//! `waitForResponse`/`waitForRequest`/`waitForURL` read, plus the renderer metrics `settle` uses.

use std::time::{Duration, Instant};

use openside_browser::{DocumentWait, LoadWait, NetworkEvent, NetworkEventKind, WaitUntil};
use serde_json::{json, Map, Value};

use super::{
    error::{browser_error, OpError, OpResult},
    locate::located,
    timeout_of, Driver, STOPPED,
};

/// Network idle means this long without a request in flight, counted from `load`.
const NETWORK_IDLE_QUIET: Duration = Duration::from_millis(500);
/// How long one browser-side wait runs before the driver checks whether the script was stopped.
const WAIT_SLICE: Duration = Duration::from_millis(200);
/// Largest response body handed to the script, before base64 or JSON quoting.
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
/// Most log entries one `net_events` reply carries; the script asks again from its cursor.
const MAX_EVENTS: usize = 200;

impl Driver {
    /// Handle the operations this module owns; `None` means the name belongs elsewhere.
    pub(super) async fn wait_op(&self, name: &str, args: &Value) -> Option<OpResult<Value>> {
        Some(match name {
            "wait_for" => self.wait_for(args).await,
            "load_state" => self.load_state(args).await,
            "net_events" => self.net_events(args).await,
            "net_body" => self.net_body(args).await,
            "sleep" => self.sleep(args).await,
            _ => return None,
        })
    }

    pub(super) async fn wait_for(&self, args: &Value) -> OpResult<Value> {
        let state = args["state"].as_str().unwrap_or("visible").to_owned();
        let located = located(args)?;
        self.gate_page(None).await?;
        let timeout = located
            .timeout
            .map(Duration::from_millis)
            .unwrap_or(self.limits.action_timeout);
        let deadline = Instant::now() + timeout;
        loop {
            let resolved = self.resolve(&located.steps).await?;
            let done = match (state.as_str(), resolved.found.as_slice()) {
                ("attached", found) => !found.is_empty(),
                ("detached", found) => found.is_empty(),
                ("hidden", []) => true,
                ("hidden", [index]) => !self.visible(&resolved.dom.node_ref(*index)).await?,
                (_, [index]) => self.visible(&resolved.dom.node_ref(*index)).await?,
                (_, []) => false,
                (_, found) => {
                    return Err(OpError::new(format!(
                        "strict mode violation: {} resolved to {} elements",
                        located.description,
                        found.len()
                    )))
                }
            };
            if done {
                return Ok(Value::Null);
            }
            if Instant::now() >= deadline {
                return Err(OpError::timeout(format!(
                    "Timeout {}ms exceeded waiting for {} to be {state}",
                    timeout.as_millis(),
                    located.description
                )));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub(super) async fn metrics(&self) -> OpResult<Value> {
        if !self.metrics.get() {
            self.cmd("Performance.enable", json!({})).await?;
            self.metrics.set(true);
        }
        self.cmd("Performance.getMetrics", json!({})).await
    }

    /// DOMContentLoaded of the current document, read from `Performance.getMetrics` instead of
    /// `document.readyState` so that no expression is evaluated in the page.
    pub(super) async fn wait_loaded(&self, deadline: Instant) -> OpResult<()> {
        loop {
            let metrics = self.metrics().await?;
            let loaded = metric(&metrics, "DomContentLoaded");
            if loaded > 0.0 && loaded >= metric(&metrics, "NavigationStart") {
                self.frame().await?;
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(OpError::timeout("timeout waiting for DOMContentLoaded"));
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Wait for a lifecycle state of a document until `deadline`. The wait itself runs in the
    /// browser crate; it is cut into slices so that a stopped script ends it. `Ok(Err(..))`
    /// is a deadline miss and carries the requests still in flight, so callers can say what the
    /// page was waiting on.
    pub(super) async fn reach(
        &self,
        until: WaitUntil,
        document: DocumentWait<'_>,
        deadline: Instant,
    ) -> OpResult<Result<(), Vec<String>>> {
        loop {
            if self.stopped() {
                return Err(OpError::new(STOPPED));
            }
            let slice = (Instant::now() + WAIT_SLICE).min(deadline);
            let outcome = self
                .browser
                .wait_load_state(
                    &self.owner,
                    &self.tab,
                    until,
                    document,
                    NETWORK_IDLE_QUIET,
                    slice,
                )
                .await
                .map_err(browser_error)?;
            match outcome {
                LoadWait::Reached => return Ok(Ok(())),
                LoadWait::TimedOut { in_flight, .. } if Instant::now() >= deadline => {
                    return Ok(Err(in_flight));
                }
                LoadWait::TimedOut { .. } => {}
            }
        }
    }

    pub(super) async fn load_state(&self, args: &Value) -> OpResult<Value> {
        let state = args["state"].as_str().unwrap_or("load");
        let until = match state {
            "domcontentloaded" => WaitUntil::DomContentLoaded,
            "load" => WaitUntil::Load,
            "networkidle" => WaitUntil::NetworkIdle,
            other => {
                return Err(OpError::new(format!(
                    "page.waitForLoadState: state must be \"load\", \"domcontentloaded\" or \
                     \"networkidle\", got {other:?}"
                )))
            }
        };
        let timeout = timeout_of(args, self.limits.action_timeout);
        if let Err(pending) = self
            .reach(until, DocumentWait::Current, Instant::now() + timeout)
            .await?
        {
            return Err(OpError::timeout(format!(
                "page.waitForLoadState: Timeout {}ms exceeded waiting for load state \"{state}\"{}",
                timeout.as_millis(),
                pending_note(&pending)
            )));
        }
        self.gate_page(None).await?;
        Ok(Value::Null)
    }

    /// Entries of the tab's network log after the script's cursor. Without `after` nothing is
    /// returned and the cursor is the current end of the log.
    async fn net_events(&self, args: &Value) -> OpResult<Value> {
        self.gate_page(None).await?;
        let kinds: Option<Vec<&str>> = args["kinds"]
            .as_array()
            .map(|kinds| kinds.iter().filter_map(Value::as_str).collect());
        let log = self
            .browser
            .network_events(
                &self.owner,
                &self.tab,
                args["after"].as_u64().unwrap_or(u64::MAX),
            )
            .await
            .map_err(browser_error)?;
        let mut cursor = log.cursor;
        let mut events = Vec::new();
        for event in log.events {
            if events.len() == MAX_EVENTS {
                break;
            }
            cursor = event.seq;
            if kinds
                .as_ref()
                .is_none_or(|kinds| kinds.contains(&kind_name(event.kind)))
            {
                events.push(event_json(&event));
            }
        }
        Ok(json!({"events": events, "cursor": cursor}))
    }

    /// Body of a response the script waited for.
    async fn net_body(&self, args: &Value) -> OpResult<Value> {
        self.gate_page(None).await?;
        let id = args["requestId"]
            .as_str()
            .ok_or_else(|| OpError::new("response body requires a request id"))?;
        let reply = self
            .cmd("Network.getResponseBody", json!({"requestId": id}))
            .await
            .map_err(|error| {
                if error.message.contains("No resource") {
                    OpError::new(
                        "the response body is no longer available: the browser keeps bodies only \
                         while the page lives, and never for redirects",
                    )
                } else {
                    error
                }
            })?;
        let body = reply["body"].as_str().unwrap_or_default();
        if body.len() > MAX_BODY_BYTES {
            return Err(OpError::new(format!(
                "the response body is larger than {MAX_BODY_BYTES} bytes"
            )));
        }
        Ok(json!({"body": body, "base64": reply["base64Encoded"] == true}))
    }

    pub(super) async fn sleep(&self, args: &Value) -> OpResult<Value> {
        let ms = args["ms"]
            .as_u64()
            .unwrap_or_default()
            .min(self.limits.timeout.as_millis() as u64);
        tokio::time::sleep(Duration::from_millis(ms)).await;
        Ok(Value::Null)
    }
}

pub(super) fn metric(metrics: &Value, name: &str) -> f64 {
    metrics["metrics"]
        .as_array()
        .and_then(|metrics| metrics.iter().find(|metric| metric["name"] == name))
        .and_then(|metric| metric["value"].as_f64())
        .unwrap_or_default()
}

/// `; 2 requests still in flight: GET a, GET b`, or nothing when the page is quiet.
pub(super) fn pending_note(in_flight: &[String]) -> String {
    if in_flight.is_empty() {
        return String::new();
    }
    let shown: Vec<String> = in_flight
        .iter()
        .take(3)
        .map(|request| request.chars().take(120).collect())
        .collect();
    let rest = in_flight.len() - shown.len();
    format!(
        "; {} request{} still in flight: {}{}",
        in_flight.len(),
        if in_flight.len() == 1 { "" } else { "s" },
        shown.join(", "),
        if rest > 0 {
            format!(" and {rest} more")
        } else {
            String::new()
        }
    )
}

/// Header names are case-insensitive; the script gets them lower-cased like Playwright's.
pub(super) fn lowercase_headers(headers: &Value) -> Value {
    let mut out = Map::new();
    if let Some(headers) = headers.as_object() {
        for (name, value) in headers {
            out.insert(name.to_lowercase(), value.clone());
        }
    }
    Value::Object(out)
}

fn kind_name(kind: NetworkEventKind) -> &'static str {
    match kind {
        NetworkEventKind::Request => "request",
        NetworkEventKind::Response => "response",
        NetworkEventKind::Failed => "failed",
        NetworkEventKind::Navigated => "navigated",
    }
}

fn event_json(event: &NetworkEvent) -> Value {
    json!({
        "seq": event.seq,
        "at": event.at_ms,
        "kind": kind_name(event.kind),
        "requestId": event.request_id,
        "url": event.url,
        "method": event.method,
        "resourceType": event.resource_type,
        "frameId": event.frame_id,
        "status": event.status,
        "statusText": event.status_text,
        "headers": lowercase_headers(&event.headers),
        "error": event.error_text,
    })
}
