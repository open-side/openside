use super::*;

#[derive(Default)]
pub(super) struct DocumentEpoch {
    current: u64,
    next: u64,
    pending: Option<(String, u64)>,
    loader: String,
}

impl network::Tracker {
    pub(super) fn begin_document(&self, tab: &str, request: &str) {
        let mut epochs = self.request_epochs.lock();
        let epoch = epochs.entry(tab.into()).or_default();
        if epoch
            .pending
            .as_ref()
            .is_some_and(|(known, _)| known == request)
        {
            return;
        }
        epoch.next += 1;
        epoch.pending = Some((request.into(), epoch.next));
    }

    pub(super) fn commit_document(&self, tab: &str, loader: &str) {
        let mut epochs = self.request_epochs.lock();
        let epoch = epochs.entry(tab.into()).or_default();
        if loader == epoch.loader {
            return;
        }
        epoch.current = match epoch.pending.take() {
            Some((_, pending)) => pending,
            None => {
                epoch.next += 1;
                epoch.next
            }
        };
        epoch.loader = loader.into();
    }

    pub(super) fn document_epoch(&self, tab: &str, top_navigation: bool) -> u64 {
        self.request_epochs.lock().get(tab).map_or(0, |epoch| {
            if top_navigation {
                epoch
                    .pending
                    .as_ref()
                    .map_or(epoch.current, |(_, pending)| *pending)
            } else {
                epoch.current
            }
        })
    }

    pub(super) fn owner_restricted(&self, owner: &str) -> bool {
        !self.protected_sites(owner, false).is_empty()
    }

    pub(super) fn protected_sites(&self, owner: &str, force: bool) -> Vec<String> {
        let check = self
            .connection_restrictions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        check.map_or_else(Vec::new, |check| check(owner, force))
    }

    pub(super) fn effective_bypass(&self, session: &str) -> bool {
        self.request_modes
            .lock()
            .get(session)
            .copied()
            .unwrap_or(false)
            || self
                .service_worker_bypass
                .lock()
                .get(session)
                .copied()
                .unwrap_or(false)
    }
}

pub(super) fn session_tab<'a>(state: &'a State, session: &str) -> Option<&'a str> {
    state
        .sessions
        .iter()
        .find(|(_, known)| *known == session)
        .map(|(tab, _)| tab.as_str())
        .or_else(|| {
            state
                .oopif_sessions
                .values()
                .chain(state.worker_sessions.values())
                .find(|(_, known)| known == session)
                .map(|(tab, _)| tab.as_str())
        })
}

pub(super) async fn arm_session(
    inner: &Inner,
    session: &str,
    restricted: bool,
    reset: bool,
) -> Result<(), BrowserError> {
    let previous = inner.network.request_modes.lock().get(session).copied();
    if !reset && previous == Some(restricted) {
        return Ok(());
    }
    let worker = inner
        .state
        .lock()
        .await
        .worker_sessions
        .values()
        .any(|(_, known)| known == session);
    if worker {
        inner
            .network
            .request_modes
            .lock()
            .insert(session.into(), restricted);
        return refresh_worker_fence(inner).await;
    }
    if restricted {
        if !worker {
            inner
                .cdp
                .call(
                    Some(session),
                    "Network.setBypassServiceWorker",
                    json!({"bypass":true}),
                )
                .await?;
        }
        inner
            .cdp
            .call(
                Some(session),
                "Fetch.enable",
                json!({"patterns":[{"urlPattern":"*","requestStage":"Request"}]}),
            )
            .await?;
    } else if reset || previous == Some(true) {
        inner
            .cdp
            .call(Some(session), "Fetch.disable", json!({}))
            .await?;
        let bypass = inner
            .network
            .service_worker_bypass
            .lock()
            .get(session)
            .copied()
            .unwrap_or(false);
        if !worker {
            inner
                .cdp
                .call(
                    Some(session),
                    "Network.setBypassServiceWorker",
                    json!({"bypass":bypass}),
                )
                .await?;
        }
    }
    inner
        .network
        .request_modes
        .lock()
        .insert(session.into(), restricted);
    Ok(())
}

