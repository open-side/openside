use std::collections::{BTreeMap, BTreeSet};

use openside_core::{
    MessageId, MessageQueue, MessageStatus, QueueMode, SessionId, SubmitOutcome, TicketId,
};
use openside_proto::{
    Attachment, Event, EventKind, Message, MessageOutcome, PolicyMode, Session, SessionStatus,
    Ticket, TicketDeferral, TicketStatus,
};

use crate::{Error, Result};

/// Replayed durable session state, including terminal message and ticket records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionState {
    /// Stable session identifier.
    pub id: SessionId,
    /// Conversation title supplied by the creation event.
    pub title: String,
    /// Provider-qualified model supplied by the creation event.
    pub model: String,
    /// Permission ceiling supplied by the creation event.
    pub mode: PolicyMode,
    /// Canonical absolute workspace path used by daemon policy and file tools.
    pub workspace: Option<String>,
    /// User-task lifecycle state, independent of individual model requests.
    pub status: SessionStatus,
    /// Latest applied per-session sequence.
    pub last_seq: u64,
    /// Queue with pending order, edited text, and terminal idempotency history.
    pub queue: MessageQueue,
    /// All dispatched message identifiers, including interrupted dispatches.
    pub injected: BTreeSet<MessageId>,
    /// Terminal outcomes; injected identifiers absent here need reconciliation.
    pub completions: BTreeMap<MessageId, MessageOutcome>,
    /// All ticket records; filter `Pending` when exposing outstanding human actions.
    pub tickets: BTreeMap<TicketId, Ticket>,
    /// Set by `session.deleted`; the log is retained but the session is hidden and frozen.
    pub deleted: bool,
    /// Latest `session.updated` pinned flag.
    pub pinned: bool,
    /// Latest `session.updated` archived flag.
    pub archived: bool,
    /// Fixed by `session.created`; private sessions are never indexed into memory.
    pub incognito: bool,
    /// Original text of the first queued message, used for list search.
    pub first_message: Option<String>,
    /// Stored attachments per message, delivered to the runtime with the message.
    pub attachments: BTreeMap<MessageId, Vec<Attachment>>,
}

impl SessionState {
    /// Public projection with the pending queue and derived human-waiting status.
    pub fn session(&self) -> Session {
        Session {
            id: self.id.clone(),
            title: self.title.clone(),
            model: self.model.clone(),
            mode: self.mode,
            workspace: self.workspace.clone(),
            status: if self.status == SessionStatus::Idle
                && self
                    .tickets
                    .values()
                    .any(|ticket| ticket.status == TicketStatus::Pending)
            {
                SessionStatus::WaitingHuman
            } else {
                self.status
            },
            last_seq: self.last_seq,
            queue: self
                .queue
                .pending_ids()
                .filter_map(|id| {
                    Some(Message {
                        text: self.queue.text(id)?.to_owned(),
                        status: self.queue.status(id)?.clone(),
                        id: id.clone(),
                    })
                })
                .collect(),
            pinned: self.pinned,
            archived: self.archived,
            incognito: self.incognito,
        }
    }

    pub(crate) fn created(event: &Event) -> Result<Self> {
        let EventKind::SessionCreated {
            title,
            model,
            mode,
            workspace,
            incognito,
        } = &event.kind
        else {
            return Err(Error::InvalidEvent(
                "first event must be session.created".into(),
            ));
        };
        if event.seq != 1 {
            return Err(Error::InvalidEvent("first sequence must be one".into()));
        }
        Ok(Self {
            id: event.session_id.clone(),
            title: title.clone(),
            model: model.clone(),
            mode: *mode,
            workspace: workspace.clone(),
            status: SessionStatus::Idle,
            last_seq: 1,
            queue: MessageQueue::new(),
            injected: BTreeSet::new(),
            completions: BTreeMap::new(),
            tickets: BTreeMap::new(),
            deleted: false,
            pinned: false,
            archived: false,
            incognito: *incognito,
            first_message: None,
            attachments: BTreeMap::new(),
        })
    }

