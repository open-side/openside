use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use openside_core::{
    MessageId, MessageQueue, MessageStatus, QueueMode, SessionId, SubmitOutcome, TicketId,
};
use openside_proto::{
    Attachment, BrowserSpace, Event, EventKind, Message, MessageOutcome, PolicyMode, Session,
    SessionStatus, Ticket, TicketDeferral, TicketStatus,
};

use crate::{Error, Result};

/// Replayed durable session state, including terminal message and ticket records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionState {
    /// Immutable creation tag; home identity is owned by the separate agent registry.
    pub agent_id: Option<String>,
    /// Stable session identifier.
    pub id: SessionId,
    /// Conversation title supplied by the creation event.
    pub title: String,
    /// A supplied title or a metadata rename prevents model replacement, even mid-request.
    pub title_locked: bool,
    /// Only the first valid model proposal can name a conversation.
    pub title_proposed: bool,
    /// Provider-qualified model supplied by the creation event.
    pub model: String,
    /// Permission ceiling supplied by the creation event.
    pub mode: PolicyMode,
    /// Canonical absolute workspace path used by daemon policy and file tools.
    pub workspace: Option<String>,
    /// User-task lifecycle state, independent of individual model requests.
    pub status: SessionStatus,
    /// Latest short activity outside a human-waiting state.
    pub activity: String,
    /// Background work can still be browsing after the front agent has replied.
    pub active_subagents: BTreeSet<String>,
    waiting_subagents: BTreeSet<String>,
    foreground_running: bool,
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
    /// Durable presentations keyed by daemon-assigned identity for replay idempotency.
    pub cards: BTreeMap<String, openside_proto::Card>,
    /// Agent-client identities recorded durably before their work reaches the runtime.
    pub driver_ids: BTreeSet<String>,
    /// Set by `session.deleted`; the log is retained but the session is hidden and frozen.
    pub deleted: bool,
    /// Latest `session.updated` pinned flag.
    pub pinned: bool,
    /// Latest `session.updated` archived flag.
    pub archived: bool,
    /// Fixed by `session.created`; private sessions are never indexed into memory.
    pub incognito: bool,
    /// Fixed by `session.created`; isolated sessions browse in their own browser context.
    pub browser_space: BrowserSpace,
    /// Original text of the first queued message, used for list search.
    pub first_message: Option<String>,
    /// Bounded latest message preview, rebuilt during replay instead of rescanning each list.
    pub last_message_preview: String,
    /// Stored attachments per message, delivered to the runtime with the message.
    pub attachments: BTreeMap<MessageId, Vec<Attachment>>,
    /// Immutable daemon authority, retained after delivery for authorization and replay.
    pub message_authors: BTreeMap<MessageId, openside_proto::MessageAuthorKind>,
    /// Only actual team deliveries have a sender; delivery never erases this provenance.
    pub message_senders: BTreeMap<MessageId, String>,
    /// Timestamp of the latest event other than a metadata update; orders the session list.
    pub last_activity: DateTime<Utc>,
}

