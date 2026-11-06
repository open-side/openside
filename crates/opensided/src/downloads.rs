//! Downloads the agent's browsing starts (`docs/specs/artifacts.md` § Browser downloads).
//!
//! Every browser the daemon drives (shared profile, isolated spaces, incognito and account
//! browsers) reports `DownloadStarted`/`DownloadFinished` to its event pump, which hands the
//! owner's events to [`observe`]. A completed download is imported into the owner's session
//! artifacts as `downloads/<guid>/<name>` when the session's policy allows it and it fits the
//! per-file and per-session ceilings; the browser itself cancels a transfer that outgrows the
//! per-file ceiling ([`openside_browser::Browser::set_download_limit`]). Each step is published
//! on the download's guid, which the web thread renders as a download chip, and the
//! `browser_downloads` tool lists (and can wait for) the same records.
//!
//! The record of a download is in memory only and bounded per session; the imported file and
//! its `artifact.saved` event are what persist, and they go with the session's artifacts. The
//! file Chromium wrote is deleted from the browser profile once the download settles, whatever
//! the outcome ([`sweep`] catches what a crash left).

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use openside_browser::{Browser, BrowserEvent, DownloadStatus};
use openside_policy::{Action, Capability, Effect};
use openside_proto::{EventKind, SessionId};
use serde_json::{json, Value};

use crate::engine::Shared;

/// Artifact folder of imported downloads.
pub(crate) const ARTIFACT_DIR: &str = "downloads";
/// `artifact.saved` tool name of an imported download.
pub(crate) const ARTIFACT_TOOL: &str = "browser_download";
/// Stored type of a download whose bytes `artifacts::sniff` does not recognise.
const UNRECOGNISED_MIME: &str = "application/octet-stream";
/// Largest single download kept; the browser cancels larger transfers as they grow.
pub(crate) const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
/// Total of imported downloads one session may hold.
pub(crate) const MAX_SESSION_BYTES: u64 = 200 * 1024 * 1024;
/// A download file Chromium left in a profile this long ago is swept even if never released.
const RAW_FILE_TTL: Duration = Duration::from_secs(3600);
const SWEEP_EVERY: Duration = Duration::from_secs(600);
const MAX_GUID_BYTES: usize = 64;
const TRACKED_PER_SESSION: usize = 64;

/// Name extensions that run code or install software when opened on the person's computer
/// (or render active content), so the file is offered for download only.
const QUARANTINED_EXTENSIONS: &[&str] = &[
    "action",
    "app",
    "apk",
    "appimage",
    "applescript",
    "bash",
    "bat",
    "bin",
    "chm",
    "cmd",
    "com",
    "command",
    "cpl",
    "crx",
    "csh",
    "deb",
    "desktop",
    "dll",
    "dmg",
    "dylib",
    "exe",
    "fish",
    "gadget",
    "hta",
    "htm",
    "html",
    "img",
    "inf",
    "ipa",
    "iso",
    "jar",
    "jnlp",
    "js",
    "jse",
    "ksh",
    "lnk",
    "mht",
    "mhtml",
    "mjs",
    "msc",
    "msi",
    "msix",
    "msp",
    "php",
    "pif",
    "pkg",
    "pl",
    "ps1",
    "psm1",
    "py",
    "pyc",
    "pyw",
    "pyz",
    "rb",
    "reg",
    "rpm",
    "run",
    "scf",
    "scpt",
    "scr",
    "sh",
    "so",
    "svg",
    "url",
    "vb",
    "vbe",
    "vbs",
    "vhd",
    "vhdx",
    "workflow",
    "ws",
    "wsf",
    "xhtml",
    "xpi",
    "xsl",
    "xslt",
    "zsh",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    Canceled,
    TooLarge,
    SessionQuota,
    Policy,
    Unavailable,
}

