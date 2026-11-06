//! Agent tab cleanup (`preferences.agent_tab_cleanup_minutes`): tabs an agent opened close
//! once their chat has been idle for that many minutes; 0 keeps them open.
//!
//! A chat is idle when it is not running, has no background work and no pending human step,
//! and neither its event log nor a tool call (agent, REPL or `/v1/.../tools` caller) has
//! touched it for the whole period. Only `opened_by: agent` tabs are closed: tabs the person
//! adopted, popups and the person's own browser are never touched, and neither is a tab under
//! a takeover or held by a running Task. Tool calls don't append events, so their use is
//! tracked here ([`in_use`]).

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock},
    time::Duration,
};

use chrono::{DateTime, Utc};
use openside_browser::{Browser, OpenedBy};
use openside_proto::{SessionId, SessionStatus, TicketStatus};
use openside_sessions::SessionState;

use crate::engine::Shared;

const SWEEP_EVERY: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Default)]
struct ToolUse {
    running: usize,
    last: Option<DateTime<Utc>>,
}

static TOOL_USE: LazyLock<parking_lot::Mutex<HashMap<SessionId, ToolUse>>> =
    LazyLock::new(Default::default);

/// Marks a tool call of `session` as running until dropped; the drop time counts as its
/// latest use.
pub(crate) struct InUse(SessionId);

pub(crate) fn in_use(session: &SessionId) -> InUse {
    let mut uses = TOOL_USE.lock();
    let entry = uses.entry(session.clone()).or_default();
    entry.running += 1;
    entry.last = Some(Utc::now());
    InUse(session.clone())
}

impl Drop for InUse {
    fn drop(&mut self) {
        if let Some(entry) = TOOL_USE.lock().get_mut(&self.0) {
            entry.running = entry.running.saturating_sub(1);
            entry.last = Some(Utc::now());
        }
    }
}

fn tool_use(session: &SessionId) -> ToolUse {
    TOOL_USE.lock().get(session).copied().unwrap_or_default()
}

/// Whether a chat with these facts has been idle for `minutes` at `now`.
pub(crate) fn due(
    now: DateTime<Utc>,
    minutes: u32,
    state: &SessionState,
    tool_running: bool,
    tool_last: Option<DateTime<Utc>>,
) -> bool {
    if minutes == 0
        || tool_running
        || state.deleted
        || state.status == SessionStatus::Running
        || !state.active_subagents.is_empty()
        || state
            .tickets
            .values()
            .any(|ticket| ticket.status == TicketStatus::Pending)
    {
        return false;
    }
    let idle_since = tool_last.map_or(state.last_activity, |last| last.max(state.last_activity));
    now.signed_duration_since(idle_since) >= chrono::Duration::minutes(i64::from(minutes))
}

fn session_due(now: DateTime<Utc>, minutes: u32, state: &SessionState) -> bool {
    let used = tool_use(&state.id);
    due(now, minutes, state, used.running > 0, used.last)
}

pub(crate) fn start(shared: &Shared) {
    let shared = shared.clone();
    let mut shutdown = shared.shutdown.subscribe();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SWEEP_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = shutdown.recv() => break,
                _ = tick.tick() => { sweep(&shared, Utc::now()).await; }
            }
        }
    });
}

/// Closes the agent tabs of every chat idle at `now`; returns how many closed.
pub(crate) async fn sweep(shared: &Shared, now: DateTime<Utc>) -> usize {
    let minutes = shared.preferences.get().agent_tab_cleanup_minutes;
    let Ok((due_sessions, known)) = shared
        .db
        .store(move |db| {
            let mut due_sessions = Vec::new();
            let mut known = HashSet::new();
            for session in db.list_sessions() {
                let Some(state) = db.session(&session.id) else {
                    continue;
                };
                known.insert(session.id.clone());
                if session_due(now, minutes, state) {
                    due_sessions.push(session.id);
                }
            }
            Ok((due_sessions, known))
        })
        .await
    else {
        return 0;
    };
    TOOL_USE
        .lock()
        .retain(|id, used| used.running > 0 || known.contains(id));
    if minutes == 0 || !shared.registry.browser.available() {
        return 0;
    }
    let mut closed = 0;
    for session in due_sessions {
        closed += close_session_tabs(shared, &session, now).await;
    }
    closed
}

