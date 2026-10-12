//! Isolated browser spaces for sessions created with `browser_space: "isolated"`.
//!
//! The session's agent tabs live in its own CDP browser context inside the shared managed
//! Chromium ([`Browser::open_space`]): own cookies, storage and cache, in memory only, while
//! policy, approvals, the vault, takeover, preview and screencast work on its tabs exactly as
//! on shared tabs (they address tabs by target id in the same browser). Browsing history is
//! not indexed into memory (`memory::index_page`).
//!
//! Lifetime: the space is opened on the session's first browser use and disposed (tabs closed,
//! data wiped) when the session is archived or deleted. A daemon restart keeps it while the
//! detached browser survives; spaces are saved with the browser state and restored before
//! their tabs are re-adopted, and any other CDP context is pruned. When the browser itself
//! ends (crash, kill, or a clean daemon stop that shuts Chromium down), the space is gone:
//! the session gets a steer notice at once and its next browser call is refused once with
//! `browser_space_reset`, after which it continues in a fresh, empty space.

use super::*;
use openside_proto::BrowserSpace;

const BROWSER_SPACE_RESET: &str = "browser_space_reset";

#[derive(Deserialize, Serialize)]
pub(super) struct SavedSpace {
    pub(super) owner: String,
    pub(super) context: String,
}

fn reset_text() -> String {
    format!(
        "{BROWSER_SPACE_RESET}: the browser restarted and this session's isolated browser space was lost with its cookies, logins and tabs. A new empty space is ready; sign in again where needed and open a new tab with browser_open_tab."
    )
}

impl Controller {
    /// Opens the session's space in the shared browser if it has none yet. Refuses the call
    /// once when an earlier space of this session died with the browser.
    pub(super) async fn ensure_space(
        &self,
        shared: &Shared,
        browser: &Browser,
        session: &SessionId,
    ) -> std::result::Result<(), ToolError> {
        // Browser retrieval can await a launch while archive/deletion publishes its state.
        let state = shared
            .db
            .state(session)
            .await
            .map_err(|_| ToolError("session_unavailable: session no longer exists".into()))?;
        if state.deleted {
            return Err(ToolError("session_unavailable: session was deleted".into()));
        }
        if state.archived {
            return Err(ToolError("session_archived: this archived session's isolated browser space was wiped; unarchive the session to browse again".into()));
        }
        if browser.space(session.as_str()).await.is_some() {
            return Ok(());
        }
        browser
            .open_space(session.as_str())
            .await
            .map_err(browser_error)?;
        // Saved before any tab exists, so a daemon crash cannot leave an unknown context
        // behind (recovery also prunes unknown contexts).
        self.persist(shared)
            .await
            .map_err(|_| ToolError("Browser state could not be saved".into()))?;
        if self.reset_spaces.lock().remove(session) {
            return Err(ToolError(reset_text()));
        }
        Ok(())
    }

    /// Archive or deletion: dispose the session's space, closing its tabs and wiping its data.
    /// The wipe is guaranteed eventually rather than reported: a disposal that fails here stays
    /// pending in the browser, the session's tabs are refused meanwhile, and the 2-second poll
    /// retries it (and the next start prunes it if the daemon stops first).
    pub(crate) async fn close_space(&self, shared: &Shared, session: &SessionId) {
        // The gate drains tool calls that still act in the space; later calls see the archive
        // or tombstone and are refused before they reach the browser (`for_session`).
        let _gate = self.gate.lock().await;
        self.reset_spaces.lock().remove(session);
        let Ok(browser) = self.existing().await else {
            return;
        };
        match browser.close_space(session.as_str()).await {
            Ok(true) => {
                let _ = self.persist(shared).await;
            }
            Ok(false) => {}
            Err(error) => {
                // Not forgotten: the context stays pending and its owner's tabs are refused.
                let _ = self.persist(shared).await;
                crate::diag!("opensided: isolated space disposal pending a retry: {error}");
            }
        }
    }

    /// Poll: retry disposals and leftover-context pruning that failed earlier.
    pub(super) async fn settle_spaces(&self, _shared: &Shared, browser: &Browser) {
        if let Err(error) = browser.settle_spaces().await {
            crate::diag!("opensided: isolated space disposal still pending: {error}");
        }
    }

    /// Reconnect: rebind saved spaces of live isolated sessions, report the ones Chromium no
    /// longer has, and dispose every other context.
    pub(super) async fn restore_spaces(
        &self,
        shared: &Shared,
        browser: &Browser,
        saved: Vec<SavedSpace>,
    ) {
        let mut lost = Vec::new();
        for space in saved {
            let owner = SessionId::new(&space.owner);
            if !isolated(shared, &owner).await {
                continue;
            }
            if !matches!(
                browser.restore_space(&space.owner, &space.context).await,
                Ok(true)
            ) {
                lost.push(owner);
            }
        }
        // A failure stays due in the browser and is retried by the poll (`settle_spaces`).
        if let Err(error) = browser.prune_spaces().await {
            crate::diag!("opensided: leftover browser contexts could not be pruned yet: {error}");
        }
        self.spaces_lost(shared, lost);
    }

    /// Start after the saved browser was found dead: its spaces ended with it.
    pub(super) async fn spaces_lost_at_start(&self, shared: &Shared, saved: Vec<SavedSpace>) {
        let mut lost = Vec::new();
        for space in saved {
            let owner = SessionId::new(space.owner);
            if isolated(shared, &owner).await {
                lost.push(owner);
            }
        }
        self.spaces_lost(shared, lost);
    }

    /// A dead browser's spaces: tell their sessions now and refuse their next call once.
    pub(super) async fn forget_dead_spaces(&self, shared: &Shared, dead: &Browser) {
        let owners = dead
            .spaces()
            .await
            .into_iter()
            .map(|(owner, _)| SessionId::new(owner))
            .collect();
        self.spaces_lost(shared, owners);
    }

    fn spaces_lost(&self, shared: &Shared, owners: Vec<SessionId>) {
        for owner in owners {
            self.reset_spaces.lock().insert(owner.clone());
            // Spawned: callers may hold browser locks inside that session's own tool call.
            let shared = shared.clone();
            tokio::spawn(async move {
                crate::browser::steer_notice(
                    &shared,
                    &owner,
                    format!("[browser event] {}", reset_text()),
                )
                .await;
            });
        }
    }
}

/// A live (not deleted or archived) session created with `browser_space: isolated`.
async fn isolated(shared: &Shared, session: &SessionId) -> bool {
    shared.db.state(session).await.is_ok_and(|state| {
        state.browser_space == BrowserSpace::Isolated && !state.deleted && !state.archived
    })
}
