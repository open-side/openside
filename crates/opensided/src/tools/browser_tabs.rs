//! PP-23 ② and B-7 ③ ④ for the daemon's browsers.
//!
//! Crash recovery: a browser whose CDP transport closed (Chromium was killed or crashed) is
//! replaced on the next use or poll by a relaunch on the same managed profile. Its in-memory
//! ledger still names the agent tabs that died with it: each owner session receives
//! `tab.closed` for them plus a steer notice, and a later call naming one of those tabs is
//! refused with `browser_restarted` so the agent opens a new tab instead of retrying a dead
//! id. A browser that keeps dying soon after each relaunch is relaunched with a bounded
//! exponential backoff instead of in a tight loop; calls in the wait get `browser_restarting`.
//!
//! Sleeping tabs: on the patched browser the gateway reports the browser's own tab state
//! (`live`/`asleep`), which the poll copies into the ledger so `TabAsleep` follows real
//! discards. Stock Chromium exposes no discard state over CDP; there only the ledger marks.
//!
//! Agent Tabs group: on the patched browser every agent-created tab (including tabs opened
//! for external agents through `POST /v1/sessions/{id}/repl`, the MCP/CLI path) sits in the
//! browser's "Agent Tabs" tab group; `browser_tabs` reports the gateway's group title. Stock
//! Chromium has no tab groups over CDP, so `group` is null there and grouping is the
//! per-session ownership ledger alone.

use super::*;
use openside_browser::{GatewayTab, TabInfo};
use std::time::{Duration, Instant};

/// The fixed reason prefixes the model and clients match on.
const BROWSER_RESTARTED: &str = "browser_restarted";
const BROWSER_RESTARTING: &str = "browser_restarting";

/// A browser that dies sooner than this after its launch counts toward a crash loop.
const QUICK_DEATH: Duration = Duration::from_secs(30);
const FIRST_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Relaunch bookkeeping for the shared browser. Any first death relaunches at once; a
/// relaunched browser that dies again soon doubles the wait each time, up to `MAX_BACKOFF`.
#[derive(Default)]
pub(super) struct Relaunch {
    relaunched_at: Option<Instant>,
    quick_deaths: u32,
    not_before: Option<Instant>,
    pending: bool,
}

impl Relaunch {
    pub(super) fn died(&mut self, now: Instant) {
        let quick = self
            .relaunched_at
            .is_some_and(|at| now.saturating_duration_since(at) < QUICK_DEATH);
        self.quick_deaths = if quick {
            self.quick_deaths.saturating_add(1)
        } else {
            0
        };
        self.not_before = (self.quick_deaths > 0).then(|| now + backoff(self.quick_deaths));
        self.pending = true;
    }

    /// A failed launch is a browser that died immediately.
    pub(super) fn launched(&mut self, now: Instant, ok: bool) {
        if self.pending {
            self.relaunched_at = Some(now);
        }
        if ok {
            self.pending = false;
        } else {
            self.died(now);
        }
    }

    fn wait(&self, now: Instant) -> Option<Duration> {
        self.not_before
            .map(|at| at.saturating_duration_since(now))
            .filter(|wait| !wait.is_zero())
    }
}

fn backoff(quick_deaths: u32) -> Duration {
    FIRST_BACKOFF
        .saturating_mul(1 << quick_deaths.saturating_sub(1).min(16))
        .min(MAX_BACKOFF)
}

impl Controller {
    /// Called with the browser slot locked, after the cached browser lost its transport.
    pub(super) async fn forget_dead_browser(&self, shared: &Shared, dead: Arc<Browser>) {
        self.forget_lost(shared, dead).await;
        self.asleep.lock().await.clear();
    }

