//! Page events and files: dialogs, popups, downloads, file choosers, `setInputFiles`.
//!
//! Events reach the script through a pump in the prelude that repeatedly asks for the next event
//! (`event_next`). That operation, and the ones a handler needs while another operation is still
//! waiting on the page (`dialog_handle`, `popup_info`, `events_close`), skip the one-at-a-time
//! queue of [`Driver::dispatch`]: a click that opens a dialog does not return until the dialog is
//! answered, and the answer comes from a handler running meanwhile.
//!
//! While a script listens for dialogs it holds the tab's dialog claim, so the daemon's automatic
//! answer does not race the handler; releasing it hands an unanswered dialog back (see
//! [`openside_browser::DialogClaim`]).

use std::{
    cell::{Cell, RefCell},
    path::Path,
    time::{Duration, Instant},
};

use openside_browser::{Browser, DialogClaim, DownloadInfo, DownloadStatus, PageEvent};
use serde_json::{json, Value};
use tokio::sync::{broadcast, watch};

use super::{
    error::{browser_error, OpError, OpResult},
    locate::located,
    Driver,
};
use crate::{dom::NodeRef, Frame, Gate};

/// Commands the browser answers without the page's renderer, which a JavaScript dialog
/// freezes. Everything else (DOM, accessibility, runtime, input, navigation, capture) waits for
/// the dialog to close, so the driver refuses it with an error instead of hanging.
const RUNS_DURING_DIALOG: &[&str] = &[
    "Browser.",
    "Emulation.",
    "Network.",
    "Page.handleJavaScriptDialog",
    "Page.setInterceptFileChooserDialog",
    "Target.",
];

/// How long a fresh popup may stay blank before its address is reported as it is.
const POPUP_SETTLE: Duration = Duration::from_secs(2);

/// Longest dialog text quoted back in an error.
const DIALOG_QUOTE: usize = 120;

/// Per-execution event state of a [`Driver`].
pub(super) struct Events {
    receiver: tokio::sync::Mutex<broadcast::Receiver<PageEvent>>,
    closed: watch::Sender<bool>,
    claim: RefCell<Option<DialogClaim>>,
    intercepting: Cell<bool>,
    choosers: RefCell<Vec<NodeRef>>,
}

impl Events {
    /// Subscribed from the start, so an event that fires before the script registers interest
    /// in it is not lost to a race with the registration.
    pub(super) fn new(browser: &Browser) -> Self {
        Self {
            receiver: tokio::sync::Mutex::new(browser.page_events()),
            closed: watch::channel(false).0,
            claim: RefCell::new(None),
            intercepting: Cell::new(false),
            choosers: RefCell::new(Vec::new()),
        }
    }
}

/// Operations that run next to a pending one; see the module documentation.
pub(super) fn runs_beside_others(name: &str) -> bool {
    matches!(
        name,
        "event_next" | "dialog_handle" | "popup_info" | "events_close"
    )
}

impl Driver {
    /// Handle the operations this module owns; `None` means the name belongs elsewhere.
    pub(super) async fn events_op(&self, name: &str, args: &Value) -> Option<OpResult<Value>> {
        Some(match name {
            "events_set" => self.events_set(args).await,
            "event_next" => self.event_next(args).await,
            "events_close" => {
                self.end_events().await;
                Ok(Value::Null)
            }
            "dialog_handle" => self.dialog_handle(args).await,
            "popup_info" => self.popup_info(args).await,
            "download_save" => self.download_save(args).await,
            "download_failure" => self.download_failure(args).await,
            "set_input_files" => self.set_input_files(args).await,
            "chooser_set_files" => self.chooser_set_files(args).await,
            _ => return None,
        })
    }

    /// Refuse a command that would hang behind an open dialog, naming the dialog so the script
    /// can answer it.
    pub(super) fn dialog_blocks(&self, method: &str) -> OpResult<()> {
        if RUNS_DURING_DIALOG
            .iter()
            .any(|prefix| method.starts_with(prefix))
        {
            return Ok(());
        }
        match self.browser.open_dialog(&self.tab) {
            None => Ok(()),
            Some(dialog) => {
                let text: String = dialog.message.chars().take(DIALOG_QUOTE).collect();
                Err(OpError {
                    name: "DialogError",
                    message: format!(
                        "a JavaScript {} dialog is open ({text:?}) and blocks {method}; answer it from a page.on('dialog') handler",
                        dialog.kind
                    ),
                })
            }
        }
    }

    /// Stop the event pump and give back everything it held: the dialog claim and the file
    /// chooser interception. Safe to call more than once.
    pub(crate) async fn end_events(&self) {
        self.events.closed.send_replace(true);
        self.events.claim.borrow_mut().take();
        self.events.choosers.borrow_mut().clear();
        if self.events.intercepting.replace(false) {
            self.cleanup_cmd(
                "Page.setInterceptFileChooserDialog",
                json!({"enabled": false}),
            )
            .await;
        }
    }

