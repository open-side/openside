use std::future::Future;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::sync::mpsc;

pub(crate) const QUEUE: usize = 8;
pub(crate) const CONTROL_SLOTS: usize = 4;
const WRITE_DEADLINE: Duration = Duration::from_secs(5);
const CLOSE_GRACE: Duration = Duration::from_secs(1);

pub(crate) fn text(message: &impl Serialize) -> Message {
    Message::Text(serde_json::to_string(message).unwrap_or_default().into())
}

// Writes cannot pin capture resources indefinitely when a peer stops reading.
pub(crate) async fn write_all<T, F, Fut>(
    mut sink: futures_util::stream::SplitSink<WebSocket, Message>,
    mut queue: mpsc::Receiver<T>,
    mut admit: F,
) where
    F: FnMut(T) -> Fut,
    Fut: Future<Output = Option<Message>>,
{
    while let Some(queued) = queue.recv().await {
        let Some(message) = admit(queued).await else {
            continue;
        };
        if !matches!(
            tokio::time::timeout(WRITE_DEADLINE, sink.send(message)).await,
            Ok(Ok(()))
        ) {
            return;
        }
    }
}

pub(crate) async fn close<T>(
    out: mpsc::Sender<T>,
    mut writer: tokio::task::JoinHandle<()>,
    mut source: futures_util::stream::SplitStream<WebSocket>,
    ended: Option<T>,
    close: T,
) {
    let farewell = async {
        if let Some(ended) = ended {
            let _ = out.send(ended).await;
        }
        let _ = out.send(close).await;
        drop(out);
        let _ = (&mut writer).await;
        // An unread close reply can cause a TCP reset that discards the final message.
        while let Some(Ok(message)) = source.next().await {
            if matches!(message, Message::Close(_)) {
                break;
            }
        }
    };
    if tokio::time::timeout(CLOSE_GRACE, farewell).await.is_err() {
        writer.abort();
    }
}
