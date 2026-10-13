use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use crate::BrowserError;

type Reply = oneshot::Sender<Result<Value, BrowserError>>;
#[derive(Clone)]
struct Context {
    id: i64,
    unique: String,
    frame: String,
}

type Contexts = Arc<Mutex<HashMap<String, Vec<Context>>>>;

#[derive(Clone, Copy)]
enum ResourceKind {
    Init,
    Objects,
    Target,
    Attach,
    // Canceling the caller does not cancel an issued profile mutation.
    ProfileWrite,
}
enum Cleanup {
    Init(String),
    Objects(Vec<String>),
    Target(String),
    Attach(String),
}
struct ResourceRegistration {
    session: Option<String>,
    kind: ResourceKind,
    cleanup: Option<Cleanup>,
    abandoned: bool,
    cleanup_queued: bool,
}
type ResourceRegistrations = Arc<Mutex<HashMap<u64, ResourceRegistration>>>;

#[derive(Clone)]
struct InitFence {
    resources: ResourceRegistrations,
    pending: Arc<Mutex<HashMap<u64, Reply>>>,
    outgoing: mpsc::UnboundedSender<Message>,
    ids: Arc<AtomicU64>,
    changed: Arc<tokio::sync::Notify>,
}

impl InitFence {
    fn remove(&self, original: u64, session: Option<&str>, identifier: String) {
        let id = self.ids.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        lock(&self.pending).insert(id, sender);
        let mut request = json!({"id":id,"method":"Page.removeScriptToEvaluateOnNewDocument","params":{"identifier":identifier}});
        if let Some(session) = session {
            request["sessionId"] = json!(session);
        }
        if self
            .outgoing
            .send(Message::Text(request.to_string().into()))
            .is_err()
        {
            lock(&self.pending).remove(&id);
            return;
        }
        let fence = self.clone();
        tokio::spawn(async move {
            let acknowledged = matches!(
                tokio::time::timeout(Duration::from_millis(250), receiver).await,
                Ok(Ok(Ok(_)))
            );
            lock(&fence.pending).remove(&id);
            if acknowledged {
                lock(&fence.resources).remove(&original);
            }
            fence.changed.notify_waiters();
        });
    }

    async fn wait(&self, session: &str, bound: Duration) -> bool {
        tokio::time::timeout(bound, async {
            loop {
                let changed = self.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if !lock(&self.resources).values().any(|entry| {
                    matches!(entry.kind, ResourceKind::Init)
                        && entry.session.as_deref() == Some(session)
                }) {
                    return;
                }
                changed.await;
            }
        })
        .await
        .is_ok()
    }
}

fn resource_reply(
    registrations: &ResourceRegistrations,
    id: u64,
    result: &Value,
) -> Option<(Option<String>, Cleanup)> {
    let mut registrations = lock(registrations);
    let entry = registrations.get_mut(&id)?;
    let cleanup = match entry.kind {
        ResourceKind::Init => result["identifier"]
            .as_str()
            .map(|id| Cleanup::Init(id.to_owned())),
        ResourceKind::Target => result["targetId"]
            .as_str()
            .map(|id| Cleanup::Target(id.to_owned())),
        ResourceKind::Attach => result["sessionId"]
            .as_str()
            .map(|id| Cleanup::Attach(id.to_owned())),
        ResourceKind::Objects => {
            let mut ids = Vec::new();
            let mut add = |object: &Value| {
                if let Some(id) = object["objectId"].as_str() {
                    if !ids.iter().any(|known| known == id) {
                        ids.push(id.to_owned());
                    }
                }
            };
            add(&result["result"]);
            add(&result["object"]);
            add(&result["exceptionDetails"]["exception"]);
            for field in ["result", "internalProperties", "privateProperties"] {
                for property in result[field].as_array().into_iter().flatten() {
                    for field in ["value", "get", "set", "symbol"] {
                        add(&property[field]);
                    }
                }
            }
            (!ids.is_empty()).then_some(Cleanup::Objects(ids))
        }
        ResourceKind::ProfileWrite => None,
    };
    if cleanup.is_none() {
        registrations.remove(&id);
        return None;
    }
    entry.cleanup = cleanup;
    if entry.abandoned {
        if matches!(entry.kind, ResourceKind::Init) {
            if entry.cleanup_queued {
                return None;
            }
            entry.cleanup_queued = true;
            return entry
                .cleanup
                .take()
                .map(|cleanup| (entry.session.clone(), cleanup));
        }
        let entry = registrations.remove(&id)?;
        return entry.cleanup.map(|cleanup| (entry.session, cleanup));
    }
    None
}