    /// Records every owned tab of a dead browser as lost and tells its owner session.
    pub(super) async fn forget_lost(&self, shared: &Shared, dead: Arc<Browser>) {
        let tabs = dead.tabs().await;
        drop(dead);
        let mut lost = self.lost_tabs.lock().await;
        let mut snapshots = self.snapshots.lock().await;
        let mut by_owner: HashMap<String, Vec<String>> = HashMap::new();
        for tab in tabs {
            let Some(owner) = tab.owner_session_id else {
                continue;
            };
            snapshots.remove(&tab.id);
            lost.insert(tab.id.clone(), owner.clone());
            by_owner.entry(owner).or_default().push(tab.id);
        }
        drop(snapshots);
        drop(lost);
        for (owner, tabs) in by_owner {
            let owner = SessionId::new(owner);
            for tab in &tabs {
                let _ = shared
                    .emit(
                        &owner,
                        None,
                        EventKind::TabClosed {
                            tab_id: tab.clone(),
                        },
                    )
                    .await;
            }
            // A run in flight learns at its next model request, before it retries a dead id.
            // Spawned: this runs under a browser lock, possibly inside that session's own
            // tool call, and must not wait on the session actor.
            let text = format!(
                "[browser event] {BROWSER_RESTARTED}: the browser stopped unexpectedly and was restarted; tabs {} are gone. Open a new tab with browser_open_tab.",
                tabs.join(", ")
            );
            let shared = shared.clone();
            tokio::spawn(async move { crate::browser::steer_notice(&shared, &owner, text).await });
        }
    }

    /// Refuses a relaunch while a crash-loop backoff is running.
    pub(super) fn relaunch_allowed(&self) -> std::result::Result<(), ToolError> {
        match self.relaunch.lock().wait(Instant::now()) {
            Some(wait) => Err(ToolError(format!(
                "{BROWSER_RESTARTING}: the browser stopped repeatedly and is relaunched in {} s; retry then.",
                wait.as_secs().max(1)
            ))),
            None => Ok(()),
        }
    }

    /// Relaunches the shared browser if it died, or once a crash-loop backoff has run out; a
    /// no-op while it is alive or was never started.
    pub(super) async fn revive(&self, shared: &Shared) {
        let dead = matches!(self.existing().await, Ok(browser) if !browser.is_connected());
        let pending = self.relaunch.lock().pending;
        if dead || pending {
            let _ = self.get(shared).await;
        }
    }

    /// Refuses a call that names a tab lost in a browser crash, telling the agent why. The
    /// entry is dropped once told; a later call gets the ordinary not-owned denial.
    pub(crate) async fn lost_tab(
        &self,
        session: &SessionId,
        args: &Value,
    ) -> std::result::Result<(), ToolError> {
        let Some(tab) = args.get("tab_id").and_then(Value::as_str) else {
            return Ok(());
        };
        let mut lost = self.lost_tabs.lock().await;
        if lost.get(tab).map(String::as_str) == Some(session.as_str()) {
            lost.remove(tab);
            return Err(ToolError(format!(
                "{BROWSER_RESTARTED}: the browser stopped unexpectedly and was restarted; tab {tab} is gone. Open a new tab with browser_open_tab."
            )));
        }
        Ok(())
    }

    /// Deletion clears transient tab state; stop/resume keeps the session's current tab.
    pub(crate) async fn forget_lost_tabs(&self, session: &SessionId) {
        self.last_tab.lock().remove(session);
        self.lost_tabs
            .lock()
            .await
            .retain(|_, owner| owner.as_str() != session.as_str());
    }
}

/// Group title per CDP target id, from the patched browser's gateway; empty on stock Chromium.
pub(super) async fn tab_groups(browser: &Browser) -> HashMap<String, String> {
    let Some(gateway) = browser.gateway() else {
        return HashMap::new();
    };
    let Ok(tabs) = gateway.tabs().await else {
        return HashMap::new();
    };
    tabs.tabs
        .into_iter()
        .filter_map(|tab| Some((tab.cdp_target_id?, tab.group_title?)))
        .collect()
}

