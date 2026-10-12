use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
#[cfg(unix)]
use nix::sys::signal::{kill, killpg, Signal};
#[cfg(unix)]
use nix::unistd::Pid;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tempfile::TempDir;
#[cfg(unix)]
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
#[cfg(unix)]
use tokio::process::Command;
use tokio::sync::{mpsc, Mutex, OwnedMutexGuard};
use tokio::task::JoinHandle;

use crate::cdp::Cdp;
#[cfg(unix)]
use crate::gateway::GatewayLaunch;
use crate::gateway::{Gateway, GatewayConfig, GatewayOwner};
use crate::snapshot::{merge_frames, nodes_in_view, process, RefTarget, References};
use crate::{BrowserError, Snapshot};

#[path = "browser_pins.rs"]
mod pins;
#[path = "pointer.rs"]
mod pointer;
pub use pointer::{PointHit, PointLabel, PointerAction, VIEWPORT};
#[path = "screencast.rs"]
mod screencast;
pub use screencast::{Screencast, ScreencastFrame};
#[path = "network.rs"]
mod network;
#[path = "spaces.rs"]
mod spaces;
pub use network::{
    DocumentResponse, DocumentWait, LoadState, LoadWait, NetworkEvent, NetworkEventKind,
    NetworkEvents, WaitUntil,
};
#[path = "page_events.rs"]
mod page_events;
pub use page_events::{DialogClaim, DialogInfo, DownloadInfo, FileChooserInfo, PageEvent};
#[path = "downloads.rs"]
mod downloads;
#[path = "screenshot.rs"]
mod screenshot;
pub use screenshot::{ScreenshotArea, ScreenshotClip};
#[path = "frames.rs"]
mod frames;
use frames::same_origin;
pub use frames::{FrameHit, FrameInfo};
#[path = "cdp_scope.rs"]
mod cdp_scope;
#[cfg(unix)]
#[path = "uploads.rs"]
mod uploads;
#[cfg(not(unix))]
mod uploads {
    use super::{Browser, BrowserError, Path, PathBuf};

    fn unsupported() -> std::io::Error {
        std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Private upload staging requires Unix descriptor confinement",
        )
    }

    pub(super) fn remove_tab(_: &Path, _: &str) -> std::io::Result<()> {
        Err(unsupported())
    }

    impl Browser {
        pub async fn stage_upload(
            &self,
            _: &str,
            _: &str,
            _: &Path,
            _: &str,
        ) -> Result<PathBuf, BrowserError> {
            Err(unsupported().into())
        }

        pub(super) async fn clear_session_uploads(&self, _: &str) -> Result<(), BrowserError> {
            Err(unsupported().into())
        }

        pub(super) async fn prune_uploads(&self) -> Result<(), BrowserError> {
            Err(unsupported().into())
        }

        pub(super) async fn wipe_uploads(&self) -> Result<(), BrowserError> {
            Err(unsupported().into())
        }
    }
}

/// Isolated browser launch and operation deadlines.
#[derive(Clone, Debug)]
pub struct BrowserConfig {
    /// Discovered or explicitly configured Chromium-compatible executable.
    pub executable: PathBuf,
    /// Deadline for reading the browser debugging endpoint.
    pub launch_timeout: Duration,
    /// Deadline for each protocol operation.
    pub command_timeout: Duration,
    /// Deadline racing the entire readiness/attach/setup operation.
    pub attach_timeout: Duration,
    /// Explicit durable profile directory; None creates an isolated temporary profile.
    pub profile_dir: Option<PathBuf>,
    /// Preserve Chromium on controller drop or parent exit so a reconnecting daemon adopts it;
    /// shutdown remains explicit. `die_with_spawner` opts a detached launch back into
    /// kernel-enforced exit with the spawning thread (tests only).
    pub detached: bool,
    /// Kill Chromium when the thread that spawned it exits, even when `detached` would
    /// otherwise preserve it. Parent-death supervision (PR_SET_PDEATHSIG on Linux) covers
    /// normal launches; a detached launch needs this override so a test process killed
    /// without unwinding leaves no browser. Ignored on non-Linux platforms.
    pub die_with_spawner: bool,
    /// Opt-in patched-browser gateway; stock Chromium falls back to CDP only.
    pub gateway: Option<GatewayConfig>,
    /// Explicit unpacked extensions; empty keeps all extensions disabled.
    pub extensions: Vec<PathBuf>,
}

impl BrowserConfig {
    /// Use an explicit executable with the standard operation deadlines.
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            launch_timeout: Duration::from_secs(20),
            command_timeout: Duration::from_secs(12),
            attach_timeout: Duration::from_secs(12),
            profile_dir: None,
            detached: false,
            die_with_spawner: false,
            gateway: None,
            extensions: Vec::new(),
        }
    }
}

/// Non-secret connection metadata for a browser that survives controller restarts.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BrowserEndpoint {
    /// Browser process and isolated process-group identifier.
    pub pid: u32,
    /// Loopback browser-level CDP WebSocket URL.
    pub ws_url: String,
    /// Managed profile containing the matching DevToolsActivePort file.
    pub profile_dir: PathBuf,
}

/// A document-bound node identity that can survive a CDP controller reconnect.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct NodeHandle {
    /// Chromium backend node identity; no DOM content is retained.
    pub backend_node_id: u64,
    /// Opaque frame, loader, URL, and document-root identity.
    pub document_id: String,
    /// Exact top-frame security origin at resolution time.
    pub origin: String,
}

/// The provenance of a tab, independent of its current URL.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OpenedBy {
    /// Explicitly created by an openside agent session.
    Agent,
    /// Explicitly adopted as a user-owned tab.
    User,
    /// Discovered without an openside creation operation, including popups.
    External,
}

/// Ledger entry for a live page target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabInfo {
    /// CDP page target identifier.
    pub id: String,
    /// Current URL from target lifecycle metadata.
    pub url: String,
    /// Current title from target lifecycle metadata.
    pub title: String,
    /// Immutable provenance except explicit user adoption.
    pub opened_by: OpenedBy,
    /// Agent session responsible for explicit creation, or the popup's opener session.
    pub owner_session_id: Option<String>,
    /// Caller-provided discarded state; stock CDP has no discarded flag.
    pub discarded: bool,
    /// Caller-provided unloaded state; sleeping tabs cannot attach.
    pub unloaded: bool,
    /// CDP browser context of the isolated space ([`Browser::open_space`]) the tab lives in;
    /// `None` for the managed profile's default context.
    pub browser_context_id: Option<String>,
}

/// Reference-targeted interaction; callers must refresh snapshots after navigation.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    /// Scroll the target into view and click its content-box center.
    Click,
    /// Insert at the target's current caret or selection, optionally submitting with Enter.
    Type {
        /// Text is inserted through CDP, not evaluated as JavaScript.
        text: String,
        /// Send Enter after insertion; no submission occurs when false.
        submit: bool,
    },
    /// Focus the target and dispatch a key or modifier chord.
    Press {
        /// Named key (for example Enter), character, or chord such as Control+A.
        key: String,
    },
    /// Change a select element's option values and emit input/change events.
    Select {
        /// Existing option values; multiple values require a multiple-select target.
        values: Vec<String>,
    },
    /// Scroll the target into view and move the pointer to its content-box center.
    Hover,
    /// Focus within the tab without activating its native window.
    Focus,
}

/// The top-frame URL observed after an interaction settles.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ActOutcome {
    /// Current top-frame URL, including a navigation triggered by this interaction.
    pub url: String,
    /// True when the top-frame URL or document loader changed.
    pub navigated: bool,
}

/// Terminal state of a Chromium download.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadStatus {
    /// Chromium completed the download.
    Completed,
    /// Chromium canceled or failed the download.
    Canceled,
}

/// Browser observations delivered losslessly to the controller's event receiver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowserEvent {
    /// A page target entered the ledger.
    TabOpened {
        /// Complete provenance and ownership metadata.
        tab: TabInfo,
    },
    /// A ledger page target was destroyed.
    TabClosed {
        /// Last known provenance and ownership metadata.
        tab: TabInfo,
    },
    /// A page created another page through `window.open` or a target-blank link.
    PopupOpened {
        /// The popup's ledger entry.
        tab: TabInfo,
        /// Target identifier of the page opening the popup.
        opener_id: String,
    },
    /// Chromium began a download in the isolated profile.
    DownloadStarted {
        /// Chromium download identifier.
        guid: String,
        /// Source URL.
        url: String,
        /// Suggested download filename.
        suggested_filename: String,
        /// Page target matched from the download's frame identifier.
        tab_id: Option<String>,
    },
    /// Chromium reported a terminal download state.
    DownloadFinished {
        /// Chromium download identifier.
        guid: String,
        /// Completed or canceled terminal state.
        status: DownloadStatus,
    },
    /// An attached page opened a JavaScript dialog.
    DialogOpened {
        /// Page target that opened the dialog.
        tab_id: String,
        /// CDP dialog type, such as `alert` or `confirm`.
        kind: String,
        /// Dialog text; no automatic approval is performed.
        message: String,
    },
    /// A target opening brought the live tab count to five or more.
    TooManyTabs {
        /// Total live page targets, including user and external targets.
        count: usize,
    },
}

#[derive(Default)]
struct State {
    tabs: HashMap<String, TabInfo>,
    sessions: HashMap<String, String>,
    frames: HashMap<String, String>,
    references: HashMap<String, References>,
    /// Per-tab rules from [`Browser::guard_navigation`], keyed by target id.
    navigation_rules: HashMap<String, NavigationRule>,
    /// Popup target id -> opener target id, for popups whose opener was not in `tabs` when the
    /// popup was created. Their requests are held until the opener's owner is known.
    pending_openers: HashMap<String, String>,
    /// Owner session -> CDP browser context of its isolated space.
    spaces: HashMap<String, String>,
    /// CDP-created contexts seen on this controller; Chromium also reports a default-context id.
    isolated_contexts: HashSet<String>,
    /// Owners that had a space on this controller; their tabs never go to the default context.
    isolated: HashSet<String>,
    /// Context -> owner, for spaces whose disposal Chromium has not confirmed yet.
    closing: HashMap<String, String>,
    /// A leftover-context prune failed and must be retried.
    prune_due: bool,
    /// Target id -> raw `browserContextId` reported by Chromium, for every page target. Only
    /// ids in `spaces` are copied into [`TabInfo::browser_context_id`].
    tab_contexts: HashMap<String, String>,
    /// Out-of-process iframe target id -> (owning tab id, flattened session), attached on
    /// demand by the frame helpers in `frames.rs`.
    oopif_sessions: HashMap<String, (String, String)>,
    uploads: HashMap<String, PathBuf>,
    upload_closing: HashSet<String>,
    upload_epochs: HashMap<String, u64>,
    closing_tabs: HashSet<String>,
}

fn check_owned(state: &State, owner: &str, id: &str) -> Result<(), BrowserError> {
    if state.closing_tabs.contains(id) {
        return Err(BrowserError::TabNotReady(id.to_owned()));
    }
    let tab = state
        .tabs
        .get(id)
        .ok_or_else(|| BrowserError::TabNotReady(id.to_owned()))?;
    if tab.owner_session_id.as_deref() != Some(owner) || tab.opened_by == OpenedBy::User {
        return Err(BrowserError::OwnershipDenied(id.to_owned()));
    }
    let context = state
        .tab_contexts
        .get(id)
        .filter(|context| state.isolated_contexts.contains(*context))
        .or(tab.browser_context_id.as_ref());
    let space = state.spaces.get(owner);
    if context != space
        || context.is_some_and(|context| state.closing.contains_key(context))
        || (space.is_none() && state.isolated.contains(owner))
        || (space.is_none() && tab.opened_by != OpenedBy::Agent)
    {
        return Err(BrowserError::OwnershipDenied(id.to_owned()));
    }
    Ok(())
}

/// Decides whether a top-frame document request to this URL may proceed.
pub type NavigationCheck = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Decides whether a top-frame document request of a tab owned by `owner` (first argument)
/// to the URL (second argument) may proceed. See [`Browser::set_navigation_policy`].
pub type OwnerNavigationCheck = Arc<dyn Fn(&str, &str) -> bool + Send + Sync>;

struct NavigationRule {
    active: Arc<std::sync::atomic::AtomicBool>,
    allow: NavigationCheck,
    /// Also decide the document requests of the tab's subframes.
    frames: bool,
}