fn cleanup_messages(
    next_id: &AtomicU64,
    session: Option<&str>,
    cleanup: Cleanup,
    mut send: impl FnMut(Message),
) {
    let mut command = |method: &str, params: Value| {
        let mut request =
            json!({"id":next_id.fetch_add(1,Ordering::Relaxed),"method":method,"params":params});
        if let Some(session) = session {
            request["sessionId"] = json!(session);
        }
        send(Message::Text(request.to_string().into()));
    };
    match cleanup {
        Cleanup::Init(identifier) => command(
            "Page.removeScriptToEvaluateOnNewDocument",
            json!({"identifier":identifier}),
        ),
        Cleanup::Target(target) => command("Target.closeTarget", json!({"targetId":target})),
        Cleanup::Attach(session) => {
            command("Target.detachFromTarget", json!({"sessionId":session}))
        }
        Cleanup::Objects(ids) => {
            // A later execution can already own this group; release only the orphan's exact ids.
            for object in ids {
                command("Runtime.releaseObject", json!({"objectId":object}));
            }
        }
    }
}

fn record_context(contexts: &Contexts, event: &Value) {
    if event["method"] == "Target.detachedFromTarget" {
        if let Some(session) = event["params"]["sessionId"].as_str() {
            lock(contexts).remove(session);
        }
        return;
    }
    let Some(session) = event["sessionId"].as_str() else {
        return;
    };
    let mut contexts = lock(contexts);
    match event["method"].as_str() {
        Some("Runtime.executionContextCreated") => {
            let context = &event["params"]["context"];
            if let (Some(id), Some(frame)) = (
                context["id"].as_i64(),
                context["auxData"]["frameId"].as_str(),
            ) {
                let entries = contexts.entry(session.to_owned()).or_default();
                entries.retain(|entry| entry.id != id);
                entries.push(Context {
                    id,
                    unique: context["uniqueId"].as_str().unwrap_or_default().to_owned(),
                    frame: frame.to_owned(),
                });
            }
        }
        Some("Runtime.executionContextDestroyed") => {
            if let Some(entries) = contexts.get_mut(session) {
                entries.retain(|entry| {
                    Some(entry.id) != event["params"]["executionContextId"].as_i64()
                        && Some(entry.unique.as_str())
                            != event["params"]["executionContextUniqueId"].as_str()
                });
            }
        }
        Some("Runtime.executionContextsCleared") => {
            contexts.remove(session);
        }
        _ => {}
    }
}

pub(crate) struct Cdp {
    next_id: Arc<AtomicU64>,
    pending: Arc<Mutex<HashMap<u64, Reply>>>,
    outgoing: mpsc::UnboundedSender<Message>,
    task: JoinHandle<()>,
    timeout: Duration,
    // Opt-in audit of issued commands; None (the default) records nothing.
    recorder: Mutex<Option<Vec<(String, Value)>>>,
    contexts: Contexts,
    resources: ResourceRegistrations,
    init_fence: InitFence,
    closing: AtomicBool,
    shutdown_complete: AtomicBool,
    shutdown_crash_recorded: AtomicBool,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct PendingRequest<'a> {
    id: u64,
    pending: &'a Mutex<HashMap<u64, Reply>>,
    resources: &'a Mutex<HashMap<u64, ResourceRegistration>>,
    outgoing: &'a mpsc::UnboundedSender<Message>,
    next_id: &'a AtomicU64,
    complete: bool,
    has_resources: bool,
    init_fence: &'a InitFence,
}

