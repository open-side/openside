//! Script-facing CDP stays within an owned page session; global authority stays in the host.

use serde_json::{json, Value};

use super::Browser;
use crate::BrowserError;

const DOMAINS: &[&str] = &[
    "DOM",
    "CSS",
    "Runtime",
    "Page",
    "Network",
    "Emulation",
    "Input",
    "Accessibility",
    "Overlay",
    "Performance",
    "Log",
    "DOMSnapshot",
    "DOMStorage",
    "IndexedDB",
    "CacheStorage",
    "Animation",
    "LayerTree",
    "Profiler",
    "HeapProfiler",
    "Debugger",
];

pub(super) fn validate(method: &str) -> Result<(), BrowserError> {
    let domain = method.split_once('.').map(|(domain, _)| domain);
    if !domain.is_some_and(|domain| DOMAINS.contains(&domain))
        || matches!(
            method,
            "DOM.setFileInputFiles"
                | "DOM.getFileInfo"
                | "Page.setDownloadBehavior"
                | "Page.addScriptToEvaluateOnLoad"
                | "Input.synthesizeScrollGesture"
                | "Input.synthesizePinchGesture"
                | "Input.synthesizeTapGesture"
                | "Network.getAllCookies"
                | "Network.clearBrowserCookies"
                | "Network.clearBrowserCache"
                | "Network.setCookie"
                | "Network.setCookies"
                | "Network.deleteCookies"
        )
    {
        return Err(BrowserError::InvalidAction(
            "cdp method is reserved by the host",
        ));
    }
    Ok(())
}

impl Browser {
    /// Validate a script-facing method without issuing it. File upload uses the trusted host
    /// path only after its file and receiving-frame gates have admitted the request.
    pub fn validate_page_cdp(method: &str) -> Result<(), BrowserError> {
        validate(method)
    }

    /// Wait until abandoned init registrations have been removed and Chromium acknowledged it.
    pub async fn fence_init_scripts(
        &self,
        owner: &str,
        tab: &str,
        bound: std::time::Duration,
    ) -> Result<bool, BrowserError> {
        let session = self.owned_session(owner, tab).await?;
        Ok(self.inner.cdp.fence_init(&session, bound).await)
    }