pub(super) async fn refresh_worker_fence(inner: &Inner) -> Result<(), BrowserError> {
    let _fence = inner.network.worker_fence_gate.lock().await;
    let sites = {
        let state = inner.state.lock().await;
        let modes = inner.network.request_modes.lock();
        let mut sites = std::collections::BTreeSet::new();
        for (tab, session) in state.worker_sessions.values() {
            if modes.get(session) != Some(&true) {
                continue;
            }
            if let Some(owner) = state
                .tabs
                .get(tab)
                .and_then(|tab| tab.owner_session_id.as_deref())
            {
                sites.extend(
                    inner
                        .network
                        .protected_sites(owner, !inner.network.owner_restricted(owner)),
                );
            }
        }
        sites
    };
    let active = !sites.is_empty();
    if !active && !inner.network.worker_fence_active.load(Ordering::Acquire) {
        return Ok(());
    }
    let mut patterns = vec![
        json!({"urlPattern":"*","resourceType":DOCUMENT_INTERCEPTION,"requestStage":"Request"}),
    ];
    for site in sites {
        if site == "*" {
            patterns.push(json!({"urlPattern":"*","requestStage":"Request"}));
            continue;
        }
        let host = if site.contains(':') {
            format!("[{site}]")
        } else {
            site
        };
        for url in [
            format!("*://{host}/*"),
            format!("*://{host}:*/*"),
            format!("*://*.{host}/*"),
            format!("*://*.{host}:*/*"),
        ] {
            patterns.push(json!({"urlPattern":url,"requestStage":"Request"}));
        }
    }
    inner
        .cdp
        .call(None, "Fetch.enable", json!({"patterns":patterns}))
        .await?;
    inner
        .network
        .worker_fence_active
        .store(active, Ordering::Release);
    Ok(())
}

pub(super) async fn observe_request(inner: &Arc<Inner>, event: &Value) {
    if !inner.network.worker_fence_active.load(Ordering::Acquire)
        || event["method"] != "Network.requestWillBeSent"
    {
        return;
    }
    let (Some(session), Some(id)) = (
        event["sessionId"].as_str(),
        event["params"]["requestId"].as_str(),
    ) else {
        return;
    };
    let target = {
        let state = inner.state.lock().await;
        session_tab(&state, session).map(str::to_owned)
    };
    if let Some(target) = target {
        let mut targets = inner.network.worker_request_targets.lock();
        let mut order = inner.network.worker_request_order.lock();
        if !targets.contains_key(id) {
            order.push_back(id.into());
        }
        targets.insert(id.into(), target);
        while order.len() > 512 {
            if let Some(id) = order.pop_front() {
                targets.remove(&id);
            }
        }
    }
}

impl Browser {
    /// Re-arm existing targets before publishing a grant downgrade; no lock crosses CDP.
    pub async fn rearm_connection_requests(
        &self,
        owner: Option<&str>,
        force: bool,
    ) -> Result<(), BrowserError> {
        let sessions: Vec<_> = {
            let state = self.inner.state.lock().await;
            state
                .sessions
                .values()
                .chain(state.oopif_sessions.values().map(|(_, session)| session))
                .chain(state.worker_sessions.values().map(|(_, session)| session))
                .filter_map(|session| {
                    let tab = state.tabs.get(requests::session_tab(&state, session)?)?;
                    let principal = tab.owner_session_id.as_deref()?;
                    if owner.is_some_and(|owner| owner != principal) {
                        return None;
                    }
                    Some((
                        session.clone(),
                        force || self.inner.network.owner_restricted(principal),
                    ))
                })
                .collect()
        };
        for (session, restricted) in sessions {
            requests::arm_session(&self.inner, &session, restricted, false).await?;
        }
        requests::refresh_worker_fence(&self.inner).await?;
        Ok(())
    }

    /// Existing documents cannot retain authenticated state after their permission is removed.
    pub async fn close_connection_tabs(
        &self,
        owner: Option<&str>,
        matches: impl Fn(&str) -> bool,
    ) -> Result<(), BrowserError> {
        for tab in self.tabs().await {
            let Some(principal) = tab.owner_session_id.as_deref() else {
                continue;
            };
            if owner.is_some_and(|owner| owner != principal) {
                continue;
            }
            let same_site = matches(&tab.url)
                || self
                    .frames(principal, &tab.id)
                    .await?
                    .iter()
                    .any(|frame| matches(&frame.url));
            if same_site {
                self.close_agent_tab(principal, &tab.id).await?;
            }
        }
        Ok(())
    }

    /// The current committed document, never another tab or a pending redirect's predecessor.
    pub async fn connection_document_epoch(
        &self,
        owner: &str,
        tab: &str,
    ) -> Result<u64, BrowserError> {
        self.check_owned(owner, tab).await?;
        Ok(self.inner.network.document_epoch(tab, false))
    }

    pub(super) async fn restore_connection_requests(
        &self,
        session: &str,
    ) -> Result<(), BrowserError> {
        let restricted = {
            let state = self.inner.state.lock().await;
            requests::session_tab(&state, session)
                .and_then(|tab| state.tabs.get(tab))
                .and_then(|tab| tab.owner_session_id.as_deref())
                .is_some_and(|owner| self.inner.network.owner_restricted(owner))
        };
        requests::arm_session(&self.inner, session, restricted, true).await
    }
}
