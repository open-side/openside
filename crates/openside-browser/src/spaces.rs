//! Isolated spaces: an owner session's agent tabs in a CDP browser context of the same
//! managed Chromium (`Target.createBrowserContext`), with its own cookies, site storage and
//! cache, all in memory. Its downloads go to a private per-space directory,
//! `<profile>/space-downloads/<context>` (0700), deleted with the space. Contexts are created
//! with `disposeOnDetach: false`: a controller reconnect (daemon restart) keeps them, a browser
//! restart loses them.
//!
//! Lifecycle invariants:
//! - An owner that ever had a space here is isolated for the life of this controller. Its tabs
//!   are never created in the default context: with no bound space, creation is refused with
//!   [`BrowserError::SpaceClosed`] (a closed space is not a permission to use the shared
//!   profile).
//! - Closing moves the context to `closing` before any CDP call and keeps it there until
//!   Chromium confirms the disposal, so a failed or cancelled close never forgets a live
//!   context; [`Browser::settle_spaces`] retries it.

use std::collections::HashSet;
use std::path::PathBuf;

use super::*;

impl Browser {
    /// The owner's isolated space, created on first use. Every later
    /// [`Browser::create_agent_tab`] of this owner, and every popup of those tabs, lives in it.
    pub async fn open_space(&self, owner: &str) -> Result<String, BrowserError> {
        // Held across the CDP calls so two first uses cannot create two contexts.
        let mut state = self.inner.state.lock().await;
        state.isolated.insert(owner.to_owned());
        if let Some(context) = state.spaces.get(owner) {
            return Ok(context.clone());
        }
        let created = self
            .inner
            .cdp
            .call(
                None,
                "Target.createBrowserContext",
                json!({"disposeOnDetach": false}),
            )
            .await?;
        let context = required_string(&created, "browserContextId")?.to_owned();
        state.isolated_contexts.insert(context.clone());
        // Tracked before anything else can fail, so an error below still disposes it.
        state.closing.insert(context.clone(), owner.to_owned());
        let downloads = self.space_downloads(&context);
        let behavior = match private_dir(&downloads).await {
            Ok(()) => self
                .inner
                .cdp
                .call(
                    None,
                    "Browser.setDownloadBehavior",
                    json!({"behavior":"allowAndName","browserContextId":context,"downloadPath":downloads,"eventsEnabled":true}),
                )
                .await
                .map(drop),
            Err(error) => Err(error.into()),
        };
        if let Err(error) = behavior {
            drop(state);
            let _ = self.settle_spaces().await;
            return Err(error);
        }
        state.closing.remove(&context);
        state.spaces.insert(owner.to_owned(), context.clone());
        Ok(context)
    }

    /// The owner's isolated space, if it has one.
    pub async fn space(&self, owner: &str) -> Option<String> {
        self.inner.state.lock().await.spaces.get(owner).cloned()
    }

    /// Every isolated space as `(owner, browser context id)`, for persistence.
    pub async fn spaces(&self) -> Vec<(String, String)> {
        let mut spaces: Vec<_> = self
            .inner
            .state
            .lock()
            .await
            .spaces
            .iter()
            .map(|(owner, context)| (owner.clone(), context.clone()))
            .collect();
        spaces.sort();
        spaces
    }

    /// Contexts whose disposal has not been confirmed yet (a failed or cancelled close).
    pub async fn closing_spaces(&self) -> usize {
        self.inner.state.lock().await.closing.len()
    }

    /// Dispose the owner's space: Chromium closes its tabs (each reported as
    /// [`BrowserEvent::TabClosed`]), drops its cookies and storage, and its downloads are
    /// deleted. False when the owner had none. On error the context stays in `closing` and
    /// [`Browser::settle_spaces`] retries it; the owner's tabs are refused meanwhile.
    pub async fn close_space(&self, owner: &str) -> Result<bool, BrowserError> {
        let had_space = {
            let mut state = self.inner.state.lock().await;
            if let Some(context) = state.spaces.remove(owner) {
                state.closing.insert(context, owner.to_owned());
                true
            } else {
                false
            }
        };
        self.clear_session_uploads(owner).await?;
        if !had_space {
            return Ok(false);
        }
        self.settle_spaces().await?;
        Ok(true)
    }

    /// After a reconnect: bind a saved space to its owner again. False, and nothing bound,
    /// when Chromium no longer has that context (the browser restarted). The owner is
    /// isolated either way, so a lost space is never replaced by the default context.
    pub async fn restore_space(&self, owner: &str, context: &str) -> Result<bool, BrowserError> {
        self.inner
            .state
            .lock()
            .await
            .isolated
            .insert(owner.to_owned());
        if !self
            .live_contexts()
            .await?
            .iter()
            .any(|live| live == context)
        {
            return Ok(false);
        }
        let mut state = self.inner.state.lock().await;
        if state.spaces.values().any(|bound| bound == context) {
            return Ok(false);
        }
        state.spaces.insert(owner.to_owned(), context.to_owned());
        if !state.isolated_contexts.contains(context) {
            state.isolated_contexts.insert(context.to_owned());
        }
        let State {
            tabs, tab_contexts, ..
        } = &mut *state;
        for (id, tab) in tabs.iter_mut() {
            if tab_contexts.get(id).map(String::as_str) == Some(context) {
                tab.browser_context_id = Some(context.to_owned());
                tab.owner_session_id = Some(owner.to_owned());
            }
        }
        Ok(true)
    }

