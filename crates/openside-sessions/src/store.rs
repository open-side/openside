use std::{collections::BTreeMap, path::Path, time::Duration};

use chrono::{DateTime, Utc};
use openside_core::{AgentState, SessionId, TicketId};
use openside_proto::{
    BrowserSpace, Event, EventKind, PolicyMode, Session, TicketDeferral, TicketKind, TicketStatus,
};
use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};

use crate::{Error, Result, SessionState};

const COLUMNS: &str = "session_id, seq, ts, run_id, payload";

/// A user-submitted message with its recording time, as originally queued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedMessage {
    /// Session that received the message.
    pub session_id: SessionId,
    /// Idempotency key the message was submitted under.
    pub message_id: openside_core::MessageId,
    /// Original text, before any edits.
    pub text: String,
    /// Time the queue event was recorded.
    pub at: DateTime<Utc>,
}

/// Outcome of atomically claiming a daemon-owned approval authorization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalClaim {
    /// Authorization was durably consumed before any execution.
    Claimed,
    /// Full arguments or policy action targets differ from the approved invocation.
    ArgsMismatch,
    /// A previous invocation already consumed the authorization.
    AlreadyUsed,
    /// The ticket does not authorize this session, call, and tool.
    Invalid,
}

/// Synchronous SQLite event store with a cache of committed replay projections.
///
/// Own this on a dedicated thread or call through `spawn_blocking`. One append is
/// one `IMMEDIATE` transaction; separate store connections can safely share a file.
/// `session` and `list_sessions` read this connection's cache. Call `replay` to
/// refresh all sessions after writes from another connection; append refreshes its
/// target session automatically inside the write transaction.
pub struct Store {
    connection: Connection,
    states: BTreeMap<SessionId, SessionState>,
}