async fn close_session_tabs(shared: &Shared, session: &SessionId, now: DateTime<Utc>) -> usize {
    let registry = &shared.registry.browser;
    // Browser tools hold this gate for their whole call, so no agent operation is mid-flight
    // on these tabs while they close.
    let _gate = registry.gate.lock().await;
    // The person may have turned cleanup off or lengthened it, and a message or tool call may
    // have arrived, while this waited for the gate; only the live setting and state decide.
    let live = shared.preferences.get().agent_tab_cleanup_minutes;
    match shared.db.state(session).await {
        Ok(state) if session_due(now, live, &state) => {}
        _ => return 0,
    }
    let mut tabs: Vec<(Arc<Browser>, String)> = Vec::new();
    if let Some(browser) = registry.session_browser(session).await {
        for tab in browser.tabs().await {
            if tab.opened_by == OpenedBy::Agent
                && tab.owner_session_id.as_deref() == Some(session.as_str())
            {
                tabs.push((browser.clone(), tab.id));
            }
        }
    }
    for tab in registry.accounts.tabs(session.as_str()).await {
        if tab.opened_by != OpenedBy::Agent {
            continue;
        }
        if let Some(browser) = registry.accounts.for_tab(session.as_str(), &tab.id).await {
            tabs.push((browser, tab.id));
        }
    }
    let mut closed = 0;
    for (browser, tab) in tabs {
        if registry.task_tabs.holder(session, &tab).is_some()
            || crate::takeover::blocked(
                shared,
                session,
                "browser_close_tab",
                &serde_json::json!({ "tab_id": tab }),
            )
            .await
            .is_some()
        {
            continue;
        }
        if browser
            .close_agent_tab(session.as_str(), &tab)
            .await
            .is_ok()
        {
            registry.forget_captures(&tab).await;
            registry.retire_warm(session, &tab);
            closed += 1;
        }
    }
    if closed > 0 {
        let _ = registry.persist(shared).await;
    }
    closed
}

#[cfg(test)]
mod tests {
    use super::*;
    use openside_proto::PolicyMode;

    fn state(at: DateTime<Utc>) -> SessionState {
        let mut store = openside_sessions::Store::open(":memory:").unwrap();
        let id = SessionId::new("s");
        store
            .create_session(id.clone(), "chat", "m", PolicyMode::Guard, None)
            .unwrap();
        let mut state = store.session(&id).unwrap().clone();
        state.last_activity = at;
        state
    }

    fn at(minute: i64) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap()
            + chrono::Duration::minutes(minute)
    }

    #[test]
    fn idle_chat_is_due_only_after_the_chosen_minutes() {
        let idle = state(at(0));
        assert!(!due(at(14), 15, &idle, false, None));
        assert!(due(at(15), 15, &idle, false, None));
        assert!(due(at(500), 15, &idle, false, None));
    }

    #[test]
    fn zero_minutes_never_closes() {
        assert!(!due(at(10_000), 0, &state(at(0)), false, None));
    }

    #[test]
    fn a_recent_or_running_tool_call_keeps_tabs_open() {
        let idle = state(at(0));
        assert!(!due(at(20), 15, &idle, false, Some(at(10))));
        assert!(due(at(25), 15, &idle, false, Some(at(10))));
        assert!(!due(at(500), 15, &idle, true, Some(at(0))));
    }

    #[test]
    fn working_or_waiting_chats_are_never_due() {
        let mut running = state(at(0));
        running.status = SessionStatus::Running;
        assert!(!due(at(500), 15, &running, false, None));

        let mut background = state(at(0));
        background.active_subagents.insert("task".into());
        assert!(!due(at(500), 15, &background, false, None));

        let mut waiting = state(at(0));
        let ticket: openside_proto::Ticket = serde_json::from_value(serde_json::json!({
            "id": "t", "session_id": "s", "tool_call_id": "call", "deferral": "external",
            "kind": "info", "summary": "A human step", "instructions": null, "options": [],
            "origin": {"subagent_id": null, "device_id": null, "driver_ids": []},
            "tool": null, "target": null, "resume_when": null,
            "created_at": "2026-01-01T00:00:00Z", "expires_at": "2030-01-01T00:00:00Z",
            "status": "pending", "resolution": null,
        }))
        .unwrap();
        waiting.tickets.insert(ticket.id.clone(), ticket);
        assert!(!due(at(500), 15, &waiting, false, None));

        let mut deleted = state(at(0));
        deleted.deleted = true;
        assert!(!due(at(500), 15, &deleted, false, None));
    }

    #[test]
    fn tool_use_guard_counts_running_calls_and_stamps_the_end() {
        let session = SessionId::new("tab-cleanup-guard");
        let first = in_use(&session);
        let second = in_use(&session);
        assert_eq!(tool_use(&session).running, 2);
        drop(first);
        assert_eq!(tool_use(&session).running, 1);
        drop(second);
        let used = tool_use(&session);
        assert_eq!(used.running, 0);
        assert!(used.last.is_some());
        TOOL_USE.lock().remove(&session);
    }
}