    /// Dispose every CDP-created browser context that is not a bound space (a closing one, or
    /// one left by a controller that died between creating a space and saving it) and delete
    /// space download directories that belong to no bound space. Returns how many contexts
    /// were disposed. Runs under the ledger lock so a space being opened is never mistaken
    /// for a leftover.
    pub async fn prune_spaces(&self) -> Result<usize, BrowserError> {
        let mut state = self.inner.state.lock().await;
        // Due until a prune completes, so a failed one is retried by `settle_spaces`.
        state.prune_due = true;
        let live = self.live_contexts().await?;
        for context in &live {
            if !state.isolated_contexts.contains(context) {
                state.isolated_contexts.insert(context.clone());
            }
        }
        let bound: HashSet<String> = state.spaces.values().cloned().collect();
        let mut pruned = 0;
        for context in live.iter().filter(|context| !bound.contains(*context)) {
            self.dispose(context).await?;
            state.closing.remove(context);
            pruned += 1;
        }
        // Confirmed gone: Chromium no longer lists them.
        state.closing.retain(|context, _| live.contains(context));
        sweep_downloads(self.profile_path(), &bound).await?;
        state.prune_due = false;
        Ok(pruned)
    }

    /// Finish pending disposals ([`Browser::close_space`] that failed or was cancelled).
    /// Cheap when nothing is pending; meant for a periodic poll.
    pub async fn settle_spaces(&self) -> Result<(), BrowserError> {
        #[cfg(unix)]
        if !self.is_connected() && process_gone(self.process.group, &self.process.child) {
            // A closed transport alone is not proof that Chromium stopped writing files.
            self.wipe_space_downloads().await?;
            return Ok(());
        }
        if self.inner.state.lock().await.prune_due {
            return self.prune_spaces().await.map(drop);
        }
        let pending: Vec<String> = self
            .inner
            .state
            .lock()
            .await
            .closing
            .keys()
            .cloned()
            .collect();
        if pending.is_empty() {
            return Ok(());
        }
        let live = self.live_contexts().await?;
        for context in pending {
            if live.contains(&context) {
                self.dispose(&context).await?;
            }
            remove_tree(&self.space_downloads(&context)).await?;
            self.inner.state.lock().await.closing.remove(&context);
        }
        Ok(())
    }

    /// Browser contexts Chromium reports as created over CDP; the default context is never
    /// among them.
    pub(super) async fn live_contexts(&self) -> Result<Vec<String>, BrowserError> {
        let listed = self
            .inner
            .cdp
            .call(None, "Target.getBrowserContexts", json!({}))
            .await?;
        let Value::Array(contexts) = &listed["browserContextIds"] else {
            return Err(BrowserError::InvalidResponse("browserContextIds"));
        };
        Ok(contexts
            .iter()
            .filter_map(|context| context.as_str().map(str::to_owned))
            .collect())
    }

    /// Where a space's downloads land.
    pub fn space_downloads(&self, context: &str) -> PathBuf {
        self.profile_path().join(SPACE_DOWNLOADS).join(context)
    }

    /// Remove every private download after verified process death; no live space remains.
    pub(super) async fn wipe_space_downloads(&self) -> Result<(), BrowserError> {
        sweep_downloads(self.profile_path(), &HashSet::new()).await?;
        Ok(())
    }

    async fn dispose(&self, context: &str) -> Result<(), BrowserError> {
        // Disposal cancels the context's downloads before its directory is deleted.
        self.inner
            .cdp
            .call(
                None,
                "Target.disposeBrowserContext",
                json!({"browserContextId": context}),
            )
            .await?;
        remove_tree(&self.space_downloads(context)).await?;
        Ok(())
    }
}

const SPACE_DOWNLOADS: &str = "space-downloads";

pub(super) async fn sweep_downloads(
    profile: &std::path::Path,
    bound: &HashSet<String>,
) -> std::io::Result<()> {
    let root = profile.join(SPACE_DOWNLOADS);
    let mut entries = match tokio::fs::read_dir(&root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    while let Some(entry) = entries.next_entry().await? {
        if !bound.contains(entry.file_name().to_string_lossy().as_ref()) {
            remove_tree(&entry.path()).await?;
        }
    }
    Ok(())
}

#[cfg(unix)]
async fn private_dir(path: &std::path::Path) -> std::io::Result<()> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&path)
    })
    .await
    .map_err(std::io::Error::other)?
}

