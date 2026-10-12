//! Follow-up queue state machine (A-4).
//!
//! The daemon owns pending delivery with these runtime-independent semantics:
//! * `Queue` messages are delivered after the current turn would otherwise stop.
//! * `Steer` messages are delivered at the next step boundary of the running turn.
//! * Queued messages can be edited, removed, moved within their lane, or promoted to `Steer`.
//! * Nothing is ever dropped silently: every submitted message ends up `Delivered`,
//!   `Dropped(reason)` or is still pending, and `submit` is idempotent per `MessageId`.

use std::collections::{HashMap, VecDeque};

use crate::ids::MessageId;

/// How a message should be delivered while a turn is running.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum QueueMode {
    /// After the current turn finishes.
    Queue,
    /// Into the running turn at its next boundary.
    Steer,
}

/// Why a message was dropped (never silent).
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DropReason {
    /// The user removed it before delivery.
    Removed,
    /// The session was reset or deleted.
    SessionReset,
}

/// Lifecycle of one message.
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    /// Waiting for delivery.
    Queued(QueueMode),
    /// Handed to the agent runtime.
    Delivered,
    /// Will never be delivered.
    Dropped(DropReason),
}

/// Result of `submit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitOutcome {
    /// Stored as pending.
    Accepted,
    /// Same id was submitted before; nothing changed (idempotent retry).
    Duplicate,
}

/// Errors from queue operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueError {
    /// No message with this id.
    Unknown(MessageId),
    /// The message is not pending (already delivered or dropped).
    NotPending(MessageId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    text: String,
    status: MessageStatus,
}

/// Pure queue state.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MessageQueue {
    entries: HashMap<MessageId, Entry>,
    steer: VecDeque<MessageId>,
    follow_up: VecDeque<MessageId>,
    submitted: usize,
}

impl MessageQueue {
    /// Creates an empty queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Submits a message. Re-submitting the same id is a no-op (`Duplicate`).
    pub fn submit(
        &mut self,
        id: MessageId,
        text: impl Into<String>,
        mode: QueueMode,
    ) -> SubmitOutcome {
        if self.entries.contains_key(&id) {
            return SubmitOutcome::Duplicate;
        }
        self.entries.insert(
            id.clone(),
            Entry {
                text: text.into(),
                status: MessageStatus::Queued(mode),
            },
        );
        match mode {
            QueueMode::Steer => self.steer.push_back(id),
            QueueMode::Queue => self.follow_up.push_back(id),
        }
        self.submitted += 1;
        SubmitOutcome::Accepted
    }

    /// Replaces the text of a pending message.
    pub fn edit(&mut self, id: &MessageId, text: impl Into<String>) -> Result<(), QueueError> {
        let entry = self.pending_entry_mut(id)?;
        entry.text = text.into();
        Ok(())
    }

    /// Removes a pending message; it ends as `Dropped(Removed)`.
    pub fn remove(&mut self, id: &MessageId) -> Result<(), QueueError> {
        self.drop_message(id, DropReason::Removed)
    }

    /// Drops one pending message while preserving its terminal reason for replay.
    pub fn drop_message(&mut self, id: &MessageId, reason: DropReason) -> Result<(), QueueError> {
        let entry = self.pending_entry_mut(id)?;
        entry.status = MessageStatus::Dropped(reason);
        self.steer.retain(|m| m != id);
        self.follow_up.retain(|m| m != id);
        Ok(())
    }

    /// Moves a pending follow-up to the steering queue.
    pub fn promote(&mut self, id: &MessageId) -> Result<(), QueueError> {
        let entry = self.pending_entry_mut(id)?;
        if entry.status == MessageStatus::Queued(QueueMode::Steer) {
            return Ok(());
        }
        entry.status = MessageStatus::Queued(QueueMode::Steer);
        self.follow_up.retain(|m| m != id);
        self.steer.push_back(id.clone());
        Ok(())
    }

    /// Moves a pending message to `position` (zero-based, clamped to the lane length) inside
    /// its own lane. Lanes never mix: a follow-up becomes a steer only through `promote`.
    pub fn reorder(&mut self, id: &MessageId, position: usize) -> Result<(), QueueError> {
        let lane = match self.pending_entry_mut(id)?.status {
            MessageStatus::Queued(QueueMode::Steer) => &mut self.steer,
            _ => &mut self.follow_up,
        };
        lane.retain(|m| m != id);
        lane.insert(position.min(lane.len()), id.clone());
        Ok(())
    }