/// Copies the patched browser's live/asleep tab state into the ledger, so a tab the browser
/// discarded is refused with `TabAsleep` and a tab it woke is usable again. Stock: no-op.
pub(super) async fn sync_gateway_sleep(browser: &Browser) {
    let Some(gateway) = browser.gateway() else {
        return;
    };
    let Ok(sample) = gateway.tabs().await else {
        return;
    };
    for (tab, asleep) in sleep_changes(&sample.tabs, &browser.tabs().await) {
        let _ = browser.mark_asleep(&tab, asleep, false).await;
    }
}

fn sleep_changes(gateway: &[GatewayTab], ledger: &[TabInfo]) -> Vec<(String, bool)> {
    ledger
        .iter()
        .filter_map(|tab| {
            let state = &gateway
                .iter()
                .find(|record| record.cdp_target_id.as_deref() == Some(tab.id.as_str()))?
                .state;
            let sleeping = tab.discarded || tab.unloaded;
            match state.as_str() {
                "asleep" if !sleeping => Some((tab.id.clone(), true)),
                "live" if sleeping => Some((tab.id.clone(), false)),
                _ => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use openside_browser::OpenedBy;

    fn ledger(id: &str, discarded: bool) -> TabInfo {
        TabInfo {
            id: id.into(),
            url: "about:blank".into(),
            title: String::new(),
            opened_by: OpenedBy::Agent,
            owner_session_id: Some("s".into()),
            discarded,
            unloaded: false,
        }
    }

    fn record(target: &str, state: &str) -> GatewayTab {
        serde_json::from_value(json!({
            "tab_id": format!("g-{target}"), "cdp_target_id": target, "owner": "agent",
            "session_id": "s", "subagent_id": null, "label": "", "background": true,
            "opened_by": "agent", "url": "about:blank", "title": "", "state": state,
        }))
        .unwrap()
    }

    #[test]
    fn gateway_sleep_state_marks_and_wakes_ledger_tabs() {
        let gateway = [
            record("discarded", "asleep"),
            record("woken", "live"),
            record("steady", "live"),
            record("closing", "closed"),
        ];
        let tabs = [
            ledger("discarded", false),
            ledger("woken", true),
            ledger("steady", false),
            ledger("closing", false),
            ledger("unknown-to-gateway", true),
        ];
        assert_eq!(
            sleep_changes(&gateway, &tabs),
            [("discarded".to_owned(), true), ("woken".to_owned(), false)]
        );
    }

    #[test]
    fn crash_loops_back_off_exponentially_up_to_the_cap() {
        let start = Instant::now();
        let mut relaunch = Relaunch::default();
        relaunch.launched(start, true);
        // The first death relaunches at once, even right after the initial launch.
        let early = start + Duration::from_secs(1);
        relaunch.died(early);
        assert_eq!(relaunch.wait(early), None);
        assert!(relaunch.pending);
        relaunch.launched(early, true);
        assert!(!relaunch.pending);
        // A relaunched browser that ran a long time also relaunches at once.
        let late = early + QUICK_DEATH + Duration::from_secs(1);
        relaunch.died(late);
        assert_eq!(relaunch.wait(late), None);
        relaunch.launched(late, true);
        // Each quick death doubles the wait: 1, 2, 4, 8, 16, then 30 s forever.
        let mut now = late;
        for expected in [1, 2, 4, 8, 16, 30, 30, 30] {
            now += Duration::from_millis(100);
            relaunch.died(now);
            assert_eq!(relaunch.wait(now), Some(Duration::from_secs(expected)));
            now += Duration::from_secs(expected);
            assert_eq!(
                relaunch.wait(now),
                None,
                "the wait is over after {expected} s"
            );
            relaunch.launched(now, true);
        }
        // A failed launch counts as a quick death.
        relaunch.launched(now, false);
        assert_eq!(relaunch.wait(now), Some(MAX_BACKOFF));
    }
}
