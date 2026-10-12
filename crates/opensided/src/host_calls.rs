use std::{collections::HashMap, sync::Arc};

use openside_proto::SessionId;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub(crate) struct Calls(parking_lot::Mutex<HashMap<(SessionId, String), Arc<TaskCalls>>>);

#[derive(Default)]
struct TaskCalls {
    cancel: CancellationToken,
    active: parking_lot::Mutex<usize>,
    drained: Notify,
}

pub(crate) struct ActiveCall(Arc<TaskCalls>);

impl Calls {
    pub(crate) fn enter(&self, session: &SessionId, task: &str) -> ActiveCall {
        let mut tasks = self.0.lock();
        let calls = tasks
            .entry((session.clone(), task.to_owned()))
            .or_default()
            .clone();
        *calls.active.lock() += 1;
        ActiveCall(calls)
    }

    pub(crate) async fn cancel(&self, session: &SessionId, task: &str) {
        let calls = self
            .0
            .lock()
            .entry((session.clone(), task.to_owned()))
            .or_default()
            .clone();
        calls.cancel.cancel();
        loop {
            let drained = calls.drained.notified();
            tokio::pin!(drained);
            drained.as_mut().enable();
            if *calls.active.lock() == 0 {
                return;
            }
            drained.await;
        }
    }
}

impl ActiveCall {
    pub(crate) fn token(&self) -> CancellationToken {
        self.0.cancel.clone()
    }
}

impl Drop for ActiveCall {
    fn drop(&mut self) {
        *self.0.active.lock() -= 1;
        self.0.drained.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancellation_fences_late_calls_and_waits_for_cleanup_without_cancelling_siblings() {
        let calls = Arc::new(Calls::default());
        let session = SessionId::new("session");
        let active = calls.enter(&session, "first");
        let sibling = calls.enter(&session, "second");
        let token = active.token();
        let cancelling = {
            let calls = calls.clone();
            let session = session.clone();
            tokio::spawn(async move { calls.cancel(&session, "first").await })
        };
        token.cancelled().await;
        assert!(!cancelling.is_finished());
        assert!(!sibling.token().is_cancelled());
        let late = calls.enter(&session, "first");
        assert!(late.token().is_cancelled());
        drop(late);
        drop(active);
        cancelling.await.unwrap();
    }
}