impl SessionState {
    /// Public projection with the pending queue and derived human-waiting status.
    pub fn session(&self) -> Session {
        Session {
            agent_id: self.agent_id.clone(),
            id: self.id.clone(),
            title: self.title.clone(),
            model: self.model.clone(),
            mode: self.mode,
            workspace: self.workspace.clone(),
            activity: if self
                .tickets
                .values()
                .any(|ticket| ticket.status == TicketStatus::Pending)
            {
                "Waiting for you".into()
            } else {
                self.activity.clone()
            },
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
                        author_kind: self.message_authors.get(id).copied().unwrap_or_default(),
                        from_agent: self.message_senders.get(id).cloned(),
                    })
                })
                .collect(),
            pinned: self.pinned,
            archived: self.archived,
            incognito: self.incognito,
            browser_space: self.browser_space,
        }
    }

    pub(crate) fn created(event: &Event) -> Result<Self> {
        let EventKind::SessionCreated {
            agent_id,
            title,
            model,
            mode,
            workspace,
            incognito,
            browser_space,
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
            agent_id: agent_id.clone(),
            id: event.session_id.clone(),
            title: title.clone(),
            title_locked: title != openside_proto::DEFAULT_SESSION_TITLE,
            title_proposed: false,
            model: model.clone(),
            mode: *mode,
            workspace: workspace.clone(),
            status: SessionStatus::Idle,
            activity: String::new(),
            active_subagents: BTreeSet::new(),
            waiting_subagents: BTreeSet::new(),
            foreground_running: false,
            last_seq: 1,
            queue: MessageQueue::new(),
            injected: BTreeSet::new(),
            completions: BTreeMap::new(),
            tickets: BTreeMap::new(),
            cards: BTreeMap::new(),
            driver_ids: BTreeSet::new(),
            deleted: false,
            pinned: false,
            archived: false,
            incognito: *incognito,
            browser_space: *browser_space,
            first_message: None,
            last_message_preview: String::new(),
            attachments: BTreeMap::new(),
            message_authors: BTreeMap::new(),
            message_senders: BTreeMap::new(),
            last_activity: event.ts,
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
            | EventKind::TicketExpired { ticket_id, .. }
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
        if let EventKind::MessageQueued { text, .. } | EventKind::AssistantFinal { text, .. } =
            &event.kind
        {
            self.last_message_preview = text.chars().take(160).collect();
        }
        match &event.kind {
            EventKind::MessageQueued {
                message_id,
                text,
                mode,
                attachments,
                author_kind,
                from_agent,
            } => {
                if self.queue.submit(message_id.clone(), text.clone(), *mode)
                    != SubmitOutcome::Accepted
                {
                    return Err(Error::InvalidEvent("duplicate submission".into()));
                }
                self.message_authors
                    .insert(message_id.clone(), *author_kind);
                if let Some(sender) = from_agent {
                    self.message_senders
                        .insert(message_id.clone(), sender.clone());
                }
                if self.first_message.is_none() {
                    self.first_message = Some(text.clone());
                }
                if self.status != SessionStatus::Running {
                    self.activity = "Working".into();
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
                self.foreground_running = true;
                self.status = SessionStatus::Running;
                self.activity = "Working".into();
            }
            EventKind::MessageCompleted {
                message_id,
                outcome,
                ..
            } => {
                self.completions.insert(message_id.clone(), *outcome);
                self.foreground_running = self.injected.len() > self.completions.len();
                if !self.foreground_running && self.active_subagents.is_empty() {
                    self.activity = match outcome {
                        MessageOutcome::Completed => "Done",
                        MessageOutcome::Aborted => "Stopped",
                        MessageOutcome::Failed => "Couldn't finish",
                        MessageOutcome::Deferred => "Waiting for you",
                    }
                    .into();
                }
                if self.status != SessionStatus::Stopped {
                    self.status = if self.foreground_running || !self.active_subagents.is_empty() {
                        SessionStatus::Running
                    } else {
                        SessionStatus::Idle
                    };
                }
            }
            EventKind::SessionDriverRecorded { device_id } => {
                self.driver_ids.insert(device_id.clone());
            }
            EventKind::CardPresented { card, .. } => {
                self.cards
                    .entry(card.id.clone())
                    .or_insert_with(|| card.clone());
            }
            EventKind::TeamHired {
                card: Some(card), ..
            }
            | EventKind::TeamHandoff {
                card: Some(card), ..
            }
            | EventKind::TeamHandoffEnded {
                card: Some(card), ..
            }
            | EventKind::TeamQuestion {
                card: Some(card), ..
            }
            | EventKind::TeamAnswered {
                card: Some(card), ..
            }
            | EventKind::TeamTransferred {
                card: Some(card), ..
            }
            | EventKind::TeamRetired {
                card: Some(card), ..
            }
            | EventKind::TeamRestored {
                card: Some(card), ..
            } => {
                self.cards.insert(card.id.clone(), card.clone());
            }
            EventKind::ToolEnded {
                card: Some(card), ..
            } => {
                if let Some(id) = &card.ticket_id {
                    if let Some(ticket) = self.tickets.get_mut(id) {
                        ticket.card = Some(card.clone());
                    }
                }
            }
            EventKind::TicketOpened { ticket } => {
                self.tickets
                    .insert(ticket.id.clone(), ticket.as_ref().clone());
            }
            EventKind::TicketResolved {
                card,
                ticket_id,
                resolution,
            } => {
                let ticket = self
                    .tickets
                    .get_mut(ticket_id)
                    .ok_or_else(|| Error::InvalidEvent("unknown ticket".into()))?;
                ticket.status = TicketStatus::Resolved;
                ticket.resolution = Some(resolution.clone());
                ticket.card = card.clone();
            }
            EventKind::TicketRejected {
                ticket_id,
                reason,
                card,
            } => {
                let ticket = self
                    .tickets
                    .get_mut(ticket_id)
                    .ok_or_else(|| Error::InvalidEvent("unknown ticket".into()))?;
                ticket.status = TicketStatus::Rejected;
                ticket.card = card.clone();
                ticket.resolution = Some(serde_json::json!({
                    "status": "rejected",
                    "reason": reason,
                }));
            }
            EventKind::TicketExpired { ticket_id, card } => {
                let ticket = self
                    .tickets
                    .get_mut(ticket_id)
                    .ok_or_else(|| Error::InvalidEvent("unknown ticket".into()))?;
                ticket.status = TicketStatus::Expired;
                ticket.card = card.clone();
            }
            EventKind::TurnStarted { .. } => {
                self.foreground_running = true;
                self.status = SessionStatus::Running;
                self.activity = "Working".into();
            }
            // A model request can end before its tools and the user task finish.
            EventKind::TurnEnded { .. } => {}
            EventKind::SessionSettled { .. } => {
                self.foreground_running = false;
                self.status = if self.active_subagents.is_empty() {
                    SessionStatus::Idle
                } else {
                    SessionStatus::Running
                };
                if !self.active_subagents.is_empty() {
                    self.activity = "Working".into();
                } else if self.activity != "Couldn't finish" && self.activity != "Stopped" {
                    self.activity = "Done".into();
                }
            }
            EventKind::SessionStopped { .. } => {
                self.foreground_running = false;
                self.status = SessionStatus::Stopped;
                self.activity = "Stopped".into();
            }
            EventKind::SubagentStarted { subagent_id, .. } => {
                self.active_subagents.insert(subagent_id.clone());
                self.status = SessionStatus::Running;
                self.activity = "Working".into();
            }
            EventKind::SubagentLifecycle {
                subagent_id,
                status,
            } => {
                if status == "waiting_human" && self.active_subagents.remove(subagent_id) {
                    self.waiting_subagents.insert(subagent_id.clone());
                    if !self.foreground_running
                        && self.active_subagents.is_empty()
                        && self.status != SessionStatus::Stopped
                    {
                        self.status = SessionStatus::Idle;
                        self.activity = "Waiting for you".into();
                    }
                } else if status == "running" && self.waiting_subagents.remove(subagent_id) {
                    self.active_subagents.insert(subagent_id.clone());
                    if self.status != SessionStatus::Stopped {
                        self.status = SessionStatus::Running;
                        self.activity = "Working".into();
                    }
                }
            }
            EventKind::SubagentEnded {
                subagent_id,
                status,
                ..
            } => {
                let was_waiting = self.waiting_subagents.remove(subagent_id);
                let was_running = self.active_subagents.remove(subagent_id);
                if (was_running || was_waiting)
                    && !self.foreground_running
                    && self.active_subagents.is_empty()
                    && self.status != SessionStatus::Stopped
                {
                    self.status = SessionStatus::Idle;
                    self.activity = match status {
                        openside_proto::SubagentStatus::Failed => "Couldn't finish",
                        openside_proto::SubagentStatus::Cancelled => "Stopped",
                        _ => "Done",
                    }
                    .into();
                }
            }
            EventKind::BrowserState(state)
                if self.status == SessionStatus::Running || !self.active_subagents.is_empty() =>
            {
                self.activity = match state.phase {
                    openside_proto::BrowserPhase::WaitingHuman => "Waiting for you".into(),
                    openside_proto::BrowserPhase::Error => "Couldn't finish".into(),
                    _ if !state.host.is_empty() => format!("Browsing {}", state.host),
                    _ => "Browsing".into(),
                };
            }
            EventKind::SessionTitleProposed { title } => {
                if !self.title_locked && !self.title_proposed {
                    if let Some(title) = openside_proto::normalize_model_title(title) {
                        self.title = title;
                        self.title_proposed = true;
                    }
                }
            }
            EventKind::SessionDeleted { .. } => self.deleted = true,
            EventKind::SessionUpdated {
                mode,
                title,
                pinned,
                archived,
            } => {
                if let Some(mode) = mode {
                    self.mode = *mode;
                }
                if let Some(title) = title {
                    self.title.clone_from(title);
                    self.title_locked = true;
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
        if matches!(
            event.kind,
            EventKind::TicketResolved { .. }
                | EventKind::TicketRejected { .. }
                | EventKind::TicketExpired { .. }
        ) {
            // API-origin tickets have no agent continuation to produce a later settled event.
            let continuing = self.status == SessionStatus::Running
                || !self.active_subagents.is_empty()
                || self
                    .completions
                    .values()
                    .any(|outcome| *outcome == MessageOutcome::Deferred);
            // A resolved deferred call still owes a continuation. Publishing idle here
            // lets resume clients stop at the resolution before the runtime starts it.
            if continuing
                && self.status != SessionStatus::Stopped
                && !self
                    .tickets
                    .values()
                    .any(|ticket| ticket.status == TicketStatus::Pending)
            {
                self.status = SessionStatus::Running;
            }
            self.activity = if self.status == SessionStatus::Stopped {
                "Stopped"
            } else if continuing {
                "Working"
            } else if self.activity == "Couldn't finish" {
                "Couldn't finish"
            } else {
                "Done"
            }
            .into();
        }
        if !matches!(
            event.kind,
            EventKind::SessionUpdated { .. } | EventKind::SessionTitleProposed { .. }
        ) {
            self.last_activity = event.ts;
        }
        self.last_seq = event.seq;
        Ok(())
    }
}

fn queue_error(error: openside_core::QueueError) -> Error {
    Error::InvalidEvent(format!("queue operation rejected: {error:?}"))
}
