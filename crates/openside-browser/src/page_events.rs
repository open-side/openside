//! Page-level observations for script drivers: JavaScript dialogs, popups, downloads, and file
//! choosers of the tabs this controller attaches to.
//!
//! [`BrowserEvent`]s go to the one controller receiver, which answers dialogs on its own
//! (`opensided`: alerts accepted, confirms and prompts cancelled). A script that wants to decide
//! therefore claims a tab's dialogs ([`Browser::claim_dialogs`]): while the claim lives, a dialog
//! of that tab is delivered only on the [`PageEvent`] broadcast. Dropping the claim hands a
//! dialog nobody answered back to the controller, so it is never left open.

use std::collections::{HashMap, VecDeque};

use tokio::sync::broadcast;

use super::*;

const EVENT_BACKLOG: usize = 256;
// Finished downloads stay addressable for `saveAs` until this many newer ones push them out.
const DOWNLOAD_RECORDS: usize = 256;

/// A JavaScript dialog that is open in a tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DialogInfo {
    /// Page target that opened the dialog.
    pub tab_id: String,
    /// CDP dialog type: `alert`, `confirm`, `prompt`, or `beforeunload`.
    pub kind: String,
    /// Dialog text, as the page wrote it.
    pub message: String,
    /// Prompt default value; empty for other kinds.
    pub default_prompt: String,
}

/// A download Chromium started, with its attribution to a tab and owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadInfo {
    /// Chromium download identifier; the file is named after it until it is saved.
    pub guid: String,
    /// Source URL.
    pub url: String,
    /// File name the page suggested.
    pub suggested_filename: String,
    /// Page target matched from the download's frame identifier.
    pub tab_id: Option<String>,
    /// Session that owns that tab.
    pub owner: Option<String>,
    /// CDP browser context of the owner's isolated space, `None` for the shared profile.
    pub space: Option<String>,
    /// Terminal state, once Chromium reported one.
    pub status: Option<DownloadStatus>,
    /// Bytes Chromium reported written so far.
    pub received_bytes: u64,
    /// Expected size; 0 while the server has not announced one.
    pub total_bytes: u64,
    /// Canceled by this controller because it outgrew [`Browser::set_download_limit`].
    pub over_limit: bool,
    /// URL of the starting tab's page when the download began: what policy judges for a
    /// `blob:` or `data:` download, which has no origin of its own.
    pub page_url: Option<String>,
}

/// A file chooser the page opened while interception is on (`Page.setInterceptFileChooserDialog`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChooserInfo {
    /// Page target that opened the chooser.
    pub tab_id: String,
    /// Document that activated the input, including same-process child frames.
    pub frame_id: Option<String>,
    /// OOPIF target hosting the input; None for the page session.
    pub host: Option<String>,
    /// The `<input type=file>` the page activated; absent when Chromium did not report one.
    pub backend_node_id: Option<u64>,
    /// Whether the input accepts several files.
    pub multiple: bool,
}

/// What happened in a tab, for drivers that wait on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageEvent {
    /// A dialog opened.
    DialogOpened(DialogInfo),
    /// A dialog was answered or its page went away.
    DialogClosed {
        /// Page target the dialog belonged to.
        tab_id: String,
    },
    /// A page created another page.
    PopupOpened {
        /// The popup's ledger entry.
        tab: TabInfo,
        /// Target identifier of the page opening the popup.
        opener_id: String,
    },
    /// A download began.
    DownloadStarted(DownloadInfo),
    /// A download reached a terminal state.
    DownloadFinished {
        /// Chromium download identifier.
        guid: String,
        /// Completed or canceled.
        status: DownloadStatus,
    },
    /// The page asked for files.
    FileChooserOpened(FileChooserInfo),
}

#[derive(Clone)]
pub(super) struct PageHub(Arc<Shared>);

struct Shared {
    events: broadcast::Sender<PageEvent>,
    state: parking_lot::Mutex<HubState>,
}