    /// Next steering message for the running turn, marking it delivered.
    pub fn take_steer(&mut self) -> Option<(MessageId, String)> {
        let id = self.steer.pop_front()?;
        Some(self.deliver(id))
    }

    /// Next follow-up for when the turn would otherwise end. Steering messages always go
    /// first: returns `None` while any steering message is pending.
    pub fn take_follow_up(&mut self) -> Option<(MessageId, String)> {
        if !self.steer.is_empty() {
            return None;
        }
        let id = self.follow_up.pop_front()?;
        Some(self.deliver(id))
    }

    /// Drops every pending message with the given reason (session reset/delete).
    pub fn drop_all(&mut self, reason: DropReason) -> Vec<MessageId> {
        let ids: Vec<MessageId> = self
            .steer
            .drain(..)
            .chain(self.follow_up.drain(..))
            .collect();
        for id in &ids {
            if let Some(e) = self.entries.get_mut(id) {
                e.status = MessageStatus::Dropped(reason.clone());
            }
        }
        ids
    }

    /// Status of a message, if it was ever submitted.
    pub fn status(&self, id: &MessageId) -> Option<&MessageStatus> {
        self.entries.get(id).map(|e| &e.status)
    }

    /// Text of a message, if it was ever submitted.
    pub fn text(&self, id: &MessageId) -> Option<&str> {
        self.entries.get(id).map(|e| e.text.as_str())
    }

    /// Pending messages in delivery order: steering first, then follow-ups.
    pub fn pending(&self) -> Vec<MessageId> {
        self.pending_ids().cloned().collect()
    }

    /// Borrows pending ids in delivery order without allocating a snapshot.
    pub fn pending_ids(&self) -> impl Iterator<Item = &MessageId> {
        self.steer.iter().chain(self.follow_up.iter())
    }

    /// Checks the conservation invariant: every submitted message is accounted for exactly
    /// once and the pending lists agree with the statuses.
    pub fn check_invariants(&self) -> Result<(), String> {
        if self.entries.len() != self.submitted {
            return Err(format!(
                "entries {} != submitted {}",
                self.entries.len(),
                self.submitted
            ));
        }
        let mut pending_in_entries = 0usize;
        for (id, e) in &self.entries {
            if let MessageStatus::Queued(mode) = e.status {
                pending_in_entries += 1;
                let list = if mode == QueueMode::Steer {
                    &self.steer
                } else {
                    &self.follow_up
                };
                if list.iter().filter(|m| *m == id).count() != 1 {
                    return Err(format!("{id} not exactly once in its queue"));
                }
            }
        }
        if pending_in_entries != self.steer.len() + self.follow_up.len() {
            return Err("pending lists disagree with statuses".into());
        }
        Ok(())
    }

    fn deliver(&mut self, id: MessageId) -> (MessageId, String) {
        let text = match self.entries.get_mut(&id) {
            Some(e) => {
                e.status = MessageStatus::Delivered;
                e.text.clone()
            }
            None => String::new(),
        };
        (id, text)
    }