impl Drop for PendingRequest<'_> {
    fn drop(&mut self) {
        lock(self.pending).remove(&self.id);
        if !self.has_resources {
            return;
        }
        let mut resources = lock(self.resources);
        if self.complete {
            resources.remove(&self.id);
            self.init_fence.changed.notify_waiters();
            return;
        }
        let Some(entry) = resources.get_mut(&self.id) else {
            return;
        };
        entry.abandoned = true;
        if let Some(cleanup) = entry.cleanup.take() {
            match cleanup {
                Cleanup::Init(identifier) => {
                    if !entry.cleanup_queued {
                        entry.cleanup_queued = true;
                        self.init_fence
                            .remove(self.id, entry.session.as_deref(), identifier);
                    }
                }
                cleanup => {
                    cleanup_messages(self.next_id, entry.session.as_deref(), cleanup, |message| {
                        let _ = self.outgoing.send(message);
                    });
                    resources.remove(&self.id);
                }
            }
        }
    }
}

impl Cdp {
    pub(crate) async fn connect(
        url: &str,
        timeout: Duration,
    ) -> Result<(Self, mpsc::UnboundedReceiver<Value>), BrowserError> {
        let (socket, _) = tokio::time::timeout(timeout, tokio_tungstenite::connect_async(url))
            .await
            .map_err(|_| BrowserError::Timeout("CDP connection"))?
            .map_err(|error| BrowserError::Transport(error.to_string()))?;
        let (outgoing, mut messages) = mpsc::unbounded_channel();
        let (events, receiver) = mpsc::unbounded_channel();
        let pending: Arc<Mutex<HashMap<u64, Reply>>> = Arc::default();
        let replies = Arc::clone(&pending);
        let next_id = Arc::new(AtomicU64::new(1));
        let actor_ids = Arc::clone(&next_id);
        let contexts: Contexts = Arc::default();
        let observed_contexts = Arc::clone(&contexts);
        let resources: ResourceRegistrations = Arc::default();
        let actor_resources = Arc::clone(&resources);
        let actor_outgoing = outgoing.clone();
        let init_fence = InitFence {
            resources: Arc::clone(&resources),
            pending: Arc::clone(&pending),
            outgoing: outgoing.clone(),
            ids: Arc::clone(&next_id),
            changed: Arc::new(tokio::sync::Notify::new()),
        };
        let actor_init_fence = init_fence.clone();
        let task = tokio::spawn(async move {
            let (mut sink, mut stream) = socket.split();
            loop {
                tokio::select! {
                    message = messages.recv() => {
                        let Some(message) = message else { break };
                        if sink.send(message).await.is_err() { break; }
                    }
                    message = stream.next() => {
                        match message {
                            Some(Ok(Message::Text(text))) => {
                                let Ok(mut value) = serde_json::from_str::<Value>(&text) else { continue };
                                if let Some(id) = value["id"].as_u64() {
                                    if let Some((session, cleanup)) = resource_reply(&actor_resources,id,&value["result"]) {
                                        match cleanup {
                                            Cleanup::Init(identifier)=>actor_init_fence.remove(id,session.as_deref(),identifier),
                                            cleanup=>cleanup_messages(&actor_ids,session.as_deref(),cleanup,|message| {let _=actor_outgoing.send(message);}),
                                        }
                                    }
                                    actor_init_fence.changed.notify_waiters();
                                    let reply = lock(&replies).remove(&id);
                                    if let Some(reply) = reply {
                                        let result = if value.get("error").is_some() {
                                            Err(BrowserError::Cdp {
                                                code: value["error"]["code"].as_i64().unwrap_or(-1),
                                                message: value["error"]["message"].as_str().unwrap_or("unknown CDP error").to_owned(),
                                            })
                                        } else {
                                            Ok(value.get_mut("result").map(Value::take).unwrap_or(Value::Null))
                                        };
                                        let _ = reply.send(result);
                                    }
                                } else {
                                    record_context(&observed_contexts, &value);
                                    if events.send(value).is_err() { break; }
                                }
                            }
                            Some(Ok(Message::Ping(bytes))) => {
                                if sink.send(Message::Pong(bytes)).await.is_err() { break; }
                            }
                            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                            _ => {}
                        }
                    }
                }
            }
            for (_, reply) in lock(&replies).drain() {
                let _ = reply.send(Err(BrowserError::Disconnected));
            }
        });
        Ok((
            Self {
                next_id,
                pending,
                outgoing,
                task,
                timeout,
                recorder: Mutex::new(None),
                contexts,
                resources,
                init_fence,
                closing: AtomicBool::new(false),
                shutdown_complete: AtomicBool::new(false),
                shutdown_crash_recorded: AtomicBool::new(false),
            },
            receiver,
        ))
    }