    /// Declare which event families the script listens to: `dialog` claims the tab's dialogs,
    /// `filechooser` turns on file chooser interception.
    async fn events_set(&self, args: &Value) -> OpResult<Value> {
        // A registration queued behind a slow action must not re-claim after the script ended.
        if *self.events.closed.borrow() {
            return Ok(Value::Null);
        }
        let dialog = args["dialog"] == true;
        let claimed = self.events.claim.borrow().is_some();
        if dialog && !claimed {
            let claim = self
                .browser
                .claim_dialogs(&self.owner, &self.tab)
                .await
                .map_err(browser_error)?;
            *self.events.claim.borrow_mut() = Some(claim);
        } else if !dialog && claimed {
            self.events.claim.borrow_mut().take();
        }
        let chooser = args["filechooser"] == true;
        if chooser != self.events.intercepting.get() {
            self.cmd(
                "Page.setInterceptFileChooserDialog",
                json!({"enabled": chooser}),
            )
            .await?;
            self.events.intercepting.set(chooser);
        }
        Ok(Value::Null)
    }

    /// The next event of this tab, or `null` after `timeout` milliseconds (never, when absent)
    /// and once the events are closed.
    async fn event_next(&self, args: &Value) -> OpResult<Value> {
        let deadline = args["timeout"]
            .as_u64()
            .filter(|ms| *ms > 0)
            .map(|ms| tokio::time::Instant::now() + Duration::from_millis(ms));
        let mut closed = self.events.closed.subscribe();
        let mut receiver = self.events.receiver.lock().await;
        loop {
            if *closed.borrow() || self.stopped() {
                return Ok(Value::Null);
            }
            let timer = async {
                match deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                _ = closed.changed() => return Ok(Value::Null),
                () = timer => return Ok(Value::Null),
                received = receiver.recv() => match received {
                    Ok(event) => {
                        if let Some(value) = self.deliverable(event).await? {
                            return Ok(value);
                        }
                    }
                    // A burst past the backlog: later events still arrive.
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return Ok(Value::Null),
                },
            }
        }
    }

    /// The script-facing form of an event that belongs to this tab, after policy has seen it.
    async fn deliverable(&self, event: PageEvent) -> OpResult<Option<Value>> {
        Ok(match event {
            PageEvent::DialogOpened(dialog) if dialog.tab_id == self.tab => {
                // No CDP here: while a dialog is open the page does not answer frame queries.
                // The origin is the one of the last known top-frame URL; navigations are
                // already policed when they start.
                let origin = origin_of(&self.url.borrow());
                self.gate(Gate::Page {
                    origin: &origin,
                    frame: None,
                    target: None,
                })
                .await?;
                Some(json!({
                    "kind": "dialog",
                    "type": dialog.kind,
                    "message": dialog.message,
                    "default_value": dialog.default_prompt,
                }))
            }
            PageEvent::PopupOpened { tab, opener_id }
                if opener_id == self.tab
                    && tab.owner_session_id.as_deref() == Some(self.owner.as_str()) =>
            {
                Some(json!({"kind": "popup", "tab_id": tab.id}))
            }
            PageEvent::DownloadStarted(download)
                if download.tab_id.as_deref() == Some(self.tab.as_str())
                    && download.owner.as_deref() == Some(self.owner.as_str()) =>
            {
                self.gate_download(&download).await?;
                Some(json!({
                    "kind": "download",
                    "guid": download.guid,
                    "url": download.url,
                    "suggested_filename": download.suggested_filename,
                }))
            }
            PageEvent::FileChooserOpened(chooser) if chooser.tab_id == self.tab => {
                let frame = chooser
                    .frame_id
                    .ok_or_else(|| OpError::new("file chooser has no receiving frame"))?;
                let node = NodeRef {
                    host: chooser.host.clone(),
                    frame: Some(frame.clone()),
                    backend: chooser.backend_node_id.unwrap_or_default(),
                };
                self.gate_node_page(&node, None).await?;
                let chooser_id = self.events.choosers.borrow().len();
                self.events.choosers.borrow_mut().push(node);
                Some(json!({
                    "kind": "filechooser",
                    "chooser_id": chooser_id,
                    "backend_node_id": chooser.backend_node_id,
                    "frame": frame,
                    "host": chooser.host,
                    "multiple": chooser.multiple,
                }))
            }
            _ => None,
        })
    }

    /// `dialog.accept(promptText)` / `dialog.dismiss()`.
    async fn dialog_handle(&self, args: &Value) -> OpResult<Value> {
        self.cmd(
            "Page.handleJavaScriptDialog",
            json!({
                "accept": args["accept"] == true,
                "promptText": args["text"].as_str().unwrap_or_default(),
            }),
        )
        .await?;
        Ok(Value::Null)
    }

