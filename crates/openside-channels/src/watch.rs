use std::{sync::Arc, time::Duration};

use openside_proto::{Event, EventKind, MessageOutcome};
use tokio::time::Instant;

use crate::{
    core::{Core, Watcher},
    error::log,
    messenger::Messenger,
    model::{MsgRef, ThreadKey},
    text::split_index,
};

const MAX_BACKOFF: Duration = Duration::from_secs(10);

/// Assistant text being streamed into one chat message (or several, past the platform limit).
struct Reply {
    text: String,
    /// Bytes of `text` already finalized in earlier messages.
    committed: usize,
    message: Option<MsgRef>,
    /// Text currently shown in `message`.
    shown: String,
    dirty: bool,
    last_edit: Instant,
}

impl Reply {
    fn new() -> Self {
        Self {
            text: String::new(),
            committed: 0,
            message: None,
            shown: String::new(),
            dirty: false,
            last_edit: Instant::now(),
        }
    }
}

impl<M: Messenger> Core<M> {
    /// Keep exactly one event watcher per mapped session of the thread.
    pub(crate) fn ensure_watcher(self: &Arc<Self>, thread: &ThreadKey, session: &str) {
        let mut watchers = self.watchers.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(existing) = watchers.get(&thread.id()) {
            if existing.session == session && !existing.task.is_finished() {
                return;
            }
            existing.task.abort();
        }
        let core = self.clone();
        let (key, id) = (thread.clone(), session.to_owned());
        let task = tokio::spawn(async move { core.watch(key, id).await });
        watchers.insert(
            thread.id(),
            Watcher {
                session: session.to_owned(),
                task,
            },
        );
    }

    pub(crate) fn stop_watcher(&self, thread: &ThreadKey) {
        let mut watchers = self.watchers.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(watcher) = watchers.remove(&thread.id()) {
            watcher.task.abort();
        }
    }

    /// Re-attach to every persisted thread after a restart so approval cards and
    /// replies produced while the bot was down still arrive.
    pub async fn resume_all(self: &Arc<Self>) {
        for state in self.store.sessions(M::PLATFORM) {
            if let Some(session) = &state.session_id {
                // Rebinding must not replay another owner's replies before the next inbound.
                if self.daemon.has_owner()
                    && !self
                        .daemon
                        .session(session)
                        .await
                        .is_ok_and(|session| self.daemon.owns_session(&session))
                {
                    continue;
                }
                self.ensure_watcher(&state.key, session);
            }
        }
    }

    /// Stop every watcher; used on shutdown so no task outlives the runtime.
    pub fn stop_all(&self) {
        let mut watchers = self.watchers.lock().unwrap_or_else(|p| p.into_inner());
        for (_, watcher) in watchers.drain() {
            watcher.task.abort();
        }
    }