    pub(crate) fn context_frame(
        &self,
        session: &str,
        id: Option<i64>,
        unique: Option<&str>,
    ) -> Option<String> {
        lock(&self.contexts)
            .get(session)?
            .iter()
            .find(|context| {
                id.is_some_and(|id| context.id == id)
                    || unique.is_some_and(|unique| context.unique == unique)
            })
            .map(|context| context.frame.clone())
    }

    pub(crate) async fn fence_init(&self, session: &str, bound: Duration) -> bool {
        self.init_fence.wait(session, bound).await
    }

    pub(crate) fn forget_session(&self, session: &str) {
        lock(&self.resources).retain(|_, entry| entry.session.as_deref() != Some(session));
        lock(&self.contexts).remove(session);
        self.init_fence.changed.notify_waiters();
    }

    pub(crate) fn admit_profile_write(&self, writes: &AtomicUsize) -> bool {
        // Raw CDP connections must share the same admission gate as graceful shutdown.
        let _admission = lock(&self.pending);
        if self.is_closing() {
            return false;
        }
        writes.fetch_add(1, Ordering::AcqRel);
        true
    }

    pub(crate) fn begin_shutdown(&self) -> bool {
        let idle = {
            let pending = lock(&self.pending);
            self.closing.store(true, Ordering::Release);
            pending.is_empty()
        };
        // Resource cleanup takes these locks in the opposite order.
        idle && lock(&self.resources).is_empty()
    }

    pub(crate) fn is_closing(&self) -> bool {
        self.closing.load(Ordering::Acquire)
    }

    pub(crate) fn shutdown_complete(&self) -> bool {
        self.shutdown_complete.load(Ordering::Acquire)
    }

    pub(crate) fn complete_shutdown(&self) {
        self.shutdown_complete.store(true, Ordering::Release);
    }

    pub(crate) fn record_shutdown_crash(&self) -> bool {
        !self.shutdown_crash_recorded.swap(true, Ordering::Relaxed)
    }

    pub(crate) fn pending_requests(&self) -> usize {
        lock(&self.pending).len()
    }