/// Active per-tab navigation rule from [`Browser::guard_navigation`]; dropping it removes the
/// rule. The owner-level policy keeps applying.
pub struct NavigationGuard {
    active: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for NavigationGuard {
    fn drop(&mut self) {
        // Drop cannot await the state lock: the inactive rule is discarded by the next paused
        // request.
        self.active
            .store(false, std::sync::atomic::Ordering::Release);
    }
}

const DOCUMENT_INTERCEPTION: &str = "Document";

/// Continue or fail one browser-level `Fetch.requestPaused` request.
///
/// Interception is enabled once on the browser target ([`Browser::initialize`]), so every
/// document request of every page, including a popup's first request and each redirect hop,
/// pauses here before it reaches the network. A page's main frame id equals its target id;
/// top-frame requests of agent-owned tabs (popups inherit the opener's owner) must pass the
/// owner policy and any active [`Browser::guard_navigation`] rule. Subframe requests continue
/// unless their tab runs a [`Browser::guard_documents`] rule that refuses them.
async fn handle_paused(inner: &Arc<Inner>, event: &Value) {
    let session = event["sessionId"].as_str().map(str::to_owned);
    let params = &event["params"];
    let request_id = params["requestId"].clone();
    let url = params["request"]["url"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let frame = params["frameId"].as_str().unwrap_or_default().to_owned();
    if params["resourceType"] != DOCUMENT_INTERCEPTION {
        return respond(inner, session.as_deref(), request_id, true);
    }
    let known = {
        let mut state = inner.state.lock().await;
        state
            .navigation_rules
            .retain(|_, rule| rule.active.load(std::sync::atomic::Ordering::Acquire));
        settle_openers(&mut state);
        if state.pending_openers.contains_key(&frame) {
            None
        } else if state.tabs.contains_key(&frame) {
            Some(top_frame_allowed(inner, &state, &frame, &url))
        } else {
            state
                .frames
                .get(&frame)
                .map(|target| subframe_allowed(&state, target, &url))
        }
    };
    if let Some(allowed) = known {
        return respond(inner, session.as_deref(), request_id, allowed);
    }
    // Unknown frame: a subframe, or a page whose Target.targetCreated has not been handled yet.
    // Reconcile off the event task; the request stays paused meanwhile and is denied if its
    // ownership cannot be established within the attach timeout.
    let inner = Arc::clone(inner);
    tokio::spawn(async move {
        let allowed = tokio::time::timeout(inner.attach_timeout, reconcile(&inner, &frame, &url))
            .await
            .unwrap_or(false);
        respond(&inner, session.as_deref(), request_id, allowed);
    });
}

const RECONCILE_INTERVAL: Duration = Duration::from_millis(50);

/// Owner of a tab seen in the ledger: `Some(None)` is a positively non-agent tab, `None` means
/// the tab is missing or is itself a popup whose ownership is still unresolved.
fn opener_owner(state: &State, opener: &str) -> Option<Option<String>> {
    if state.pending_openers.contains_key(opener) {
        return None;
    }
    state
        .tabs
        .get(opener)
        .map(|tab| tab.owner_session_id.clone())
}

/// Gives popups created before their opener reached the ledger that opener's owner, once it
/// has; repeated so a chain of such popups resolves in one pass.
fn settle_openers(state: &mut State) {
    loop {
        let ready: Vec<(String, Option<String>)> = state
            .pending_openers
            .iter()
            .filter_map(|(child, opener)| {
                opener_owner(state, opener).map(|owner| (child.clone(), owner))
            })
            .collect();
        if ready.is_empty() {
            return;
        }
        for (child, owner) in ready {
            state.pending_openers.remove(&child);
            if let Some(tab) = state.tabs.get_mut(&child) {
                tab.owner_session_id = owner;
            }
        }
    }
}

/// Decide a document request whose frame is not in the ledger. Requests bypass the owner
/// policy only when Chromium reports a non-page/subframe, or a default-context page with no
/// opener. An unresolved isolated context or opener waits until the caller's deadline denies it.
async fn reconcile(inner: &Inner, frame: &str, url: &str) -> bool {
    loop {
        {
            let mut state = inner.state.lock().await;
            if state.tabs.contains_key(frame) {
                settle_openers(&mut state);
                // A popup whose opener is still unknown keeps waiting for it.
                if !state.pending_openers.contains_key(frame) {
                    return top_frame_allowed(inner, &state, frame, url);
                }
            } else if let Some(target) = state.frames.get(frame) {
                return subframe_allowed(&state, target, url);
            }
        }
        match inner
            .cdp
            .call(None, "Target.getTargetInfo", json!({"targetId": frame}))
            .await
        {
            Err(BrowserError::Cdp { message, .. })
                if message.contains("No target with given id") =>
            {
                return unplaced_frame_allowed(&*inner.state.lock().await, url);
            }
            Ok(info) if info["targetInfo"]["type"] != "page" => {
                return unplaced_frame_allowed(&*inner.state.lock().await, url);
            }
            Ok(info) => {
                let target = &info["targetInfo"];
                let mut state = inner.state.lock().await;
                let default_context = if let Some(context) = target["browserContextId"].as_str() {
                    if state.closing.contains_key(context) {
                        return false;
                    }
                    if let Some((owner, _)) =
                        state.spaces.iter().find(|(_, space)| *space == context)
                    {
                        return owner_allows(inner, owner, url);
                    }
                    if state.isolated_contexts.contains(context) {
                        false
                    } else {
                        let listed = inner
                            .cdp
                            .call(None, "Target.getBrowserContexts", json!({}))
                            .await;
                        if let Some(ids) = listed
                            .as_ref()
                            .ok()
                            .and_then(|listed| listed["browserContextIds"].as_array())
                        {
                            state
                                .isolated_contexts
                                .extend(ids.iter().filter_map(|id| id.as_str().map(str::to_owned)));
                            !state.isolated_contexts.contains(context)
                        } else {
                            false
                        }
                    }
                } else {
                    true
                };
                if default_context {
                    match target["openerId"].as_str() {
                        None => return true,
                        Some(opener) => {
                            if let Some(owner) = opener_owner(&state, opener) {
                                return owner
                                    .map_or(true, |owner| owner_allows(inner, &owner, url));
                            }
                        }
                    }
                }
            }
            Err(_) => {}
        }
        tokio::time::sleep(RECONCILE_INTERVAL).await;
    }
}

fn top_frame_allowed(inner: &Inner, state: &State, target: &str, url: &str) -> bool {
    // Both checks always run: a guard_navigation rule records its own decision per hop.
    let rule = state
        .navigation_rules
        .get(target)
        .map_or(true, |rule| (rule.allow)(url));
    let owner = state
        .tabs
        .get(target)
        .and_then(|tab| tab.owner_session_id.as_deref())
        .map_or(true, |owner| owner_allows(inner, owner, url));
    let active_context = state
        .tab_contexts
        .get(target)
        .filter(|context| state.isolated_contexts.contains(*context))
        .map_or(true, |context| {
            !state.closing.contains_key(context)
                && state
                    .tabs
                    .get(target)
                    .and_then(|tab| tab.owner_session_id.as_ref())
                    .and_then(|owner| state.spaces.get(owner))
                    == Some(context)
        });
    rule && owner && active_context
}

impl NavigationRule {
    fn covers_frames(&self) -> bool {
        self.frames && self.active.load(std::sync::atomic::Ordering::Acquire)
    }
}

/// A subframe's document request: refused when its tab runs a frame-scoped rule that refuses it.
fn subframe_allowed(state: &State, target: &str, url: &str) -> bool {
    state
        .navigation_rules
        .get(target)
        .map_or(true, |rule| !rule.covers_frames() || (rule.allow)(url))
}

/// A frame the ledger cannot place may belong to any tab, so every frame-scoped rule judges it.
fn unplaced_frame_allowed(state: &State, url: &str) -> bool {
    state
        .navigation_rules
        .values()
        .all(|rule| !rule.covers_frames() || (rule.allow)(url))
}

fn owner_allows(inner: &Inner, owner: &str, url: &str) -> bool {
    let policy = inner
        .navigation_policy
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    policy.map_or(true, |policy| policy(owner, url))
}

fn respond(inner: &Inner, session: Option<&str>, request_id: Value, allow: bool) {
    if allow {
        inner.cdp.send(
            session,
            "Fetch.continueRequest",
            json!({"requestId": request_id}),
        );
    } else {
        // Aborted, unlike BlockedByClient, commits no error page, so the tab stays on the
        // document it was authorized for.
        inner.cdp.send(
            session,
            "Fetch.failRequest",
            json!({"requestId": request_id, "errorReason": "Aborted"}),
        );
    }
}

struct Inner {
    cdp: Cdp,
    state: Mutex<State>,
    attach_lock: Mutex<()>,
    capture_lock: Arc<Mutex<()>>,
    interaction_lock: Mutex<()>,
    network: network::Tracker,
    attach_timeout: Duration,
    events: mpsc::UnboundedSender<BrowserEvent>,
    navigation_policy: std::sync::RwLock<Option<OwnerNavigationCheck>>,
    screencasts: screencast::Slots,
    page: page_events::PageHub,
}

impl Inner {
    async fn begin_focus(self: Arc<Self>, session: String) -> Result<FocusCleanup, BrowserError> {
        let lock = Arc::clone(&self.capture_lock).lock_owned().await;
        let cleanup = FocusCleanup {
            inner: Arc::clone(&self),
            session,
            lock: Some(lock),
            armed: true,
        };
        let enabled = self
            .cdp
            .call(
                Some(&cleanup.session),
                "Emulation.setFocusEmulationEnabled",
                json!({"enabled":true}),
            )
            .await;
        if let Err(error) = enabled {
            let _ = cleanup.reset().await;
            return Err(error);
        }
        Ok(cleanup)
    }

    async fn capture(
        self: Arc<Self>,
        session: String,
        params: Value,
    ) -> Result<Vec<u8>, BrowserError> {
        let _capture = self.capture_lock.lock().await;
        let enabled = self
            .cdp
            .call(
                Some(&session),
                "Emulation.setFocusEmulationEnabled",
                json!({"enabled":true}),
            )
            .await;
        let captured = match enabled {
            Ok(_) => {
                self.cdp
                    .call(Some(&session), "Page.captureScreenshot", params)
                    .await
            }
            Err(error) => Err(error),
        };
        // Even an enable timeout is ambiguous: Chromium may already have enabled focus emulation.
        let reset = self
            .cdp
            .call(
                Some(&session),
                "Emulation.setFocusEmulationEnabled",
                json!({"enabled":false}),
            )
            .await;
        let result = captured?;
        reset?;
        base64::engine::general_purpose::STANDARD
            .decode(required_string(&result, "data")?)
            .map_err(|_| BrowserError::InvalidResponse("screenshot data"))
    }
}

struct Process {
    child: parking_lot::Mutex<Option<Child>>,
    #[cfg(unix)]
    group: Pid,
    #[cfg(not(unix))]
    pid: u32,
    profile: PathBuf,
    temporary_profile: Option<TempDir>,
    stderr: Option<JoinHandle<()>>,
    detached: bool,
    running: bool,
}

#[cfg(unix)]
/// Send SIGKILL (to the group first, then to the pid alone) and wait for `deadline` for the
/// process to be verified gone. A signalling error other than "no such process" fails, and
/// "no such process" is not believed either: the process must actually be seen gone.
async fn terminate_process(
    pid: Pid,
    child: &parking_lot::Mutex<Option<Child>>,
    signal: impl Fn(Pid, bool) -> nix::Result<()>,
    deadline: Duration,
) -> Result<(), BrowserError> {
    use nix::errno::Errno;
    match signal(pid, true) {
        Ok(()) | Err(Errno::ESRCH) => {}
        // A browser adopted from a previous run is not necessarily its own group leader.
        Err(_) => match signal(pid, false) {
            Ok(()) | Err(Errno::ESRCH) => {}
            Err(error) => return Err(std::io::Error::from(error).into()),
        },
    }
    tokio::time::timeout(deadline, async {
        while !process_gone(pid, child) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| BrowserError::Timeout("Chromium termination"))
}

#[cfg(unix)]
fn process_gone(pid: Pid, child: &parking_lot::Mutex<Option<Child>>) -> bool {
    match child.lock().as_mut() {
        // Only reaping proves a child dead: a killed one stays signalable as a zombie.
        Some(child) => matches!(child.try_wait(), Ok(Some(_))),
        None => matches!(kill(pid, None), Err(nix::errno::Errno::ESRCH)),
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if !self.detached && self.running {
            // The browser's renderers inherit its dedicated group, not the host's group.
            #[cfg(unix)]
            let _ = killpg(self.group, Signal::SIGKILL);
            if let Some(child) = self.child.get_mut() {
                let _ = child.start_kill();
            }
        }
        if self.detached {
            if let Some(profile) = self.temporary_profile.take() {
                let _ = profile.keep();
            }
        }
        if let Some(stderr) = &self.stderr {
            stderr.abort();
        }
    }
}

struct AttachCleanup {
    inner: Arc<Inner>,
    session: String,
    armed: bool,
}

impl Drop for AttachCleanup {
    fn drop(&mut self) {
        if self.armed {
            let inner = Arc::clone(&self.inner);
            let session = self.session.clone();
            tokio::spawn(async move {
                let _ = inner
                    .cdp
                    .call(
                        None,
                        "Target.detachFromTarget",
                        json!({"sessionId":session}),
                    )
                    .await;
            });
        }
    }
}

struct FocusCleanup {
    inner: Arc<Inner>,
    session: String,
    lock: Option<OwnedMutexGuard<()>>,
    armed: bool,
}

impl FocusCleanup {
    async fn reset(mut self) -> Result<(), BrowserError> {
        let reset = self
            .inner
            .cdp
            .call(
                Some(&self.session),
                "Emulation.setFocusEmulationEnabled",
                json!({"enabled":false}),
            )
            .await;
        self.armed = false;
        reset.map(|_| ())
    }
}

impl Drop for FocusCleanup {
    fn drop(&mut self) {
        if self.armed {
            let inner = Arc::clone(&self.inner);
            let session = std::mem::take(&mut self.session);
            let lock = self.lock.take();
            // Cancellation must not release serialization before Chromium has reset emulated focus.
            tokio::spawn(async move {
                let _lock = lock;
                let _ = inner
                    .cdp
                    .call(
                        Some(&session),
                        "Emulation.setFocusEmulationEnabled",
                        json!({"enabled":false}),
                    )
                    .await;
            });
        }
    }
}

/// Managed Chromium with ownership-enforced tabs and optional durable process/profile lifetime.
pub struct Browser {
    inner: Arc<Inner>,
    process: Process,
    ws_url: String,
    event_task: JoinHandle<()>,
    gateway: Option<Gateway>,
    gateway_dir: Option<PathBuf>,
    /// Started by this code with the profile preferences that keep documents from reaching a
    /// tab without a network request (see `harden_profile`).
    isolated: bool,
}

fn required_string<'a>(value: &'a Value, field: &'static str) -> Result<&'a str, BrowserError> {
    value[field]
        .as_str()
        .ok_or(BrowserError::InvalidResponse(field))
}

fn target_tab(info: &Value) -> Option<TabInfo> {
    if info["type"] != "page" {
        return None;
    }
    Some(TabInfo {
        id: info["targetId"].as_str()?.to_owned(),
        url: info["url"].as_str().unwrap_or_default().to_owned(),
        title: info["title"].as_str().unwrap_or_default().to_owned(),
        opened_by: OpenedBy::External,
        owner_session_id: None,
        discarded: false,
        unloaded: false,
        browser_context_id: None,
    })
}

/// Page-session methods accepted by [`Browser::page_command`]. The REPL's own operations use
/// these; the script-facing `page.cdp(...)` goes through [`Browser::page_cdp`] instead.
///
/// Entries with a comment were added for the REPL port slices and name their use.
pub const PAGE_COMMANDS: &[&str] = &[
    "Accessibility.getFullAXTree",
    "Accessibility.getPartialAXTree",
    "Accessibility.queryAXTree",
    "DOM.describeNode",
    "DOM.discardSearchResults", // xpath locators, with performSearch
    "DOM.focus",
    "DOM.getAttributes",
    "DOM.getBoxModel",
    "DOM.getContentQuads", // click points of transformed or fragmented boxes
    "DOM.getDocument",
    "DOM.getFrameOwner", // iframe element of a frame, for coordinate translation
    "DOM.getOuterHTML",
    "DOM.getSearchResults", // xpath locators, with performSearch
    "DOM.performSearch",    // xpath locators
    "DOM.querySelectorAll",
    "DOM.requestNode", // node id of a remote object, for evaluate handles
    "DOM.resolveNode", // remote object of a backend node, for locator.evaluate
    "DOM.scrollIntoViewIfNeeded",
    "DOM.setFileInputFiles", // setInputFiles by backend node id, no page script
    "Emulation.setFocusEmulationEnabled",
    "Input.dispatchDragEvent", // drag and drop
    "Input.dispatchKeyEvent",
    "Input.dispatchMouseEvent",
    "Input.imeSetComposition", // trusted IME preedit, e.g. Hangul composition
    "Input.insertText",
    "Storage.getStorageKeyForFrame", // trusted binding of raw page storage parameters
    "Input.setInterceptDrags",       // drag and drop
    "Network.disable",               // in-flight request tracking for network idle
    "Network.enable",                // in-flight request tracking for network idle
    "Network.getResponseBody",       // response.text() / json() of a waited-for response
    "Page.captureScreenshot",        // clip / full-page screenshots
    "Page.addScriptToEvaluateOnNewDocument", // page.addInitScript
    "Page.createIsolatedWorld",      // evaluate in a world page scripts cannot reach
    "Page.getFrameTree",
    "Page.getLayoutMetrics", // full-page size, device-pixel ratio, scroll offsets
    "Page.getNavigationHistory",
    "Page.handleJavaScriptDialog", // accept or dismiss a dialog the page opened
    "Page.navigate",
    "Page.navigateToHistoryEntry",
    "Page.reload",
    "Page.removeScriptToEvaluateOnNewDocument", // page.addInitScript cleanup
    "Page.setInterceptFileChooserDialog",       // file chooser interception
    "Page.setLifecycleEventsEnabled",           // DOMContentLoaded / load / network idle events
    "Page.stopLoading",
    "Performance.enable",
    "Performance.getMetrics",
    "Runtime.awaitPromise",   // page.evaluate and handles
    "Runtime.callFunctionOn", // locator.evaluate, $eval, evaluateHandle
    "Runtime.compileScript",
    "Runtime.disable",
    "Runtime.enable",        // execution contexts per frame and world
    "Runtime.evaluate",      // page.evaluate, waitForFunction
    "Runtime.getProperties", // evaluateHandle properties
    "Runtime.releaseObject",
    "Runtime.releaseObjectGroup",
    "Runtime.runScript",
    "Runtime.terminateExecution", // bounded recovery of an abandoned renderer call
    "Target.setAutoAttach",       // page-level OOPIF auto-attach, never browser-level
];

// Runs in an isolated world of the top frame and validates and writes within one JavaScript turn,
// so page script cannot move focus, navigate, swap the target, or hook the setter and checks. The
// value is a call argument, never source text; only static status codes leave the page.
// `not_editable` guarantees nothing was written, while `failed` means the outcome is unknown.
// It writes exactly the bound node; spreading a code over split boxes is the OTP-only
// [`fill_otp_group_script`].
const FILL_NODE: &str = r"function(value, expectedOrigin) {
    let wrote = false;
    try {
        if (window.top !== window || !this.isConnected || this.ownerDocument !== document) return 'stale';
        if (location.origin !== expectedOrigin) return 'origin';
        let prototype;
        if (this instanceof HTMLTextAreaElement) {
            prototype = HTMLTextAreaElement.prototype;
        } else if (this instanceof HTMLInputElement
                && ['text', 'password', 'email', 'tel', 'url', 'search', 'number'].includes(this.type)) {
            prototype = HTMLInputElement.prototype;
        } else {
            return 'not_editable';
        }
        if (this.disabled || this.readOnly) return 'not_editable';
        wrote = true;
        Object.getOwnPropertyDescriptor(prototype, 'value').set.call(this, value);
        this.dispatchEvent(new Event('input', { bubbles: true }));
        this.dispatchEvent(new Event('change', { bubbles: true }));
        return 'filled';
    } catch (_) {
        return wrote ? 'failed' : 'not_editable';
    }
}";

// Shared by the OTP box scripts: which inputs may hold one character of a split code.
const OTP_BOX_RULES: &str = r"
    const textTypes = ['text', 'password', 'email', 'tel', 'url', 'search', 'number'];
    const visible = (node) => {
        const rect = node.getBoundingClientRect();
        const style = getComputedStyle(node);
        return rect.width > 0 && rect.height > 0 && style.visibility === 'visible'
            && style.display !== 'none' && Number(style.opacity) > 0
            && (!node.checkVisibility || node.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true }));
    };
    const box = (node) => node instanceof HTMLInputElement && textTypes.includes(node.type)
        && !node.disabled && !node.readOnly && node.isConnected && node.ownerDocument === document
        && visible(node);
    const scope = (node) => Array.from((node.form || document).querySelectorAll('input'));