    /// Quarantine before closing an owned tab whose init-script cleanup cannot be confirmed.
    /// The exact-target retry owns its fence independently of cancellation or the caller's bound.
    pub async fn quarantine_close(
        &self,
        owner: &str,
        tab: &str,
        bound: std::time::Duration,
    ) -> Result<(), BrowserError> {
        let mut state = self.inner.state.lock().await;
        super::check_owned(&state, owner, tab)?;
        state.closing_tabs.insert(tab.to_owned());
        drop(state);
        let inner = std::sync::Arc::clone(&self.inner);
        let (tab, root, url, pid) = (
            tab.to_owned(),
            self.profile_path().to_owned(),
            self.ws_url.clone(),
            self.process_id(),
        );
        let (finished, completion) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let mut replacement = None;
            loop {
                let gone = {
                    #[cfg(unix)]
                    {
                        i32::try_from(pid).ok().is_some_and(|pid| {
                            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None)
                                == Err(nix::errno::Errno::ESRCH)
                        })
                    }
                    #[cfg(not(unix))]
                    {
                        false
                    }
                };
                if gone {
                    break;
                }
                if inner.cdp.closed()
                    && replacement
                        .as_ref()
                        .map_or(true, |(cdp, _): &(crate::cdp::Cdp, _)| cdp.closed())
                {
                    if let Ok(connection) =
                        crate::cdp::Cdp::connect(&url, std::time::Duration::from_secs(1)).await
                    {
                        replacement = Some(connection);
                    }
                }
                let cdp = replacement.as_ref().map_or(&inner.cdp, |(cdp, _)| cdp);
                let _ = tokio::time::timeout(
                    std::time::Duration::from_millis(500),
                    cdp.call(None, "Target.closeTarget", json!({"targetId":tab})),
                )
                .await;
                let listed = tokio::time::timeout(
                    std::time::Duration::from_millis(500),
                    cdp.call(None, "Target.getTargets", json!({})),
                )
                .await;
                if matches!(&listed,Ok(Ok(listed)) if listed["targetInfos"].as_array().is_some_and(|targets|!targets.iter().any(|target|target["targetId"]==tab)))
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            let mut state = inner.state.lock().await;
            state.closing_tabs.remove(&tab);
            if let Some(session) = state.sessions.remove(&tab) {
                inner.cdp.forget_session(&session);
            }
            state.references.remove(&tab);
            state.frames.retain(|_, target| target != &tab);
            state.tab_contexts.remove(&tab);
            state.pending_openers.remove(&tab);
            state.oopif_sessions.retain(|_, (target, session)| {
                if target == &tab {
                    inner.cdp.forget_session(session);
                    false
                } else {
                    true
                }
            });
            state.uploads.remove(&tab);
            if let Some(closed) = state.tabs.remove(&tab) {
                let _ = inner
                    .events
                    .send(super::BrowserEvent::TabClosed { tab: closed });
            }
            drop(state);
            let _ =
                tokio::task::spawn_blocking(move || super::uploads::remove_tab(&root, &tab)).await;
            let _ = finished.send(());
        });
        tokio::time::timeout(bound, completion)
            .await
            .map_err(|_| BrowserError::Timeout("tab close confirmation"))?
            .map_err(|_| BrowserError::Disconnected)
    }

    /// Resolve a page-session execution context to its current frame, never by executing
    /// script in an unadmitted context. Unknown, destroyed and foreign contexts fail closed.
    pub async fn page_context_frame(
        &self,
        owner: &str,
        tab_id: &str,
        id: Option<i64>,
        unique: Option<&str>,
    ) -> Result<String, BrowserError> {
        let session = self.owned_session(owner, tab_id).await?;
        self.inner
            .cdp
            .call(Some(&session), "Runtime.enable", json!({}))
            .await?;
        self.inner
            .cdp
            .context_frame(&session, id, unique)
            .ok_or(BrowserError::InvalidAction(
                "unknown page execution context",
            ))
    }

    /// Bind an exact, possibly partitioned storage key to one current document of the owned tab.
    pub async fn page_storage_key_frame(
        &self,
        owner: &str,
        tab_id: &str,
        key: &str,
    ) -> Result<String, BrowserError> {
        for frame in self.frames(owner, tab_id).await? {
            let reply = self
                .host_command(
                    owner,
                    tab_id,
                    frame.host.as_deref(),
                    "Storage.getStorageKeyForFrame",
                    json!({"frameId": frame.id}),
                )
                .await?;
            if reply["storageKey"] == key {
                return Ok(frame.id);
            }
        }
        Err(BrowserError::InvalidAction(
            "storage key does not belong to this tab",
        ))
    }

    /// Resolve a page-session DOM node's document without reading that document's content.
    pub async fn page_node_frame(
        &self,
        owner: &str,
        tab_id: &str,
        params: Value,
    ) -> Result<(String, u64), BrowserError> {
        let session = self.owned_session(owner, tab_id).await?;
        let described = self
            .inner
            .cdp
            .call(Some(&session), "DOM.describeNode", params)
            .await?;
        let backend = described["node"]["backendNodeId"]
            .as_u64()
            .ok_or(BrowserError::InvalidResponse("file input node"))?;
        // Node ancestry is renderer-local; frame ids only occur on document/owner nodes.
        let document = self
            .inner
            .cdp
            .call(
                Some(&session),
                "DOM.getDocument",
                json!({"depth": -1, "pierce": true}),
            )
            .await?;
        fn find(node: &Value, backend: u64, frame: Option<&str>) -> Option<String> {
            let frame = node["frameId"].as_str().or(frame);
            if node["backendNodeId"].as_u64() == Some(backend) {
                return frame.map(str::to_owned);
            }
            for field in ["children", "shadowRoots"] {
                for child in node[field].as_array().into_iter().flatten() {
                    if let Some(found) = find(child, backend, frame) {
                        return Some(found);
                    }
                }
            }
            if let Some(child) = node.get("contentDocument") {
                if let Some(found) = find(child, backend, frame) {
                    return Some(found);
                }
            }
            None
        }
        let top = self
            .inner
            .cdp
            .call(Some(&session), "Page.getFrameTree", json!({}))
            .await?;
        let frame = find(
            &document["root"],
            backend,
            top["frameTree"]["frame"]["id"].as_str(),
        )
        .ok_or(BrowserError::StaleReference)?;
        Ok((frame, backend))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn fixture(cdp: crate::cdp::Cdp, url: String, profile: &std::path::Path) -> Browser {
        use super::super::{Inner, OpenedBy, Process, State, TabInfo};
        let mut state = State::default();
        for (tab, session) in [("tab", "page"), ("sibling", "sibling-page")] {
            state.tabs.insert(
                tab.into(),
                TabInfo {
                    id: tab.into(),
                    url: "about:blank".into(),
                    title: String::new(),
                    opened_by: OpenedBy::Agent,
                    owner_session_id: Some("owner".into()),
                    discarded: false,
                    unloaded: false,
                    browser_context_id: None,
                },
            );
            state.sessions.insert(tab.into(), session.into());
        }
        let (events, _) = tokio::sync::mpsc::unbounded_channel();
        Browser {
            inner: std::sync::Arc::new(Inner {
                cdp,
                state: tokio::sync::Mutex::new(state),
                attach_lock: Default::default(),
                capture_lock: Default::default(),
                interaction_lock: Default::default(),
                network: Default::default(),
                attach_timeout: std::time::Duration::from_secs(2),
                events,
                navigation_policy: std::sync::RwLock::new(None),
                screencasts: Default::default(),
                page: Default::default(),
                profile_writes: Default::default(),
            }),
            process: Process {
                child: parking_lot::Mutex::new(None),
                group: nix::unistd::Pid::from_raw(i32::try_from(std::process::id()).unwrap()),
                profile: profile.to_owned(),
                temporary_profile: None,
                stderr: None,
                detached: true,
                running: false.into(),
            },
            ws_url: url,
            event_task: tokio::spawn(std::future::pending()),
            gateway: None,
            gateway_dir: None,
            isolated: true,
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn destroyed_remote_object_cleanup_preserves_only_successful_operation_replies() {
        use futures_util::{SinkExt, StreamExt};
        for (operation_ok, code, message, cleanup_ok) in [
            (true, -32000, "Cannot find context with specified id", true),
            (true, -32000, "Could not find object with given id", true),
            (true, -32000, "unexpected release failure", false),
            (true, -32602, "Cannot find context with specified id", false),
            (false, -32000, "Cannot find context with specified id", true),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("ws://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
                for method in [
                    "DOM.resolveNode",
                    "Runtime.callFunctionOn",
                    "Runtime.releaseObject",
                ] {
                    let text = socket.next().await.unwrap().unwrap().into_text().unwrap();
                    let request: Value = serde_json::from_str(&text).unwrap();
                    assert_eq!(request["method"], method);
                    let reply = match method {
                        "DOM.resolveNode" => {
                            json!({"id":request["id"],"result":{"object":{"objectId":"object"}}})
                        }
                        "Runtime.callFunctionOn" if operation_ok => {
                            json!({"id":request["id"],"result":{"result":{"value":"filled"}}})
                        }
                        "Runtime.callFunctionOn" => {
                            json!({"id":request["id"],"error":{"code":-32000,"message":"injected operation failure"}})
                        }
                        _ => json!({"id":request["id"],"error":{"code":code,"message":message}}),
                    };
                    socket
                        .send(tokio_tungstenite::tungstenite::Message::Text(
                            reply.to_string().into(),
                        ))
                        .await
                        .unwrap();
                }
            });
            let (cdp, _events) = crate::cdp::Cdp::connect(&url, std::time::Duration::from_secs(2))
                .await
                .unwrap();
            let profile = tempfile::tempdir().unwrap();
            let browser = fixture(cdp, url, profile.path());
            let result = browser
                .call_on_node_in(
                    "page",
                    1,
                    Some(1),
                    "function() { return 'filled'; }",
                    json!([]),
                )
                .await;
            if operation_ok && cleanup_ok {
                assert_eq!(result.unwrap(), "filled");
            } else {
                let BrowserError::Cdp {
                    code: actual_code,
                    message: actual_message,
                } = result.unwrap_err()
                else {
                    panic!("unexpected operation failure");
                };
                assert_eq!(actual_code, if operation_ok { code } else { -32000 });
                assert_eq!(
                    actual_message,
                    if operation_ok {
                        message
                    } else {
                        "injected operation failure"
                    }
                );
            }
            server.await.unwrap();
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_late_init_registration_is_removed_before_the_next_navigation() {
        use futures_util::{SinkExt, StreamExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let (seen, registered) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let (removal_seen, removal_received) = tokio::sync::oneshot::channel();
        let (ack_release, ack_released) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            let text = socket.next().await.unwrap().unwrap().into_text().unwrap();
            let request: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(request["method"], "Page.addScriptToEvaluateOnNewDocument");
            seen.send(()).unwrap();
            released.await.unwrap();
            socket
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    json!({"id":request["id"],"result":{"identifier":"old-init"}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            let mut active = true;
            let mut removal_seen = Some(removal_seen);
            let mut ack_released = Some(ack_released);
            loop {
                let text = socket.next().await.unwrap().unwrap().into_text().unwrap();
                let request: Value = serde_json::from_str(&text).unwrap();
                match request["method"].as_str().unwrap() {
                    "Page.removeScriptToEvaluateOnNewDocument" => {
                        assert_eq!(request["params"]["identifier"], "old-init");
                        removal_seen.take().unwrap().send(()).unwrap();
                        ack_released.take().unwrap().await.unwrap();
                        active = false;
                    }
                    "Page.navigate" => {
                        assert!(!active, "an old init must not enter the next Task document");
                    }
                    other => panic!("unexpected command {other}"),
                }
                socket
                    .send(tokio_tungstenite::tungstenite::Message::Text(
                        json!({"id":request["id"],"result":{}}).to_string().into(),
                    ))
                    .await
                    .unwrap();
                if request["method"] == "Page.navigate" {
                    break;
                }
            }
        });
        let (cdp, _events) = crate::cdp::Cdp::connect(&url, std::time::Duration::from_secs(2))
            .await
            .unwrap();
        let profile = tempfile::tempdir().unwrap();
        let browser = fixture(cdp, url, profile.path());
        let mut call = Box::pin(browser.page_cdp(
            "owner",
            "tab",
            "Page.addScriptToEvaluateOnNewDocument",
            json!({"source":"window.marker=1"}),
        ));
        tokio::select! {result=&mut call=>panic!("returned before release {result:?}"),result=registered=>result.unwrap()}
        drop(call);
        assert!(!browser
            .fence_init_scripts("owner", "tab", std::time::Duration::from_millis(10))
            .await
            .unwrap());
        release.send(()).unwrap();
        removal_received.await.unwrap();
        assert!(!browser
            .fence_init_scripts("owner", "tab", std::time::Duration::from_millis(10))
            .await
            .unwrap());
        ack_release.send(()).unwrap();
        assert!(browser
            .fence_init_scripts("owner", "tab", std::time::Duration::from_secs(1))
            .await
            .unwrap());
        browser
            .page_cdp(
                "owner",
                "tab",
                "Page.navigate",
                json!({"url":"about:blank"}),
            )
            .await
            .unwrap();
        server.await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unconfirmed_init_cleanup_quarantines_then_retries_only_its_owned_target() {
        use futures_util::{SinkExt, StreamExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let (seen, registered) = tokio::sync::oneshot::channel();
        let (first_close, closing) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            let text = socket.next().await.unwrap().unwrap().into_text().unwrap();
            let request: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(request["method"], "Page.addScriptToEvaluateOnNewDocument");
            seen.send(()).unwrap();
            let mut closes = 0;
            let mut first_close = Some(first_close);
            loop {
                let text = socket.next().await.unwrap().unwrap().into_text().unwrap();
                let request: Value = serde_json::from_str(&text).unwrap();
                let result = match request["method"].as_str().unwrap() {
                    "Target.closeTarget" => {
                        assert_eq!(request["params"]["targetId"], "tab");
                        closes += 1;
                        if closes == 1 {
                            first_close.take().unwrap().send(()).unwrap();
                            continue;
                        }
                        json!({"success":true})
                    }
                    "Target.getTargets" => {
                        if closes < 2 {
                            json!({"targetInfos":[{"targetId":"tab"},{"targetId":"sibling"}]})
                        } else {
                            json!({"targetInfos":[{"targetId":"sibling"}]})
                        }
                    }
                    other => panic!("unexpected command {other}"),
                };
                socket
                    .send(tokio_tungstenite::tungstenite::Message::Text(
                        json!({"id":request["id"],"result":result})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
                if request["method"] == "Target.getTargets" && closes >= 2 {
                    break;
                }
            }
        });
        let (cdp, _events) = crate::cdp::Cdp::connect(&url, std::time::Duration::from_secs(2))
            .await
            .unwrap();
        let profile = tempfile::tempdir().unwrap();
        let browser = fixture(cdp, url, profile.path());
        let mut call = Box::pin(browser.page_cdp(
            "owner",
            "tab",
            "Page.addScriptToEvaluateOnNewDocument",
            json!({"source":"window.marker=1"}),
        ));
        tokio::select! {result=&mut call=>panic!("returned before release {result:?}"),result=registered=>result.unwrap()}
        drop(call);
        assert!(!browser
            .fence_init_scripts("owner", "tab", std::time::Duration::from_millis(10))
            .await
            .unwrap());
        assert!(browser
            .quarantine_close("owner", "tab", std::time::Duration::from_millis(10))
            .await
            .is_err());
        closing.await.unwrap();
        assert!(browser.check_owned("owner", "tab").await.is_err());
        browser.check_owned("owner", "sibling").await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while browser.inner.state.lock().await.tabs.contains_key("tab") {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(browser
            .inner
            .state
            .lock()
            .await
            .tabs
            .contains_key("sibling"));
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn reconnect_sweep_rebinds_live_uploads_and_tab_destruction_removes_them() {
        use futures_util::StreamExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            while socket.next().await.is_some() {}
        });
        let (cdp, _events) = crate::cdp::Cdp::connect(&url, std::time::Duration::from_secs(2))
            .await
            .unwrap();
        let profile = tempfile::tempdir().unwrap();
        let source = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(source.path(), b"lazy upload bytes").unwrap();
        let browser = fixture(cdp, url, profile.path());
        let staged = browser
            .stage_upload(
                "owner",
                "tab",
                &source.path().canonicalize().unwrap(),
                "chosen.txt",
            )
            .await
            .unwrap();
        let orphan = profile.path().join("upload-staging/orphan");
        std::fs::create_dir(&orphan).unwrap();
        browser.inner.state.lock().await.uploads.clear();
        browser.prune_uploads().await.unwrap();
        assert_eq!(std::fs::read(&staged).unwrap(), b"lazy upload bytes");
        assert!(!orphan.exists());
        assert!(browser.inner.state.lock().await.uploads.contains_key("tab"));
        super::super::handle_event(
            &browser.inner,
            json!({"method":"Target.targetDestroyed","params":{"targetId":"tab"}}),
        )
        .await;
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while staged.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        browser.check_owned("owner", "sibling").await.unwrap();
        server.abort();
    }

    #[test]
    fn global_and_file_authority_is_not_page_cdp() {
        for method in [
            "Target.createTarget",
            "Browser.close",
            "SystemInfo.getInfo",
            "Storage.getCookies",
            "Tethering.bind",
            "Fetch.disable",
            "WebAuthn.getCredentials",
            "Security.disable",
            "ServiceWorker.stopAllWorkers",
            "BackgroundService.startObserving",
            "PWA.launch",
            "Extensions.loadUnpacked",
            "DOM.setFileInputFiles",
            "DOM.getFileInfo",
            "Page.setDownloadBehavior",
            "Page.addScriptToEvaluateOnLoad",
            "Network.getAllCookies",
            "Network.clearBrowserCookies",
        ] {
            assert!(validate(method).is_err(), "{method}");
        }
        for method in [
            "Input.synthesizeScrollGesture",
            "Input.synthesizePinchGesture",
            "Input.synthesizeTapGesture",
        ] {
            assert!(validate(method).is_err(), "{method}");
        }
        for domain in DOMAINS {
            assert!(validate(&format!("{domain}.example")).is_ok());
        }
        for method in [
            "Runtime.evaluate",
            "Page.close",
            "Page.crash",
            "DOM.getDocument",
        ] {
            assert!(validate(method).is_ok());
        }
    }
}
