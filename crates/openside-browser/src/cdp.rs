use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use crate::BrowserError;

type Reply = oneshot::Sender<Result<Value, BrowserError>>;

pub(crate) struct Cdp {
    next_id: Arc<AtomicU64>,
    pending: Arc<Mutex<HashMap<u64, Reply>>>,
    outgoing: mpsc::UnboundedSender<Message>,
    task: JoinHandle<()>,
    timeout: Duration,
    // Opt-in audit of issued commands; None (the default) records nothing.
    recorder: Mutex<Option<Vec<(String, Value)>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct PendingRequest<'a> {
    id: u64,
    pending: &'a Mutex<HashMap<u64, Reply>>,
}

impl Drop for PendingRequest<'_> {
    fn drop(&mut self) {
        lock(self.pending).remove(&self.id);
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
                                        if let Err(Ok(orphan)) = reply.send(result) {
                                            if let Some(session) = orphan["sessionId"].as_str() {
                                                let detach = json!({"id":actor_ids.fetch_add(1, Ordering::Relaxed),"method":"Target.detachFromTarget","params":{"sessionId":session}});
                                                if sink.send(Message::Text(detach.to_string().into())).await.is_err() { break; }
                                            }
                                        }
                                    } else if let Some(session) = value["result"]["sessionId"].as_str() {
                                        // A canceled attach can still succeed remotely; never leave its session attached.
                                        let detach = json!({"id":actor_ids.fetch_add(1, Ordering::Relaxed),"method":"Target.detachFromTarget","params":{"sessionId":session}});
                                        if sink.send(Message::Text(detach.to_string().into())).await.is_err() { break; }
                                    }
                                } else if events.send(value).is_err() {
                                    break;
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
            },
            receiver,
        ))
    }

    pub(crate) async fn call(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, BrowserError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Some(log) = lock(&self.recorder).as_mut() {
            log.push((method.to_owned(), params.clone()));
        }
        let mut request = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            request["sessionId"] = Value::String(session.to_owned());
        }
        let (sender, receiver) = oneshot::channel();
        lock(&self.pending).insert(id, sender);
        // Removing canceled requests prevents attach timeouts from accumulating reply senders.
        let _request = PendingRequest {
            id,
            pending: &self.pending,
        };
        self.outgoing
            .send(Message::Text(request.to_string().into()))
            .map_err(|_| BrowserError::Disconnected)?;
        tokio::time::timeout(self.timeout, receiver)
            .await
            .map_err(|_| BrowserError::Timeout("CDP command"))?
            .map_err(|_| BrowserError::Disconnected)?
    }

    /// Queue a command without awaiting its reply, for paths that cannot await (event
    /// handling, `Drop`). The reply is discarded; message order is preserved.
    pub(crate) fn send(&self, session: Option<&str>, method: &str, params: Value) {
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