";

/// The visible run of one-character code boxes starting at the bound node, in tree order: inputs
/// of the same form (or document) that all have `maxlength=1`, or all share
/// `autocomplete=one-time-code`, up to the first input that differs or is hidden. Returns the
/// boxes (at least two) or just the bound node. Pinned once, when a code is asked for.
fn otp_group_script() -> String {
    format!(
        r"function() {{
    {OTP_BOX_RULES}
    if (window.top !== window || !box(this)) return [this];
    const oneTimeCode = (node) =>
        (node.getAttribute('autocomplete') || '').trim().toLowerCase() === 'one-time-code';
    const sameKind = this.maxLength === 1 ? (node) => node.maxLength === 1
        : oneTimeCode(this) ? oneTimeCode : null;
    if (!sameKind) return [this];
    const inputs = scope(this);
    const group = [];
    for (let at = inputs.indexOf(this); at >= 0 && at < inputs.length && group.length < 12; at++) {{
        if (!box(inputs[at]) || !sameKind(inputs[at])) break;
        group.push(inputs[at]);
    }}
    return group.length >= 2 ? group : [this];
}}"
    )
}

/// Spread a code over the boxes pinned by [`otp_group_script`], one character each, with
/// keydown/beforeinput/input/keyup/change. Before every box, and again after the page's
/// synchronous keydown/beforeinput handlers, the whole group must still be the same visible,
/// writable, consecutive run in the expected origin; a cancelled keydown or beforeinput stops
/// the fill like a page that refuses input. Same status codes as `FILL_NODE`, plus
/// `split_mismatch` (nothing written) when the code length differs from the box count.
fn fill_otp_group_script() -> String {
    format!(
        r"function(value, expectedOrigin, ...boxes) {{
    let wrote = false;
    try {{
        {OTP_BOX_RULES}
        const chars = Array.from(value);
        if (boxes.length < 2 || boxes[0] !== this) return 'stale';
        if (chars.length !== boxes.length) return 'split_mismatch';
        const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
        const intact = () => {{
            if (window.top !== window || location.origin !== expectedOrigin) return false;
            const inputs = scope(boxes[0]);
            const start = inputs.indexOf(boxes[0]);
            return start >= 0 && boxes.every((node, at) => inputs[start + at] === node && box(node));
        }};
        for (let at = 0; at < chars.length; at++) {{
            const node = boxes[at];
            const key = chars[at];
            if (!intact()) return wrote ? 'failed' : 'stale';
            node.focus();
            const typed = node.dispatchEvent(new KeyboardEvent('keydown', {{ key, bubbles: true, cancelable: true }}))
                && node.dispatchEvent(new InputEvent('beforeinput',
                    {{ inputType: 'insertText', data: key, bubbles: true, cancelable: true }}));
            // The handlers above ran synchronously and may have swapped, hidden or moved boxes.
            if (!intact()) return wrote ? 'failed' : 'stale';
            if (!typed) return wrote ? 'failed' : 'not_editable';
            wrote = true;
            setter.call(node, key);
            node.dispatchEvent(new InputEvent('input', {{ inputType: 'insertText', data: key, bubbles: true }}));
            node.dispatchEvent(new KeyboardEvent('keyup', {{ key, bubbles: true, cancelable: true }}));
            node.dispatchEvent(new Event('change', {{ bubbles: true }}));
        }}
        return 'filled';
    }} catch (_) {{
        return wrote ? 'failed' : 'not_editable';
    }}
}}"
    )
}

fn text_insertion_error(error: BrowserError) -> BrowserError {
    match error {
        BrowserError::StaleReference => BrowserError::StaleReference,
        BrowserError::OriginMismatch => BrowserError::OriginMismatch,
        // Static messages only; callers rely on this variant meaning nothing was written.
        BrowserError::InvalidAction(reason) => BrowserError::InvalidAction(reason),
        _ => BrowserError::TextInsertionFailed,
    }
}

/// Command-line marker of a browser started with the preferences from [`harden_profile`].
const ISOLATION_SWITCH: &str = "--openside-isolation=1";

/// Profile preferences the agent browser must not run without. Preloading is turned off: a page
/// can ask for another document to be prerendered and later activate it into its tab without
/// any network request, which neither navigation rule can see; `--disable-features` no longer
/// reaches that path. Autofill and Chromium's own password manager are turned off too: they
/// would otherwise keep every identifier typed into a sign-in form in `Web Data` and `Login
/// Data`, next to the vault that is meant to be the only store.
async fn harden_profile(profile: &Path) -> Result<(), BrowserError> {
    let directory = profile.join("Default");
    let path = directory.join("Preferences");
    let mut preferences = match tokio::fs::read(&path).await {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|_| json!({})),
        Err(_) => json!({}),
    };
    if !preferences.is_object() {
        preferences = json!({});
    }
    // 2 = NetworkPredictionOptions::kDisabled
    preferences["net"]["network_prediction_options"] = json!(2);
    preferences["autofill"]["profile_enabled"] = json!(false);
    preferences["autofill"]["credit_card_enabled"] = json!(false);
    preferences["credentials_enable_service"] = json!(false);
    preferences["profile"]["password_manager_enabled"] = json!(false);
    tokio::fs::create_dir_all(&directory).await?;
    tokio::fs::write(&path, serde_json::to_vec(&preferences).unwrap_or_default()).await?;
    Ok(())
}

fn endpoint_url(contents: &str) -> Option<String> {
    let mut lines = contents.lines();
    let port = lines.next()?.parse::<u16>().ok()?;
    let path = lines.next()?;
    (port != 0 && path.starts_with("/devtools/browser/"))
        .then(|| format!("ws://127.0.0.1:{port}{path}"))
}

async fn endpoint(
    profile: &Path,
    mut receiver: mpsc::UnboundedReceiver<String>,
    read_stderr: bool,
) -> Result<String, BrowserError> {
    let mut poll = tokio::time::interval(Duration::from_millis(25));
    loop {
        tokio::select! {
            line = receiver.recv(), if read_stderr => {
                match line {
                    Some(line) => {
                        if let Some(url) = line.strip_prefix("DevTools listening on ") {
                            return Ok(url.trim().to_owned());
                        }
                    }
                    None => return Err(BrowserError::Disconnected),
                }
            }
            _ = poll.tick() => {
                if let Ok(contents) = tokio::fs::read_to_string(profile.join("DevToolsActivePort")).await {
                    if let Some(url) = endpoint_url(&contents) {
                        return Ok(url);
                    }
                }
            }
        }
    }
}

fn key_event(key: &str) -> Result<Value, BrowserError> {
    let (modifiers, key) = match key.rsplit_once('+') {
        Some((prefix, key)) => {
            let mut modifiers = 0;
            for modifier in prefix.split('+') {
                modifiers |= match modifier {
                    "Alt" => 1,
                    "Control" | "Ctrl" => 2,
                    "Meta" | "Command" => 4,
                    "Shift" => 8,
                    _ => return Err(BrowserError::InvalidAction("unsupported key modifier")),
                };
            }
            (modifiers, key)
        }
        None => (0, key),
    };
    let (key, code, virtual_key, text) = match key {
        "Enter" => ("Enter", "Enter".to_owned(), 13, "\r"),
        "Tab" => ("Tab", "Tab".to_owned(), 9, ""),
        "Escape" | "Esc" => ("Escape", "Escape".to_owned(), 27, ""),
        "Backspace" => ("Backspace", "Backspace".to_owned(), 8, ""),
        "Delete" => ("Delete", "Delete".to_owned(), 46, ""),
        "ArrowLeft" => (key, key.to_owned(), 37, ""),
        "ArrowUp" => (key, key.to_owned(), 38, ""),
        "ArrowRight" => (key, key.to_owned(), 39, ""),
        "ArrowDown" => (key, key.to_owned(), 40, ""),
        "Home" => (key, key.to_owned(), 36, ""),
        "End" => (key, key.to_owned(), 35, ""),
        "PageUp" => (key, key.to_owned(), 33, ""),
        "PageDown" => (key, key.to_owned(), 34, ""),
        "Space" | " " => (" ", "Space".to_owned(), 32, " "),
        _ if key.chars().count() == 1 => {
            let character = key
                .chars()
                .next()
                .ok_or(BrowserError::InvalidAction("empty key"))?;
            let code = if character.is_ascii_alphabetic() {
                format!("Key{}", character.to_ascii_uppercase())
            } else if character.is_ascii_digit() {
                format!("Digit{character}")
            } else {
                String::new()
            };
            let virtual_key = if character.is_ascii() {
                u32::from(character.to_ascii_uppercase())
            } else {
                0
            };
            (key, code, virtual_key, key)
        }
        _ => return Err(BrowserError::InvalidAction("unsupported key")),
    };
    let mut event = json!({
        "type":"keyDown","key":key,"code":code,"modifiers":modifiers,
        "windowsVirtualKeyCode":virtual_key
    });
    if !text.is_empty() && modifiers & 7 == 0 {
        event["text"] = json!(text);
        event["unmodifiedText"] = json!(text);
    }
    Ok(event)
}

impl Browser {
    #[cfg(not(unix))]
    /// Reject launches where process-group supervision is unavailable.
    pub async fn launch(
        _config: BrowserConfig,
    ) -> Result<(Self, mpsc::UnboundedReceiver<BrowserEvent>), BrowserError> {
        Err(BrowserError::InvalidEndpoint(
            "Browser execution is unsupported on this platform",
        ))
    }