#[derive(Default)]
struct HubState {
    claims: HashMap<String, Arc<ClaimState>>,
    open_dialogs: HashMap<String, DialogInfo>,
    downloads: HashMap<String, DownloadInfo>,
    download_order: VecDeque<String>,
    /// Per-file byte ceiling; `None` leaves downloads unbounded.
    download_limit: Option<u64>,
    /// Finished downloads whose Chromium file was deleted, with the copy the caller kept (if
    /// any); `save_download` reads that copy instead. Evicted with the download's record.
    released: HashMap<String, Option<std::path::PathBuf>>,
}

impl Default for PageHub {
    fn default() -> Self {
        Self(Arc::new(Shared {
            events: broadcast::channel(EVENT_BACKLOG).0,
            state: parking_lot::Mutex::default(),
        }))
    }
}

struct ClaimState {
    inner: parking_lot::Mutex<ClaimInner>,
    controller: mpsc::UnboundedSender<BrowserEvent>,
}

struct ClaimInner {
    active: bool,
    /// A dialog delivered to the claim and not answered yet.
    pending: Option<DialogInfo>,
}

impl PageHub {
    pub(super) fn subscribe(&self) -> broadcast::Receiver<PageEvent> {
        self.0.events.subscribe()
    }

    pub(super) fn publish(&self, event: PageEvent) {
        // No subscriber is the normal case.
        let _ = self.0.events.send(event);
    }

    /// Deliver a dialog: to the claim of its tab when there is a live one, else to the controller.
    pub(super) fn dialog_opened(
        &self,
        controller: &mpsc::UnboundedSender<BrowserEvent>,
        info: DialogInfo,
    ) {
        let claim = {
            let mut state = self.0.state.lock();
            state.open_dialogs.insert(info.tab_id.clone(), info.clone());
            state.claims.get(&info.tab_id).cloned()
        };
        let claimed = claim.is_some_and(|claim| {
            let mut inner = claim.inner.lock();
            if inner.active {
                inner.pending = Some(info.clone());
            }
            inner.active
        });
        self.publish(PageEvent::DialogOpened(info.clone()));
        if !claimed {
            let _ = controller.send(BrowserEvent::DialogOpened {
                tab_id: info.tab_id,
                kind: info.kind,
                message: info.message,
            });
        }
    }

    pub(super) fn dialog_closed(&self, tab_id: &str) {
        let claim = {
            let mut state = self.0.state.lock();
            state.open_dialogs.remove(tab_id);
            state.claims.get(tab_id).cloned()
        };
        if let Some(claim) = claim {
            claim.inner.lock().pending = None;
        }
        self.publish(PageEvent::DialogClosed {
            tab_id: tab_id.to_owned(),
        });
    }

    /// A destroyed page answers nothing; forget its dialog.
    pub(super) fn tab_gone(&self, tab_id: &str) {
        let had_dialog = self.0.state.lock().open_dialogs.contains_key(tab_id);
        if had_dialog {
            self.dialog_closed(tab_id);
        }
    }

    pub(super) fn download_started(&self, info: DownloadInfo) {
        {
            let mut state = self.0.state.lock();
            if state.download_order.len() >= DOWNLOAD_RECORDS {
                if let Some(oldest) = state.download_order.pop_front() {
                    state.downloads.remove(&oldest);
                    state.released.remove(&oldest);
                }
            }
            state.download_order.push_back(info.guid.clone());
            state.downloads.insert(info.guid.clone(), info.clone());
        }
        self.publish(PageEvent::DownloadStarted(info));
    }

    pub(super) fn download_finished(&self, guid: &str, status: DownloadStatus) {
        if let Some(record) = self.0.state.lock().downloads.get_mut(guid) {
            record.status = Some(status);
        }
        self.publish(PageEvent::DownloadFinished {
            guid: guid.to_owned(),
            status,
        });
    }

    pub(super) fn download(&self, guid: &str) -> Option<DownloadInfo> {
        self.0.state.lock().downloads.get(guid).cloned()
    }

    pub(super) fn downloads_of(&self, owner: &str) -> Vec<DownloadInfo> {
        let state = self.0.state.lock();
        state
            .download_order
            .iter()
            .filter_map(|guid| state.downloads.get(guid))
            .filter(|download| download.owner.as_deref() == Some(owner))
            .cloned()
            .collect()
    }

    pub(super) fn set_download_limit(&self, limit: Option<u64>) {
        self.0.state.lock().download_limit = limit;
    }