impl Store {
    /// Opens a database, installs append-only guards, and replays all committed events.
    ///
    /// `:memory:` is supported. Parent directories must already exist. WAL and
    /// `synchronous=FULL` make committed transactions durable without an explicit close.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             CREATE TABLE IF NOT EXISTS events (
                 session_id TEXT NOT NULL,
                 seq INTEGER NOT NULL CHECK(seq > 0),
                 ts TEXT NOT NULL,
                 run_id TEXT,
                 payload TEXT NOT NULL CHECK(json_valid(payload)),
                 submission_id TEXT,
                 PRIMARY KEY(session_id, seq),
                 UNIQUE(session_id, submission_id)
             );
             CREATE TRIGGER IF NOT EXISTS events_no_update BEFORE UPDATE ON events
             BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;
             CREATE TRIGGER IF NOT EXISTS events_no_delete BEFORE DELETE ON events
             BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;
             CREATE TRIGGER IF NOT EXISTS events_gapless BEFORE INSERT ON events
             WHEN NEW.seq != COALESCE((SELECT MAX(seq) FROM events WHERE session_id=NEW.session_id), 0) + 1
             BEGIN SELECT RAISE(ABORT, 'event sequence must be gapless'); END;
             CREATE TABLE IF NOT EXISTS agent_state (
                 session_id TEXT PRIMARY KEY,
                 format TEXT NOT NULL,
                 data TEXT NOT NULL,
                 updated_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS approval_bindings (
                 session_id TEXT NOT NULL,
                 ticket_id TEXT NOT NULL,
                 binding BLOB NOT NULL CHECK(length(binding)=32),
                 consumed INTEGER NOT NULL DEFAULT 0 CHECK(consumed IN (0,1)),
                 PRIMARY KEY(session_id, ticket_id)
             );
             CREATE TABLE IF NOT EXISTS ticket_deliveries (
                 session_id TEXT NOT NULL,
                 ticket_id TEXT NOT NULL,
                 PRIMARY KEY(session_id, ticket_id)
             );"
        )?;
        let states = replay_connection(&connection)?;
        Ok(Self { connection, states })
    }

    /// Creates a session by committing its first `session.created` event.
    /// Existing session ids are rejected rather than changing their metadata.
    /// The daemon must canonicalize the optional absolute workspace before creation.
    pub fn create_session(
        &mut self,
        id: SessionId,
        title: impl Into<String>,
        model: impl Into<String>,
        mode: PolicyMode,
        workspace: Option<String>,
    ) -> Result<Event> {
        self.create_session_with_privacy(id, title, model, mode, workspace, false)
    }

    /// [`Self::create_session`] with the immutable incognito flag recorded in `session.created`.
    pub fn create_session_with_privacy(
        &mut self,
        id: SessionId,
        title: impl Into<String>,
        model: impl Into<String>,
        mode: PolicyMode,
        workspace: Option<String>,
        incognito: bool,
    ) -> Result<Event> {
        self.append(
            &id,
            None,
            EventKind::SessionCreated {
                title: title.into(),
                model: model.into(),
                mode,
                workspace,
                incognito,
                browser_space: BrowserSpace::Shared,
            },
        )
    }

    /// Validates and atomically appends an event, assigning the next sequence and UTC time.
    ///
    /// Retrying `message.queued` with an already submitted MessageId returns its original
    /// event without appending, changing text, or redelivering—even after restart. Other
    /// invalid operations fail without consuming a sequence. Injection must identify the
    /// queue head (steering before follow-up), otherwise it fails loudly. No runtime
    /// side effect is performed here; deliver only after this operation succeeds.
    /// SQLite's signed integer sequences are supported through `i64::MAX`; overflow fails.
    pub fn append(
        &mut self,
        session_id: &SessionId,
        run_id: Option<String>,
        kind: EventKind,
    ) -> Result<Event> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let last_seq: Option<i64> = transaction
            .query_row(
                "SELECT seq FROM events WHERE session_id=?1 ORDER BY seq DESC LIMIT 1",
                [session_id.as_str()],
                |row| row.get(0),
            )
            .optional()?;
        let creating = matches!(&kind, EventKind::SessionCreated { .. });
        if creating && last_seq.is_some() {
            return Err(Error::SessionExists(session_id.clone()));
        }
        if !creating && last_seq.is_none() {
            return Err(Error::UnknownSession(session_id.clone()));
        }
        let previous = last_seq.unwrap_or(0);
        if !creating
            && self.states.get(session_id).map(|state| state.last_seq) != Some(previous as u64)
        {
            let state = replay_session(&transaction, session_id)?
                .ok_or_else(|| Error::UnknownSession(session_id.clone()))?;
            self.states.insert(session_id.clone(), state);
        }
        // Before the duplicate-submission shortcut: a deleted session must not answer
        // "success" for a message id it queued before the tombstone.
        if self
            .states
            .get(session_id)
            .is_some_and(|state| state.deleted)
        {
            return Err(Error::InvalidEvent("session was deleted".into()));
        }
        if let EventKind::MessageQueued { message_id, .. } = &kind {
            let sql =
                format!("SELECT {COLUMNS} FROM events WHERE session_id=?1 AND submission_id=?2");
            if let Some(original) = transaction
                .query_row(
                    &sql,
                    params![session_id.as_str(), message_id.as_str()],
                    decode_event,
                )
                .optional()?
            {
                return Ok(original);
            }
        }
        if let Some(state) = self.states.get(session_id) {
            state.validate(&kind)?;
        }
        let seq = previous.checked_add(1).ok_or(Error::SequenceExhausted)?;
        let event = Event {
            session_id: session_id.clone(),
            seq: seq as u64,
            ts: Utc::now(),
            run_id,
            kind,
        };
        let submission_id = match &event.kind {
            EventKind::MessageQueued { message_id, .. } => Some(message_id.as_str()),
            _ => None,
        };
        let payload = serde_json::to_string(&event.kind)?;
        transaction.execute(
            "INSERT INTO events(session_id,seq,ts,run_id,payload,submission_id) VALUES(?1,?2,?3,?4,?5,?6)",
            params![session_id.as_str(), seq, event.ts.to_rfc3339(), event.run_id, payload, submission_id],
        )?;
        transaction.commit()?;
        if creating {
            self.states
                .insert(session_id.clone(), SessionState::created(&event)?);
        } else {
            self.states
                .get_mut(session_id)
                .ok_or_else(|| Error::UnknownSession(session_id.clone()))?
                .apply(&event)?;
        }
        Ok(event)
    }

    /// Reads events strictly after `after_seq`, ordered by sequence and bounded by `limit`.
    /// Unknown sessions are rejected. A zero limit returns an empty page.
    pub fn events_after(
        &self,
        session_id: &SessionId,
        after_seq: u64,
        limit: usize,
    ) -> Result<Vec<Event>> {
        let exists: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE session_id=?1)",
            [session_id.as_str()],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(Error::UnknownSession(session_id.clone()));
        }
        let Ok(after_seq) = i64::try_from(after_seq) else {
            return Ok(Vec::new());
        };
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let sql = format!(
            "SELECT {COLUMNS} FROM events WHERE session_id=?1 AND seq>?2 ORDER BY seq LIMIT ?3"
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows =
            statement.query_map(params![session_id.as_str(), after_seq, limit], decode_event)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Lists cached public session projections, sorted by identifier. Deleted sessions are hidden.
    pub fn list_sessions(&self) -> Vec<Session> {
        self.states
            .values()
            .filter(|state| !state.deleted)
            .map(SessionState::session)
            .collect()
    }

    /// Borrows the cached durable projection, including terminal idempotency history.
    /// Deleted sessions are reported as unknown; their retained log is still readable
    /// through [`Store::events_after`] so subscribers can observe the tombstone.
    pub fn session(&self, session_id: &SessionId) -> Option<&SessionState> {
        self.states.get(session_id).filter(|state| !state.deleted)
    }

    /// Most recent `limit` user messages (original text) across live sessions, oldest first.
    pub fn queued_messages(&self, limit: usize) -> Result<Vec<QueuedMessage>> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut statement = self.connection.prepare(
            "SELECT session_id, ts, payload FROM events
             WHERE json_extract(payload, '$.kind') = 'message.queued'
             ORDER BY ts DESC, rowid DESC LIMIT ?1",
        )?;
        let rows = statement.query_map([limit], |row| {
            let session: String = row.get(0)?;
            let ts: String = row.get(1)?;
            let payload: String = row.get(2)?;
            Ok((session, ts, payload))
        })?;
        let mut messages = Vec::new();
        for row in rows {
            let (session, ts, payload) = row?;
            let session_id = SessionId::new(session);
            if self.session(&session_id).is_none() {
                continue;
            }
            let Ok(EventKind::MessageQueued {
                message_id, text, ..
            }) = serde_json::from_str(&payload)
            else {
                continue;
            };
            let Ok(at) = DateTime::parse_from_rfc3339(&ts) else {
                continue;
            };
            messages.push(QueuedMessage {
                session_id,
                message_id,
                text,
                at: at.with_timezone(&Utc),
            });
        }
        messages.reverse();
        Ok(messages)
    }

    /// Persists the full invocation digest separately from redacted public events.
    /// Write this before publishing the ticket; an orphan binding cannot authorize a call.
    pub fn bind_approval(
        &mut self,
        session: &SessionId,
        ticket: &TicketId,
        binding: &[u8; 32],
    ) -> Result<()> {
        self.connection.execute(
            "INSERT INTO approval_bindings(session_id,ticket_id,binding) VALUES(?1,?2,?3)",
            params![session.as_str(), ticket.as_str(), binding.as_slice()],
        )?;
        Ok(())
    }

    /// Checks a pending ticket's immutable digest without consuming its authorization.
    pub fn approval_matches(
        &self,
        session: &SessionId,
        ticket: &TicketId,
        binding: &[u8; 32],
    ) -> Result<bool> {
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM approval_bindings
             WHERE session_id=?1 AND ticket_id=?2 AND binding=?3)",
            params![session.as_str(), ticket.as_str(), binding.as_slice()],
            |row| row.get(0),
        )?)
    }

    /// Validates and consumes an exact approval in one durable write transaction.
    /// Replay inside the transaction prevents a stale cache from granting authorization.
    pub fn claim_approval(
        &mut self,
        session: &SessionId,
        ticket: &TicketId,
        tool_call_id: &str,
        tool_name: &str,
        subagent: Option<&str>,
        binding: &[u8; 32],
    ) -> Result<ApprovalClaim> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let state = replay_session(&transaction, session)?
            .ok_or_else(|| Error::UnknownSession(session.clone()))?;
        let valid = state.tickets.get(ticket).is_some_and(|ticket| {
            ticket.session_id == *session
                && ticket.tool_call_id == tool_call_id
                // A parent (None) and each child are distinct principals; call ids are not.
                && ticket.origin.subagent_id.as_deref() == subagent
                && ticket.deferral == TicketDeferral::Approval
                && ticket.kind == TicketKind::Confirm
                && ticket.status == TicketStatus::Resolved
                && ticket
                    .resolution
                    .as_ref()
                    .is_some_and(|value| value["approved"] == true)
                && ticket
                    .tool
                    .as_ref()
                    .is_some_and(|tool| tool.name == tool_name)
        });
        if !valid {
            return Ok(ApprovalClaim::Invalid);
        }
        let stored: Option<(Vec<u8>, bool)> = transaction.query_row(
            "SELECT binding,consumed FROM approval_bindings WHERE session_id=?1 AND ticket_id=?2",
            params![session.as_str(), ticket.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        let Some((stored, consumed)) = stored else {
            return Ok(ApprovalClaim::Invalid);
        };
        if consumed {
            return Ok(ApprovalClaim::AlreadyUsed);
        }
        if stored != binding.as_slice() {
            return Ok(ApprovalClaim::ArgsMismatch);
        }
        transaction.execute(
            "UPDATE approval_bindings SET consumed=1 WHERE session_id=?1 AND ticket_id=?2",
            params![session.as_str(), ticket.as_str()],
        )?;
        transaction.commit()?;
        Ok(ApprovalClaim::Claimed)
    }

    /// Marks terminal resolution delivery before runtime I/O; false means it was already sent.
    pub fn mark_ticket_delivered(
        &mut self,
        session: &SessionId,
        ticket: &TicketId,
    ) -> Result<bool> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let state = replay_session(&transaction, session)?
            .ok_or_else(|| Error::UnknownSession(session.clone()))?;
        if !state
            .tickets
            .get(ticket)
            .is_some_and(|ticket| ticket.status != TicketStatus::Pending)
        {
            return Err(Error::InvalidEvent(
                "cannot deliver a pending or unknown ticket".into(),
            ));
        }
        let inserted = transaction.execute(
            "INSERT OR IGNORE INTO ticket_deliveries(session_id,ticket_id) VALUES(?1,?2)",
            params![session.as_str(), ticket.as_str()],
        )?;
        transaction.commit()?;
        Ok(inserted == 1)
    }

    /// Finds terminal tickets needing delivery independently of the user-message queue.
    pub fn undelivered_tickets(&self, session: &SessionId) -> Result<Vec<TicketId>> {
        let state = self
            .states
            .get(session)
            .ok_or_else(|| Error::UnknownSession(session.clone()))?;
        let mut query = self.connection.prepare(
            "SELECT EXISTS(SELECT 1 FROM ticket_deliveries WHERE session_id=?1 AND ticket_id=?2)",
        )?;
        let mut tickets = Vec::new();
        for ticket in state
            .tickets
            .values()
            .filter(|ticket| ticket.status != TicketStatus::Pending)
        {
            let delivered: bool = query
                .query_row(params![session.as_str(), ticket.id.as_str()], |row| {
                    row.get(0)
                })?;
            if !delivered {
                tickets.push(ticket.id.clone());
            }
        }
        Ok(tickets)
    }

    /// Atomically replaces a session's opaque run-end snapshot.
    ///
    /// The session must already have a creation event. Snapshot data bypasses the
    /// event log and is interpreted only by the sidecar; redact before storing it.
    pub fn put_agent_state(&mut self, session_id: &SessionId, state: &AgentState) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE session_id=?1)",
            [session_id.as_str()],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(Error::UnknownSession(session_id.clone()));
        }
        transaction.execute(
            "INSERT INTO agent_state(session_id,format,data,updated_at) VALUES(?1,?2,?3,?4)
             ON CONFLICT(session_id) DO UPDATE SET
                 format=excluded.format, data=excluded.data, updated_at=excluded.updated_at",
            params![
                session_id.as_str(),
                state.format,
                state.data,
                Utc::now().to_rfc3339()
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Reads the latest opaque snapshot; known sessions without a snapshot return `None`.
    pub fn agent_state(&self, session_id: &SessionId) -> Result<Option<AgentState>> {
        let exists: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE session_id=?1)",
            [session_id.as_str()],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(Error::UnknownSession(session_id.clone()));
        }
        Ok(self
            .connection
            .query_row(
                "SELECT format,data FROM agent_state WHERE session_id=?1",
                [session_id.as_str()],
                |row| {
                    Ok(AgentState {
                        format: row.get(0)?,
                        data: row.get(1)?,
                    })
                },
            )
            .optional()?)
    }

    /// Rebuilds every session purely from a consistent snapshot of committed events.
    /// Invalid logs fail loudly; the previous cache remains intact on failure.
    pub fn replay(&mut self) -> Result<&BTreeMap<SessionId, SessionState>> {
        self.states = replay_connection(&self.connection)?;
        Ok(&self.states)
    }
}