    #[cfg(unix)]
    /// Launch isolated headless Chromium and subscribe to its browser events.
    pub async fn launch(
        config: BrowserConfig,
    ) -> Result<(Self, mpsc::UnboundedReceiver<BrowserEvent>), BrowserError> {
        if !config.executable.is_file() {
            return Err(BrowserError::ChromiumMissing(
                config.executable.display().to_string(),
            ));
        }
        let (profile, temporary_profile) = match config.profile_dir {
            Some(path) => {
                tokio::fs::create_dir_all(&path).await?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                        .await?;
                }
                (tokio::fs::canonicalize(path).await?, None)
            }
            None => {
                let temporary = tempfile::Builder::new()
                    .prefix("openside-chromium-")
                    .tempdir()?;
                (temporary.path().to_owned(), Some(temporary))
            }
        };
        harden_profile(&profile).await?;
        let active_port = profile.join("DevToolsActivePort");
        if let Ok(contents) = tokio::fs::read_to_string(&active_port).await {
            if let Some(url) = endpoint_url(&contents) {
                if Cdp::connect(&url, Duration::from_secs(1)).await.is_ok() {
                    return Err(BrowserError::InvalidEndpoint(
                        "profile is already in use; reconnect to its saved endpoint",
                    ));
                }
            }
            // A stale endpoint from the previous process must not race the new launch.
            tokio::fs::remove_file(active_port).await?;
        }
        let mut command = Command::new(&config.executable);
        #[cfg(target_os = "linux")]
        let supervise = !config.detached || config.die_with_spawner;
        // PR_SET_PDEATHSIG lets the kernel kill Chromium when the spawning thread dies —
        // abort, SIGKILL, a harness timeout — where no destructor can run. Detached launches
        // stay unsupervised: a reconnecting daemon must adopt a live browser whose spawner
        // already exited. The signal binds to the forking *thread* (POSIX thread-group
        // semantics), so launches must come from runtime worker threads that live as long as
        // the process; never spawn from a short-lived or spawn_blocking thread.
        #[cfg(target_os = "linux")]
        if supervise {
            use std::os::unix::process::CommandExt;
            // SAFETY: the hook runs only async-signal-safe work between fork and exec: one
            // prctl(2) syscall through nix. The workspace denies unsafe_code except at
            // audited call sites like this one.
            #[allow(unsafe_code)]
            unsafe {
                command.as_std_mut().pre_exec(|| {
                    nix::sys::prctl::set_pdeathsig(Signal::SIGKILL).map_err(std::io::Error::from)
                });
            }
        }
        command
            .args([
                "--headless=new",
                "--remote-debugging-port=0",
                "--remote-debugging-address=127.0.0.1",
                "--disable-backgrounding-occluded-windows",
                "--disable-renderer-backgrounding",
                "--disable-background-timer-throttling",
                // Distribution wrappers must not opt isolated profiles into user keyrings.
                "--password-store=basic",
                "--disable-sync",
                "--no-first-run",
                "--no-default-browser-check",
                // Local-first (PP-25): no update, safe-browsing, metrics or prefetch traffic
                // that the user did not ask for.
                "--disable-background-networking",
                "--disable-component-update",
                "--disable-domain-reliability",
                "--disable-client-side-phishing-detection",
                "--disable-default-apps",
                "--no-pings",
                "--dns-prefetch-disable",
                // AimEnabled: omnibox AI-mode eligibility fetch to www.google.com at startup.
                // PreconnectToSearch/PreconnectFromKeyedService: speculative DNS+connect to the
                // default search engine.
                "--disable-features=OptimizationHints,MediaRouter,DialMediaRouteProvider,AutofillServerCommunication,NetworkTimeServiceQuerying,Translate,AimEnabled,PreconnectToSearch,PreconnectFromKeyedService",
                // Lets a later `connect` tell from the running process that it was started with
                // the preferences written by `harden_profile`; Chromium ignores the switch.
                ISOLATION_SWITCH,
                // Stock builds check in to GCM at startup even with background networking
                // disabled and no switch turns it off; point it at the loopback discard port.
                "--gcm-checkin-url=http://127.0.0.1:9/checkin",
                "--gcm-registration-url=http://127.0.0.1:9/register",
                "--gcm-mcs-endpoint=https://127.0.0.1:9",
                // --disable-component-update stops scheduled checks but not on-demand ones;
                // Chromium 154's on-device model manifest broker requests one at startup
                // (update.googleapis.com). A plain-HTTP loopback source makes the updater drop
                // every such request before any lookup or connection.
                "--component-updater=url-source=http://127.0.0.1:9/update",
                "about:blank",
            ])
            .arg(format!("--user-data-dir={}", profile.display()))
            // The cache stays inside the managed profile even when the profile sits under
            // XDG_CONFIG_HOME, where Chromium would mirror it into XDG_CACHE_HOME. The XDG
            // environment itself is inherited unchanged: native-messaging hosts (password
            // managers) run with the browser's environment and find their desktop app through
            // the user's XDG_CACHE_HOME / XDG_RUNTIME_DIR.
            .arg(format!("--disk-cache-dir={}", profile.join("cache").display()))
            // Crashpad keeps its database under the default user-data dir, which follows the
            // inherited XDG_CONFIG_HOME, not --user-data-dir; this switch (not the
            // BREAKPAD_DUMP_LOCATION variable, which native hosts would inherit) keeps it in
            // the profile.
            .arg(format!(
                "--breakpad-dump-location={}",
                profile.join("Crash Reports").display()
            ))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(if config.detached {
                std::process::Stdio::null()
            } else {
                std::process::Stdio::piped()
            })
            .kill_on_drop(!config.detached)
            .process_group(0);
        if config.extensions.is_empty() {
            command.arg("--disable-extensions");
        } else {
            let mut paths = Vec::with_capacity(config.extensions.len());
            for path in &config.extensions {
                let text = path.to_str().filter(|text| !text.contains([',', '\0']));
                if !path.is_absolute() || !path.is_dir() || text.is_none() {
                    return Err(BrowserError::InvalidEndpoint(
                        "invalid unpacked extension directory",
                    ));
                }
                paths.push(text.unwrap_or_default());
            }
            let paths = paths.join(",");
            command.arg(format!("--load-extension={paths}"));
            command.arg(format!("--disable-extensions-except={paths}"));
        }
        let gateway_launch = match &config.gateway {
            Some(gateway) => GatewayLaunch::prepare(gateway)?,
            None => None,
        };
        if let Some(launch) = &gateway_launch {
            command.args(launch.args());
        }
        let mut child = command.spawn()?;
        let group = Pid::from_raw(
            i32::try_from(child.id().ok_or(BrowserError::Disconnected)?)
                .map_err(|_| BrowserError::InvalidResponse("Chromium pid"))?,
        );
        let (lines, receiver) = mpsc::unbounded_channel();
        let stderr = child.stderr.take().map(|stderr| {
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    // Drain non-detached diagnostics without retaining page or input data.
                    let _ = lines.send(line);
                }
            })
        });
        let process = Process {
            child: parking_lot::Mutex::new(Some(child)),
            group,
            profile,
            temporary_profile,
            stderr,
            // Failed startup must clean up even when a successful launch would be detached.
            detached: false,
            running: true,
        };
        let url = tokio::time::timeout(
            config.launch_timeout,
            endpoint(&process.profile, receiver, !config.detached),
        )
        .await
        .map_err(|_| BrowserError::Timeout("Chromium launch"))??;
        let (mut browser, events) =
            Self::initialize(process, url, config.command_timeout, config.attach_timeout).await?;
        // A fresh process has no restored spaces; stale private downloads must not survive it.
        browser.prune_spaces().await?;
        if let (Some(launch), Some(gateway)) = (gateway_launch, &config.gateway) {
            browser.gateway_dir = Some(launch.directory());
            // A patched browser publishes discovery before its debugging endpoint, so an
            // optional gateway needs no grace period: absent now means a stock browser.
            let wait = if gateway.required {
                gateway.discovery_timeout
            } else {
                Duration::ZERO
            };
            match Gateway::discover(&launch, wait, config.command_timeout).await {
                Ok(connected) => browser.gateway = Some(connected),
                // Stock Chromium ignores the gateway switches and never publishes discovery.
                Err(BrowserError::Timeout("gateway discovery")) if !gateway.required => {}
                Err(BrowserError::Timeout("gateway discovery")) => {
                    return Err(BrowserError::InvalidEndpoint(
                        "browser published no gateway; use the patched browser",
                    ));
                }
                Err(error) => return Err(error),
            }
        }
        // Only a fully validated launch may outlive this controller; every error above drops a
        // non-detached Process, which kills the browser group and removes its temp profile.
        browser.process.detached = config.detached;
        browser.isolated = true;
        Ok((browser, events))
    }

    #[cfg(not(unix))]
    /// Reject reconnects where process identity cannot be verified.
    pub async fn connect(
        _endpoint: BrowserEndpoint,
    ) -> Result<(Self, mpsc::UnboundedReceiver<BrowserEvent>), BrowserError> {
        Err(BrowserError::InvalidEndpoint(
            "Browser execution is unsupported on this platform",
        ))
    }

    #[cfg(unix)]
    /// Reconnect only to the live isolated process matching the saved profile and CDP endpoint.
    pub async fn connect(
        endpoint: BrowserEndpoint,
    ) -> Result<(Self, mpsc::UnboundedReceiver<BrowserEvent>), BrowserError> {
        let pid = i32::try_from(endpoint.pid)
            .ok()
            .filter(|pid| *pid > 0)
            .map(Pid::from_raw)
            .ok_or(BrowserError::InvalidEndpoint("invalid process identifier"))?;
        if kill(pid, None) == Err(nix::errno::Errno::ESRCH) {
            spaces::sweep_downloads(&endpoint.profile_dir, &HashSet::new()).await?;
            uploads::sweep_dead_profile(&endpoint.profile_dir).await?;
        }
        if kill(pid, None).is_err() || nix::unistd::getpgid(Some(pid)).ok() != Some(pid) {
            return Err(BrowserError::InvalidEndpoint(
                "browser process is not running",
            ));
        }
        let saved =
            tokio::fs::read_to_string(endpoint.profile_dir.join("DevToolsActivePort")).await?;
        if endpoint_url(&saved).as_deref() != Some(endpoint.ws_url.as_str()) {
            return Err(BrowserError::InvalidEndpoint(
                "profile endpoint has changed",
            ));
        }
        let process = Process {
            child: parking_lot::Mutex::new(None),
            group: pid,
            profile: endpoint.profile_dir,
            temporary_profile: None,
            stderr: None,
            detached: true,
            running: true,
        };
        let (mut browser, events) = Self::initialize(
            process,
            endpoint.ws_url,
            Duration::from_secs(12),
            Duration::from_secs(12),
        )
        .await?;
        // Only the running process can say how it was started; a file marker could be stale.
        browser.isolated = browser
            .inner
            .cdp
            .call(None, "SystemInfo.getInfo", json!({}))
            .await
            .ok()
            .and_then(|info| {
                info["commandLine"]
                    .as_str()
                    .map(|line| line.contains(ISOLATION_SWITCH))
            })
            .unwrap_or(false);
        Ok((browser, events))
    }

    #[cfg(unix)]
    async fn initialize(
        process: Process,
        ws_url: String,
        command_timeout: Duration,
        attach_timeout: Duration,
    ) -> Result<(Self, mpsc::UnboundedReceiver<BrowserEvent>), BrowserError> {
        let (cdp, mut notifications) = Cdp::connect(&ws_url, command_timeout).await?;
        let processes = cdp
            .call(None, "SystemInfo.getProcessInfo", json!({}))
            .await?;
        let reported_pid = processes["processInfo"]
            .as_array()
            .and_then(|processes| {
                processes
                    .iter()
                    .find(|process| process["type"] == "browser")
            })
            .and_then(|process| process["id"].as_i64());
        if reported_pid != Some(i64::from(process.group.as_raw())) {
            return Err(BrowserError::InvalidEndpoint(
                "CDP process identity mismatch",
            ));
        }
        let (events, receiver) = mpsc::unbounded_channel();
        let inner = Arc::new(Inner {
            cdp,
            state: Mutex::default(),
            attach_lock: Mutex::default(),
            capture_lock: Arc::default(),
            interaction_lock: Mutex::default(),
            network: Default::default(),
            attach_timeout,
            events,
            navigation_policy: std::sync::RwLock::new(None),
            screencasts: Default::default(),
            page: Default::default(),
        });
        let weak = Arc::downgrade(&inner);
        let event_task = tokio::spawn(async move {
            while let Some(event) = notifications.recv().await {
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                if event["method"] == "Page.screencastFrame" {
                    screencast::deliver(&inner, &event);
                    continue;
                }
                if event["method"] == "Fetch.requestPaused" {
                    handle_paused(&inner, &event).await;
                    continue;
                }
                if event["method"] == "Target.attachedToTarget" {
                    frames::adopt(&inner, &event).await;
                }
                inner.network.observe(&event);
                handle_event(&inner, event).await;
            }
        });
        let browser = Self {
            inner,
            process,
            ws_url,
            event_task,
            gateway: None,
            gateway_dir: None,
            isolated: false,
        };
        let contexts = browser.live_contexts().await?;
        browser
            .inner
            .state
            .lock()
            .await
            .isolated_contexts
            .extend(contexts);
        browser
            .inner
            .cdp
            .call(None, "Target.setDiscoverTargets", json!({"discover":true}))
            .await?;
        let mut targets = browser
            .inner
            .cdp
            .call(None, "Target.getTargets", json!({}))
            .await?;
        let targets = targets["targetInfos"].take();
        let Value::Array(targets) = targets else {
            return Err(BrowserError::InvalidResponse("page targets"));
        };
        for target in targets {
            handle_event(
                &browser.inner,
                json!({"method":"Target.targetCreated","params":{"targetInfo":target}}),
            )
            .await;
        }
        browser.prune_uploads().await?;
        // Discovery supplies lifecycle events; automatic attachment would violate the asleep-tab gate.
        browser
            .inner
            .cdp
            .call(
                None,
                "Target.setAutoAttach",
                json!({"autoAttach":false,"waitForDebuggerOnStart":false,"flatten":true}),
            )
            .await?;
        // Browser-level interception pauses document requests of every target, including a
        // popup's first request, without attaching to (and waking) any tab; see handle_paused.
        browser
            .inner
            .cdp
            .call(
                None,
                "Fetch.enable",
                json!({"patterns": [{"urlPattern": "*", "resourceType": DOCUMENT_INTERCEPTION, "requestStage": "Request"}]}),
            )
            .await?;
        let downloads = browser.profile_path().join("downloads");
        tokio::fs::create_dir_all(&downloads).await?;
        browser
            .inner
            .cdp
            .call(
                None,
                "Browser.setDownloadBehavior",
                json!({"behavior":"allowAndName","downloadPath":downloads,"eventsEnabled":true}),
            )
            .await?;
        Ok((browser, receiver))
    }

    /// Managed profile path; explicitly configured profiles are never removed automatically.
    pub fn profile_path(&self) -> &Path {
        &self.process.profile
    }

    /// Chromium's process identifier, useful for explicit cleanup verification.
    pub fn process_id(&self) -> u32 {
        #[cfg(unix)]
        {
            self.process.group.as_raw() as u32
        }
        #[cfg(not(unix))]
        {
            self.process.pid
        }
    }

    /// Persist this non-secret endpoint to reconnect after a controller restart.
    pub fn endpoint(&self) -> BrowserEndpoint {
        BrowserEndpoint {
            pid: self.process_id(),
            ws_url: self.ws_url.clone(),
            profile_dir: self.process.profile.clone(),
        }
    }

    /// Disconnect without terminating a detached browser or deleting its live profile.
    pub async fn detach(self) -> Result<(), BrowserError> {
        if !self.process.detached {
            return Err(BrowserError::InvalidAction(
                "only detached browsers can be disconnected",
            ));
        }
        Ok(())
    }

    /// False once the browser-level CDP transport closed, e.g. after Chromium was killed.
    pub fn is_connected(&self) -> bool {
        !self.inner.cdp.closed()
    }

    /// Whether this process was started by [`Browser::launch`] with the profile preferences
    /// that keep prerendered documents out of tabs. A browser re-adopted from an older build
    /// reports false, and nothing about its running state can be repaired from outside.
    pub fn documents_isolated(&self) -> bool {
        self.isolated
    }

    /// Return ledger entries in stable target-id order, with titles refreshed from
    /// `Target.getTargets`. Chrome's `targetInfoChanged` reports the document title after a
    /// tab's first load but not after a later same-tab navigation, so the ledger's own copy can
    /// stay at the provisional `host/path` title forever. The URL stays the ledger's: Chrome
    /// also reports pending navigations the ledger never committed (such as policy-blocked
    /// popups), so a title is taken only when Chrome's URL equals the ledger URL. If the call
    /// fails, the ledger copy is returned as is.
    pub async fn tabs(&self) -> Vec<TabInfo> {
        let current = self
            .inner
            .cdp
            .call(None, "Target.getTargets", json!({}))
            .await
            .ok();
        let mut tabs: Vec<_> = self
            .inner
            .state
            .lock()
            .await
            .tabs
            .values()
            .cloned()
            .collect();
        if let Some(Value::Array(infos)) = current.as_ref().map(|result| &result["targetInfos"]) {
            for info in infos {
                let Some(tab) = tabs
                    .iter_mut()
                    .find(|tab| info["targetId"].as_str() == Some(tab.id.as_str()))
                else {
                    continue;
                };
                if info["url"].as_str() == Some(tab.url.as_str()) {
                    if let Some(title) = info["title"].as_str() {
                        tab.title = title.to_owned();
                    }
                }
            }
        }
        tabs.sort_by(|a, b| a.id.cmp(&b.id));
        tabs
    }

    /// Create a background tab in its own window and fix its viewport to 1440×900. An owner
    /// with an isolated space ([`Browser::open_space`]) gets the tab in that space.
    pub async fn create_agent_tab(&self, owner: &str, url: &str) -> Result<TabInfo, BrowserError> {
        self.create_agent_tab_cancellable(owner, url, std::future::pending())
            .await
    }

    /// Drain creation to learn the target id, then close a cancelled new tab before returning.
    pub async fn create_agent_tab_cancellable(
        &self,
        owner: &str,
        url: &str,
        cancellation: impl Future<Output = ()>,
    ) -> Result<TabInfo, BrowserError> {
        // Hold the ledger lock through createTarget so its earlier event cannot misclassify ownership.
        let mut state = self.inner.state.lock().await;
        let context = state.spaces.get(owner).cloned();
        if context.is_none() && state.isolated.contains(owner) {
            // Fail closed: a closed or lost space never falls back to the shared profile.
            return Err(BrowserError::SpaceClosed(owner.to_owned()));
        }
        let mut params = json!({"url":url,"newWindow":true,"background":true});
        if let Some(context) = &context {
            params["browserContextId"] = json!(context);
        }
        let result = self
            .inner
            .cdp
            .call(None, "Target.createTarget", params)
            .await?;
        let id = required_string(&result, "targetId")?.to_owned();
        let tab = TabInfo {
            id: id.clone(),
            url: url.to_owned(),
            title: String::new(),
            opened_by: OpenedBy::Agent,
            owner_session_id: Some(owner.to_owned()),
            discarded: false,
            unloaded: false,
            browser_context_id: context.clone(),
        };
        if let Some(context) = context {
            state.tab_contexts.insert(id.clone(), context);
        }
        state.tabs.insert(id.clone(), tab.clone());
        let _ = self
            .inner
            .events
            .send(BrowserEvent::TabOpened { tab: tab.clone() });
        warn_tabs(&self.inner, state.tabs.len());
        drop(state);
        let ready = async {
            let session = self.session(&id).await?;
            self.wait_loaded(&session, None, url != "about:blank")
                .await?;
            Ok::<_, BrowserError>(())
        };
        tokio::pin!(ready);
        let ready = tokio::select! {
            biased;
            () = cancellation => {
                if self.close_agent_tab(owner, &id).await.is_err() {
                    // If closing was not acknowledged, retain admission until the issued load settles.
                    let _ = ready.await;
                }
                return Err(BrowserError::InvalidAction("browser operation cancelled"));
            }
            result = &mut ready => result,
        };
        if let Err(error) = ready {
            let _ = self.close_agent_tab(owner, &id).await;
            return Err(error);
        }
        Ok(tab)
    }

    /// Mark a discovered tab as user-owned; agent-created tabs cannot be reclassified.
    pub async fn adopt_user_tab(&self, id: &str) -> Result<(), BrowserError> {
        let created = self.live_contexts().await?;
        let mut state = self.inner.state.lock().await;
        if state.closing_tabs.contains(id) {
            return Err(BrowserError::TabNotReady(id.to_owned()));
        }
        // A space's storage belongs to its owner session; no person adopts a tab inside it,
        // bound or not.
        let in_space = state
            .tab_contexts
            .get(id)
            .is_some_and(|context| created.contains(context));
        let tab = state
            .tabs
            .get_mut(id)
            .ok_or_else(|| BrowserError::TabNotReady(id.to_owned()))?;
        if tab.opened_by == OpenedBy::Agent || in_space {
            return Err(BrowserError::OwnershipDenied(id.to_owned()));
        }
        tab.opened_by = OpenedBy::User;
        tab.owner_session_id = None;
        Ok(())
    }

    /// Restore a persisted agent owner after reconnect; user-owned or other-owner tabs are
    /// denied, and so is a tab outside the owner's isolated space (or inside another space).
    /// Restore spaces ([`Browser::restore_space`]) before their tabs.
    pub async fn adopt_tab(&self, target_id: &str, owner: &str) -> Result<(), BrowserError> {
        // Chromium's own record decides, so a space tab is never adopted as a default-context
        // tab just because its space was not restored (or was lost).
        let created = self.live_contexts().await?;
        let mut state = self.inner.state.lock().await;
        if state.closing_tabs.contains(target_id) {
            return Err(BrowserError::TabNotReady(target_id.to_owned()));
        }
        let space = state.spaces.get(owner).cloned();
        let context = state
            .tab_contexts
            .get(target_id)
            .filter(|context| created.contains(context))
            .cloned();
        state.isolated_contexts.extend(created);
        let tab = state
            .tabs
            .get_mut(target_id)
            .ok_or_else(|| BrowserError::TabNotReady(target_id.to_owned()))?;
        if tab.opened_by == OpenedBy::User
            || (tab.opened_by == OpenedBy::Agent && tab.owner_session_id.as_deref() != Some(owner))
            // A space tab carries its context owner from the start; only that owner adopts it.
            || (tab.opened_by == OpenedBy::External
                && tab.owner_session_id.is_some()
                && (space.is_none() || tab.owner_session_id.as_deref() != Some(owner)))
            || context != space
        {
            return Err(BrowserError::OwnershipDenied(target_id.to_owned()));
        }
        tab.opened_by = OpenedBy::Agent;
        tab.owner_session_id = Some(owner.to_owned());
        drop(state);
        self.attach(target_id).await
    }

    /// Record sleep state before attempting attachment; sleeping tabs are also detached.
    pub async fn mark_asleep(
        &self,
        id: &str,
        discarded: bool,
        unloaded: bool,
    ) -> Result<(), BrowserError> {
        let _attach = self.inner.attach_lock.lock().await;
        let mut state = self.inner.state.lock().await;
        let tab = state
            .tabs
            .get_mut(id)
            .ok_or_else(|| BrowserError::TabNotReady(id.to_owned()))?;
        tab.discarded = discarded;
        tab.unloaded = unloaded;
        let session = if discarded || unloaded {
            state.sessions.remove(id)
        } else {
            None
        };
        drop(state);
        if let Some(session) = session {
            self.inner
                .cdp
                .call(
                    None,
                    "Target.detachFromTarget",
                    json!({"sessionId":session}),
                )
                .await?;
        }
        Ok(())
    }

    /// Attach in flattened mode only after the ledger and live page metadata pass readiness checks.
    pub async fn attach(&self, id: &str) -> Result<(), BrowserError> {
        tokio::time::timeout(self.inner.attach_timeout, async {
            let _attach = self.inner.attach_lock.lock().await;
            let state = self.inner.state.lock().await;
            if state.closing_tabs.contains(id) {
                return Err(BrowserError::TabNotReady(id.to_owned()));
            }
            let tab = state
                .tabs
                .get(id)
                .ok_or_else(|| BrowserError::TabNotReady(id.to_owned()))?;
            if tab.discarded || tab.unloaded {
                return Err(BrowserError::TabAsleep(id.to_owned()));
            }
            if state.sessions.contains_key(id) {
                return Ok(());
            }
            drop(state);
            let info = self
                .inner
                .cdp
                .call(None, "Target.getTargetInfo", json!({"targetId":id}))
                .await?;
            if info["targetInfo"]["type"] != "page"
                || info["targetInfo"]["subtype"]
                    .as_str()
                    .is_some_and(|subtype| !subtype.is_empty())
            {
                return Err(BrowserError::TabNotReady(id.to_owned()));
            }
            let result = self
                .inner
                .cdp
                .call(
                    None,
                    "Target.attachToTarget",
                    json!({"targetId":id,"flatten":true}),
                )
                .await?;
            let session = required_string(&result, "sessionId")?.to_owned();
            let mut cleanup = AttachCleanup {
                inner: Arc::clone(&self.inner),
                session: session.clone(),
                armed: true,
            };
            // The patched browser emulates its agent tabs itself and undoes any other
            // client's device-metrics override, so a second owner here would only fight it.
            let browser_owned = match &self.gateway {
                Some(gateway) => gateway
                    .tab_for_target(id)
                    .await?
                    .is_some_and(|tab| tab.owner == GatewayOwner::Agent),
                None => false,
            };
            if !browser_owned {
                self.inner
                    .cdp
                    .call(
                        Some(&session),
                        "Emulation.setDeviceMetricsOverride",
                        json!({"width":1440,"height":900,"deviceScaleFactor":1,"mobile":false}),
                    )
                    .await?;
            }
            self.inner
                .cdp
                .call(Some(&session), "Page.enable", json!({}))
                .await?;
            // A crashed controller may have left emulation enabled on a surviving renderer.
            self.inner
                .cdp
                .call(
                    Some(&session),
                    "Emulation.setFocusEmulationEnabled",
                    json!({"enabled":false}),
                )
                .await?;
            // Runtime.enable is observable from page script; the patched browser does not need it.
            if self.gateway.is_none() {
                self.inner
                    .cdp
                    .call(Some(&session), "Runtime.enable", json!({}))
                    .await?;
            }
            self.inner
                .cdp
                .call(Some(&session), "Accessibility.enable", json!({}))
                .await?;
            let frame_tree = self
                .inner
                .cdp
                .call(Some(&session), "Page.getFrameTree", json!({}))
                .await?;
            let mut state = self.inner.state.lock().await;
            state.sessions.insert(id.to_owned(), session);
            if let Some(frame) = frame_tree["frameTree"]["frame"]["id"].as_str() {
                state.frames.insert(frame.to_owned(), id.to_owned());
            }
            cleanup.armed = false;
            Ok(())
        })
        .await
        .map_err(|_| BrowserError::Timeout("tab attach"))?
    }

    async fn session(&self, id: &str) -> Result<String, BrowserError> {
        self.attach(id).await?;
        self.inner
            .state
            .lock()
            .await
            .sessions
            .get(id)
            .cloned()
            .ok_or(BrowserError::Disconnected)
    }

    /// Check session ownership and active space membership without attaching the target.
    /// Space-owned popups retain their External provenance but are usable by that owner.
    pub async fn check_owned(&self, owner: &str, id: &str) -> Result<(), BrowserError> {
        if !self.is_connected() {
            return Err(BrowserError::Disconnected);
        }
        check_owned(&*self.inner.state.lock().await, owner, id)
    }

    async fn owned_session(&self, owner: &str, id: &str) -> Result<String, BrowserError> {
        self.check_owned(owner, id).await?;
        let session = self.session(id).await?;
        // Attachment awaits CDP; disposal may have unbound the context in the meantime.
        self.check_owned(owner, id).await?;
        Ok(session)
    }

    async fn top_frame(&self, session: &str) -> Result<Value, BrowserError> {
        let mut result = self
            .inner
            .cdp
            .call(Some(session), "Page.getFrameTree", json!({}))
            .await?;
        let frame = result["frameTree"]["frame"].take();
        required_string(&frame, "url")?;
        Ok(frame)
    }

    async fn document_identity(&self, session: &str) -> Result<(String, String), BrowserError> {
        let frame = self.top_frame(session).await?;
        let document = self
            .inner
            .cdp
            .call(Some(session), "DOM.getDocument", json!({"depth":0}))
            .await?;
        let root = document["root"]["backendNodeId"]
            .as_u64()
            .ok_or(BrowserError::InvalidResponse("document backend node"))?;
        let id = serde_json::to_string(&(
            required_string(&frame, "id")?,
            required_string(&frame, "loaderId")?,
            required_string(&frame, "url")?,
            root,
        ))
        .map_err(|_| BrowserError::InvalidResponse("document identity"))?;
        Ok((
            id,
            frame["securityOrigin"]
                .as_str()
                .unwrap_or("null")
                .to_owned(),
        ))
    }

    /// Return an opaque live document identity for document-scoped persisted policy.
    pub async fn document_id(&self, tab_id: &str) -> Result<String, BrowserError> {
        let session = self.session(tab_id).await?;
        let (id, _) = self.document_identity(&session).await?;
        Ok(id)
    }

    /// An opaque identity that lasts as long as the tab's current document: unlike
    /// [`Self::document_id`] it leaves out the URL, so a same-document history or hash change
    /// (which keeps every input value) keeps it, while any new document changes it.
    pub async fn document_lifetime_id(&self, tab_id: &str) -> Result<String, BrowserError> {
        let session = self.session(tab_id).await?;
        let frame = self.top_frame(&session).await?;
        let document = self
            .inner
            .cdp
            .call(Some(&session), "DOM.getDocument", json!({"depth":0}))
            .await?;
        let root = document["root"]["backendNodeId"]
            .as_u64()
            .ok_or(BrowserError::InvalidResponse("document backend node"))?;
        serde_json::to_string(&(
            required_string(&frame, "id")?,
            required_string(&frame, "loaderId")?,
            root,
        ))
        .map_err(|_| BrowserError::InvalidResponse("document identity"))
    }

    /// Capture a non-secret node identity while its latest snapshot reference is still valid.
    pub async fn resolve_ref(
        &self,
        owner: &str,
        tab_id: &str,
        reference: &str,
    ) -> Result<NodeHandle, BrowserError> {
        let _interaction = self.inner.interaction_lock.lock().await;
        let session = self.owned_session(owner, tab_id).await?;
        let before = self.document_identity(&session).await?;
        let backend_node_id = self.target_node(&session, tab_id, reference).await?;
        let after = self.document_identity(&session).await?;
        if before != after {
            return Err(BrowserError::StaleReference);
        }
        let (document_id, origin) = after;
        if !origin.contains("://") {
            return Err(BrowserError::InvalidResponse("top-frame origin is opaque"));
        }
        Ok(NodeHandle {
            backend_node_id,
            document_id,
            origin,
        })
    }

    async fn check_node_document(
        &self,
        session: &str,
        node: &NodeHandle,
    ) -> Result<(), BrowserError> {
        let (document, origin) = self.document_identity(session).await?;
        if document != node.document_id {
            return Err(BrowserError::StaleReference);
        }
        if origin != node.origin {
            return Err(BrowserError::OriginMismatch);
        }
        Ok(())
    }

    async fn wait_loaded(
        &self,
        session: &str,
        loader: Option<&str>,
        require_document: bool,
    ) -> Result<Value, BrowserError> {
        self.track_session(session).await?;
        tokio::time::timeout(self.inner.attach_timeout, async {
            let mut poll = tokio::time::interval(Duration::from_millis(20));
            loop {
                poll.tick().await;
                let frame = self.top_frame(session).await?;
                if loader.is_some_and(|loader| frame["loaderId"] != loader)
                    || (require_document && frame["url"] == "about:blank")
                {
                    continue;
                }
                if self
                    .inner
                    .network
                    .loaded(session, frame["loaderId"].as_str())?
                {
                    let after = self.top_frame(session).await?;
                    if after["url"] == frame["url"] && after["loaderId"] == frame["loaderId"] {
                        return Ok(after);
                    }
                }
            }
        })
        .await
        .map_err(|_| BrowserError::Timeout("page load"))?
    }

    async fn target_node(
        &self,
        session: &str,
        tab_id: &str,
        target: &str,
    ) -> Result<u64, BrowserError> {
        let document = self
            .inner
            .cdp
            .call(Some(session), "DOM.getDocument", json!({"depth":0}))
            .await?;
        let root = document["root"]["backendNodeId"]
            .as_u64()
            .ok_or(BrowserError::InvalidResponse("document backend node"))?;
        let backend = self
            .inner
            .state
            .lock()
            .await
            .references
            .get(tab_id)
            .and_then(|refs| refs.resolve(target, root))
            .ok_or(BrowserError::StaleReference)?;
        if backend.frame.is_some() {
            return Err(BrowserError::InvalidAction(
                "the reference is inside an iframe; act on it with browser_repl",
            ));
        }
        let backend = backend.backend;
        let connected = self
            .call_on_node(
                session,
                backend,
                "function() { return this.isConnected; }",
                json!([]),
            )
            .await?;
        if connected != true {
            return Err(BrowserError::StaleReference);
        }
        Ok(backend)
    }

    async fn call_on_node(
        &self,
        session: &str,
        backend: u64,
        function: &str,
        arguments: Value,
    ) -> Result<Value, BrowserError> {
        self.call_on_node_in(session, backend, None, function, arguments)
            .await
    }

    async fn call_on_node_in(
        &self,
        session: &str,
        backend: u64,
        context: Option<i64>,
        function: &str,
        arguments: Value,
    ) -> Result<Value, BrowserError> {
        let mut params = json!({"backendNodeId":backend});
        if let Some(context) = context {
            params["executionContextId"] = json!(context);
        }
        let resolved = self
            .inner
            .cdp
            .call(Some(session), "DOM.resolveNode", params)
            .await
            .map_err(|error| match error {
                BrowserError::Cdp { .. } => BrowserError::StaleReference,
                other => other,
            })?;
        let object = required_string(&resolved["object"], "objectId")
            .map_err(|_| BrowserError::StaleReference)?;
        let called = self
            .inner
            .cdp
            .call(
                Some(session),
                "Runtime.callFunctionOn",
                json!({"objectId":object,"functionDeclaration":function,"arguments":arguments,
                    "returnByValue":true,"userGesture":true}),
            )
            .await;
        let released = self
            .inner
            .cdp
            .call(
                Some(session),
                "Runtime.releaseObject",
                json!({"objectId":object}),
            )
            .await;
        let mut result = called?;
        released?;
        if result.get("exceptionDetails").is_some() {
            return Err(BrowserError::InvalidAction("target rejected the operation"));
        }
        Ok(result["result"]["value"].take())
    }

    async fn focus_node(&self, session: &str, backend: u64) -> Result<(), BrowserError> {
        self.inner
            .cdp
            .call(Some(session), "DOM.focus", json!({"backendNodeId":backend}))
            .await?;
        Ok(())
    }

    async fn press_key(&self, session: &str, key: &str) -> Result<(), BrowserError> {
        let mut event = key_event(key)?;
        self.inner
            .cdp
            .call(Some(session), "Input.dispatchKeyEvent", event.clone())
            .await?;
        event["type"] = json!("keyUp");
        if let Some(object) = event.as_object_mut() {
            object.remove("text");
            object.remove("unmodifiedText");
        }
        self.inner
            .cdp
            .call(Some(session), "Input.dispatchKeyEvent", event)
            .await?;
        Ok(())
    }

    /// Act only on a reference in the latest snapshot of an explicitly owned agent tab.
    pub async fn act(
        &self,
        owner: &str,
        tab_id: &str,
        target: &str,
        action: Action,
    ) -> Result<ActOutcome, BrowserError> {
        let _interaction = self.inner.interaction_lock.lock().await;
        let session = self.owned_session(owner, tab_id).await?;
        let backend = self.target_node(&session, tab_id, target).await?;
        let before = self.top_frame(&session).await?;
        let cleanup = Arc::clone(&self.inner).begin_focus(session.clone()).await?;
        let acted = async {
            match action {
                Action::Click | Action::Hover => {
                    self.pointer_at(&session, backend, matches!(action, Action::Click))
                        .await?;
                }
                Action::Type { text, submit } => {
                    self.focus_node(&session, backend).await?;
                    self.inner
                        .cdp
                        .call(Some(&session), "Input.insertText", json!({"text":text}))
                        .await?;
                    if submit {
                        self.press_key(&session, "Enter").await?;
                    }
                }
                Action::Press { key } => {
                    self.focus_node(&session, backend).await?;
                    self.press_key(&session, &key).await?;
                }
                Action::Select { values } => {
                    let selected = self
                        .call_on_node(
                            &session,
                            backend,
                            "function(values) {
                                if (!this.isConnected || this.tagName !== 'SELECT') return false;
                                if (!this.multiple && values.length > 1) return false;
                                const options = Array.from(this.options);
                                if (values.some(v => !options.some(o => o.value === v))) return false;
                                for (const option of options) option.selected = values.includes(option.value);
                                if (values.length === 0) this.selectedIndex = -1;
                                this.dispatchEvent(new Event('input', {bubbles:true}));
                                this.dispatchEvent(new Event('change', {bubbles:true}));
                                return true;
                            }",
                            json!([{"value":values}]),
                        )
                        .await?;
                    if selected != true {
                        return Err(BrowserError::InvalidAction(
                            "select requires existing option values",
                        ));
                    }
                }
                Action::Focus => self.focus_node(&session, backend).await?,
            }
            Ok::<_, BrowserError>(())
        }
        .await;
        let reset = cleanup.reset().await;
        acted?;
        reset?;
        // A CDP response can precede the navigation commit triggered by an input/focus handler.
        let grace = tokio::time::Instant::now() + Duration::from_millis(200);
        let after = loop {
            let frame = self.top_frame(&session).await?;
            if frame["loaderId"] != before["loaderId"] || frame["url"] != before["url"] {
                break self.wait_loaded(&session, None, false).await?;
            }
            if tokio::time::Instant::now() >= grace {
                break frame;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        let navigated = after["loaderId"] != before["loaderId"] || after["url"] != before["url"];
        if navigated {
            if let Some(refs) = self.inner.state.lock().await.references.get_mut(tab_id) {
                refs.invalidate();
            }
        }
        Ok(ActOutcome {
            url: required_string(&after, "url")?.to_owned(),
            navigated,
        })
    }

    /// Insert without submitting; errors never retain text or page-generated diagnostics.
    pub async fn insert_text(
        &self,
        owner: &str,
        tab_id: &str,
        target: &str,
        text: &str,
    ) -> Result<(), BrowserError> {
        let node = self
            .resolve_ref(owner, tab_id, target)
            .await
            .map_err(text_insertion_error)?;
        self.insert_text_node(owner, tab_id, &node, text).await
    }

    /// Replace the value of a persisted node only in its original document and exact origin,
    /// without submitting. `InvalidAction`, `StaleReference`, and `OriginMismatch` guarantee that
    /// nothing was written; any other error leaves the outcome unknown.
    pub async fn insert_text_node(
        &self,
        owner: &str,
        tab_id: &str,
        node: &NodeHandle,
        text: &str,
    ) -> Result<(), BrowserError> {
        self.insert_nodes(owner, tab_id, std::slice::from_ref(node), text)
            .await
    }

    /// Pin the split one-time-code boxes that start at `first` (see [`otp_group_script`]):
    /// the boxes after `first`, empty when it is a single field. Call it when the code is asked
    /// for, not when it arrives, so a page cannot add boxes later; [`Self::insert_otp_nodes`]
    /// then writes only these, after rechecking each one.
    pub async fn resolve_otp_group(
        &self,
        owner: &str,
        tab_id: &str,
        first: &NodeHandle,
    ) -> Result<Vec<NodeHandle>, BrowserError> {
        let _interaction = self.inner.interaction_lock.lock().await;
        let session = self.owned_session(owner, tab_id).await?;
        self.check_node_document(&session, first).await?;
        let context = self.isolated_world(&session).await?;
        let resolved = self
            .inner
            .cdp
            .call(
                Some(&session),
                "DOM.resolveNode",
                json!({"backendNodeId":first.backend_node_id,"executionContextId":context}),
            )
            .await
            .map_err(|_| BrowserError::StaleReference)?;
        let object = required_string(&resolved["object"], "objectId")
            .map_err(|_| BrowserError::StaleReference)?
            .to_owned();
        let mut objects = vec![object.clone()];
        let group = async {
            let array = self
                .inner
                .cdp
                .call(
                    Some(&session),
                    "Runtime.callFunctionOn",
                    json!({"objectId":object,"functionDeclaration":otp_group_script(),
                        "returnByValue":false}),
                )
                .await?;
            if array.get("exceptionDetails").is_some() {
                return Err(BrowserError::InvalidAction("target rejected the operation"));
            }
            let array = required_string(&array["result"], "objectId")?.to_owned();
            objects.push(array.clone());
            let properties = self
                .inner
                .cdp
                .call(
                    Some(&session),
                    "Runtime.getProperties",
                    json!({"objectId":array,"ownProperties":true}),
                )
                .await?;
            let mut boxes = Vec::new();
            for property in properties["result"].as_array().into_iter().flatten() {
                let Some(index) = property["name"]
                    .as_str()
                    .and_then(|name| name.parse::<usize>().ok())
                else {
                    continue;
                };
                let element = required_string(&property["value"], "objectId")?.to_owned();
                objects.push(element.clone());
                let described = self
                    .inner
                    .cdp
                    .call(
                        Some(&session),
                        "DOM.describeNode",
                        json!({"objectId":element}),
                    )
                    .await?;
                let backend = described["node"]["backendNodeId"]
                    .as_u64()
                    .ok_or(BrowserError::InvalidResponse("box backend node"))?;
                boxes.push((index, backend));
            }
            boxes.sort_unstable();
            Ok(boxes)
        }
        .await;
        for object in objects {
            let _ = self
                .inner
                .cdp
                .call(
                    Some(&session),
                    "Runtime.releaseObject",
                    json!({"objectId":object}),
                )
                .await;
        }
        let boxes = group?;
        if boxes.first().map(|(_, backend)| *backend) != Some(first.backend_node_id) {
            return Err(BrowserError::StaleReference);
        }
        self.check_node_document(&session, first).await?;
        Ok(boxes
            .into_iter()
            .skip(1)
            .map(|(_, backend_node_id)| NodeHandle {
                backend_node_id,
                document_id: first.document_id.clone(),
                origin: first.origin.clone(),
            })
            .collect())
    }

    /// Write a one-time code into `first` and the boxes [`Self::resolve_otp_group`] pinned
    /// after it, one character each; with no boxes this is [`Self::insert_text_node`]. Same
    /// error guarantees.
    pub async fn insert_otp_nodes(
        &self,
        owner: &str,
        tab_id: &str,
        first: &NodeHandle,
        boxes: &[NodeHandle],
        text: &str,
    ) -> Result<(), BrowserError> {
        let mut nodes = Vec::with_capacity(boxes.len() + 1);
        nodes.push(first.clone());
        nodes.extend_from_slice(boxes);
        self.insert_nodes(owner, tab_id, &nodes, text).await
    }

    /// Evaluate `expression` against the owned tab's DOM in an isolated world, out of reach of
    /// page script, and return its JSON value. Exception details are never returned, since
    /// they could quote page content the caller treats as secret.
    pub async fn evaluate_isolated(
        &self,
        owner: &str,
        tab_id: &str,
        expression: &str,
    ) -> Result<Value, BrowserError> {
        let _interaction = self.inner.interaction_lock.lock().await;
        let session = self.owned_session(owner, tab_id).await?;
        let context = self.isolated_world(&session).await?;
        let result = self
            .inner
            .cdp
            .call(
                Some(&session),
                "Runtime.evaluate",
                json!({"expression":expression,"contextId":context,"returnByValue":true}),
            )
            .await?;
        if result.get("exceptionDetails").is_some() {
            return Err(BrowserError::InvalidAction("isolated evaluation failed"));
        }
        Ok(result["result"]["value"].clone())
    }

    async fn isolated_world(&self, session: &str) -> Result<i64, BrowserError> {
        let frame = self.top_frame(session).await?;
        // Page script can redefine prototype accessors and builtins in its own world; an
        // isolated world has pristine intrinsics while sharing the same DOM.
        let world = self
            .inner
            .cdp
            .call(
                Some(session),
                "Page.createIsolatedWorld",
                json!({"frameId":required_string(&frame, "id")?,
                    "worldName":"openside-fill","grantUniveralAccess":false}),
            )
            .await?;
        world["executionContextId"]
            .as_i64()
            .ok_or(BrowserError::InvalidResponse("isolated world context"))
    }

    async fn insert_nodes(
        &self,
        owner: &str,
        tab_id: &str,
        nodes: &[NodeHandle],
        text: &str,
    ) -> Result<(), BrowserError> {
        let node = nodes.first().ok_or(BrowserError::StaleReference)?;
        if nodes
            .iter()
            .any(|other| other.document_id != node.document_id || other.origin != node.origin)
        {
            return Err(BrowserError::StaleReference);
        }
        let _interaction = self.inner.interaction_lock.lock().await;
        let inserted = async {
            let session = self.owned_session(owner, tab_id).await?;
            self.check_node_document(&session, node).await?;
            let connected = self
                .call_on_node(
                    &session,
                    node.backend_node_id,
                    "function() { return this.isConnected; }",
                    json!([]),
                )
                .await?;
            if connected != true {
                return Err(BrowserError::StaleReference);
            }
            let cleanup = Arc::clone(&self.inner).begin_focus(session.clone()).await?;
            let mut boxes = Vec::new();
            let result = async {
                // Focus only serves keyboard UX and focus-dependent page validation; the value is
                // written to the bound node below, so a focus change cannot redirect it.
                self.focus_node(&session, node.backend_node_id).await?;
                self.check_node_document(&session, node).await?;
                let context = self.isolated_world(&session).await?;
                let function = if nodes.len() == 1 {
                    FILL_NODE.to_owned()
                } else {
                    fill_otp_group_script()
                };
                let mut arguments = vec![json!({"value":text}), json!({"value":node.origin})];
                if nodes.len() > 1 {
                    for other in nodes {
                        let resolved = self
                            .inner
                            .cdp
                            .call(
                                Some(&session),
                                "DOM.resolveNode",
                                json!({"backendNodeId":other.backend_node_id,
                                    "executionContextId":context}),
                            )
                            .await
                            .map_err(|_| BrowserError::StaleReference)?;
                        let object = required_string(&resolved["object"], "objectId")
                            .map_err(|_| BrowserError::StaleReference)?
                            .to_owned();
                        arguments.push(json!({"objectId":object}));
                        boxes.push(object);
                    }
                }
                let status = self
                    .call_on_node_in(
                        &session,
                        node.backend_node_id,
                        Some(context),
                        &function,
                        Value::Array(arguments),
                    )
                    .await
                    .map_err(|error| match error {
                        // An exception escaping the fill function may follow a write.
                        BrowserError::InvalidAction(_) => BrowserError::TextInsertionFailed,
                        other => other,
                    })?;
                match status.as_str() {
                    Some("filled") => Ok(()),
                    Some("stale") => Err(BrowserError::StaleReference),
                    Some("origin") => Err(BrowserError::OriginMismatch),
                    Some("not_editable") => Err(BrowserError::InvalidAction(
                        "target is not an editable text field",
                    )),
                    Some("split_mismatch") => Err(BrowserError::InvalidAction(
                        "code length differs from the number of code boxes",
                    )),
                    _ => Err(BrowserError::TextInsertionFailed),
                }
            }
            .await;
            for object in boxes {
                let _ = self
                    .inner
                    .cdp
                    .call(
                        Some(&session),
                        "Runtime.releaseObject",
                        json!({"objectId":object}),
                    )
                    .await;
            }
            let reset = cleanup.reset().await;
            result?;
            reset?;
            Ok::<_, BrowserError>(())
        }
        .await;
        inserted.map_err(text_insertion_error)
    }

    /// Return the live top-frame security origin, including its non-default port.
    pub async fn origin(&self, tab_id: &str) -> Result<String, BrowserError> {
        let session = self.session(tab_id).await?;
        let frame = self.top_frame(&session).await?;
        frame["securityOrigin"]
            .as_str()
            .filter(|origin| origin.contains("://"))
            .map(str::to_owned)
            .ok_or(BrowserError::InvalidResponse("top-frame origin is opaque"))
    }

    /// Return the live top-frame document URL (after redirects and script navigation).
    pub async fn url(&self, tab_id: &str) -> Result<String, BrowserError> {
        let session = self.session(tab_id).await?;
        let frame = self.top_frame(&session).await?;
        Ok(required_string(&frame, "url")?.to_owned())
    }

    /// Navigate an explicitly owned tab and wait for its new top-frame document to load.
    pub async fn navigate(&self, owner: &str, tab_id: &str, url: &str) -> Result<(), BrowserError> {
        self.navigate_cancellable(owner, tab_id, url, std::future::pending())
            .await
    }

    /// A cancelled navigation acknowledges stopLoading before the caller can release ownership.
    pub async fn navigate_cancellable(
        &self,
        owner: &str,
        tab_id: &str,
        url: &str,
        cancellation: impl Future<Output = ()>,
    ) -> Result<(), BrowserError> {
        // The lock only fences in-flight interactions and the reference invalidation. It must
        // not be held across the navigation: a server that never answers would stall every
        // other session's interactions for the whole navigation timeout.
        let session = {
            let _interaction = self.inner.interaction_lock.lock().await;
            let session = self.owned_session(owner, tab_id).await?;
            if let Some(refs) = self.inner.state.lock().await.references.get_mut(tab_id) {
                refs.invalidate();
            }
            session
        };
        let operation = async {
            self.track_session(&session).await?;
            let navigated = self
                .inner
                .cdp
                .call(Some(&session), "Page.navigate", json!({"url":url}))
                .await;
            // The reply comes only when the navigation commits; a server that never answers
            // would otherwise leave the load pending and the tab busy long after this error.
            if matches!(navigated, Err(BrowserError::Timeout(_))) {
                let _ = self
                    .inner
                    .cdp
                    .call(Some(&session), "Page.stopLoading", json!({}))
                    .await;
            }
            let result = navigated?;
            if result.get("errorText").is_some() || result["isDownload"] == true {
                return Err(BrowserError::NavigationFailed);
            }
            self.wait_loaded(&session, result["loaderId"].as_str(), false)
                .await?;
            Ok(())
        };
        tokio::pin!(operation);
        tokio::select! {
            result = &mut operation => result,
            () = cancellation => {
                let stopped = tokio::time::timeout(
                    Duration::from_secs(3),
                    self.inner.cdp.call(Some(&session), "Page.stopLoading", json!({})),
                ).await;
                if !matches!(stopped, Ok(Ok(_))) {
                    // An unacknowledged stop cannot fence the remote command; drain it instead.
                    let _ = operation.await;
                }
                Err(BrowserError::InvalidAction("browser operation cancelled"))
            }
        }
    }

    /// Close a session-owned tab, including a popup in that session's active isolated space.
    pub async fn close_agent_tab(&self, owner: &str, id: &str) -> Result<(), BrowserError> {
        let mut state = self.inner.state.lock().await;
        check_owned(&state, owner, id)?;
        let result = self
            .inner
            .cdp
            .call(None, "Target.closeTarget", json!({"targetId":id}))
            .await?;
        if result["success"] != true {
            return Err(BrowserError::TabNotReady(id.to_owned()));
        }
        state.upload_closing.insert(id.to_owned());
        *state.upload_epochs.entry(id.to_owned()).or_default() += 1;
        state.uploads.remove(id);
        drop(state);
        let (root, tab) = (self.profile_path().to_owned(), id.to_owned());
        tokio::task::spawn_blocking(move || uploads::remove_tab(&root, &tab))
            .await
            .map_err(std::io::Error::other)??;
        Ok(())
    }

    /// Evaluate JavaScript in an attached page and return its JSON value.
    pub async fn evaluate(&self, id: &str, expression: &str) -> Result<Value, BrowserError> {
        let session = self.session(id).await?;
        let result = self.inner.cdp.call(Some(&session), "Runtime.evaluate", json!({"expression":expression,"awaitPromise":true,"returnByValue":true,"userGesture":true})).await?;
        if let Some(exception) = result.get("exceptionDetails") {
            return Err(BrowserError::JavaScript(exception.to_string()));
        }
        Ok(result["result"]["value"].clone())
    }

    /// Install the owner-level navigation policy: every top-frame document request of a tab
    /// with an owner session, including its first request, every redirect hop, link and form
    /// navigations, and popups (which inherit the opener's owner) is paused before it reaches
    /// the network and proceeds only when `check(owner, url)` is true. Refused requests fail
    /// with `net::ERR_ABORTED`. Without a policy, owned tabs navigate freely. Navigations that
    /// never reach the network (back/forward cache restores, `about:`/`data:` URLs) are not
    /// seen.
    pub fn set_navigation_policy(&self, check: OwnerNavigationCheck) {
        *self
            .inner
            .navigation_policy
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(check);
    }

    /// Add a per-tab rule on top of the owner policy ([`Browser::set_navigation_policy`]) for
    /// an owned agent tab: each top-frame document request, redirect hop included, proceeds
    /// only when both allow it. `allow(url)` is called for every hop, even when the owner
    /// policy refuses. Dropping the guard removes the rule.
    pub async fn guard_navigation(
        &self,
        owner: &str,
        tab_id: &str,
        allow: NavigationCheck,
    ) -> Result<NavigationGuard, BrowserError> {
        self.owned_session(owner, tab_id).await?;
        self.install_rule(tab_id, allow, false).await
    }

    /// Like [`Browser::guard_navigation`], but the rule also decides the document requests of
    /// every subframe of the tab, so no document of another origin can load next to the one
    /// the caller authorized. A frame the ledger cannot yet place is judged by every active
    /// rule of this kind, which can refuse a frame of an unrelated tab for as long as one runs.
    /// Frames that were already committed are not affected; the caller inspects the frame tree.
    pub async fn guard_documents(
        &self,
        owner: &str,
        tab_id: &str,
        allow: NavigationCheck,
    ) -> Result<NavigationGuard, BrowserError> {
        self.owned_session(owner, tab_id).await?;
        self.install_rule(tab_id, allow, true).await
    }

    async fn install_rule(
        &self,
        tab_id: &str,
        allow: NavigationCheck,
        frames: bool,
    ) -> Result<NavigationGuard, BrowserError> {
        let active = Arc::new(std::sync::atomic::AtomicBool::new(true));
        self.inner.state.lock().await.navigation_rules.insert(
            tab_id.to_owned(),
            NavigationRule {
                active: Arc::clone(&active),
                allow,
                frames,
            },
        );
        Ok(NavigationGuard { active })
    }

    /// Issue one page command from [`PAGE_COMMANDS`] to an explicitly owned agent tab.
    pub async fn page_command(
        &self,
        owner: &str,
        tab_id: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, BrowserError> {
        if !PAGE_COMMANDS.contains(&method) {
            return Err(BrowserError::InvalidAction("page command is not allowed"));
        }
        let session = self.owned_session(owner, tab_id).await?;
        self.inner.cdp.call(Some(&session), method, params).await
    }

    /// Issue tab-confined CDP on an explicitly owned page session. The caller must authorize
    /// every addressed frame, execution context and input receiver; host-global domains and
    /// file-transfer commands are unavailable through this untrusted entry point.
    pub async fn page_cdp(
        &self,
        owner: &str,
        tab_id: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, BrowserError> {
        cdp_scope::validate(method)?;
        let session = self.owned_session(owner, tab_id).await?;
        self.inner.cdp.call(Some(&session), method, params).await
    }

    /// Issue one `WebAuthn.*` command (virtual authenticators) on an explicitly owned agent
    /// tab. Deliberately separate from [`PAGE_COMMANDS`]: `WebAuthn.getCredentials` returns
    /// private keys, so only trusted host code (the vault) may call this, never the REPL.
    pub async fn webauthn_command(
        &self,
        owner: &str,
        tab_id: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, BrowserError> {
        if !method.starts_with("WebAuthn.") {
            return Err(BrowserError::InvalidAction("not a WebAuthn command"));
        }
        let session = self.owned_session(owner, tab_id).await?;
        self.inner.cdp.call(Some(&session), method, params).await
    }

    /// Node of a reference in the latest snapshot of an owned tab, qualified by its frame and
    /// hosting session. Callers must still confirm the node is connected before acting on it.
    pub async fn reference_node(
        &self,
        owner: &str,
        tab_id: &str,
        reference: &str,
    ) -> Result<RefTarget, BrowserError> {
        let session = self.owned_session(owner, tab_id).await?;
        let document = self
            .inner
            .cdp
            .call(Some(&session), "DOM.getDocument", json!({"depth":0}))
            .await?;
        let root = document["root"]["backendNodeId"]
            .as_u64()
            .ok_or(BrowserError::InvalidResponse("document backend node"))?;
        self.inner
            .state
            .lock()
            .await
            .references
            .get(tab_id)
            .and_then(|refs| refs.resolve(reference, root))
            .ok_or(BrowserError::StaleReference)
    }

    /// Start (`true`) or stop recording every CDP command this controller issues, for
    /// no-injection audits. Recorded parameters can include typed text and stay in memory.
    pub fn record_protocol(&self, enabled: bool) {
        self.inner.cdp.record(enabled);
    }

    /// Drain recorded `(method, params)` pairs in issue order; empty when not recording.
    pub fn take_protocol_log(&self) -> Vec<(String, Value)> {
        self.inner.cdp.recorded()
    }

    /// Collect the full accessibility tree and stable-reference compact representation.
    /// Child frames are included when they share the top frame's origin: content of other
    /// origins needs a policy decision of its own (see [`Browser::snapshot_frames`]).
    pub async fn snapshot(&self, id: &str) -> Result<Snapshot, BrowserError> {
        self.capture(id, false, &same_origin).await
    }

    /// Like [`Browser::snapshot`], but only nodes whose layout box overlaps the visual
    /// viewport at capture time, plus their ancestors. Refs are the tab's usual stable refs.
    pub async fn snapshot_viewport(&self, id: &str) -> Result<Snapshot, BrowserError> {
        self.capture(id, true, &same_origin).await
    }

    /// [`Browser::snapshot`] (or the viewport form) of an owned tab with the child frames
    /// `keep(top, frame)` accepts; a rejected frame's subframes are left out with it.
    pub async fn snapshot_frames(
        &self,
        owner: &str,
        id: &str,
        viewport: bool,
        keep: &(dyn Fn(&FrameInfo, &FrameInfo) -> bool + Sync),
    ) -> Result<Snapshot, BrowserError> {
        self.owned_session(owner, id).await?;
        self.capture(id, viewport, keep).await
    }

    async fn capture(
        &self,
        id: &str,
        viewport: bool,
        keep: &(dyn Fn(&FrameInfo, &FrameInfo) -> bool + Sync),
    ) -> Result<Snapshot, BrowserError> {
        let _interaction = self.inner.interaction_lock.lock().await;
        let session = self.session(id).await?;
        let mut raw = self
            .inner
            .cdp
            .call(Some(&session), "Accessibility.getFullAXTree", json!({}))
            .await?;
        let view = if viewport {
            let metrics = self
                .inner
                .cdp
                .call(Some(&session), "Page.getLayoutMetrics", json!({}))
                .await?;
            let visual = &metrics["cssVisualViewport"];
            let [left, top, width, height] = ["pageX", "pageY", "clientWidth", "clientHeight"]
                .map(|field| visual[field].as_f64());
            let (Some(left), Some(top), Some(width), Some(height)) = (left, top, width, height)
            else {
                return Err(BrowserError::InvalidResponse("layout metrics"));
            };
            let dom = self
                .inner
                .cdp
                .call(
                    Some(&session),
                    "DOMSnapshot.captureSnapshot",
                    json!({"computedStyles": []}),
                )
                .await?;
            Some(nodes_in_view(&dom, left, top, width, height)?)
        } else {
            None
        };
        let frames = self.frame_trees(&session, id, view.as_ref(), keep).await?;
        merge_frames(&mut raw, frames);
        let mut state = self.inner.state.lock().await;
        process(
            raw,
            state.references.entry(id.to_owned()).or_default(),
            view.as_ref(),
        )
    }

    /// Authenticated patched-browser gateway; None when running stock Chromium over CDP only.
    pub fn gateway(&self) -> Option<&Gateway> {
        self.gateway.as_ref()
    }

    /// Capture a hidden-page PNG with focus emulation reset on both success and failure.
    pub async fn screenshot(&self, id: &str) -> Result<Vec<u8>, BrowserError> {
        // The patched browser captures owned hidden tabs natively, without focus emulation.
        if let Some(gateway) = &self.gateway {
            if let Some(tab) = gateway.tab_for_target(id).await? {
                if tab.owner == GatewayOwner::Agent {
                    return Ok(gateway.screenshot(&tab.tab_id).await?.png);
                }
            }
        }
        let session = self.session(id).await?;
        // The task owns the finally path even if the caller cancels its screenshot future.
        let params = json!({"format":"png","captureBeyondViewport":false});
        tokio::spawn(Arc::clone(&self.inner).capture(session, params))
            .await
            .map_err(|error| BrowserError::Transport(error.to_string()))?
    }

    /// Explicitly answer a previously observed JavaScript dialog.
    pub async fn handle_dialog(
        &self,
        id: &str,
        accept: bool,
        prompt_text: Option<&str>,
    ) -> Result<(), BrowserError> {
        let session = self.session(id).await?;
        self.inner
            .cdp
            .call(
                Some(&session),
                "Page.handleJavaScriptDialog",
                json!({"accept":accept,"promptText":prompt_text.unwrap_or_default()}),
            )
            .await?;
        Ok(())
    }

    #[cfg(not(unix))]
    /// Reject termination without verified process-group supervision.
    pub async fn terminate(&self) -> Result<(), BrowserError> {
        Err(BrowserError::InvalidEndpoint(
            "Browser execution is unsupported on this platform",
        ))
    }

    #[cfg(unix)]
    /// Kill Chromium's process group and return once the browser process is verified gone (its
    /// child status reaped, or its pid no longer known to the system) - never on the strength
    /// of a signal that reported success or a transport that happens to be closed. Unlike
    /// [`Browser::shutdown`] this works through a shared handle, for callers that must be sure
    /// no tab survives; every tab of every session is lost.
    pub async fn terminate(&self) -> Result<(), BrowserError> {
        terminate_process(
            self.process.group,
            &self.process.child,
            |pid, group| {
                if group {
                    killpg(pid, Signal::SIGKILL)
                } else {
                    kill(pid, Signal::SIGKILL)
                }
            },
            Duration::from_secs(5),
        )
        .await?;
        // The transport follows the process at once; callers read `is_connected` right after.
        let _ = tokio::time::timeout(Duration::from_secs(1), async {
            while !self.inner.cdp.closed() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        self.wipe_space_downloads().await?;
        Ok(())
    }

    #[cfg(not(unix))]
    /// Reject shutdown without verified process-group supervision.
    pub async fn shutdown(self) -> Result<(), BrowserError> {
        Err(BrowserError::InvalidEndpoint(
            "Browser execution is unsupported on this platform",
        ))
    }

    #[cfg(unix)]
    /// Terminate Chromium even when detached; configured persistent profiles remain on disk.
    pub async fn shutdown(mut self) -> Result<(), BrowserError> {
        self.process.detached = false;
        let close = self.inner.cdp.call(None, "Browser.close", json!({})).await;
        let exited = tokio::time::timeout(Duration::from_secs(10), async {
            if let Some(child) = self.process.child.get_mut() {
                child.wait().await?;
                return Ok::<_, BrowserError>(());
            }
            let mut poll = tokio::time::interval(Duration::from_millis(20));
            loop {
                poll.tick().await;
                match kill(self.process.group, None) {
                    Err(nix::errno::Errno::ESRCH) => return Ok(()),
                    Err(error) => return Err(std::io::Error::from(error).into()),
                    Ok(()) => {}
                }
            }
        })
        .await;
        if exited.is_err() {
            let _ = killpg(self.process.group, Signal::SIGKILL);
            if let Some(child) = self.process.child.get_mut() {
                child.wait().await?;
            }
            return Err(BrowserError::Timeout("Chromium shutdown"));
        }
        exited.map_err(|_| BrowserError::Timeout("Chromium shutdown"))??;
        self.process.running = false;
        self.event_task.abort();
        self.wipe_space_downloads().await?;
        self.wipe_uploads().await?;
        match close {
            Err(BrowserError::Disconnected) => Ok(()),
            result => result.map(|_| ()),
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.event_task.abort();
        // A killed browser cannot remove its own discovery files; detached ones still own them.
        if !self.process.detached {
            if let Some(directory) = self.gateway_dir.take() {
                let _ = std::fs::remove_dir_all(directory);
            }
        }
    }
}

fn warn_tabs(inner: &Inner, count: usize) {
    if count >= 5 {
        let _ = inner.events.send(BrowserEvent::TooManyTabs { count });
    }
}

async fn handle_event(inner: &Inner, event: Value) {
    let params = &event["params"];
    let mut state = inner.state.lock().await;
    match event["method"].as_str().unwrap_or_default() {
        "Target.targetCreated" => {
            if let Some(mut tab) = target_tab(&params["targetInfo"]) {
                if state.tabs.contains_key(&tab.id) {
                    return;
                }
                if let Some(context) = params["targetInfo"]["browserContextId"].as_str() {
                    state
                        .tab_contexts
                        .insert(tab.id.clone(), context.to_owned());
                    // Every target of a space belongs to its owner, whatever its opener says
                    // (or whether it has one, e.g. a service-worker window).
                    let owner = state
                        .spaces
                        .iter()
                        .find(|(_, space)| *space == context)
                        .map(|(owner, _)| owner.clone());
                    if let Some(owner) = owner {
                        tab.browser_context_id = Some(context.to_owned());
                        tab.owner_session_id = Some(owner);
                    } else if let Some(owner) = state.closing.get(context) {
                        tab.owner_session_id = Some(owner.clone());
                    }
                }
                let opener = params["targetInfo"]["openerId"].as_str();
                if let Some(opener) = opener.filter(|_| tab.owner_session_id.is_none()) {
                    match opener_owner(&state, opener) {
                        Some(owner) => tab.owner_session_id = owner,
                        // The opener is not (yet) in the ledger: ownership is unresolved, not
                        // "nobody's". Requests of this popup wait for it (see `settle_openers`).
                        None => {
                            state
                                .pending_openers
                                .insert(tab.id.clone(), opener.to_owned());
                        }
                    }
                }
                state.tabs.insert(tab.id.clone(), tab.clone());
                settle_openers(&mut state);
                let _ = inner
                    .events
                    .send(BrowserEvent::TabOpened { tab: tab.clone() });
                if let Some(opener) = opener {
                    inner.page.publish(PageEvent::PopupOpened {
                        tab: tab.clone(),
                        opener_id: opener.to_owned(),
                    });
                    let _ = inner.events.send(BrowserEvent::PopupOpened {
                        tab,
                        opener_id: opener.to_owned(),
                    });
                }
                warn_tabs(inner, state.tabs.len());
            }
        }
        "Target.targetInfoChanged" => {
            if let Some(tab) = target_tab(&params["targetInfo"]) {
                if let Some(current) = state.tabs.get_mut(&tab.id) {
                    current.url = tab.url;
                    current.title = tab.title;
                }
            }
        }
        "Target.targetDestroyed" => {
            if let Some(id) = params["targetId"].as_str() {
                state.closing_tabs.remove(id);
                state.upload_closing.remove(id);
                state.upload_epochs.remove(id);
                if let Some(root) = state.uploads.remove(id) {
                    let tab = id.to_owned();
                    tokio::task::spawn_blocking(move || uploads::remove_tab(&root, &tab));
                }
                if let Some(session) = state.sessions.remove(id) {
                    inner.cdp.forget_session(&session);
                }
                state.references.remove(id);
                state.frames.retain(|_, target| target != id);
                state.pending_openers.remove(id);
                state.tab_contexts.remove(id);
                state.oopif_sessions.retain(|frame, (tab, session)| {
                    if frame == id || tab == id {
                        inner.cdp.forget_session(session);
                        false
                    } else {
                        true
                    }
                });
                inner.page.tab_gone(id);
                if let Some(tab) = state.tabs.remove(id) {
                    let _ = inner.events.send(BrowserEvent::TabClosed { tab });
                }
            }
        }
        "Target.detachedFromTarget" => {
            if let Some(session) = params["sessionId"].as_str() {
                state.sessions.retain(|_, value| value != session);
                state
                    .oopif_sessions
                    .retain(|_, (_, value)| value != session);
            }
        }
        // A subframe is attached before its first document request is made, so recording it
        // here lets that request be tied to its tab (see `subframe_allowed`).
        "Page.frameAttached" => {
            let target = event["sessionId"].as_str().and_then(|session| {
                state
                    .sessions
                    .iter()
                    .find(|(_, value)| *value == session)
                    .map(|(target, _)| target.clone())
            });
            if let (Some(target), Some(frame)) = (target, params["frameId"].as_str()) {
                state.frames.insert(frame.to_owned(), target);
            }
        }
        "Page.frameNavigated" => {
            let target = event["sessionId"].as_str().and_then(|session| {
                state
                    .sessions
                    .iter()
                    .find(|(_, value)| *value == session)
                    .map(|(target, _)| target.clone())
            });
            if let Some(target) = target {
                if params["frame"].get("parentId").is_none() {
                    if let Some(refs) = state.references.get_mut(&target) {
                        refs.invalidate();
                    }
                }
                if let Some(frame) = params["frame"]["id"].as_str() {
                    state.frames.insert(frame.to_owned(), target);
                }
            }
        }
        "Input.dragIntercepted" => {
            if let Some(session) = event["sessionId"].as_str() {
                pointer::record_drag(session, params["data"].clone());
            }
        }
        "Page.javascriptDialogOpening"
        | "Page.javascriptDialogClosed"
        | "Page.fileChooserOpened" => {
            page_events::page_event(
                inner,
                &state,
                event["method"].as_str().unwrap_or_default(),
                event["sessionId"].as_str(),
                params,
            );
        }
        "Browser.downloadWillBegin" => {
            let text = |field: &str| params[field].as_str().unwrap_or_default().to_owned();
            let tab_id = params["frameId"]
                .as_str()
                .and_then(|id| state.frames.get(id).cloned());
            let tab = tab_id.as_ref().and_then(|id| state.tabs.get(id));
            inner.page.download_started(DownloadInfo {
                guid: text("guid"),
                url: text("url"),
                suggested_filename: text("suggestedFilename"),
                tab_id: tab_id.clone(),
                owner: tab.and_then(|tab| tab.owner_session_id.clone()),
                space: tab.and_then(|tab| tab.browser_context_id.clone()),
                status: None,
            });
            let _ = inner.events.send(BrowserEvent::DownloadStarted {
                guid: text("guid"),
                url: text("url"),
                suggested_filename: text("suggestedFilename"),
                tab_id,
            });
        }
        "Browser.downloadProgress" => {
            let status = match params["state"].as_str() {
                Some("completed") => Some(DownloadStatus::Completed),
                Some("canceled") => Some(DownloadStatus::Canceled),
                _ => None,
            };
            if let Some(status) = status {
                let guid = params["guid"].as_str().unwrap_or_default().to_owned();
                inner.page.download_finished(&guid, status);
                let _ = inner
                    .events
                    .send(BrowserEvent::DownloadFinished { guid, status });
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    use super::*;

    #[cfg(not(unix))]
    #[tokio::test]
    async fn unsupported_process_supervision_rejects_launch_and_reconnect() {
        let launched = Browser::launch(BrowserConfig::new("not-a-browser")).await;
        assert!(matches!(
            launched,
            Err(BrowserError::InvalidEndpoint(
                "Browser execution is unsupported on this platform"
            ))
        ));
        let connected = Browser::connect(BrowserEndpoint {
            pid: 1,
            ws_url: "ws://127.0.0.1:1".into(),
            profile_dir: PathBuf::from("not-a-profile"),
        })
        .await;
        assert!(matches!(
            connected,
            Err(BrowserError::InvalidEndpoint(
                "Browser execution is unsupported on this platform"
            ))
        ));
    }

    #[test]
    fn action_schema_and_key_events() {
        let action: Action =
            serde_json::from_value(json!({"action":"type","text":"hello","submit":true})).unwrap();
        assert_eq!(
            action,
            Action::Type {
                text: "hello".to_owned(),
                submit: true
            }
        );
        assert_eq!(
            serde_json::to_value(Action::Hover).unwrap(),
            json!({"action":"hover"})
        );
        assert_eq!(key_event("Enter").unwrap()["text"], "\r");
        let chord = key_event("Control+Shift+A").unwrap();
        assert_eq!(chord["modifiers"], 10);
        assert_eq!(chord["code"], "KeyA");
        assert!(chord.get("text").is_none());
        assert!(matches!(
            key_event("not-a-key"),
            Err(BrowserError::InvalidAction(_))
        ));
    }

    #[tokio::test]
    async fn screenshot_errors_still_attempt_focus_reset() {
        for fail_enable in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("ws://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
                for stage in 0..if fail_enable { 2 } else { 3 } {
                    let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
                        panic!("expected screenshot command")
                    };
                    let request: Value = serde_json::from_str(&text).unwrap();
                    let is_capture = !fail_enable && stage == 1;
                    if is_capture {
                        assert_eq!(request["method"], "Page.captureScreenshot");
                    } else {
                        assert_eq!(request["method"], "Emulation.setFocusEmulationEnabled");
                        assert_eq!(request["params"]["enabled"], stage == 0);
                    }
                    let error = (fail_enable && stage == 0) || is_capture;
                    let reply = if error {
                        json!({"id":request["id"],"error":{"code":-32000,"message":"injected screenshot failure"}})
                    } else {
                        json!({"id":request["id"],"result":{}})
                    };
                    socket
                        .send(Message::Text(reply.to_string().into()))
                        .await
                        .unwrap();
                }
            });
            let (cdp, _notifications) = Cdp::connect(&url, Duration::from_secs(2)).await.unwrap();
            let (events, _receiver) = mpsc::unbounded_channel();
            let inner = Arc::new(Inner {
                cdp,
                state: Mutex::default(),
                attach_lock: Mutex::default(),
                capture_lock: Arc::default(),
                interaction_lock: Mutex::default(),
                network: Default::default(),
                attach_timeout: Duration::from_secs(2),
                events,
                navigation_policy: std::sync::RwLock::new(None),
                screencasts: Default::default(),
                page: Default::default(),
            });
            assert!(matches!(
                inner
                    .capture("session".to_owned(), json!({"format": "png"}))
                    .await,
                Err(BrowserError::Cdp { .. })
            ));
            tokio::time::timeout(Duration::from_secs(2), server)
                .await
                .unwrap()
                .unwrap();
        }
    }

    #[tokio::test]
    async fn interaction_focus_resets_after_success_cancellation_and_enable_failure() {
        for mode in 0..3 {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("ws://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
                for enabled in [true, false] {
                    let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
                        panic!("expected focus-emulation command")
                    };
                    let request: Value = serde_json::from_str(&text).unwrap();
                    assert_eq!(request["method"], "Emulation.setFocusEmulationEnabled");
                    assert_eq!(request["params"]["enabled"], enabled);
                    let reply = if enabled && mode == 2 {
                        json!({"id":request["id"],"error":{"code":-32000,"message":"injected enable failure"}})
                    } else {
                        json!({"id":request["id"],"result":{}})
                    };
                    socket
                        .send(Message::Text(reply.to_string().into()))
                        .await
                        .unwrap();
                }
            });
            let (cdp, _notifications) = Cdp::connect(&url, Duration::from_secs(2)).await.unwrap();
            let (events, _receiver) = mpsc::unbounded_channel();
            let inner = Arc::new(Inner {
                cdp,
                state: Mutex::default(),
                attach_lock: Mutex::default(),
                capture_lock: Arc::default(),
                interaction_lock: Mutex::default(),
                network: Default::default(),
                attach_timeout: Duration::from_secs(2),
                events,
                navigation_policy: std::sync::RwLock::new(None),
                screencasts: Default::default(),
                page: Default::default(),
            });
            let cleanup = Arc::clone(&inner).begin_focus("session".to_owned()).await;
            match mode {
                0 => cleanup.unwrap().reset().await.unwrap(),
                1 => drop(cleanup.unwrap()),
                _ => assert!(matches!(cleanup, Err(BrowserError::Cdp { .. }))),
            }
            tokio::time::timeout(Duration::from_secs(2), server)
                .await
                .unwrap()
                .unwrap();
            let _lock = tokio::time::timeout(Duration::from_secs(2), inner.capture_lock.lock())
                .await
                .expect("focus reset retained the serialization lock");
        }
    }

    #[cfg(unix)]
    /// Termination is believed only when the process is seen gone: not from a signal that
    /// claims success or "no such process", and not from a transport that is already closed.
    #[tokio::test]
    async fn termination_is_confirmed_by_the_process_and_not_by_signals() {
        use nix::errno::Errno;
        let child = Command::new("sleep")
            .arg("30")
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = Pid::from_raw(child.id().unwrap() as i32);
        let child = parking_lot::Mutex::new(Some(child));
        let alive = || matches!(child.lock().as_mut().unwrap().try_wait(), Ok(None));
        let short = Duration::from_millis(300);

        // Signalling fails outright (the group and the pid alone): an error, still running.
        let refused = terminate_process(pid, &child, |_, _| Err(Errno::EPERM), short).await;
        assert!(matches!(refused, Err(BrowserError::Io(_))), "{refused:?}");
        assert!(alive());
        // "No such process" and "sent" that kill nothing are not believed either.
        for answer in [Err(Errno::ESRCH), Ok(())] {
            let unproven = terminate_process(pid, &child, |_, _| answer, short).await;
            assert!(
                matches!(unproven, Err(BrowserError::Timeout(_))),
                "{unproven:?}"
            );
            assert!(alive());
        }

        let killed = terminate_process(
            pid,
            &child,
            |pid, group| {
                if group {
                    killpg(pid, Signal::SIGKILL)
                } else {
                    kill(pid, Signal::SIGKILL)
                }
            },
            Duration::from_secs(5),
        )
        .await;
        assert!(killed.is_ok(), "{killed:?}");
        assert!(!alive());
    }

    #[tokio::test]
    async fn profile_hardening_keeps_other_preferences() {
        let profile = tempfile::tempdir().unwrap();
        let path = profile.path().join("Default/Preferences");
        harden_profile(profile.path()).await.unwrap();
        let written: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["net"]["network_prediction_options"], 2);
        assert_eq!(written["autofill"]["profile_enabled"], false);
        assert_eq!(written["autofill"]["credit_card_enabled"], false);
        assert_eq!(written["credentials_enable_service"], false);
        assert_eq!(written["profile"]["password_manager_enabled"], false);

        std::fs::write(&path, br#"{"net":{"other":1},"keep":true}"#).unwrap();
        harden_profile(profile.path()).await.unwrap();
        let written: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["net"]["network_prediction_options"], 2);
        assert_eq!(written["net"]["other"], 1);
        assert_eq!(written["keep"], true);

        // An unreadable file is replaced rather than left to re-enable preloading.
        std::fs::write(&path, b"not json").unwrap();
        harden_profile(profile.path()).await.unwrap();
        let written: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["net"]["network_prediction_options"], 2);
    }

    /// A document request from a frame missing from the ledger is decided only after its
    /// ownership is positively classified; a failing lookup or a popup whose opener never
    /// appears is denied once the attach timeout elapses instead of continuing.
    #[tokio::test]
    async fn unresolved_frame_ownership_fails_closed() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let (decided, mut decisions) = mpsc::unbounded_channel::<(String, String)>();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            while let Some(Ok(Message::Text(text))) = socket.next().await {
                let request: Value = serde_json::from_str(&text).unwrap();
                let reply = match request["method"].as_str().unwrap() {
                    "Target.getTargetInfo" => {
                        let info = |extra: Value| {
                            let mut info =
                                json!({"targetId":request["params"]["targetId"],"type":"page"});
                            info.as_object_mut()
                                .unwrap()
                                .extend(extra.as_object().unwrap().clone());
                            json!({"id":request["id"],"result":{"targetInfo":info}})
                        };
                        match request["params"]["targetId"].as_str().unwrap() {
                            "subframe" => {
                                json!({"id":request["id"],"error":{"code":-32602,"message":"No target with given id found"}})
                            }
                            "flaky" => {
                                json!({"id":request["id"],"error":{"code":-32000,"message":"Internal error"}})
                            }
                            "worker" => info(json!({"type":"iframe"})),
                            "fresh" => info(json!({})),
                            "fresh-with-context" => {
                                info(json!({"browserContextId":"default-context"}))
                            }
                            "default-popup-denied" => info(
                                json!({"browserContextId":"default-context","openerId":"owned-tab"}),
                            ),
                            "orphan" => info(json!({"openerId":"missing-opener"})),
                            "space-allowed" | "space-denied" => {
                                info(json!({"browserContextId":"bound-space"}))
                            }
                            "space-closing" => info(json!({"browserContextId":"closing-space"})),
                            "space-unbound" => info(json!({"browserContextId":"unbound-space"})),
                            _ => info(json!({"openerId":"owned-tab"})),
                        }
                    }
                    "Target.getBrowserContexts" => {
                        json!({"id":request["id"],"result":{"browserContextIds":["bound-space","closing-space","unbound-space"]}})
                    }
                    method => {
                        decided
                            .send((
                                request["params"]["requestId"].as_str().unwrap().to_owned(),
                                method.to_owned(),
                            ))
                            .unwrap();
                        json!({"id":request["id"],"result":{}})
                    }
                };
                socket
                    .send(Message::Text(reply.to_string().into()))
                    .await
                    .unwrap();
            }
        });
        let (cdp, _notifications) = Cdp::connect(&url, Duration::from_secs(2)).await.unwrap();
        let (events, _receiver) = mpsc::unbounded_channel();
        let mut state = State::default();
        state
            .spaces
            .insert("space-owner".into(), "bound-space".into());
        state
            .closing
            .insert("closing-space".into(), "space-owner".into());
        state.tabs.insert(
            "owned-tab".into(),
            TabInfo {
                id: "owned-tab".into(),
                url: String::new(),
                title: String::new(),
                opened_by: OpenedBy::Agent,
                owner_session_id: Some("agent".into()),
                discarded: false,
                unloaded: false,
                browser_context_id: None,
            },
        );
        let check: OwnerNavigationCheck = Arc::new(|_, url| !url.contains("denied"));
        let inner = Arc::new(Inner {
            cdp,
            state: Mutex::new(state),
            attach_lock: Mutex::default(),
            capture_lock: Arc::default(),
            interaction_lock: Mutex::default(),
            network: Default::default(),
            attach_timeout: Duration::from_millis(400),
            events,
            navigation_policy: std::sync::RwLock::new(Some(check)),
            screencasts: Default::default(),
            page: Default::default(),
        });
        let cases = [
            ("subframe", "https://denied.test/", "Fetch.continueRequest"),
            ("worker", "https://denied.test/", "Fetch.continueRequest"),
            ("fresh", "https://denied.test/", "Fetch.continueRequest"),
            (
                "fresh-with-context",
                "https://denied.test/",
                "Fetch.continueRequest",
            ),
            (
                "default-popup-denied",
                "https://denied.test/",
                "Fetch.failRequest",
            ),
            ("popup-allowed", "https://ok.test/", "Fetch.continueRequest"),
            ("popup-denied", "https://denied.test/", "Fetch.failRequest"),
            ("orphan", "https://ok.test/", "Fetch.failRequest"),
            ("flaky", "https://ok.test/", "Fetch.failRequest"),
            ("space-allowed", "https://ok.test/", "Fetch.continueRequest"),
            ("space-denied", "https://denied.test/", "Fetch.failRequest"),
            ("space-closing", "https://ok.test/", "Fetch.failRequest"),
            ("space-unbound", "https://ok.test/", "Fetch.failRequest"),
        ];
        for (frame, url, _) in cases {
            handle_paused(
                &inner,
                &json!({"params":{
                    "requestId":frame,
                    "frameId":frame,
                    "resourceType":DOCUMENT_INTERCEPTION,
                    "request":{"url":url}
                }}),
            )
            .await;
        }
        let mut seen = HashMap::new();
        while seen.len() < cases.len() {
            let (request, method) = tokio::time::timeout(Duration::from_secs(3), decisions.recv())
                .await
                .expect("every paused request is decided within the bound")
                .unwrap();
            seen.insert(request, method);
        }
        for (frame, _, expected) in cases {
            assert_eq!(seen[frame], expected, "{frame}");
        }

        // A popup whose Target.targetCreated is handled before its opener reached the ledger is
        // not ownerless: once the opener (agent-owned) is known, its requests are judged by the
        // owner policy rather than waved through.
        handle_event(
            &inner,
            json!({"method":"Target.targetCreated","params":{"targetInfo":{
                "targetId":"late-popup","type":"page","openerId":"late-opener"}}}),
        )
        .await;
        inner.state.lock().await.tabs.insert(
            "late-opener".into(),
            TabInfo {
                id: "late-opener".into(),
                url: String::new(),
                title: String::new(),
                opened_by: OpenedBy::Agent,
                owner_session_id: Some("agent".into()),
                discarded: false,
                unloaded: false,
                browser_context_id: None,
            },
        );
        let late = [
            ("late-denied", "https://denied.test/", "Fetch.failRequest"),
            ("late-ok", "https://ok.test/", "Fetch.continueRequest"),
        ];
        for (request, url, _) in late {
            handle_paused(
                &inner,
                &json!({"params":{
                    "requestId":request,
                    "frameId":"late-popup",
                    "resourceType":DOCUMENT_INTERCEPTION,
                    "request":{"url":url}
                }}),
            )
            .await;
        }
        let mut seen = HashMap::new();
        while seen.len() < late.len() {
            let (request, method) = tokio::time::timeout(Duration::from_secs(3), decisions.recv())
                .await
                .expect("every paused request is decided within the bound")
                .unwrap();
            seen.insert(request, method);
        }
        for (request, _, expected) in late {
            assert_eq!(seen[request], expected, "{request}");
        }
        drop(inner);
        server.abort();
    }
}