#[cfg(not(unix))]
async fn private_dir(_: &std::path::Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Private browser directories require Unix permissions",
    ))
}

async fn remove_tree(path: &std::path::Path) -> std::io::Result<()> {
    match tokio::fs::remove_dir_all(path).await {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    use super::*;

    #[tokio::test]
    async fn failed_disposal_keeps_existing_tabs_inaccessible_until_retry() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            let mut contexts = vec!["old"];
            let mut fail_dispose = true;
            while let Some(Ok(Message::Text(text))) = socket.next().await {
                let request: Value = serde_json::from_str(&text).unwrap();
                let result =
                    match request["method"].as_str().unwrap() {
                        "Target.getBrowserContexts" => json!({"browserContextIds":contexts}),
                        "Target.createBrowserContext" => {
                            contexts.push("new");
                            json!({"browserContextId":"new"})
                        }
                        "Browser.setDownloadBehavior" => json!({}),
                        "Target.disposeBrowserContext" if fail_dispose => {
                            fail_dispose = false;
                            socket.send(Message::Text(json!({
                            "id":request["id"],
                            "error":{"code":-32000,"message":"injected disposal failure"}
                        }).to_string().into())).await.unwrap();
                            continue;
                        }
                        "Target.disposeBrowserContext" => {
                            contexts.retain(|context| {
                                Some(*context) != request["params"]["browserContextId"].as_str()
                            });
                            json!({})
                        }
                        method => panic!("closed target reached CDP: {method}"),
                    };
                socket
                    .send(Message::Text(
                        json!({"id":request["id"],"result":result})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
            }
        });
        let (cdp, _notifications) = Cdp::connect(&url, Duration::from_secs(2)).await.unwrap();
        let (events, _receiver) = mpsc::unbounded_channel();
        let mut state = State::default();
        state.isolated.insert("alpha".into());
        state.spaces.insert("alpha".into(), "old".into());
        state.tab_contexts.insert("tab".into(), "old".into());
        state.tabs.insert(
            "tab".into(),
            TabInfo {
                id: "tab".into(),
                url: "http://localhost/".into(),
                title: String::new(),
                opened_by: OpenedBy::Agent,
                owner_session_id: Some("alpha".into()),
                discarded: false,
                unloaded: false,
                browser_context_id: Some("old".into()),
            },
        );
        let profile = tempfile::tempdir().unwrap();
        let browser = Browser {
            inner: Arc::new(Inner {
                cdp,
                state: Mutex::new(state),
                attach_lock: Mutex::default(),
                capture_lock: Arc::default(),
                interaction_lock: Mutex::default(),
                network: Default::default(),
                attach_timeout: Duration::from_secs(2),
                events,
                navigation_policy: std::sync::RwLock::new(None),
                screencasts: Default::default(),
                page: Default::default(),
            }),
            process: Process {
                child: parking_lot::Mutex::new(None),
                group: Pid::from_raw(i32::MAX),
                profile: profile.path().to_owned(),
                temporary_profile: None,
                stderr: None,
                detached: true,
                running: false,
            },
            ws_url: url,
            event_task: tokio::spawn(std::future::pending()),
            gateway: None,
            gateway_dir: None,
            isolated: true,
        };
        browser.check_owned("alpha", "tab").await.unwrap();
        assert!(matches!(
            browser.close_space("alpha").await,
            Err(BrowserError::Cdp { .. })
        ));
        assert_eq!(browser.closing_spaces().await, 1);
        let node = NodeHandle {
            backend_node_id: 1,
            document_id: "old-document".into(),
            origin: "http://localhost".into(),
        };
        for replaced in [false, true] {
            if replaced {
                assert_eq!(browser.open_space("alpha").await.unwrap(), "new");
            }
            assert!(matches!(
                browser.check_owned("alpha", "tab").await,
                Err(BrowserError::OwnershipDenied(_))
            ));
            assert!(matches!(
                browser.navigate("alpha", "tab", "http://localhost/").await,
                Err(BrowserError::OwnershipDenied(_))
            ));
            assert!(matches!(
                browser
                    .insert_text_node("alpha", "tab", &node, "must-not-fill")
                    .await,
                Err(BrowserError::TextInsertionFailed)
            ));
            assert!(matches!(
                browser.thumbnail("alpha", "tab").await,
                Err(BrowserError::OwnershipDenied(_))
            ));
            assert!(matches!(
                browser.close_agent_tab("alpha", "tab").await,
                Err(BrowserError::OwnershipDenied(_))
            ));
        }
        browser.settle_spaces().await.unwrap();
        assert_eq!(browser.closing_spaces().await, 0);
        assert_eq!(browser.space("alpha").await.as_deref(), Some("new"));
        browser.close_space("alpha").await.unwrap();
        drop(browser);
        server.abort();
    }
}