fn decode_event(row: &Row<'_>) -> rusqlite::Result<Event> {
    let timestamp: String = row.get(2)?;
    let ts = DateTime::parse_from_rfc3339(&timestamp)
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?
        .with_timezone(&Utc);
    let payload: String = row.get(4)?;
    let kind = serde_json::from_str(&payload).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let seq: i64 = row.get(1)?;
    if seq <= 0 {
        return Err(rusqlite::Error::IntegralValueOutOfRange(1, seq));
    }
    Ok(Event {
        session_id: SessionId::new(row.get::<_, String>(0)?),
        seq: seq as u64,
        ts,
        run_id: row.get(3)?,
        kind,
    })
}

fn replay_session(connection: &Connection, session_id: &SessionId) -> Result<Option<SessionState>> {
    let sql = format!("SELECT {COLUMNS} FROM events WHERE session_id=?1 ORDER BY seq");
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map([session_id.as_str()], decode_event)?;
    let mut state: Option<SessionState> = None;
    for row in rows {
        let event = row?;
        match &mut state {
            Some(state) => state.apply(&event)?,
            None => state = Some(SessionState::created(&event)?),
        }
    }
    Ok(state)
}

fn replay_connection(connection: &Connection) -> Result<BTreeMap<SessionId, SessionState>> {
    let sql = format!("SELECT {COLUMNS} FROM events ORDER BY session_id, seq");
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map([], decode_event)?;
    let mut states = BTreeMap::<SessionId, SessionState>::new();
    for row in rows {
        let event = row?;
        match states.get_mut(&event.session_id) {
            Some(state) => state.apply(&event)?,
            None => {
                states.insert(event.session_id.clone(), SessionState::created(&event)?);
            }
        }
    }
    Ok(states)
}
