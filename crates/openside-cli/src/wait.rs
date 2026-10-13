//! Waiting for a session's run to end, shared by `openside run --wait` and the MCP tools: the
//! session's SSE stream is replayed from a known sequence and read until the run settles,
//! stops, fails, or the timeout passes.

use std::time::Duration;

use http_body_util::BodyExt;
use openside_proto::{Event, EventKind};
use serde_json::{json, Value};

use crate::{client::Client, sse::Decoder};

/// Where a wait starts and which part of the replayed log counts.
pub(crate) struct Replay {
    /// Events up to and including this sequence are not replayed.
    pub(crate) last_seen: u64,
    /// A settlement, stop or error at or before this sequence is history, not an outcome.
    pub(crate) terminal_after: u64,
    /// The message whose turn is awaited, when the wait follows a send.
    pub(crate) message_id: Option<String>,
}

/// When a wait gives up and whether a settlement with pending tickets ends it.
pub(crate) struct Until {
    /// `None` waits without a deadline.
    pub(crate) timeout: Option<Duration>,
    /// Keep waiting while the run is suspended on a person's ticket: the run resumes once the
    /// ticket is answered in the web or phone UI (or with `openside ticket`).
    pub(crate) through_human: bool,
}

struct Wait {
    last_seq: u64,
    terminal_after: u64,
    message_id: Option<String>,
    /// Replies are only ours once our message was dispatched; earlier queued turns may still
    /// finish first.
    started: bool,
    replies: Vec<String>,
    message_done: bool,
    status: Option<&'static str>,
    error: Option<String>,
}

impl Wait {
    fn new(replay: Replay) -> Self {
        Self {
            last_seq: replay.last_seen,
            terminal_after: replay.terminal_after,
            started: replay.message_id.is_none(),
            message_id: replay.message_id,
            replies: Vec::new(),
            message_done: false,
            status: None,
            error: None,
        }
    }

    fn is_ours(&self, id: &str) -> bool {
        self.message_id.as_deref() == Some(id)
    }

    fn apply(&mut self, seq: u64, kind: &EventKind) {
        let current = seq > self.terminal_after;
        match kind {
            EventKind::AssistantFinal { text } if self.started => self.replies.push(text.clone()),
            EventKind::MessageInjected { message_id } if self.is_ours(message_id.as_str()) => {
                self.started = true;
            }
            EventKind::MessageCompleted { message_id: id, .. }
            | EventKind::MessageDropped { message_id: id, .. }
                if self.is_ours(id.as_str()) =>
            {
                self.message_done = true;
            }
            EventKind::RuntimeError { code, message, .. }
                if current && code != "sandbox_unavailable" =>
            {
                self.error = Some(format!("{code}: {message}"));
                self.status = Some("error");
            }
            EventKind::SessionSettled {}
                if current && (self.message_id.is_none() || self.message_done) =>
            {
                self.status = Some("settled");
            }
            EventKind::SessionStopped {} if current => self.status = Some("stopped"),
            _ => {}
        }
    }
}

/// Pending tickets of `session`, summarized as `{id, kind, summary}`.
pub(crate) async fn pending_tickets(client: &Client, session: &str) -> Result<Vec<Value>, String> {
    let tickets: Value = client.get("/v1/tickets?status=pending").await?;
    Ok(tickets["tickets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|ticket| ticket["session_id"].as_str() == Some(session))
        .map(|ticket| json!({"id": ticket["id"], "kind": ticket["kind"], "summary": ticket["summary"]}))
        .collect())
}

/// Streams the session's events after `replay.last_seen` until the run ends or the timeout
/// passes (reported as status `timeout`, not as an error). `on_event` sees every new event.
///
/// The result is `{status, last_seq, replies, pending_tickets, error}` with status
/// `settled`, `waiting_human`, `stopped`, `error` or `timeout`.
pub(crate) async fn settle<F: FnMut(&Event)>(
    client: &Client,
    session: &str,
    replay: Replay,
    until: Until,
    mut on_event: F,
) -> Result<Value, String> {
    let mut state = Wait::new(replay);
    let drive = drive(
        client,
        session,
        &mut state,
        until.through_human,
        &mut on_event,
    );
    match until.timeout {
        Some(timeout) => match tokio::time::timeout(timeout, drive).await {
            Ok(result) => result?,
            Err(_) => state.status = Some("timeout"),
        },
        None => drive.await?,
    }
    // Read at the end, not before the replay: tickets opened and resolved while we were
    // catching up must not be reported, and one opened last must not be missed.
    let pending = pending_tickets(client, session).await?;
    if state.status == Some("settled") && !pending.is_empty() {
        state.status = Some("waiting_human");
    }
    Ok(json!({
        "status": state.status,
        "last_seq": state.last_seq,
        "replies": state.replies,
        "pending_tickets": pending,
        "error": state.error,
    }))
}

async fn drive<F: FnMut(&Event)>(
    client: &Client,
    session: &str,
    state: &mut Wait,
    through_human: bool,
    on_event: &mut F,
) -> Result<(), String> {
    let mut response = client.events(session, state.last_seq).await?;
    let mut decoder = Decoder::default();
    while let Some(frame) = response.body_mut().frame().await {
        let frame = frame.map_err(|_| "event stream connection failed".to_owned())?;
        let Ok(bytes) = frame.into_data() else {
            continue;
        };
        for record in decoder.push(&bytes)? {
            let event: Event = serde_json::from_slice(&record.data)
                .map_err(|_| "invalid event JSON in SSE stream".to_owned())?;
            if event.session_id.as_str() != session
                || event.seq == 0
                || record.id != Some(event.seq)
            {
                return Err("SSE event session/sequence does not match its envelope or id".into());
            }
            if event.seq <= state.last_seq {
                continue;
            }
            state.last_seq = event.seq;
            on_event(&event);
            state.apply(event.seq, &event.kind);
            if state.status == Some("settled")
                && through_human
                && !pending_tickets(client, session).await?.is_empty()
            {
                // Suspended on a person's answer; the run resumes once it is given.
                state.status = None;
            }
            if state.status.is_some() {
                return Ok(());
            }
        }
    }
    Err("event stream ended before the session settled or stopped".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_warning_does_not_end_a_wait_or_discard_the_reply() {
        let mut state = Wait::new(Replay {
            last_seen: 0,
            terminal_after: 0,
            message_id: None,
        });
        state.apply(
            1,
            &EventKind::runtime_error("sandbox_unavailable", "policy-only isolation"),
        );
        assert_eq!(state.status, None);
        assert_eq!(state.error, None);
        state.apply(
            2,
            &EventKind::AssistantFinal {
                text: "completed".into(),
            },
        );
        state.apply(3, &EventKind::SessionSettled {});
        assert_eq!(state.status, Some("settled"));
        assert_eq!(state.replies, ["completed"]);
        assert_eq!(state.error, None);
    }

    #[test]
    fn actual_runtime_errors_still_end_a_wait() {
        let mut state = Wait::new(Replay {
            last_seen: 0,
            terminal_after: 0,
            message_id: None,
        });
        state.apply(
            1,
            &EventKind::runtime_error("auth_unavailable", "credentials missing"),
        );
        assert_eq!(state.status, Some("error"));
        assert_eq!(
            state.error.as_deref(),
            Some("auth_unavailable: credentials missing")
        );
    }
}