    async fn watch(self: Arc<Self>, thread: ThreadKey, session: String) {
        let interval = self.options.edit_interval;
        let mut cursor = self.store.get(&thread).map_or(0, |state| state.cursor);
        let mut reply: Option<Reply> = None;
        let mut backoff = Duration::from_millis(500);
        loop {
            if self.shutdown.is_cancelled() {
                return;
            }
            match self.daemon.events(&session, cursor).await {
                Ok(mut stream) => {
                    backoff = Duration::from_millis(500);
                    loop {
                        let due = reply
                            .as_ref()
                            .filter(|reply| reply.dirty)
                            .map(|reply| reply.last_edit + interval);
                        tokio::select! {
                            _ = self.shutdown.cancelled() => return,
                            _ = async {
                                match due {
                                    Some(at) => tokio::time::sleep_until(at).await,
                                    None => std::future::pending().await,
                                }
                            } => {
                                if let Some(reply) = reply.as_mut() {
                                    self.flush(&thread, reply).await;
                                }
                            }
                            item = stream.next() => match item {
                                Some(Ok(event)) if event.seq > cursor => {
                                    cursor = event.seq;
                                    self.on_event(&thread, &mut reply, event).await;
                                }
                                Some(Ok(_)) => {}
                                Some(Err(_)) | None => break,
                            }
                        }
                    }
                }
                Err(error) if error.is_not_found() => {
                    self.session_gone(&thread, &session).await;
                    return;
                }
                Err(error) => log(format_args!("{} event stream: {error}", M::PLATFORM)),
            }
            tokio::select! {
                _ = self.shutdown.cancelled() => return,
                _ = tokio::time::sleep(backoff) => {}
            }
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    }

    async fn session_gone(&self, thread: &ThreadKey, session: &str) {
        let _ = self.store.update(thread, |state| {
            if state.session_id.as_deref() == Some(session) {
                state.session_id = None;
                state.cursor = 0;
                state.cards.clear();
            }
        });
        self.say(
            thread,
            "This thread's session no longer exists. Send a message to start a new one.",
        )
        .await;
    }

    async fn on_event(&self, thread: &ThreadKey, reply: &mut Option<Reply>, event: Event) {
        let seq = event.seq;
        // The cursor only moves past events whose effect is fully visible in chat, so a
        // crash replays an unfinished reply instead of losing it.
        let mut durable = false;
        match event.kind {
            EventKind::AssistantDelta { text } => {
                let current = reply.get_or_insert_with(Reply::new);
                current.text.push_str(&text);
                current.dirty = true;
                if current.message.is_none() {
                    self.flush(thread, current).await;
                }
            }
            EventKind::AssistantFinal { text, .. } => {
                let current = reply.get_or_insert_with(Reply::new);
                current.text = text;
                self.flush(thread, current).await;
                *reply = None;
                durable = true;
            }
            EventKind::MessageCompleted {
                outcome, reason, ..
            } => {
                self.finish(thread, reply).await;
                if outcome == MessageOutcome::Failed {
                    let detail = reason
                        .map(|reason| format!(": {reason}"))
                        .unwrap_or_default();
                    self.say(thread, &format!("The run failed{detail}")).await;
                }
                durable = true;
            }
            EventKind::SessionSettled {} | EventKind::SessionStopped {} => {
                self.finish(thread, reply).await;
                durable = true;
            }
            EventKind::RuntimeError { user_message, .. } => {
                let text = if user_message.is_empty() {
                    "Something went wrong on my side."
                } else {
                    &user_message
                };
                self.say(thread, text).await;
                durable = true;
            }
            EventKind::TicketOpened { ticket } => {
                self.post_card(thread, *ticket).await;
                durable = true;
            }
            EventKind::TicketResolved { ticket_id, .. } => {
                self.settle_card(thread, ticket_id.as_str(), "Resolved")
                    .await;
                durable = true;
            }
            EventKind::TicketRejected { ticket_id, .. } => {
                self.settle_card(thread, ticket_id.as_str(), "Rejected")
                    .await;
                durable = true;
            }
            EventKind::TicketExpired { ticket_id, .. } => {
                self.settle_card(thread, ticket_id.as_str(), "Expired")
                    .await;
                durable = true;
            }
            _ => {}
        }
        if durable {
            let _ = self
                .store
                .update(thread, |state| state.cursor = state.cursor.max(seq));
        }
    }

    /// Flush and drop an unfinished reply (run ended without `assistant.final`).
    async fn finish(&self, thread: &ThreadKey, reply: &mut Option<Reply>) {
        if let Some(mut current) = reply.take() {
            self.flush(thread, &mut current).await;
        }
    }

    /// Bring the chat message(s) up to date with the reply text, splitting at the
    /// platform message limit and editing in place otherwise.
    async fn flush(&self, thread: &ThreadKey, reply: &mut Reply) {
        reply.dirty = false;
        reply.last_edit = Instant::now();
        let max = self.messenger.max_chars();
        while reply.text[reply.committed..].chars().count() > max {
            let cut = split_index(&reply.text[reply.committed..], max);
            let chunk = reply.text[reply.committed..reply.committed + cut].to_owned();
            self.put(thread, reply, &chunk).await;
            reply.committed += cut;
            reply.message = None;
            reply.shown.clear();
        }
        let pending = reply.text[reply.committed..].to_owned();
        if pending.trim().is_empty() || pending == reply.shown {
            return;
        }
        self.put(thread, reply, &pending).await;
    }

    async fn put(&self, thread: &ThreadKey, reply: &mut Reply, text: &str) {
        if let Some(message) = &reply.message {
            match self.messenger.edit(thread, message, text, &[]).await {
                Ok(()) => {
                    reply.shown = text.to_owned();
                    return;
                }
                Err(error) => {
                    log(format_args!(
                        "{} edit failed, posting anew: {error}",
                        M::PLATFORM
                    ));
                    reply.message = None;
                }
            }
        }
        match self.messenger.send(thread, text, &[]).await {
            Ok(message) => {
                reply.message = Some(message);
                reply.shown = text.to_owned();
            }
            Err(error) => log(format_args!("{} send failed: {error}", M::PLATFORM)),
        }
    }
}