impl Failure {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Canceled => "canceled",
            Self::TooLarge => "too_large",
            Self::SessionQuota => "session_quota",
            Self::Policy => "policy",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Saved {
    pub(crate) artifact: String,
    pub(crate) file_name: String,
    pub(crate) size: u64,
    pub(crate) mime: String,
    pub(crate) quarantined: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum State {
    InProgress,
    Saving,
    Saved(Saved),
    Failed(Failure),
}

impl State {
    fn terminal(&self) -> bool {
        matches!(self, Self::Saved(_) | Self::Failed(_))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Download {
    pub(crate) guid: String,
    /// Source URL without credentials, query or fragment.
    pub(crate) url: String,
    /// Origin of the source URL, when it has one (not for `data:`).
    pub(crate) origin: Option<String>,
    /// Origin of the page whose tab started the download. `blob:` and `data:` downloads have
    /// no origin of their own, so the page is what policy judges for them.
    pub(crate) page_origin: Option<String>,
    pub(crate) tab_id: Option<String>,
    pub(crate) state: State,
}

/// The downloads of each session, newest last.
#[derive(Default)]
pub(crate) struct Downloads {
    sessions: parking_lot::Mutex<HashMap<SessionId, VecDeque<Download>>>,
    changed: tokio::sync::Notify,
    /// When the last periodic [`sweep`] ran.
    swept: parking_lot::Mutex<Option<Instant>>,
}

impl Downloads {
    pub(crate) fn of(&self, session: &SessionId) -> Vec<Download> {
        self.sessions
            .lock()
            .get(session)
            .map(|list| list.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub(crate) fn forget(&self, session: &SessionId) {
        self.sessions.lock().remove(session);
        self.changed.notify_waiters();
    }

    /// Resolves after the next change of any download; take it before checking the records,
    /// so a change between the check and the wait is not missed.
    pub(crate) fn changed(&self) -> tokio::sync::futures::Notified<'_> {
        self.changed.notified()
    }

    /// Applies `change` (which reports whether it changed anything) to the download's record,
    /// creating an in-progress record first when there is none.
    fn update(
        &self,
        session: &SessionId,
        guid: &str,
        change: impl FnOnce(&mut Download) -> bool,
    ) -> bool {
        let mut sessions = self.sessions.lock();
        let list = sessions.entry(session.clone()).or_default();
        let applied = match list.iter_mut().find(|download| download.guid == guid) {
            Some(download) => change(download),
            None => {
                let mut download = Download {
                    guid: guid.to_owned(),
                    url: String::new(),
                    origin: None,
                    page_origin: None,
                    tab_id: None,
                    state: State::InProgress,
                };
                let applied = change(&mut download);
                if list.len() >= TRACKED_PER_SESSION {
                    list.pop_front();
                }
                list.push_back(download);
                applied
            }
        };
        drop(sessions);
        if applied {
            self.changed.notify_waiters();
        }
        applied
    }

    #[cfg(test)]
    fn set(&self, session: &SessionId, guid: &str, state: State) {
        self.update(session, guid, |download| {
            download.state = state;
            true
        });
    }
}

/// Moves a running download to `next`. Chromium can report a terminal state twice (and a
/// reconnect can replay one), so only the first report settles a download: a later one must
/// neither start a second import nor overwrite the outcome or the kept copy.
fn claim(download: &mut Download, next: State) -> bool {
    if !matches!(download.state, State::InProgress) {
        return false;
    }
    download.state = next;
    true
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Recorded {
    /// The session is gone; nothing was recorded or published.
    Gone,
    /// The change did not apply (a settled download); nothing was published.
    Skipped,
    Applied,
}

/// Handles one browser event already attributed to `session`; call it after the event itself
/// was published, so the lifecycle updates follow `tool.ended` in the session log.
pub(crate) async fn observe(
    shared: &Shared,
    session: &SessionId,
    browser: Option<Arc<Browser>>,
    event: &BrowserEvent,
) {
    match event {
        BrowserEvent::DownloadStarted {
            guid, url, tab_id, ..
        } => {
            let (url, origin, tab_id) = (
                crate::browser::public_url(url),
                openside_policy::origin_of(url),
                tab_id.clone(),
            );
            let page_origin = browser
                .as_ref()
                .and_then(|browser| browser.download(guid))
                .and_then(|record| record.page_url)
                .and_then(|page| openside_policy::origin_of(&page));
            record(shared, session, guid, None, |download| {
                if !matches!(download.state, State::InProgress) {
                    return false;
                }
                download.url = url;
                download.origin = origin;
                download.page_origin = page_origin;
                download.tab_id = tab_id;
                true
            })
            .await;
        }
        BrowserEvent::DownloadFinished { guid, status } => match status {
            DownloadStatus::Canceled => {
                let over = browser
                    .as_ref()
                    .and_then(|browser| browser.download(guid))
                    .is_some_and(|record| record.over_limit);
                let failure = if over {
                    Failure::TooLarge
                } else {
                    Failure::Canceled
                };
                let settled = record(shared, session, guid, None, |download| {
                    claim(download, State::Failed(failure))
                })
                .await;
                if settled == Recorded::Skipped {
                    return;
                }
                release(browser.as_deref(), guid, None).await;
                if settled == Recorded::Applied {
                    publish_failure(shared, session, guid, failure).await;
                }
            }
            DownloadStatus::Completed => {
                match record(shared, session, guid, None, |download| {
                    claim(download, State::Saving)
                })
                .await
                {
                    Recorded::Applied => {}
                    Recorded::Skipped => return,
                    Recorded::Gone => {
                        release(browser.as_deref(), guid, None).await;
                        return;
                    }
                }
                let (shared, session, guid) = (shared.clone(), session.clone(), guid.clone());
                tokio::spawn(async move {
                    let outcome = match &browser {
                        Some(browser) => import(&shared, &session, browser, &guid).await,
                        None => Err(Failure::Unavailable),
                    };
                    match outcome {
                        Ok(saved) => {
                            let kept = crate::artifacts::session_dir(&shared, &session)
                                .join(&saved.artifact);
                            release(browser.as_deref(), &guid, Some(kept)).await;
                            let update = json!({
                                "browser_event": "download.saved",
                                "artifact": saved.artifact,
                                "file_name": saved.file_name,
                                "size": saved.size,
                                "mime": saved.mime,
                                "quarantined": saved.quarantined,
                            });
                            record(&shared, &session, &guid, Some(update), |download| {
                                download.state = State::Saved(saved);
                                true
                            })
                            .await;
                        }
                        Err(failure) => {
                            release(browser.as_deref(), &guid, None).await;
                            record(&shared, &session, &guid, None, |download| {
                                download.state = State::Failed(failure);
                                true
                            })
                            .await;
                            publish_failure(&shared, &session, &guid, failure).await;
                        }
                    }
                });
            }
        },
        _ => {}
    }
}

async fn publish_failure(shared: &Shared, session: &SessionId, guid: &str, failure: Failure) {
    let update = json!({"browser_event": "download.failed", "error": failure.as_str()});
    record(shared, session, guid, Some(update), |_| true).await;
}

/// Deletes the file Chromium wrote for a settled download, before the outcome is published, so
/// no copy outlives it in the browser profile. `kept` is the imported artifact, which the REPL's
/// `saveAs` copies from afterwards.
async fn release(browser: Option<&Browser>, guid: &str, kept: Option<PathBuf>) {
    if let Some(browser) = browser {
        if let Err(error) = browser.release_download(guid, kept).await {
            crate::diag!("opensided: download file {guid} could not be deleted: {error}");
        }
    }
}

/// Deletes download files left in `browser`'s profile past [`RAW_FILE_TTL`] (a crash between a
/// download's end and its release); at most every [`SWEEP_EVERY`] unless `now` is set.
pub(crate) async fn sweep(shared: &Shared, browser: &Browser, now: bool) {
    let downloads = &shared.registry.browser.downloads;
    {
        let mut last = downloads.swept.lock();
        if !now && last.is_some_and(|last| last.elapsed() < SWEEP_EVERY) {
            return;
        }
        *last = Some(Instant::now());
    }
    if let Err(error) = browser.sweep_stale_downloads(RAW_FILE_TTL).await {
        crate::diag!("opensided: leftover download files could not be swept: {error}");
    }
}

/// Applies `change` to the session's record and, when it applied, publishes `update`, only
/// while the session lives. Holding the artifact lifecycle gate orders this against deletion,
/// which forgets the session's records after `artifacts::remove_session` took the same gate:
/// an import that finishes after a deletion leaves neither a record nor an event behind.
async fn record(
    shared: &Shared,
    session: &SessionId,
    guid: &str,
    update: Option<Value>,
    change: impl FnOnce(&mut Download) -> bool,
) -> Recorded {
    let _gate = crate::artifacts::lifecycle(shared, session).await;
    if !shared
        .db
        .state(session)
        .await
        .is_ok_and(|state| !state.deleted)
    {
        return Recorded::Gone;
    }
    if !shared
        .registry
        .browser
        .downloads
        .update(session, guid, change)
    {
        return Recorded::Skipped;
    }
    if let Some(update) = update {
        publish(shared, session, guid, update).await;
    }
    Recorded::Applied
}

async fn publish(shared: &Shared, session: &SessionId, guid: &str, update: Value) {
    let _ = shared
        .emit(
            session,
            None,
            EventKind::ToolUpdated {
                tool_call_id: guid.to_owned(),
                update,
            },
        )
        .await;
}

/// Copies a completed download into the session's artifacts. Admission mirrors the browser
/// REPL's `saveAs` gate: the starting page's origin and the file's own origin must be
/// browsable, and the session's policy must not deny writing its download folder (a
/// read-only session denies it). A download with neither origin cannot be judged and is
/// refused.
async fn import(
    shared: &Shared,
    session: &SessionId,
    browser: &Browser,
    guid: &str,
) -> Result<Saved, Failure> {
    if guid.is_empty()
        || guid.len() > MAX_GUID_BYTES
        || !guid
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(Failure::Unavailable);
    }
    let state = shared
        .db
        .state(session)
        .await
        .map_err(|_| Failure::Unavailable)?;
    if state.deleted {
        return Err(Failure::Unavailable);
    }
    let record = browser.download(guid).ok_or(Failure::Unavailable)?;
    let env = crate::tools::ToolEnv::new(shared.clone(), &state);
    let policy = crate::policy_api::effective(shared, &env.policy);
    let folder = crate::artifacts::session_dir(shared, session).join(ARTIFACT_DIR);
    let real = crate::artifacts::resolved(&folder);
    // Both spellings: a rule may name the configured data directory or where it really is.
    let mut checks: Vec<Action> = [folder, real]
        .iter()
        .map(|path| Action::new(Capability::FsWrite, path.to_string_lossy().as_ref()))
        .collect();
    // A `blob:` or `data:` download has no origin of its own; the page that made it is judged.
    // With neither origin there is nothing to judge, so the import fails closed.
    let origins: Vec<String> = record
        .page_url
        .as_deref()
        .and_then(openside_policy::origin_of)
        .into_iter()
        .chain(openside_policy::origin_of(&record.url))
        .collect();
    if origins.is_empty() {
        return Err(Failure::Policy);
    }
    for origin in origins {
        checks.push(Action::new(Capability::Browse, origin));
    }
    if checks
        .iter()
        .any(|action| policy.decide(action).effect == Effect::Deny)
    {
        return Err(Failure::Policy);
    }
    let bytes = browser
        .read_download(session.as_str(), guid, MAX_FILE_BYTES)
        .await
        .map_err(|_| Failure::Unavailable)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(Failure::TooLarge);
    }
    let file_name = openside_browser::safe_file_name(&record.suggested_filename);
    let mime = sniffed_mime(&bytes);
    let quarantined = quarantined(&file_name, mime);
    let mime = mime.to_owned();
    let artifact = format!("{ARTIFACT_DIR}/{guid}/{file_name}");
    let size = crate::artifacts::store_download(
        shared,
        session,
        guid,
        &artifact,
        bytes,
        &mime,
        MAX_SESSION_BYTES,
    )
    .await
    .map_err(|refusal| match refusal {
        crate::artifacts::DownloadRefusal::SessionQuota => Failure::SessionQuota,
        crate::artifacts::DownloadRefusal::Unavailable => Failure::Unavailable,
    })?;
    Ok(Saved {
        artifact,
        file_name,
        size,
        mime,
        quarantined,
    })
}

/// Type an imported download is stored and served as: magic bytes only, never its name.
pub(crate) fn sniffed_mime(bytes: &[u8]) -> &'static str {
    crate::artifacts::sniff(bytes).map_or(UNRECOGNISED_MIME, |(_, mime, _)| mime)
}

/// Whether a saved download is offered for download only. The importer decides at import time
/// and the artifact list re-derives it later from the stored name and type; both go through
/// here so the timeline chip and the Files list cannot disagree.
pub(crate) fn quarantined(file_name: &str, mime: &str) -> bool {
    mime == UNRECOGNISED_MIME || quarantined_name(file_name)
}

fn quarantined_name(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, extension)| {
        QUARANTINED_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
    })
}

/// One download as tools and clients see it.
pub(crate) fn describe(download: &Download, browser: Option<&Browser>) -> Value {
    let mut out = json!({
        "download_id": download.guid,
        "url": download.url,
        "tab_id": download.tab_id,
    });
    match &download.state {
        State::InProgress => {
            out["state"] = json!("in_progress");
            if let Some(record) = browser.and_then(|browser| browser.download(&download.guid)) {
                out["received_bytes"] = json!(record.received_bytes);
                if record.total_bytes > 0 {
                    out["total_bytes"] = json!(record.total_bytes);
                }
            }
        }
        State::Saving => out["state"] = json!("saving"),
        State::Saved(saved) => {
            out["state"] = json!("saved");
            out["artifact"] = json!(saved.artifact);
            out["file_name"] = json!(saved.file_name);
            out["size"] = json!(saved.size);
            out["mime"] = json!(saved.mime);
            out["quarantined"] = json!(saved.quarantined);
        }
        State::Failed(failure) => {
            out["state"] = json!("failed");
            out["error"] = json!(failure.as_str());
        }
    }
    out
}

pub(crate) fn settled(downloads: &[Download], id: Option<&str>) -> bool {
    downloads
        .iter()
        .filter(|download| id.map_or(true, |id| download.guid == id))
        .all(|download| download.state.terminal())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_and_active_names_are_quarantined() {
        for name in [
            "setup.EXE",
            "run.sh",
            "page.html",
            "tool.AppImage",
            "x.ps1",
            "s.py",
            "s.rb",
            "s.pl",
            "a.jar",
            "a.vbs",
            "a.wsf",
            "a.hta",
            "a.lnk",
            "a.desktop",
            "a.url",
            "a.reg",
        ] {
            assert!(quarantined_name(name), "{name}");
        }
        for name in ["report.pdf", "data.csv", "notes.txt", "README"] {
            assert!(!quarantined_name(name), "{name}");
        }
    }

    #[test]
    fn records_are_bounded_per_session_and_settle_by_id() {
        let downloads = Downloads::default();
        let session = SessionId::new("s");
        for n in 0..TRACKED_PER_SESSION + 3 {
            downloads.set(&session, &format!("g{n}"), State::InProgress);
        }
        let list = downloads.of(&session);
        assert_eq!(list.len(), TRACKED_PER_SESSION);
        assert_eq!(list[0].guid, "g3");
        downloads.set(&session, "g3", State::Failed(Failure::Canceled));
        let list = downloads.of(&session);
        assert!(settled(&list, Some("g3")));
        assert!(!settled(&list, None));
        downloads.forget(&session);
        assert!(downloads.of(&session).is_empty());
    }
}
