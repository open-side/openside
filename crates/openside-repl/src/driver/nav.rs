//! Navigation operations: `goto`, `reload`, history moves, and waiting for a navigation to
//! commit and reach the requested lifecycle state.

use std::time::{Duration, Instant};

use openside_browser::{DocumentResponse, DocumentWait, WaitUntil};
use serde_json::{json, Value};

use super::{
    error::{browser_error, navigation_error, OpError, OpResult, HTTP_STATUS_FAILURE},
    timeout_of,
    wait::{lowercase_headers, pending_note},
    Driver,
};
use crate::Gate;

/// How long the main document's response event may trail the reply of the command that
/// committed it: the reply and the event travel different paths inside the controller.
const RESPONSE_GRACE: Duration = Duration::from_millis(500);

impl Driver {
    /// Handle the operations this module owns; `None` means the name belongs elsewhere.
    pub(super) async fn nav_op(&self, name: &str, args: &Value) -> Option<OpResult<Value>> {
        Some(match name {
            "goto" => self.goto(args).await,
            "reload" => self.reload(args).await,
            "history" => self.history(args).await,
            "url" => self.url().await,
            _ => return None,
        })
    }

    /// Navigation commands answer only once the navigation commits, so a server that never
    /// responds would hold the command, and with it the tab's whole session, until the
    /// transport gives up. Bound it by the navigation timeout and cancel the load.
    pub(super) async fn navigation_cmd(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> OpResult<Value> {
        match tokio::time::timeout(timeout, self.cmd(method, params)).await {
            Ok(result) => result,
            Err(_) => {
                self.cleanup_cmd("Page.stopLoading", json!({})).await;
                Err(OpError::timeout(format!(
                    "navigation timeout of {}ms exceeded",
                    timeout.as_millis()
                )))
            }
        }
    }

    /// A CDP reply can precede the navigation an input handler starts; give it a moment.
    pub(super) async fn settle(&self, before: Value) -> OpResult<()> {
        let grace = Instant::now() + Duration::from_millis(250);
        loop {
            let frame = self.frame().await?;
            if frame["loaderId"] != before["loaderId"] {
                return self
                    .wait_loaded(Instant::now() + self.limits.action_timeout)
                    .await;
            }
            if Instant::now() >= grace {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Lifecycle tracking has to be on before the navigation starts, or its events are missed.
    async fn track(&self) -> OpResult<()> {
        self.browser
            .track_network(&self.owner, &self.tab)
            .await
            .map_err(browser_error)
    }

    /// `waitUntil` of `goto`, `reload`, and history moves; Playwright's default is `load`.
    fn wait_until(args: &Value) -> OpResult<WaitUntil> {
        match args["waitUntil"].as_str() {
            None => Ok(WaitUntil::Load),
            Some(name) => WaitUntil::parse(name).ok_or_else(|| {
                OpError::new(format!(
                    "waitUntil must be \"commit\", \"domcontentloaded\", \"load\" or \
                     \"networkidle\", got {name:?}"
                ))
            }),
        }
    }

    /// Wait for the committed document to reach `until`; a miss is a transient timeout that
    /// names what the page was still waiting on.
    async fn finish_navigation(
        &self,
        until: WaitUntil,
        document: DocumentWait<'_>,
        deadline: Instant,
        timeout: Duration,
        target: &str,
    ) -> OpResult<()> {
        match self.reach(until, document, deadline).await? {
            Ok(()) => Ok(()),
            Err(pending) => Err(OpError::timeout(format!(
                "navigation timeout of {}ms exceeded waiting for \"{}\" of {target}{}",
                timeout.as_millis(),
                until.as_str(),
                pending_note(&pending)
            ))),
        }
    }

    /// The response of the document that got `loader`, as the script's `Response` data, or
    /// `null` when the navigation had none (an error page, a restored page, `about:blank`).
    async fn committed_response(&self, loader: &str) -> OpResult<Value> {
        let grace = Instant::now() + RESPONSE_GRACE;
        loop {
            let found = self
                .browser
                .document_response(&self.owner, &self.tab, loader)
                .await
                .map_err(browser_error)?;
            if let Some(response) = found {
                return Ok(response_json(&response));
            }
            if Instant::now() >= grace {
                return Ok(Value::Null);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    pub(super) async fn goto(&self, args: &Value) -> OpResult<Value> {
        let url = args["url"]
            .as_str()
            .ok_or_else(|| OpError::new("goto requires a URL"))?;
        let until = Self::wait_until(args)?;
        self.gate(Gate::Navigate { url }).await?;
        let timeout = timeout_of(args, self.limits.action_timeout);
        let deadline = Instant::now() + timeout;
        self.track().await?;
        self.frame().await?;
        let before_url = self.url.borrow().clone();
        let result = self
            .navigation_cmd("Page.navigate", json!({"url": url}), timeout)
            .await?;
        if result["isDownload"] == true {
            return Err(OpError {
                name: "NavigationError",
                message: format!(
                    "page.goto: {url} is a download, not a page (permanent: use a download \
                     wait instead)"
                ),
            });
        }
        // An empty-body 4xx/5xx makes Chromium commit its own error page and report this
        // text; the navigation still has a status, which the response carries.
        if let Some(error) = result["errorText"]
            .as_str()
            .filter(|error| *error != HTTP_STATUS_FAILURE)
        {
            return Err(navigation_error("page.goto", url, error));
        }
        let Some(loader) = result["loaderId"].as_str() else {
            // Same-document navigation (a fragment change): nothing was loaded. The frame
            // URL follows the reply by a moment.
            let settled = Instant::now() + Duration::from_millis(500);
            while self.refresh_url().await? == before_url && Instant::now() < settled {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            return Ok(Value::Null);
        };
        self.finish_navigation(until, DocumentWait::Loader(loader), deadline, timeout, url)
            .await?;
        let response = self.committed_response(loader).await?;
        self.frame().await?;
        Ok(response)
    }

    pub(super) async fn reload(&self, args: &Value) -> OpResult<Value> {
        let before = self.frame().await?;
        let before_loader = before["loaderId"].as_str().unwrap_or_default().to_owned();
        let url = before["url"].as_str().unwrap_or_default().to_owned();
        let until = Self::wait_until(args)?;
        self.gate(Gate::Navigate { url: &url }).await?;
        let timeout = timeout_of(args, self.limits.action_timeout);
        let deadline = Instant::now() + timeout;
        self.track().await?;
        self.navigation_cmd("Page.reload", json!({}), timeout)
            .await?;
        self.finish_navigation(
            until,
            DocumentWait::NotLoader(&before_loader),
            deadline,
            timeout,
            &url,
        )
        .await?;
        let loader = self.frame().await?["loaderId"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        self.committed_response(&loader).await
    }

    pub(super) async fn history(&self, args: &Value) -> OpResult<Value> {
        let delta = args["delta"].as_i64().unwrap_or(-1);
        let history = self.cmd("Page.getNavigationHistory", json!({})).await?;
        let index = history["currentIndex"].as_i64().unwrap_or_default() + delta;
        let Some(entry) = usize::try_from(index)
            .ok()
            .and_then(|index| history["entries"].get(index))
        else {
            return Ok(Value::Null);
        };
        let url = entry["url"].as_str().unwrap_or_default().to_owned();
        let until = Self::wait_until(args)?;
        self.gate(Gate::Navigate { url: &url }).await?;
        let before = self.frame().await?;
        let timeout = timeout_of(args, self.limits.action_timeout);
        let deadline = Instant::now() + timeout;
        self.track().await?;
        self.navigation_cmd(
            "Page.navigateToHistoryEntry",
            json!({"entryId": entry["id"]}),
            timeout,
        )
        .await?;
        // The entry may be a same-document one or a page restored from the back/forward
        // cache: neither commits a new document, so a changed URL is also a commit.
        let after = loop {
            let frame = self.frame().await?;
            if frame["loaderId"] != before["loaderId"] || frame["url"] != before["url"] {
                break frame;
            }
            if Instant::now() >= deadline {
                return Err(OpError::timeout(format!(
                    "navigation timeout of {}ms exceeded waiting for {url} to commit",
                    timeout.as_millis()
                )));
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        };
        if after["loaderId"] == before["loaderId"] {
            return Ok(Value::Null);
        }
        let loader = after["loaderId"].as_str().unwrap_or_default();
        self.finish_navigation(until, DocumentWait::Loader(loader), deadline, timeout, &url)
            .await?;
        self.committed_response(loader).await
    }

    pub(super) async fn url(&self) -> OpResult<Value> {
        self.gate_page(None).await?;
        Ok(json!(self.url.borrow().clone()))
    }
}

fn response_json(response: &DocumentResponse) -> Value {
    json!({
        "requestId": response.request_id,
        "url": response.url,
        "status": response.status,
        "statusText": response.status_text,
        "headers": lowercase_headers(&response.headers),
    })
}