    pub(crate) fn validate(&self, kind: &EventKind) -> Result<()> {
        if self.deleted {
            return Err(Error::InvalidEvent("session was deleted".into()));
        }
        match kind {
            EventKind::SessionCreated { .. } => return Err(Error::SessionExists(self.id.clone())),
            EventKind::MessageQueued { message_id, .. } => {
                if self.queue.status(message_id).is_some() {
                    return Err(Error::InvalidEvent(
                        "duplicate submission in event log".into(),
                    ));
                }
            }
            EventKind::MessageEdited { message_id, .. }
            | EventKind::MessagePromoted { message_id }
            | EventKind::MessageMoved { message_id, .. }
            | EventKind::MessageDropped { message_id, .. } => {
                if !matches!(
                    self.queue.status(message_id),
                    Some(MessageStatus::Queued(_))
                ) {
                    return Err(Error::InvalidEvent(format!(
                        "message {message_id} is not pending"
                    )));
                }
            }
            EventKind::MessageInjected { message_id } => {
                if self.queue.pending_ids().next() != Some(message_id) {
                    return Err(Error::InvalidEvent(format!(
                        "message {message_id} is not the delivery head"
                    )));
                }
            }
            EventKind::MessageCompleted {
                message_id,
                outcome,
                ..
            } => {
                // `deferred` is the only non-terminal outcome: the run paused on a ticket and
                // the same message later finishes (or is aborted) with a terminal outcome.
                let settles_deferral = *outcome != MessageOutcome::Deferred
                    && self.completions.get(message_id) == Some(&MessageOutcome::Deferred);
                if !self.injected.contains(message_id)
                    || (self.completions.contains_key(message_id) && !settles_deferral)
                {
                    return Err(Error::InvalidEvent(format!(
                        "message {message_id} was not injected or already completed"
                    )));
                }
            }
            EventKind::TicketOpened { ticket } => {
                if ticket.session_id != self.id
                    || self.tickets.contains_key(&ticket.id)
                    || ticket.status != TicketStatus::Pending
                    || ticket.resolution.is_some()
                    || (ticket.deferral == TicketDeferral::Approval && ticket.tool.is_none())
                {
                    return Err(Error::InvalidEvent("invalid pending ticket".into()));
                }
            }
            EventKind::TicketResolved { ticket_id, .. }
            | EventKind::TicketRejected { ticket_id, .. }
            | EventKind::TicketExpired { ticket_id }
                if !self
                    .tickets
                    .get(ticket_id)
                    .is_some_and(|ticket| ticket.status == TicketStatus::Pending) =>
            {
                return Err(Error::InvalidEvent(format!(
                    "ticket {ticket_id} is not pending"
                )));
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn apply(&mut self, event: &Event) -> Result<()> {
        if event.session_id != self.id || self.last_seq.checked_add(1) != Some(event.seq) {
            return Err(Error::InvalidEvent(
                "session mismatch or sequence gap".into(),
            ));
        }
        self.validate(&event.kind)?;
        match &event.kind {
            EventKind::MessageQueued {
                message_id,
                text,
                mode,
                attachments,
            } => {
                if self.queue.submit(message_id.clone(), text.clone(), *mode)
                    != SubmitOutcome::Accepted
                {
                    return Err(Error::InvalidEvent("duplicate submission".into()));
                }
                if self.first_message.is_none() {
                    self.first_message = Some(text.clone());
                }
                if !attachments.is_empty() {
                    self.attachments
                        .insert(message_id.clone(), attachments.clone());
                }
            }
            EventKind::MessageEdited { message_id, text } => self
                .queue
                .edit(message_id, text.clone())
                .map_err(queue_error)?,
            EventKind::MessagePromoted { message_id } => {
                self.queue.promote(message_id).map_err(queue_error)?
            }
            EventKind::MessageMoved {
                message_id,
                position,
            } => self
                .queue
                .reorder(message_id, *position as usize)
                .map_err(queue_error)?,
            EventKind::MessageDropped { message_id, reason } => {
                self.queue
                    .drop_message(message_id, reason.clone())
                    .map_err(queue_error)?;
            }
            EventKind::MessageInjected { message_id } => {
                let delivered = match self.queue.status(message_id) {
                    Some(MessageStatus::Queued(QueueMode::Steer)) => self.queue.take_steer(),
                    Some(MessageStatus::Queued(QueueMode::Queue)) => self.queue.take_follow_up(),
                    _ => None,
                };
                if delivered.as_ref().map(|(id, _)| id) != Some(message_id) {
                    return Err(Error::InvalidEvent("delivery order mismatch".into()));
                }
                self.injected.insert(message_id.clone());
                self.status = SessionStatus::Running;
            }
            EventKind::MessageCompleted {
                message_id,
                outcome,
                ..
            } => {
                self.completions.insert(message_id.clone(), *outcome);
                if self.status != SessionStatus::Stopped {
                    self.status = if self.injected.len() > self.completions.len() {
                        SessionStatus::Running
                    } else {
                        SessionStatus::Idle
                    };
                }
            }
            EventKind::TicketOpened { ticket } => {
                self.tickets
                    .insert(ticket.id.clone(), ticket.as_ref().clone());
            }
            EventKind::TicketResolved {
                ticket_id,
                resolution,
            } => {
                let ticket = self
                    .tickets
                    .get_mut(ticket_id)
                    .ok_or_else(|| Error::InvalidEvent("unknown ticket".into()))?;
                ticket.status = TicketStatus::Resolved;
                ticket.resolution = Some(resolution.clone());
            }
            EventKind::TicketRejected { ticket_id, reason } => {
                let ticket = self
                    .tickets
                    .get_mut(ticket_id)
                    .ok_or_else(|| Error::InvalidEvent("unknown ticket".into()))?;
                ticket.status = TicketStatus::Rejected;
                ticket.resolution = Some(serde_json::json!({
                    "status": "rejected",
                    "reason": reason,
                }));
            }
            EventKind::TicketExpired { ticket_id } => {
                let ticket = self
                    .tickets
                    .get_mut(ticket_id)
                    .ok_or_else(|| Error::InvalidEvent("unknown ticket".into()))?;
                ticket.status = TicketStatus::Expired;
            }
            EventKind::TurnStarted { .. } => self.status = SessionStatus::Running,
            // A model request can end before its tools and the user task finish.
            EventKind::TurnEnded { .. } => {}
            EventKind::SessionSettled { .. } => self.status = SessionStatus::Idle,
            EventKind::SessionStopped { .. } => self.status = SessionStatus::Stopped,
            EventKind::SessionDeleted { .. } => self.deleted = true,
            EventKind::SessionUpdated {
                title,
                pinned,
                archived,
            } => {
                if let Some(title) = title {
                    self.title.clone_from(title);
                }
                if let Some(pinned) = pinned {
                    self.pinned = *pinned;
                }
                if let Some(archived) = archived {
                    self.archived = *archived;
                }
            }
            _ => {}
        }
        self.last_seq = event.seq;
        Ok(())
    }
}

fn queue_error(error: openside_core::QueueError) -> Error {
    Error::InvalidEvent(format!("queue operation rejected: {error:?}"))
}