    pub(crate) async fn call_shutdown(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Value, BrowserError> {
        self.call_with_recording(None, method, params, false, true)
            .await
    }

    pub(crate) async fn call(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, BrowserError> {
        self.call_with_recording(session, method, params, false, false)
            .await
    }

    pub(crate) async fn call_sensitive(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, BrowserError> {
        self.call_with_recording(session, method, params, true, false)
            .await
    }

    async fn call_with_recording(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
        sensitive: bool,
        during_shutdown: bool,
    ) -> Result<Value, BrowserError> {
        if self.is_closing() && !during_shutdown {
            return Err(BrowserError::Disconnected);
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Some(log) = lock(&self.recorder).as_mut() {
            log.push((
                method.to_owned(),
                if sensitive {
                    json!({"redacted":true})
                } else {
                    params.clone()
                },
            ));
        }
        let mut request = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            request["sessionId"] = Value::String(session.to_owned());
        }
        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = lock(&self.pending);
            if self.is_closing() && !during_shutdown {
                return Err(BrowserError::Disconnected);
            }
            pending.insert(id, sender);
        }
        let kind = match method {
            "Page.addScriptToEvaluateOnNewDocument" => Some(ResourceKind::Init),
            "Runtime.evaluate"
            | "Runtime.callFunctionOn"
            | "Runtime.awaitPromise"
            | "Runtime.runScript"
            | "Runtime.getProperties"
            | "DOM.resolveNode"
            | "Debugger.evaluateOnCallFrame" => Some(ResourceKind::Objects),
            "Target.createTarget" => Some(ResourceKind::Target),
            "Target.attachToTarget" | "Target.attachToBrowserTarget" => Some(ResourceKind::Attach),
            "Storage.setCookies"
            | "Storage.clearCookies"
            | "Storage.clearDataForOrigin"
            | "Storage.clearDataForStorageKey"
            | "Network.setCookie"
            | "Network.setCookies"
            | "Network.deleteCookies"
            | "Network.clearBrowserCookies"
            | "DOMStorage.setDOMStorageItem"
            | "DOMStorage.removeDOMStorageItem"
            | "DOMStorage.clear"
            | "CacheStorage.deleteCache"
            | "CacheStorage.deleteEntry"
            | "IndexedDB.deleteDatabase"
            | "IndexedDB.clearObjectStore"
            | "IndexedDB.deleteObjectStoreEntries" => Some(ResourceKind::ProfileWrite),
            _ => None,
        };
        let has_resources = kind.is_some();
        if let Some(kind) = kind {
            lock(&self.resources).insert(
                id,
                ResourceRegistration {
                    session: if matches!(kind, ResourceKind::ProfileWrite) {
                        None
                    } else {
                        session.map(str::to_owned)
                    },
                    kind,
                    cleanup: None,
                    abandoned: false,
                    cleanup_queued: false,
                },
            );
        }
        // Removing canceled requests prevents attach timeouts from accumulating reply senders.
        let mut request_guard = PendingRequest {
            id,
            pending: &self.pending,
            resources: &self.resources,
            outgoing: &self.outgoing,
            next_id: &self.next_id,
            complete: false,
            has_resources,
            init_fence: &self.init_fence,
        };
        if self
            .outgoing
            .send(Message::Text(request.to_string().into()))
            .is_err()
        {
            request_guard.complete = true;
            return Err(BrowserError::Disconnected);
        }
        let result = tokio::time::timeout(self.timeout, receiver)
            .await
            .map_err(|_| BrowserError::Timeout("CDP command"))?
            .map_err(|_| BrowserError::Disconnected)?;
        request_guard.complete = true;
        result
    }

    /// Queue a command without awaiting its reply, for paths that cannot await (event
    /// handling, `Drop`). The reply is discarded; message order is preserved.
    pub(crate) fn send(&self, session: Option<&str>, method: &str, params: Value) {
        let _admission = lock(&self.pending);
        if self.is_closing() {
            return;
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Some(log) = lock(&self.recorder).as_mut() {
            log.push((method.to_owned(), params.clone()));
        }
        let mut request = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            request["sessionId"] = Value::String(session.to_owned());
        }
        let _ = self
            .outgoing
            .send(Message::Text(request.to_string().into()));
    }

    pub(crate) fn record(&self, enabled: bool) {
        *lock(&self.recorder) = enabled.then(Vec::new);
    }

    pub(crate) fn recorded(&self) -> Vec<(String, Value)> {
        lock(&self.recorder)
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// The transport actor ends only when the socket closed or failed; it never restarts.
    pub(crate) fn closed(&self) -> bool {
        self.task.is_finished()
    }
}

impl Drop for Cdp {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn canceled_profile_write_remains_pending_until_its_reply() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let (received, waiting) = oneshot::channel();
        let (release, resume) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
                panic!("expected profile write");
            };
            let request: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(request["method"], "Storage.setCookies");
            received.send(()).unwrap();
            resume.await.unwrap();
            socket
                .send(Message::Text(
                    json!({"id":request["id"],"result":{}}).to_string().into(),
                ))
                .await
                .unwrap();
            socket.close(None).await.unwrap();
        });
        let (cdp, _events) = Cdp::connect(&url, Duration::from_secs(2)).await.unwrap();
        {
            let call = cdp.call(None, "Storage.setCookies", json!({"cookies":[]}));
            tokio::pin!(call);
            tokio::select! {
                result = &mut call => panic!("unacknowledged write completed: {result:?}"),
                received = waiting => received.unwrap(),
            }
        }
        assert_eq!(cdp.pending_requests(), 0);
        assert!(
            !cdp.begin_shutdown(),
            "canceled writes are not confirmed completed"
        );
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let changed = cdp.init_fence.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if lock(&cdp.resources).is_empty() {
                    break;
                }
                changed.await;
            }
        })
        .await
        .unwrap();
        assert!(cdp.begin_shutdown());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn unconsumed_resource_replies_release_exact_resources_not_a_later_group() {
        for (kind, result, method, key, expected) in [
            (
                ResourceKind::Init,
                json!({"identifier":"old-script"}),
                "Page.removeScriptToEvaluateOnNewDocument",
                "identifier",
                "old-script",
            ),
            (
                ResourceKind::Target,
                json!({"targetId":"old-target"}),
                "Target.closeTarget",
                "targetId",
                "old-target",
            ),
            (
                ResourceKind::Attach,
                json!({"sessionId":"old-session"}),
                "Target.detachFromTarget",
                "sessionId",
                "old-session",
            ),
            (
                ResourceKind::Objects,
                json!({"result":{"objectId":"old-object","value":{"objectId":"not-a-remote-object"}}}),
                "Runtime.releaseObject",
                "objectId",
                "old-object",
            ),
        ] {
            let resources: ResourceRegistrations = Arc::default();
            let session = if matches!(kind, ResourceKind::Target | ResourceKind::Attach) {
                None
            } else {
                Some("owned-page".into())
            };
            lock(&resources).insert(
                1,
                ResourceRegistration {
                    session,
                    kind,
                    cleanup: None,
                    abandoned: false,
                    cleanup_queued: false,
                },
            );
            assert!(resource_reply(&resources, 1, &result).is_none());
            let pending = Arc::new(Mutex::new(HashMap::new()));
            let (outgoing, mut messages) = mpsc::unbounded_channel();
            let next_id = Arc::new(AtomicU64::new(10));
            let init_fence = InitFence {
                resources: Arc::clone(&resources),
                pending: Arc::clone(&pending),
                outgoing: outgoing.clone(),
                ids: Arc::clone(&next_id),
                changed: Arc::new(tokio::sync::Notify::new()),
            };
            drop(PendingRequest {
                id: 1,
                pending: &pending,
                resources: &resources,
                outgoing: &outgoing,
                next_id: &next_id,
                complete: false,
                has_resources: true,
                init_fence: &init_fence,
            });
            let Message::Text(message) = messages.try_recv().unwrap() else {
                panic!("expected cleanup command")
            };
            let command: Value = serde_json::from_str(&message).unwrap();
            assert_eq!(command["method"], method);
            assert_eq!(command["params"][key], expected);
            if matches!(kind, ResourceKind::Init) {
                assert!(
                    !init_fence
                        .wait("owned-page", Duration::from_millis(10))
                        .await,
                    "must not hand off before removal ACK"
                );
                lock(&pending)
                    .remove(&command["id"].as_u64().unwrap())
                    .unwrap()
                    .send(Ok(json!({})))
                    .unwrap();
                assert!(init_fence.wait("owned-page", Duration::from_secs(1)).await);
            }
            assert!(
                messages.try_recv().is_err(),
                "no group sweep or page-value object id"
            );
            assert!(lock(&resources).is_empty());
        }
    }

    #[tokio::test]
    async fn cancelled_resource_commands_clean_up_a_remote_success_that_arrives_later() {
        for (method, result, cleanup, key, expected, session) in [
            (
                "Page.addScriptToEvaluateOnNewDocument",
                json!({"identifier":"late-script"}),
                "Page.removeScriptToEvaluateOnNewDocument",
                "identifier",
                "late-script",
                Some("owned-page"),
            ),
            (
                "Runtime.callFunctionOn",
                json!({"result":{"objectId":"late-object"}}),
                "Runtime.releaseObject",
                "objectId",
                "late-object",
                Some("owned-page"),
            ),
            (
                "Target.createTarget",
                json!({"targetId":"late-target"}),
                "Target.closeTarget",
                "targetId",
                "late-target",
                None,
            ),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("ws://{}", listener.local_addr().unwrap());
            let (seen, request_seen) = oneshot::channel();
            let (release, released) = oneshot::channel();
            let server = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
                let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
                    panic!("expected request")
                };
                let request: Value = serde_json::from_str(&text).unwrap();
                assert_eq!(request["method"], method);
                seen.send(()).unwrap();
                released.await.unwrap();
                socket
                    .send(Message::Text(
                        json!({"id":request["id"],"result":result})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
                let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
                    panic!("expected cleanup")
                };
                let command: Value = serde_json::from_str(&text).unwrap();
                assert_eq!(command["method"], cleanup);
                assert_eq!(command["params"][key], expected);
                assert_eq!(command["sessionId"].as_str(), session);
                if method == "Page.addScriptToEvaluateOnNewDocument" {
                    socket
                        .send(Message::Text(
                            json!({"id":command["id"],"result":{}}).to_string().into(),
                        ))
                        .await
                        .unwrap();
                }
            });
            let (cdp, _events) = Cdp::connect(&url, Duration::from_secs(2)).await.unwrap();
            let mut call = Box::pin(cdp.call(session, method, json!({})));
            tokio::select! {
                result=&mut call=>panic!("command returned before release: {result:?}"),
                result=request_seen=>result.unwrap(),
            }
            drop(call);
            release.send(()).unwrap();
            tokio::time::timeout(Duration::from_secs(2), server)
                .await
                .unwrap()
                .unwrap();
            if method == "Page.addScriptToEvaluateOnNewDocument" {
                assert!(cdp.fence_init("owned-page", Duration::from_secs(1)).await);
            }
            assert!(lock(&cdp.resources).is_empty());
        }
    }

    #[test]
    fn execution_context_identity_is_scoped_to_a_live_session() {
        let contexts: Contexts = Arc::default();
        record_context(
            &contexts,
            &json!({"sessionId":"page","method":"Runtime.executionContextCreated","params":{"context":{"id":1,"uniqueId":"unique","auxData":{"frameId":"child"}}}}),
        );
        record_context(
            &contexts,
            &json!({"sessionId":"other","method":"Runtime.executionContextCreated","params":{"context":{"id":1,"uniqueId":"foreign","auxData":{"frameId":"foreign-child"}}}}),
        );
        assert_eq!(lock(&contexts)["page"][0].frame, "child");
        assert_eq!(lock(&contexts)["other"][0].frame, "foreign-child");
        record_context(
            &contexts,
            &json!({"sessionId":"page","method":"Runtime.executionContextDestroyed","params":{"executionContextUniqueId":"unique"}}),
        );
        assert!(lock(&contexts)["page"].is_empty());
        assert_eq!(lock(&contexts)["other"].len(), 1);
        record_context(
            &contexts,
            &json!({"sessionId":"other","method":"Runtime.executionContextsCleared","params":{}}),
        );
        assert!(!lock(&contexts).contains_key("other"));
    }

    #[tokio::test]
    async fn canceled_attach_detaches_a_late_successful_session() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let (seen, request_seen) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
                panic!("expected attach request")
            };
            let request: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(request["method"], "Target.attachToTarget");
            seen.send(()).unwrap();
            released.await.unwrap();
            socket
                .send(Message::Text(
                    json!({"id":request["id"],"result":{"sessionId":"late-session"}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            let Message::Text(text) = socket.next().await.unwrap().unwrap() else {
                panic!("expected detach request")
            };
            let detached: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(detached["method"], "Target.detachFromTarget");
            assert_eq!(detached["params"]["sessionId"], "late-session");
        });
        let (cdp, _events) = Cdp::connect(&url, Duration::from_secs(2)).await.unwrap();
        let mut call = Box::pin(cdp.call(
            None,
            "Target.attachToTarget",
            json!({"targetId":"tab","flatten":true}),
        ));
        tokio::select! {
            result = &mut call => panic!("attach completed before release: {result:?}"),
            result = request_seen => result.unwrap(),
        }
        drop(call);
        assert!(lock(&cdp.pending).is_empty());
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
    }
}