    fn pending_entry_mut(&mut self, id: &MessageId) -> Result<&mut Entry, QueueError> {
        match self.entries.get_mut(id) {
            None => Err(QueueError::Unknown(id.clone())),
            Some(e) if matches!(e.status, MessageStatus::Queued(_)) => Ok(e),
            Some(_) => Err(QueueError::NotPending(id.clone())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> MessageId {
        MessageId::new(s)
    }

    #[test]
    fn duplicate_submit_is_idempotent() {
        let mut q = MessageQueue::new();
        assert_eq!(
            q.submit(id("a"), "hi", QueueMode::Queue),
            SubmitOutcome::Accepted
        );
        assert_eq!(
            q.submit(id("a"), "changed", QueueMode::Steer),
            SubmitOutcome::Duplicate
        );
        assert_eq!(q.text(&id("a")), Some("hi"));
        assert_eq!(q.pending(), vec![id("a")]);
        q.check_invariants().unwrap();
    }

    #[test]
    fn steer_goes_before_follow_up_and_blocks_follow_up_delivery() {
        let mut q = MessageQueue::new();
        q.submit(id("f1"), "later", QueueMode::Queue);
        q.submit(id("s1"), "now", QueueMode::Steer);
        assert_eq!(
            q.take_follow_up(),
            None,
            "follow-up must wait while steering is pending"
        );
        assert_eq!(q.take_steer().map(|m| m.0), Some(id("s1")));
        assert_eq!(q.take_follow_up().map(|m| m.0), Some(id("f1")));
        assert_eq!(q.status(&id("f1")), Some(&MessageStatus::Delivered));
        q.check_invariants().unwrap();
    }

    #[test]
    fn edit_remove_promote_only_work_on_pending_messages() {
        let mut q = MessageQueue::new();
        q.submit(id("a"), "one", QueueMode::Queue);
        q.edit(&id("a"), "one!").unwrap();
        assert_eq!(q.text(&id("a")), Some("one!"));
        q.promote(&id("a")).unwrap();
        assert_eq!(
            q.status(&id("a")),
            Some(&MessageStatus::Queued(QueueMode::Steer))
        );
        assert_eq!(q.take_steer().map(|m| m.1), Some("one!".to_string()));
        assert_eq!(q.edit(&id("a"), "x"), Err(QueueError::NotPending(id("a"))));
        assert_eq!(q.remove(&id("zzz")), Err(QueueError::Unknown(id("zzz"))));
        q.check_invariants().unwrap();
    }

    #[test]
    fn reorder_moves_within_the_lane_clamps_and_rejects_settled_messages() {
        let mut q = MessageQueue::new();
        for name in ["a", "b", "c"] {
            q.submit(id(name), name, QueueMode::Queue);
        }
        q.submit(id("s"), "s", QueueMode::Steer);
        q.reorder(&id("c"), 0).unwrap();
        assert_eq!(q.pending(), [id("s"), id("c"), id("a"), id("b")]);
        q.reorder(&id("c"), 99).unwrap();
        assert_eq!(q.pending(), [id("s"), id("a"), id("b"), id("c")]);
        q.reorder(&id("s"), 2).unwrap();
        assert_eq!(
            q.pending()[0],
            id("s"),
            "a steer never leaves the steer lane"
        );
        assert_eq!(q.take_follow_up(), None, "steer still goes first");
        q.take_steer().unwrap();
        assert_eq!(q.take_follow_up().map(|m| m.0), Some(id("a")));
        assert_eq!(q.reorder(&id("a"), 0), Err(QueueError::NotPending(id("a"))));
        assert_eq!(
            q.reorder(&id("zzz"), 0),
            Err(QueueError::Unknown(id("zzz")))
        );
        q.check_invariants().unwrap();
    }

    #[test]
    fn removal_is_recorded_not_silent() {
        let mut q = MessageQueue::new();
        q.submit(id("a"), "x", QueueMode::Queue);
        q.remove(&id("a")).unwrap();
        assert_eq!(
            q.status(&id("a")),
            Some(&MessageStatus::Dropped(DropReason::Removed))
        );
        assert!(q.pending().is_empty());
        q.check_invariants().unwrap();
    }

    #[test]
    fn drop_all_marks_every_pending_message() {
        let mut q = MessageQueue::new();
        q.submit(id("a"), "x", QueueMode::Queue);
        q.submit(id("b"), "y", QueueMode::Steer);
        let dropped = q.drop_all(DropReason::SessionReset);
        assert_eq!(dropped.len(), 2);
        assert_eq!(
            q.status(&id("b")),
            Some(&MessageStatus::Dropped(DropReason::SessionReset))
        );
        q.check_invariants().unwrap();
    }

    /// Deterministic pseudo-random operation sequences must never violate the invariant,
    /// and a message is delivered at most once.
    #[test]
    fn random_operation_sequences_keep_invariants() {
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..200 {
            let mut q = MessageQueue::new();
            let mut delivered: HashMap<MessageId, u32> = HashMap::new();
            for step in 0..120 {
                let target = id(&format!("m{}", next() % 12));
                match next() % 8 {
                    0 | 1 => {
                        let mode = if next() % 2 == 0 {
                            QueueMode::Queue
                        } else {
                            QueueMode::Steer
                        };
                        q.submit(target, format!("t{step}"), mode);
                    }
                    2 => {
                        let _ = q.edit(&target, "e");
                    }
                    3 => {
                        let _ = q.remove(&target);
                    }
                    4 => {
                        let _ = q.promote(&target);
                        let _ = q.reorder(&target, step % 3);
                    }
                    5 => {
                        if let Some((m, _)) = q.take_steer() {
                            *delivered.entry(m).or_default() += 1;
                        }
                    }
                    6 => {
                        if let Some((m, _)) = q.take_follow_up() {
                            *delivered.entry(m).or_default() += 1;
                        }
                    }
                    _ => {
                        if next() % 40 == 0 {
                            q.drop_all(DropReason::SessionReset);
                        }
                    }
                }
                q.check_invariants().unwrap();
            }
            assert!(
                delivered.values().all(|&n| n == 1),
                "a message was delivered more than once"
            );
        }
    }
}