    /// URL and title of a popup this script's tab opened. The popup stays read-only: it is a
    /// page of the same session, not a tab the script may drive. With `settle`, waits (briefly)
    /// for a popup that was just created to leave its blank initial document.
    async fn popup_info(&self, args: &Value) -> OpResult<Value> {
        let id = args["tab_id"].as_str().unwrap_or_default();
        let settle_until = (args["settle"] == true).then(|| Instant::now() + POPUP_SETTLE);
        let tab = loop {
            let tab = self
                .browser
                .tabs()
                .await
                .into_iter()
                .find(|tab| {
                    tab.id == id && tab.owner_session_id.as_deref() == Some(self.owner.as_str())
                })
                .ok_or_else(|| OpError::new("popup is closed or not owned by this session"))?;
            let blank = tab.url.is_empty() || tab.url == "about:blank";
            match settle_until {
                Some(until) if blank && Instant::now() < until && !self.stopped() => {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                _ => break tab,
            }
        };
        let top = self.frame().await?;
        let origin = self.browser.origin(id).await.map_err(browser_error)?;
        self.gate(Gate::Page {
            origin: top["securityOrigin"].as_str().unwrap_or("null"),
            frame: None,
            target: None,
        })
        .await?;
        self.gate(Gate::Page {
            origin: &origin,
            frame: None,
            target: None,
        })
        .await?;
        Ok(json!({"tab_id": tab.id, "url": tab.url, "title": tab.title}))
    }

    async fn gate_download(&self, download: &DownloadInfo) -> OpResult<()> {
        let top = self.frame().await?;
        self.gate(Gate::Download {
            url: &download.url,
            suggested_name: &download.suggested_filename,
            origin: top["securityOrigin"].as_str().unwrap_or("null"),
        })
        .await
    }

    /// `download.saveAs(name)`: wait for the download, then copy it into the session's download
    /// directory. Returns the saved path.
    async fn download_save(&self, args: &Value) -> OpResult<Value> {
        let download = self.download_of(args)?;
        self.gate_download(&download).await?;
        let status = self.download_finish(&download, args).await?;
        if status != DownloadStatus::Completed {
            return Err(OpError::new("download was canceled"));
        }
        let path = self
            .browser
            .save_download(&self.owner, &download.guid, args["name"].as_str())
            .await
            .map_err(browser_error)?;
        Ok(json!({"path": path.to_string_lossy()}))
    }

    /// `download.failure()`: `null` for a completed download, `canceled` otherwise.
    async fn download_failure(&self, args: &Value) -> OpResult<Value> {
        let download = self.download_of(args)?;
        Ok(match self.download_finish(&download, args).await? {
            DownloadStatus::Completed => Value::Null,
            DownloadStatus::Canceled => json!("canceled"),
        })
    }

    fn download_of(&self, args: &Value) -> OpResult<DownloadInfo> {
        self.browser
            .download(args["guid"].as_str().unwrap_or_default())
            .filter(|download| download.owner.as_deref() == Some(self.owner.as_str()))
            .ok_or_else(|| OpError::new("unknown download"))
    }

    async fn download_finish(
        &self,
        download: &DownloadInfo,
        args: &Value,
    ) -> OpResult<DownloadStatus> {
        let timeout = args["timeout"]
            .as_u64()
            .map(Duration::from_millis)
            .unwrap_or(self.limits.action_timeout);
        self.browser
            .wait_download(&download.guid, Instant::now() + timeout)
            .await
            .map_err(browser_error)
    }

    /// `locator.setInputFiles(files)`.
    async fn set_input_files(&self, args: &Value) -> OpResult<Value> {
        let located = located(args)?;
        let files = file_arguments(args)?;
        let timeout = located
            .timeout
            .map(Duration::from_millis)
            .unwrap_or(self.limits.action_timeout);
        let resolved = self.single(&located, timeout, false, false).await?;
        let node = &resolved.dom.nodes[resolved.found[0]];
        let is_file_input = node.name.eq_ignore_ascii_case("input")
            && node
                .attribute("type")
                .is_some_and(|kind| kind.eq_ignore_ascii_case("file"));
        if !is_file_input {
            return Err(OpError::new(format!(
                "{} is not an <input type=file>",
                located.description
            )));
        }
        let index = resolved.found[0];
        let frame = (resolved.dom.nodes[index].frame != 0).then(|| {
            let info = &resolved.dom.frames[resolved.dom.nodes[index].frame];
            (info.id.clone(), info.origin.clone())
        });
        self.attach_files(&resolved.dom.node_ref(index), frame.as_ref(), &files)
            .await?;
        self.same_documents(&resolved).await?;
        Ok(Value::Null)
    }

    /// `fileChooser.setFiles(files)`: the input the page activated, by backend node id.
    async fn chooser_set_files(&self, args: &Value) -> OpResult<Value> {
        let files = file_arguments(args)?;
        // The script may mutate its descriptor; receiver authority stays in the host registry.
        let chooser_id = args["chooser_id"]
            .as_u64()
            .and_then(|id| usize::try_from(id).ok())
            .ok_or_else(|| OpError::new("invalid file chooser"))?;
        let node = self
            .events
            .choosers
            .borrow()
            .get(chooser_id)
            .cloned()
            .ok_or_else(|| OpError::new("file chooser is no longer available"))?;
        self.gate_node_page(&node, None).await?;
        let frames = self.frame_list().await?;
        let frame = node
            .frame
            .as_deref()
            .and_then(|id| frames.iter().find(|info| info.id == id))
            .ok_or_else(|| OpError::new("file chooser frame was detached"))?;
        if node.host != frame.host {
            return Err(OpError::new("file chooser renderer changed"));
        }
        let frame = frame
            .parent
            .as_ref()
            .map(|_| (frame.id.clone(), frame.origin.clone()));
        self.attach_files(&node, frame.as_ref(), &files).await?;
        Ok(Value::Null)
    }

    /// Set the files of an input; `frame` is the (id, origin) of the iframe holding it, if any.
    async fn attach_files(
        &self,
        node: &NodeRef,
        frame: Option<&(String, String)>,
        files: &[String],
    ) -> OpResult<()> {
        let mut paths = Vec::with_capacity(files.len());
        for file in files {
            paths.push(self.upload_path(file, frame).await?);
        }
        if paths.is_empty() {
            // DOM.setFileInputFiles with no files leaves the old selection in place.
            let object = self.node_cmd(node, "DOM.resolveNode", json!({})).await?["object"]
                ["objectId"]
                .as_str()
                .ok_or_else(|| OpError::new("the input is gone"))?
                .to_owned();
            self.host_cmd(
                node.host.as_deref(),
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object,
                    "functionDeclaration": "function () { this.value = ''; this.dispatchEvent(new Event('input', { bubbles: true })); this.dispatchEvent(new Event('change', { bubbles: true })); }",
                    "silent": true,
                }),
            )
            .await?;
            return Ok(());
        }
        self.node_cmd(node, "DOM.setFileInputFiles", json!({"files": paths}))
            .await?;
        Ok(())
    }

    /// The real path of a file the script wants to upload, after policy approved both the path
    /// as given and, when a symlink or `..`-free alias makes it differ, the file it resolves to.
    /// The first check precedes any filesystem access, so a refused path is not probed.
    pub(super) async fn upload_path(
        &self,
        path: &str,
        frame: Option<&(String, String)>,
    ) -> OpResult<String> {
        if !Path::new(path).is_absolute() {
            return Err(OpError::new("file paths must be absolute"));
        }
        let top = self.frame().await?;
        let origin = top["securityOrigin"].as_str().unwrap_or("null").to_owned();
        let frame = frame.map(|(id, origin)| Frame { id, origin });
        self.gate(Gate::Upload {
            path,
            origin: &origin,
            frame,
        })
        .await?;
        let real = tokio::fs::canonicalize(path)
            .await
            .map_err(|_| OpError::new("file not found"))?;
        let metadata = tokio::fs::metadata(&real)
            .await
            .map_err(|_| OpError::new("file not found"))?;
        if !metadata.is_file() {
            return Err(OpError::new("only regular files can be uploaded"));
        }
        let real = real
            .to_str()
            .ok_or_else(|| OpError::new("file path is not valid UTF-8"))?
            .to_owned();
        self.gate(Gate::Upload {
            path: &real,
            origin: &origin,
            frame,
        })
        .await?;
        let name = Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| OpError::new("invalid upload file name"))?;
        let staged = self
            .browser
            .stage_upload(&self.owner, &self.tab, Path::new(&real), name)
            .await
            .map_err(browser_error)?;
        staged
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| OpError::new("file path is not valid UTF-8"))
    }
}

/// `scheme://host[:port]` of an http(s) URL, `null` for anything with an opaque origin.
pub(super) fn origin_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_else(|| "null".into())
}

/// `files` of a `setInputFiles` call: the paths, or none to clear the input.
fn file_arguments(args: &Value) -> OpResult<Vec<String>> {
    args["files"]
        .as_array()
        .map(|files| {
            files
                .iter()
                .map(|file| {
                    file.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| OpError::new("files must be absolute path strings"))
                })
                .collect()
        })
        .unwrap_or_else(|| Err(OpError::new("files must be a path or an array of paths")))
}