    /// Marks a finished download released; `false` when it is unknown or still running. The
    /// first release wins, so a repeated one cannot replace the kept copy `saveAs` reads.
    pub(super) fn release(&self, guid: &str, kept: Option<std::path::PathBuf>) -> bool {
        let mut state = self.0.state.lock();
        if !state
            .downloads
            .get(guid)
            .is_some_and(|record| record.status.is_some())
        {
            return false;
        }
        state.released.entry(guid.to_owned()).or_insert(kept);
        true
    }

    /// `Some(kept)` once the download was released.
    pub(super) fn released(&self, guid: &str) -> Option<Option<std::path::PathBuf>> {
        self.0.state.lock().released.get(guid).cloned()
    }

    /// Guids of the recorded downloads Chromium is still writing.
    pub(super) fn running_downloads(&self) -> std::collections::HashSet<String> {
        self.0
            .state
            .lock()
            .downloads
            .values()
            .filter(|record| record.status.is_none())
            .map(|record| record.guid.clone())
            .collect()
    }

    /// Records progress of a running download. Returns the record the first time it grows past
    /// the limit (by its announced size or by what was written), so the caller cancels it once.
    pub(super) fn download_progress(
        &self,
        guid: &str,
        received: u64,
        total: u64,
    ) -> Option<DownloadInfo> {
        let mut state = self.0.state.lock();
        let limit = state.download_limit;
        let record = state.downloads.get_mut(guid)?;
        record.received_bytes = received;
        record.total_bytes = total;
        let over = limit.is_some_and(|limit| received > limit || total > limit);
        if !over || record.over_limit || record.status.is_some() {
            return None;
        }
        record.over_limit = true;
        Some(record.clone())
    }

    fn open_dialog(&self, tab_id: &str) -> Option<DialogInfo> {
        self.0.state.lock().open_dialogs.get(tab_id).cloned()
    }
}

/// Exclusive right to decide the dialogs of one tab; see the module documentation.
pub struct DialogClaim {
    hub: PageHub,
    tab: String,
    state: Arc<ClaimState>,
}

impl Drop for DialogClaim {
    fn drop(&mut self) {
        let pending = {
            let mut inner = self.state.inner.lock();
            inner.active = false;
            inner.pending.take()
        };
        {
            let mut hub = self.hub.0.state.lock();
            if hub
                .claims
                .get(&self.tab)
                .is_some_and(|claim| Arc::ptr_eq(claim, &self.state))
            {
                hub.claims.remove(&self.tab);
            }
        }
        if let Some(dialog) = pending {
            let _ = self.state.controller.send(BrowserEvent::DialogOpened {
                tab_id: dialog.tab_id,
                kind: dialog.kind,
                message: dialog.message,
            });
        }
    }
}

impl Browser {
    /// Subscribe to the page events of every tab; the receiver reports `Lagged` when it falls
    /// more than a few hundred events behind.
    pub fn page_events(&self) -> broadcast::Receiver<PageEvent> {
        self.inner.page.subscribe()
    }

    /// Take over the dialogs of an owned agent tab from the controller's automatic answer. A tab
    /// has at most one live claim.
    pub async fn claim_dialogs(
        &self,
        owner: &str,
        tab_id: &str,
    ) -> Result<DialogClaim, BrowserError> {
        self.owned_session(owner, tab_id).await?;
        let state = Arc::new(ClaimState {
            inner: parking_lot::Mutex::new(ClaimInner {
                active: true,
                pending: None,
            }),
            controller: self.inner.events.clone(),
        });
        let mut hub = self.inner.page.0.state.lock();
        if hub
            .claims
            .get(tab_id)
            .is_some_and(|claim| claim.inner.lock().active)
        {
            return Err(BrowserError::InvalidAction(
                "the dialogs of this tab are already handled by another script",
            ));
        }
        hub.claims.insert(tab_id.to_owned(), Arc::clone(&state));
        drop(hub);
        Ok(DialogClaim {
            hub: self.inner.page.clone(),
            tab: tab_id.to_owned(),
            state,
        })
    }

