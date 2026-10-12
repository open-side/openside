//! B-10: an incognito session browses in its own Chromium whose `--user-data-dir` is a
//! temporary profile under `<runtime_dir>/incognito-browsers/<session>/profile` (0700). The
//! shared daemon profile never sees its cookies, history or storage. The whole directory is
//! deleted when the session is deleted, when the daemon shuts down, and, after a crash, by
//! the next daemon start. Its ledger is never persisted, so nothing survives a restart.

use super::*;
use openside_browser::BrowserConfig;

#[derive(Default)]
pub(super) struct Incognito {
    browsers: Mutex<HashMap<String, Arc<Browser>>>,
    forwarders: parking_lot::Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
}

pub(crate) fn root(shared: &Shared) -> PathBuf {
    shared
        .config
        .loaded
        .paths
        .runtime_dir
        .join("incognito-browsers")
}

/// Creates a 0700 directory that is a real directory, never a planted symlink: profiles and
/// their deletion must stay under the runtime directory.
async fn private_directory(path: &Path) -> std::io::Result<()> {
    tokio::fs::create_dir_all(path).await?;
    if !real_directory(path).await {
        return Err(std::io::Error::other("not a real directory"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).await?;
    }
    Ok(())
}

async fn real_directory(path: &Path) -> bool {
    tokio::fs::symlink_metadata(path)
        .await
        .is_ok_and(|metadata| metadata.is_dir())
}

/// Terminates Chromium before its profile is removed; a killed process cannot write again.
async fn terminate(browser: Arc<Browser>) {
    match Arc::try_unwrap(browser) {
        Ok(browser) => {
            let _ = browser.shutdown().await;
        }
        Err(shared) => {
            #[cfg(unix)]
            if let Ok(pid) = i32::try_from(shared.process_id()) {
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(pid),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
            drop(shared);
        }
    }
}

async fn remove(directory: &Path) {
    match tokio::fs::remove_dir_all(directory).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => crate::diag!("opensided: incognito profile could not be removed: {error}"),
    }
}

impl Controller {
    /// The browser a session's tools act in: incognito sessions get their own temporary
    /// profile; unknown sessions are treated as incognito (fail closed, like memory indexing).
    /// Isolated sessions use the shared browser with their space open (`browser_space.rs`).
    pub(crate) async fn for_session(
        &self,
        shared: &Shared,
        session: &SessionId,
    ) -> std::result::Result<Arc<Browser>, ToolError> {
        let state = shared.db.state(session).await.ok();
        if let Some(state) = state.filter(|state| !state.incognito) {
            let browser = self.get(shared).await?;
            if state.browser_space == openside_proto::BrowserSpace::Isolated {
                // Archiving wiped the space; nothing may bring it back until unarchive. Checked
                // under the browser gate, which `close_space` also takes after the archive event.
                if state.archived {
                    return Err(ToolError(
                        "session_archived: this archived session's isolated browser space was wiped; unarchive the session to browse again".into(),
                    ));
                }
                self.ensure_space(shared, &browser, session).await?;
            }
            return Ok(browser);
        }
        let mut browsers = self.incognito.browsers.lock().await;
        if self.stopped.load(Ordering::Acquire) {
            return Err(ToolError("Browser is shutting down".into()));
        }
        if let Some(browser) = browsers.get(session.as_str()) {
            if browser.is_connected() {
                return Ok(browser.clone());
            }
            // A killed incognito browser is replaced by a fresh empty profile, never reused;
            // its tabs are reported lost like the shared browser's.
            if let Some(dead) = browsers.remove(session.as_str()) {
                self.forget_lost(shared, dead.clone()).await;
                terminate(dead).await;
            }
            remove(&root(shared).join(session.as_str())).await;
        }
        if !component(session.as_str()) {
            return Err(ToolError("Invalid session identifier".into()));
        }
        let executable = self
            .executable
            .clone()
            .ok_or_else(|| ToolError("Browser executable is unavailable".into()))?;
        let unavailable = |_| ToolError("Incognito browser profile is unavailable".into());
        let directory = root(shared).join(session.as_str());
        private_directory(&root(shared))
            .await
            .map_err(unavailable)?;
        private_directory(&directory).await.map_err(unavailable)?;
        let mut config = BrowserConfig::new(executable);
        config.profile_dir = Some(directory.join("profile"));
        // Not detached: the incognito Chromium never outlives its controller on purpose.
        let (browser, events) = match Browser::launch(config).await {
            Ok(launched) => launched,
            Err(error) => {
                remove(&directory).await;
                return Err(browser_error(error));
            }
        };
        self.install_navigation_policy(&browser, shared);
        // Lets the next daemon start terminate this Chromium if the daemon dies first.
        let endpoint = serde_json::to_vec(&browser.endpoint())
            .map_err(|_| ToolError("Incognito browser state could not be saved".into()))?;
        private_write(directory.join("endpoint.json"), endpoint, false)
            .await
            .map_err(|_| ToolError("Incognito browser state could not be saved".into()))?;
        let browser = Arc::new(browser);
        browsers.insert(session.as_str().to_owned(), browser.clone());
        self.forward_incognito(shared, session, Arc::downgrade(&browser), events);
        Ok(browser)
    }

    /// The running browser that holds a session's tabs, never launching one. Human-step
    /// targets (secret input, device approval) bind and fill there, so they work for
    /// incognito sessions whose tabs are not in the shared browser.
    pub(crate) async fn session_browser(&self, session: &SessionId) -> Option<Arc<Browser>> {
        let incognito = self
            .incognito
            .browsers
            .lock()
            .await
            .get(session.as_str())
            .cloned();
        match incognito {
            Some(browser) => Some(browser),
            None => self.existing().await.ok(),
        }
    }

    fn forward_incognito(
        &self,
        shared: &Shared,
        session: &SessionId,
        // Weak: the forwarder must not keep the browser alive, since `terminate` shuts it down
        // gracefully only as the last owner.
        browser: std::sync::Weak<Browser>,
        mut events: tokio::sync::mpsc::UnboundedReceiver<BrowserEvent>,
    ) {
        let shared = shared.clone();
        let owner = session.clone();
        let mut shutdown = shared.shutdown.subscribe();
        let task = tokio::spawn(async move {
            loop {
                let event = tokio::select! {
                    _ = shutdown.recv() => break,
                    event = events.recv() => match event { Some(event) => event, None => break },
                };
                // Every tab of this browser belongs to the one incognito session.
                if let BrowserEvent::DialogOpened { tab_id, kind, .. } = &event {
                    if let Some(browser) = browser.upgrade() {
                        let _ = browser
                            .handle_dialog(tab_id, crate::browser::accepts_dialog(kind), None)
                            .await;
                    }
                }
                if !matches!(event, BrowserEvent::TooManyTabs { .. }) {
                    let notice = crate::browser::steer_text(&event);
                    let _ = shared.emit(&owner, None, crate::browser_event(event)).await;
                    if let Some(text) = notice {
                        crate::browser::steer_notice(&shared, &owner, text).await;
                    }
                }
            }
        });
        if let Some(previous) = self
            .incognito
            .forwarders
            .lock()
            .insert(session.as_str().to_owned(), task)
        {
            previous.abort();
        }
    }

    /// Ends a session's incognito browser and deletes its profile directory.
    pub(crate) async fn close_incognito(&self, shared: &Shared, session: &SessionId) {
        // The gate drains tool calls that still hold this browser.
        let _gate = self.gate.lock().await;
        self.close_incognito_locked(shared, session.as_str()).await;
    }

    async fn close_incognito_locked(&self, shared: &Shared, session: &str) {
        if let Some(task) = self.incognito.forwarders.lock().remove(session) {
            task.abort();
        }
        let browser = self.incognito.browsers.lock().await.remove(session);
        if let Some(browser) = browser {
            terminate(browser).await;
        }
        if component(session) {
            remove(&root(shared).join(session)).await;
        }
    }

    /// Daemon shutdown: the caller holds the gate.
    pub(super) async fn close_all_incognito(&self, shared: &Shared) {
        let sessions: Vec<String> = self
            .incognito
            .browsers
            .lock()
            .await
            .keys()
            .cloned()
            .collect();
        for session in sessions {
            self.close_incognito_locked(shared, &session).await;
        }
        remove(&root(shared)).await;
    }

    /// Daemon start: profiles left by a crashed daemon are terminated and deleted. Only real
    /// directories directly under the incognito root are touched; a browser is shut down only
    /// when its recorded profile is exactly that directory's `profile`, so a planted
    /// `endpoint.json` or symlink can never stop or delete anything outside the root.
    pub(super) async fn sweep_incognito(&self, shared: &Shared) {
        let root = root(shared);
        match tokio::fs::symlink_metadata(&root).await {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                // Not ours to follow: drop the link (or file) itself.
                let _ = tokio::fs::remove_file(&root).await;
                return;
            }
            Err(_) => return,
        }
        let Ok(canonical_root) = tokio::fs::canonicalize(&root).await else {
            return;
        };
        let Ok(mut entries) = tokio::fs::read_dir(&root).await else {
            return;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let directory = entry.path();
            if !entry.file_type().await.is_ok_and(|kind| kind.is_dir()) {
                let _ = tokio::fs::remove_file(&directory).await;
                continue;
            }
            let profile = canonical_root.join(entry.file_name()).join("profile");
            if let Some(endpoint) = leftover_endpoint(&directory.join("endpoint.json")).await {
                let recorded = tokio::fs::canonicalize(&endpoint.profile_dir).await;
                let actual = tokio::fs::canonicalize(&profile).await;
                if real_directory(&profile).await
                    && matches!((&recorded, &actual), (Ok(recorded), Ok(actual)) if recorded == actual)
                {
                    // connect() also verifies pid, process group and the profile's live
                    // endpoint, so a reused pid is never signalled.
                    if let Ok((browser, _events)) = Browser::connect(endpoint).await {
                        let _ = browser.shutdown().await;
                    }
                }
            }
            // remove_dir_all unlinks symlinks inside instead of following them.
            remove(&directory).await;
        }
    }
}

async fn leftover_endpoint(path: &Path) -> Option<BrowserEndpoint> {
    if !tokio::fs::symlink_metadata(path)
        .await
        .is_ok_and(|metadata| metadata.is_file())
    {
        return None;
    }
    serde_json::from_slice(&tokio::fs::read(path).await.ok()?).ok()
}