    /// The dialog currently open in a tab, if any. While one is open, input, script, and
    /// navigation commands to the page do not return.
    pub fn open_dialog(&self, tab_id: &str) -> Option<DialogInfo> {
        self.inner.page.open_dialog(tab_id)
    }
}

/// Route one page-session event of the dialog and file-chooser family.
pub(super) fn page_event(
    inner: &Inner,
    state: &State,
    method: &str,
    session: Option<&str>,
    params: &Value,
) {
    let tab = || {
        session.and_then(|session| {
            state
                .sessions
                .iter()
                .find(|(_, value)| value.as_str() == session)
                .map(|(target, _)| target.clone())
                .or_else(|| {
                    state
                        .oopif_sessions
                        .values()
                        .find(|(_, value)| value == session)
                        .map(|(tab, _)| tab.clone())
                })
        })
    };
    match method {
        "Page.javascriptDialogOpening" => {
            if let Some(tab_id) = tab() {
                let text = |field: &str| params[field].as_str().unwrap_or_default().to_owned();
                inner.page.dialog_opened(
                    &inner.events,
                    DialogInfo {
                        tab_id,
                        kind: text("type"),
                        message: text("message"),
                        default_prompt: text("defaultPrompt"),
                    },
                );
            }
        }
        "Page.javascriptDialogClosed" => {
            if let Some(tab_id) = tab() {
                inner.page.dialog_closed(&tab_id);
            }
        }
        "Page.fileChooserOpened" => {
            if let Some(tab_id) = tab() {
                inner
                    .page
                    .publish(PageEvent::FileChooserOpened(FileChooserInfo {
                        tab_id,
                        frame_id: params["frameId"].as_str().map(str::to_owned),
                        host: session.and_then(|session| {
                            state
                                .oopif_sessions
                                .iter()
                                .find(|(_, (_, value))| value == session)
                                .map(|(host, _)| host.clone())
                        }),
                        backend_node_id: params["backendNodeId"].as_u64(),
                        multiple: params["mode"] == "selectMultiple",
                    }));
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(hub: &PageHub, guid: &str) {
        hub.download_started(DownloadInfo {
            guid: guid.into(),
            url: "https://files.example/a.bin".into(),
            suggested_filename: "a.bin".into(),
            tab_id: None,
            owner: Some("s".into()),
            space: None,
            status: None,
            received_bytes: 0,
            total_bytes: 0,
            over_limit: false,
            page_url: None,
        });
    }

    /// `handle_event` records progress before it applies the terminal state, so a download
    /// whose first and only report is a terminal one past the limit is still `over_limit`
    /// (the daemon reports it as `too_large`, not `canceled`).
    #[test]
    fn terminal_progress_past_the_limit_marks_the_download_over_limit() {
        let hub = PageHub::default();
        hub.set_download_limit(Some(1000));
        for (guid, status) in [
            ("c", DownloadStatus::Canceled),
            ("d", DownloadStatus::Completed),
        ] {
            started(&hub, guid);
            assert!(hub.download_progress(guid, 5000, 0).is_some());
            hub.download_finished(guid, status);
            let record = hub.download(guid).unwrap();
            assert!(record.over_limit, "{guid}");
            assert_eq!(record.received_bytes, 5000);
        }
        started(&hub, "small");
        assert!(hub.download_progress("small", 10, 10).is_none());
        hub.download_finished("small", DownloadStatus::Canceled);
        assert!(!hub.download("small").unwrap().over_limit);
        // Reported once: later progress for a download already over the limit cancels nothing.
        started(&hub, "twice");
        assert!(hub.download_progress("twice", 2000, 0).is_some());
        assert!(hub.download_progress("twice", 3000, 0).is_none());
    }

    #[test]
    fn a_download_is_released_once_and_keeps_its_first_copy() {
        let hub = PageHub::default();
        started(&hub, "g");
        assert!(
            !hub.release("g", None),
            "a running download is not released"
        );
        hub.download_finished("g", DownloadStatus::Completed);
        let kept = std::path::PathBuf::from("/artifacts/s/downloads/g/a.bin");
        assert!(hub.release("g", Some(kept.clone())));
        assert!(hub.release("g", None));
        assert_eq!(hub.released("g"), Some(Some(kept)));
    }
}
