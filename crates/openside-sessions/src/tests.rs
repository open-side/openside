use std::collections::BTreeSet;

use chrono::{TimeZone, Utc};
use openside_core::{
    AgentState, DropReason, MessageId, MessageQueue, MessageStatus, QueueMode, SessionId,
    SubmitOutcome, TicketId,
};
use openside_proto::{
    EventKind, PolicyMode, SessionStatus, Ticket, TicketDeferral, TicketKind, TicketOrigin,
    TicketStatus, TicketTool,
};
use proptest::prelude::*;
use rusqlite::{params, Connection};
use serde_json::json;

use crate::Store;

fn sid() -> SessionId {
    SessionId::new("session")
}
fn mid(number: u8) -> MessageId {
    MessageId::new(format!("message-{number}"))
}
fn submit(id: MessageId, text: String, mode: QueueMode) -> EventKind {
    EventKind::MessageQueued {
        message_id: id,
        text,
        mode,
        attachments: Vec::new(),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn random_queue_operations_replay_without_loss_or_duplicate_delivery(
        operations in prop::collection::vec((0u8..8, 0u8..24, "[a-z ]{0,24}", any::<bool>()), 1..140)
    ) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.db");
        let mut store = Store::open(&path).unwrap();
        store.create_session(sid(), "Property task", "mock/model", PolicyMode::Guard, None).unwrap();
        let mut live = MessageQueue::new();
        let mut submitted = BTreeSet::new();
        let mut delivered = BTreeSet::new();
        for (index, (operation, number, text, steering)) in operations.iter().enumerate() {
            let id = mid(*number);
            let previous_seq = store.session(&sid()).unwrap().last_seq;
            match operation {
                0 | 7 => {
                    let mode = if *steering { QueueMode::Steer } else { QueueMode::Queue };
                    let outcome = live.submit(id.clone(), text.clone(), mode);
                    let event = store.append(&sid(), Some("run".into()), submit(id.clone(), text.clone(), mode)).unwrap();
                    if outcome == SubmitOutcome::Duplicate {
                        prop_assert!(event.seq <= previous_seq);
                        prop_assert_eq!(store.session(&sid()).unwrap().last_seq, previous_seq);
                    } else {
                        prop_assert_eq!(event.seq, previous_seq + 1);
                        submitted.insert(id);
                    }
                }
                1 => {
                    let valid = live.edit(&id, text.clone()).is_ok();
                    let result = store.append(&sid(), None, EventKind::MessageEdited { message_id: id, text: text.clone() });
                    prop_assert_eq!(result.is_ok(), valid);
                }
                2 => {
                    let valid = live.remove(&id).is_ok();
                    let result = store.append(&sid(), None, EventKind::MessageDropped { message_id: id, reason: DropReason::Removed });
                    prop_assert_eq!(result.is_ok(), valid);
                }
                3 => {
                    let valid = live.promote(&id).is_ok();
                    let result = store.append(&sid(), None, EventKind::MessagePromoted { message_id: id });
                    prop_assert_eq!(result.is_ok(), valid);
                }
                4 | 5 => {
                    let message = if *operation == 4 { live.take_steer() } else { live.take_follow_up() };
                    if let Some((id, _)) = message {
                        prop_assert!(delivered.insert(id.clone()), "message delivered twice");
                        store.append(&sid(), Some("run".into()), EventKind::MessageInjected { message_id: id }).unwrap();
                    } else {
                        prop_assert!(store.append(&sid(), None, EventKind::MessageInjected { message_id: MessageId::new("never-submitted") }).is_err(), "unknown delivery must be rejected");
                    }
                }
                6 => {
                    for id in live.drop_all(DropReason::SessionReset) {
                        store.append(&sid(), None, EventKind::MessageDropped { message_id: id, reason: DropReason::SessionReset }).unwrap();
                    }
                }
                _ => unreachable!(),
            }
            prop_assert_eq!(&store.session(&sid()).unwrap().queue, &live);
            prop_assert!(live.check_invariants().is_ok());
            if index == operations.len() / 2 {
                drop(store);
                store = Store::open(&path).unwrap();
                prop_assert_eq!(&store.session(&sid()).unwrap().queue, &live);
            }
        }
        drop(store);
        let mut store = Store::open(&path).unwrap();
        let state = &store.replay().unwrap()[&sid()];
        prop_assert_eq!(&state.queue, &live);
        prop_assert_eq!(state.queue.pending(), live.pending());
        for id in &submitted {
            prop_assert_eq!(state.queue.status(id), live.status(id));
            prop_assert_eq!(state.queue.text(id), live.text(id));
            prop_assert!(matches!(state.queue.status(id), Some(MessageStatus::Queued(_) | MessageStatus::Delivered | MessageStatus::Dropped(_))));
        }
        let events = store.events_after(&sid(), 0, usize::MAX).unwrap();
        let mut logged_deliveries = BTreeSet::new();
        for (index, event) in events.iter().enumerate() {
            prop_assert_eq!(event.seq, index as u64 + 1);
            if let EventKind::MessageInjected { message_id } = &event.kind {
                prop_assert!(logged_deliveries.insert(message_id.clone()), "duplicate delivery in durable log");
            }
        }
        prop_assert_eq!(logged_deliveries, delivered);
        prop_assert_eq!(store.session(&sid()).unwrap().last_seq, events.len() as u64);
    }
}

#[test]
fn committed_events_survive_drop_and_uncommitted_transaction_rolls_back() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.db");
    let mut store = Store::open(&path).unwrap();
    store
        .create_session(sid(), "Crash task", "mock/model", PolicyMode::Guard, None)
        .unwrap();
    let event = store
        .append(
            &sid(),
            Some("run".into()),
            submit(mid(1), "committed".into(), QueueMode::Queue),
        )
        .unwrap();
    drop(store);
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    connection
        .execute(
            "INSERT INTO events(session_id,seq,ts,run_id,payload) VALUES(?1,3,?2,NULL,?3)",
            params![
                sid().as_str(),
                Utc::now().to_rfc3339(),
                serde_json::to_string(&EventKind::SessionStopped {}).unwrap()
            ],
        )
        .unwrap();
    drop(connection);
    let store = Store::open(&path).unwrap();
    let events = store.events_after(&sid(), 0, 100).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1], event);
    assert_eq!(
        store.session(&sid()).unwrap().queue.text(&mid(1)),
        Some("committed")
    );
    assert_eq!(store.session(&sid()).unwrap().status, SessionStatus::Idle);
}

#[test]
fn deleted_sessions_are_hidden_reject_appends_and_survive_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.db");
    let mut store = Store::open(&path).unwrap();
    let keeper = SessionId::new("keeper");
    for id in [sid(), keeper.clone()] {
        store
            .create_session(id.clone(), "Task", "model", PolicyMode::Guard, None)
            .unwrap();
        store
            .append(
                &id,
                None,
                submit(mid(1), format!("hello from {id}"), QueueMode::Queue),
            )
            .unwrap();
    }
    let tombstone = store
        .append(&sid(), None, EventKind::SessionDeleted {})
        .unwrap();

    let check = |store: &Store| {
        assert!(store.session(&sid()).is_none());
        let live: Vec<_> = store.list_sessions().into_iter().map(|s| s.id).collect();
        assert_eq!(live, std::slice::from_ref(&keeper));
        let texts: Vec<_> = store
            .queued_messages(10)
            .unwrap()
            .into_iter()
            .map(|m| m.text)
            .collect();
        assert_eq!(
            texts,
            ["hello from keeper"],
            "deleted sessions are not analysed"
        );
        // The log is retained and still readable, ending with the tombstone.
        let events = store.events_after(&sid(), 0, 100).unwrap();
        assert_eq!(events.last().unwrap(), &tombstone);
    };
    check(&store);
    // Neither a fresh id nor an id queued before the tombstone (the idempotency shortcut)
    // may succeed, from the writer, from a second connection, or after a reopen.
    let late = |store: &mut Store| {
        for id in [mid(2), mid(1)] {
            assert!(store
                .append(&sid(), None, submit(id, "late".into(), QueueMode::Queue))
                .is_err());
        }
    };
    late(&mut store);
    let mut second = Store::open(&path).unwrap();
    late(&mut second);
    drop(second);
    drop(store);
    let mut reopened = Store::open(&path).unwrap();
    check(&reopened);
    late(&mut reopened);
    assert!(reopened
        .append(&sid(), None, EventKind::SessionStopped {})
        .is_err());
}

#[test]
fn rejected_commands_leave_gapless_sequences_and_delivery_order_intact() {
    let mut store = Store::open(":memory:").unwrap();
    store
        .create_session(sid(), "Task", "model", PolicyMode::Guard, None)
        .unwrap();
    store
        .append(
            &sid(),
            None,
            submit(mid(1), "later".into(), QueueMode::Queue),
        )
        .unwrap();
    store
        .append(
            &sid(),
            None,
            submit(mid(2), "steer".into(), QueueMode::Steer),
        )
        .unwrap();
    assert!(store
        .append(
            &sid(),
            None,
            EventKind::MessageInjected { message_id: mid(1) }
        )
        .is_err());
    assert_eq!(
        store.session(&sid()).unwrap().queue.pending(),
        vec![mid(2), mid(1)]
    );
    assert_eq!(store.session(&sid()).unwrap().last_seq, 3);
    store
        .append(
            &sid(),
            None,
            EventKind::MessageInjected { message_id: mid(2) },
        )
        .unwrap();
    assert!(store
        .append(
            &sid(),
            None,
            EventKind::MessageInjected { message_id: mid(2) }
        )
        .is_err());
    assert!(store
        .append(
            &sid(),
            None,
            EventKind::MessageEdited {
                message_id: mid(2),
                text: "too late".into()
            }
        )
        .is_err());
    assert!(store
        .append(
            &sid(),
            None,
            EventKind::MessagePromoted {
                message_id: mid(99)
            }
        )
        .is_err());
    assert_eq!(store.session(&sid()).unwrap().last_seq, 4);
    assert!(store
        .create_session(sid(), "replace", "model", PolicyMode::Guard, None)
        .is_err());
    assert!(store
        .append(&SessionId::new("absent"), None, EventKind::TurnStarted {})
        .is_err());
}

#[test]
fn delivered_and_dropped_submission_ids_remain_idempotent_after_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.db");
    let mut store = Store::open(&path).unwrap();
    store
        .create_session(sid(), "Task", "model", PolicyMode::Guard, None)
        .unwrap();
    let delivered = store
        .append(
            &sid(),
            None,
            submit(mid(1), "first".into(), QueueMode::Queue),
        )
        .unwrap();
    store
        .append(
            &sid(),
            None,
            EventKind::MessageInjected { message_id: mid(1) },
        )
        .unwrap();
    let dropped = store
        .append(
            &sid(),
            None,
            submit(mid(2), "second".into(), QueueMode::Queue),
        )
        .unwrap();
    store
        .append(
            &sid(),
            None,
            EventKind::MessageDropped {
                message_id: mid(2),
                reason: DropReason::Removed,
            },
        )
        .unwrap();
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(
        store
            .append(
                &sid(),
                None,
                submit(mid(1), "changed".into(), QueueMode::Steer)
            )
            .unwrap(),
        delivered
    );
    assert_eq!(
        store
            .append(
                &sid(),
                None,
                submit(mid(2), "changed".into(), QueueMode::Steer)
            )
            .unwrap(),
        dropped
    );
    assert_eq!(store.session(&sid()).unwrap().last_seq, 5);
    assert!(store.session(&sid()).unwrap().queue.pending().is_empty());
}

fn ticket(number: u8, deferral: TicketDeferral) -> Ticket {
    Ticket {
        id: TicketId::new(format!("ticket-{number}")),
        session_id: sid(),
        tool_call_id: format!("call-{number}"),
        deferral,
        kind: if deferral == TicketDeferral::Approval {
            TicketKind::Confirm
        } else {
            TicketKind::Info
        },
        summary: "Local mock intervention".into(),
        instructions: None,
        options: vec![],
        origin: TicketOrigin {
            subagent_id: Some("child".into()),
            device_id: None,
            driver_ids: Vec::new(),
        },
        tool: (deferral == TicketDeferral::Approval).then(|| TicketTool {
            name: "mock".into(),
            args: json!({}),
        }),
        target: None,
        resume_when: None,
        created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        expires_at: Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
        status: TicketStatus::Pending,
        resolution: None,
        otp: None,
        otp_register: None,
        login: None,
        choice: None,
        approval: None,
    }
}

#[test]
fn replay_restores_metadata_mode_status_and_all_ticket_lifecycles() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.db");
    let mut store = Store::open(&path).unwrap();
    store
        .create_session(sid(), "Named task", "model", PolicyMode::ReadOnly, None)
        .unwrap();
    for (number, deferral) in [
        (1, TicketDeferral::External),
        (2, TicketDeferral::Approval),
        (3, TicketDeferral::External),
        (4, TicketDeferral::Approval),
    ] {
        store
            .append(
                &sid(),
                None,
                EventKind::TicketOpened {
                    ticket: Box::new(ticket(number, deferral)),
                },
            )
            .unwrap();
    }
    store
        .append(
            &sid(),
            None,
            EventKind::TicketResolved {
                ticket_id: TicketId::new("ticket-1"),
                resolution: json!("answer"),
            },
        )
        .unwrap();
    store
        .append(
            &sid(),
            None,
            EventKind::TicketExpired {
                ticket_id: TicketId::new("ticket-2"),
            },
        )
        .unwrap();
    store
        .append(
            &sid(),
            None,
            EventKind::TicketRejected {
                ticket_id: TicketId::new("ticket-4"),
                reason: "stopped".into(),
            },
        )
        .unwrap();
    store
        .append(&sid(), None, EventKind::TurnStarted {})
        .unwrap();
    assert_eq!(
        store.session(&sid()).unwrap().session().status,
        SessionStatus::Running
    );
    store.append(&sid(), None, EventKind::TurnEnded {}).unwrap();
    assert_eq!(
        store.session(&sid()).unwrap().status,
        SessionStatus::Running
    );
    assert_eq!(
        store.session(&sid()).unwrap().session().status,
        SessionStatus::Running
    );
    store
        .append(&sid(), None, EventKind::SessionSettled {})
        .unwrap();
    let waiting = store.session(&sid()).unwrap().clone();
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.session(&sid()).unwrap(), &waiting);
    assert_eq!(store.list_sessions()[0].status, SessionStatus::WaitingHuman);
    assert_eq!(waiting.title, "Named task");
    assert_eq!(waiting.model, "model");
    assert_eq!(waiting.mode, PolicyMode::ReadOnly);
    assert_eq!(waiting.session().mode, PolicyMode::ReadOnly);
    assert_eq!(
        waiting.tickets[&TicketId::new("ticket-1")].resolution,
        Some(json!("answer"))
    );
    assert_eq!(
        waiting.tickets[&TicketId::new("ticket-2")].status,
        TicketStatus::Expired
    );
    assert_eq!(waiting.tickets[&TicketId::new("ticket-2")].resolution, None);
    assert_eq!(
        waiting.tickets[&TicketId::new("ticket-3")].status,
        TicketStatus::Pending
    );
    assert_eq!(
        waiting.tickets[&TicketId::new("ticket-4")].status,
        TicketStatus::Rejected
    );
    assert_eq!(
        waiting.tickets[&TicketId::new("ticket-4")].resolution,
        Some(json!({"status": "rejected", "reason": "stopped"}))
    );
    store
        .append(&sid(), None, EventKind::SessionStopped {})
        .unwrap();
    let stopped = store.session(&sid()).unwrap().clone();
    assert_eq!(stopped.session().status, SessionStatus::Stopped);
    drop(store);
    assert_eq!(
        Store::open(&path).unwrap().session(&sid()).unwrap(),
        &stopped
    );
}

#[test]
fn database_rejects_updates_deletes_and_sequence_gaps() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.db");
    let mut store = Store::open(&path).unwrap();
    store
        .create_session(sid(), "Task", "model", PolicyMode::Guard, None)
        .unwrap();
    let connection = Connection::open(&path).unwrap();
    assert!(connection
        .execute("UPDATE events SET run_id='altered'", [])
        .is_err());
    assert!(connection.execute("DELETE FROM events", []).is_err());
    assert!(connection.execute("INSERT INTO events(session_id,seq,ts,payload) SELECT session_id,3,ts,payload FROM events", []).is_err());
    assert_eq!(store.events_after(&sid(), 0, 100).unwrap().len(), 1);
}

#[test]
fn separate_writers_reconcile_state_and_assign_independent_session_sequences() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.db");
    let mut first = Store::open(&path).unwrap();
    first
        .create_session(sid(), "Task", "model", PolicyMode::Guard, None)
        .unwrap();
    let other = SessionId::new("other");
    first
        .create_session(other.clone(), "Other", "model", PolicyMode::Guard, None)
        .unwrap();
    let mut second = Store::open(&path).unwrap();
    first
        .append(
            &sid(),
            None,
            submit(mid(1), "first".into(), QueueMode::Queue),
        )
        .unwrap();
    second
        .append(
            &sid(),
            None,
            submit(mid(2), "second".into(), QueueMode::Steer),
        )
        .unwrap();
    first
        .append(
            &sid(),
            None,
            EventKind::MessageInjected { message_id: mid(2) },
        )
        .unwrap();
    second
        .append(
            &sid(),
            None,
            EventKind::MessageInjected { message_id: mid(1) },
        )
        .unwrap();
    first.replay().unwrap();
    assert_eq!(first.session(&sid()), second.session(&sid()));
    assert_eq!(first.session(&other).unwrap().last_seq, 1);
    assert_eq!(first.list_sessions().len(), 2);
    assert_eq!(
        first
            .events_after(&sid(), 2, 2)
            .unwrap()
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        vec![3, 4]
    );
    assert!(first.events_after(&sid(), u64::MAX, 10).unwrap().is_empty());
    assert!(first.events_after(&sid(), 0, 0).unwrap().is_empty());
    assert!(first
        .events_after(&SessionId::new("absent"), 0, 10)
        .is_err());
}

#[test]
fn concurrent_connections_commit_gapless_events() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.db");
    let mut store = Store::open(&path).unwrap();
    store
        .create_session(sid(), "Task", "model", PolicyMode::Guard, None)
        .unwrap();
    drop(store);
    let writers: Vec<_> = (0..2)
        .map(|writer| {
            let path = path.clone();
            std::thread::spawn(move || {
                let mut store = Store::open(path).unwrap();
                for index in 0..20 {
                    store
                        .append(
                            &sid(),
                            None,
                            submit(
                                MessageId::new(format!("{writer}-{index}")),
                                "concurrent".into(),
                                QueueMode::Queue,
                            ),
                        )
                        .unwrap();
                }
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
    let store = Store::open(path).unwrap();
    let events = store.events_after(&sid(), 0, 100).unwrap();
    assert_eq!(events.len(), 41);
    assert_eq!(store.session(&sid()).unwrap().queue.pending().len(), 40);
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event.seq, index as u64 + 1);
    }
}

#[test]
fn corrupt_delivery_log_fails_replay_without_replacing_cached_state() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.db");
    let mut store = Store::open(&path).unwrap();
    store
        .create_session(sid(), "Task", "model", PolicyMode::Guard, None)
        .unwrap();
    store
        .append(
            &sid(),
            None,
            submit(mid(1), "head".into(), QueueMode::Queue),
        )
        .unwrap();
    store
        .append(
            &sid(),
            None,
            submit(mid(2), "tail".into(), QueueMode::Queue),
        )
        .unwrap();
    let cached = store.session(&sid()).unwrap().clone();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO events(session_id,seq,ts,payload) VALUES(?1,4,?2,?3)",
            params![
                sid().as_str(),
                Utc::now().to_rfc3339(),
                serde_json::to_string(&EventKind::MessageInjected { message_id: mid(2) }).unwrap()
            ],
        )
        .unwrap();
    assert!(store.replay().is_err());
    assert_eq!(store.session(&sid()).unwrap(), &cached);
    assert!(Store::open(&path).is_err());
}

#[test]
fn completions_require_injection_are_terminal_and_survive_replay() {
    use openside_proto::MessageOutcome;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.db");
    let mut store = Store::open(&path).unwrap();
    store
        .create_session(sid(), "Task", "model", PolicyMode::Guard, None)
        .unwrap();
    let completion = |id, outcome| EventKind::MessageCompleted {
        message_id: id,
        outcome,
        reason: None,
    };
    assert!(store
        .append(&sid(), None, completion(mid(1), MessageOutcome::Completed))
        .is_err());
    store
        .append(
            &sid(),
            None,
            submit(mid(1), "pending".into(), QueueMode::Queue),
        )
        .unwrap();
    assert!(store
        .append(&sid(), None, completion(mid(1), MessageOutcome::Completed))
        .is_err());
    store
        .append(
            &sid(),
            None,
            EventKind::MessageInjected { message_id: mid(1) },
        )
        .unwrap();
    store
        .append(&sid(), None, completion(mid(1), MessageOutcome::Completed))
        .unwrap();
    assert!(store
        .append(&sid(), None, completion(mid(1), MessageOutcome::Failed))
        .is_err());
    store
        .append(
            &sid(),
            None,
            submit(mid(2), "interrupted".into(), QueueMode::Queue),
        )
        .unwrap();
    store
        .append(
            &sid(),
            None,
            EventKind::MessageInjected { message_id: mid(2) },
        )
        .unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    let state = store.session(&sid()).unwrap();
    assert_eq!(state.injected, BTreeSet::from([mid(1), mid(2)]));
    assert_eq!(
        state.completions.get(&mid(1)),
        Some(&MessageOutcome::Completed)
    );
    assert!(!state.completions.contains_key(&mid(2)));
}

#[test]
fn a_deferred_completion_settles_to_exactly_one_terminal_outcome() {
    use openside_proto::MessageOutcome;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.db");
    let mut store = Store::open(&path).unwrap();
    store
        .create_session(sid(), "Task", "model", PolicyMode::Guard, None)
        .unwrap();
    let completion = |outcome| EventKind::MessageCompleted {
        message_id: mid(1),
        outcome,
        reason: None,
    };
    store
        .append(
            &sid(),
            None,
            submit(mid(1), "needs approval".into(), QueueMode::Queue),
        )
        .unwrap();
    store
        .append(
            &sid(),
            None,
            EventKind::MessageInjected { message_id: mid(1) },
        )
        .unwrap();
    store
        .append(&sid(), None, completion(MessageOutcome::Deferred))
        .unwrap();
    assert!(
        store
            .append(&sid(), None, completion(MessageOutcome::Deferred))
            .is_err(),
        "a deferral is announced once"
    );
    store
        .append(&sid(), None, completion(MessageOutcome::Completed))
        .unwrap();
    for outcome in [MessageOutcome::Aborted, MessageOutcome::Deferred] {
        assert!(store.append(&sid(), None, completion(outcome)).is_err());
    }
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store.session(&sid()).unwrap().completions.get(&mid(1)),
        Some(&MessageOutcome::Completed),
        "replay applies deferred -> completed"
    );
}

#[test]
fn resolving_a_ticket_is_terminal_and_clears_waiting_human() {
    let mut store = Store::open(":memory:").unwrap();
    store
        .create_session(sid(), "Task", "model", PolicyMode::Guard, None)
        .unwrap();
    let ticket = ticket(1, TicketDeferral::External);
    store
        .append(
            &sid(),
            None,
            EventKind::TicketOpened {
                ticket: Box::new(ticket.clone()),
            },
        )
        .unwrap();
    assert_eq!(
        store.session(&sid()).unwrap().session().status,
        SessionStatus::WaitingHuman
    );
    let resolution = EventKind::TicketResolved {
        ticket_id: ticket.id.clone(),
        resolution: json!("answer"),
    };
    store.append(&sid(), None, resolution.clone()).unwrap();
    let state = store.session(&sid()).unwrap().clone();
    assert_eq!(state.tickets[&ticket.id].status, TicketStatus::Resolved);
    assert_eq!(state.tickets[&ticket.id].resolution, Some(json!("answer")));
    assert_eq!(state.session().status, SessionStatus::Idle);
    assert!(store.append(&sid(), None, resolution).is_err());
    assert!(store
        .append(
            &sid(),
            None,
            EventKind::TicketRejected {
                ticket_id: ticket.id.clone(),
                reason: "late".into(),
            }
        )
        .is_err());
    assert!(store
        .append(
            &sid(),
            None,
            EventKind::TicketExpired {
                ticket_id: ticket.id
            }
        )
        .is_err());
    assert_eq!(store.session(&sid()).unwrap(), &state);
}

#[test]
fn rejection_and_expiration_only_apply_to_pending_tickets() {
    let mut store = Store::open(":memory:").unwrap();
    store
        .create_session(sid(), "Task", "model", PolicyMode::Guard, None)
        .unwrap();
    for number in 1..=2 {
        store
            .append(
                &sid(),
                None,
                EventKind::TicketOpened {
                    ticket: Box::new(ticket(number, TicketDeferral::Approval)),
                },
            )
            .unwrap();
    }
    let rejected_id = TicketId::new("ticket-1");
    let expired_id = TicketId::new("ticket-2");
    store
        .append(
            &sid(),
            None,
            EventKind::TicketRejected {
                ticket_id: rejected_id.clone(),
                reason: "declined".into(),
            },
        )
        .unwrap();
    store
        .append(
            &sid(),
            None,
            EventKind::TicketExpired {
                ticket_id: expired_id.clone(),
            },
        )
        .unwrap();
    let state = store.session(&sid()).unwrap().clone();
    assert_eq!(state.tickets[&rejected_id].status, TicketStatus::Rejected);
    assert_eq!(state.tickets[&expired_id].status, TicketStatus::Expired);
    assert_eq!(state.session().status, SessionStatus::Idle);
    for id in [rejected_id, expired_id, TicketId::new("unknown")] {
        for event in [
            EventKind::TicketResolved {
                ticket_id: id.clone(),
                resolution: json!(true),
            },
            EventKind::TicketRejected {
                ticket_id: id.clone(),
                reason: "late".into(),
            },
            EventKind::TicketExpired { ticket_id: id },
        ] {
            assert!(store.append(&sid(), None, event).is_err());
            assert_eq!(store.session(&sid()).unwrap(), &state);
        }
    }
}

#[test]
fn invalid_ticket_creation_does_not_consume_a_sequence() {
    let mut store = Store::open(":memory:").unwrap();
    store
        .create_session(sid(), "Task", "model", PolicyMode::Guard, None)
        .unwrap();
    let valid = ticket(1, TicketDeferral::Approval);
    let mut wrong_session = valid.clone();
    wrong_session.session_id = SessionId::new("other");
    let mut terminal = valid.clone();
    terminal.status = TicketStatus::Resolved;
    let mut resolved = valid.clone();
    resolved.resolution = Some(json!(true));
    let mut missing_tool = valid.clone();
    missing_tool.tool = None;
    for invalid in [wrong_session, terminal, resolved, missing_tool] {
        assert!(store
            .append(
                &sid(),
                None,
                EventKind::TicketOpened {
                    ticket: Box::new(invalid)
                }
            )
            .is_err());
        assert_eq!(store.session(&sid()).unwrap().last_seq, 1);
        assert!(store.session(&sid()).unwrap().tickets.is_empty());
    }
    store
        .append(
            &sid(),
            None,
            EventKind::TicketOpened {
                ticket: Box::new(valid.clone()),
            },
        )
        .unwrap();
    assert!(store
        .append(
            &sid(),
            None,
            EventKind::TicketOpened {
                ticket: Box::new(valid)
            }
        )
        .is_err());
    assert_eq!(store.session(&sid()).unwrap().last_seq, 2);
    let mut external_with_tool = ticket(2, TicketDeferral::External);
    external_with_tool.tool = Some(TicketTool {
        name: "mock".into(),
        args: json!({}),
    });
    store
        .append(
            &sid(),
            None,
            EventKind::TicketOpened {
                ticket: Box::new(external_with_tool),
            },
        )
        .unwrap();
}

#[test]
fn agent_state_upsert_replaces_without_appending_events() {
    let mut store = Store::open(":memory:").unwrap();
    store
        .create_session(sid(), "Task", "model", PolicyMode::Guard, None)
        .unwrap();
    assert_eq!(store.agent_state(&sid()).unwrap(), None);
    let first = AgentState {
        format: "format-1".into(),
        data: "first snapshot".into(),
    };
    let second = AgentState {
        format: "format-2".into(),
        data: "second snapshot".into(),
    };
    store.put_agent_state(&sid(), &first).unwrap();
    assert_eq!(store.agent_state(&sid()).unwrap(), Some(first));
    store.put_agent_state(&sid(), &second).unwrap();
    assert_eq!(store.agent_state(&sid()).unwrap(), Some(second));
    assert_eq!(store.events_after(&sid(), 0, 100).unwrap().len(), 1);
    assert_eq!(store.session(&sid()).unwrap().last_seq, 1);
}

#[test]
fn agent_state_operations_reject_unknown_sessions() {
    let mut store = Store::open(":memory:").unwrap();
    let state = AgentState {
        format: "format".into(),
        data: "snapshot".into(),
    };
    assert!(
        matches!(store.agent_state(&sid()), Err(crate::Error::UnknownSession(id)) if id == sid())
    );
    assert!(
        matches!(store.put_agent_state(&sid(), &state), Err(crate::Error::UnknownSession(id)) if id == sid())
    );
    store
        .create_session(sid(), "Task", "model", PolicyMode::Guard, None)
        .unwrap();
    assert_eq!(store.agent_state(&sid()).unwrap(), None);
}

#[test]
fn agent_state_survives_reopen_and_reads_other_connection_writes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.db");
    let mut first = Store::open(&path).unwrap();
    let mut second = Store::open(&path).unwrap();
    first
        .create_session(sid(), "Task", "model", PolicyMode::Guard, None)
        .unwrap();
    let state = AgentState {
        format: "format".into(),
        data: "opaque snapshot".into(),
    };
    second.put_agent_state(&sid(), &state).unwrap();
    assert_eq!(first.agent_state(&sid()).unwrap(), Some(state.clone()));
    drop(first);
    drop(second);
    assert_eq!(
        Store::open(&path).unwrap().agent_state(&sid()).unwrap(),
        Some(state)
    );
}

#[test]
fn workspace_and_policy_mode_survive_session_replay() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.db");
    let workspace = Some(directory.path().to_str().unwrap().to_owned());
    let mut store = Store::open(&path).unwrap();
    store
        .create_session(sid(), "Task", "model", PolicyMode::Full, workspace.clone())
        .unwrap();
    let live = store.session(&sid()).unwrap().clone();
    assert_eq!(live.workspace, workspace);
    assert_eq!(live.session().workspace, workspace);
    assert_eq!(live.session().mode, PolicyMode::Full);
    drop(store);
    assert_eq!(Store::open(&path).unwrap().session(&sid()).unwrap(), &live);
}

#[test]
fn historical_session_creation_defaults_mode_and_workspace() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.db");
    drop(Store::open(&path).unwrap());
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO events(session_id,seq,ts,payload) VALUES(?1,1,?2,?3)",
            params![
                sid().as_str(),
                Utc::now().to_rfc3339(),
                r#"{"kind":"session.created","title":"Task","model":"model"}"#
            ],
        )
        .unwrap();
    drop(connection);
    let store = Store::open(&path).unwrap();
    let session = store.session(&sid()).unwrap().session();
    assert_eq!(session.mode, PolicyMode::Guard);
    assert_eq!(session.workspace, None);
}

#[test]
fn approval_claims_and_resolution_deliveries_are_durable_and_atomic() {
    use crate::ApprovalClaim;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.db");
    let mut store = Store::open(&path).unwrap();
    store
        .create_session(sid(), "Task", "model", PolicyMode::Guard, None)
        .unwrap();
    let approval = ticket(1, TicketDeferral::Approval);
    store.bind_approval(&sid(), &approval.id, &[1; 32]).unwrap();
    store
        .append(
            &sid(),
            None,
            EventKind::TicketOpened {
                ticket: Box::new(approval.clone()),
            },
        )
        .unwrap();
    assert_eq!(
        store
            .claim_approval(
                &sid(),
                &approval.id,
                "call-1",
                "mock",
                Some("child"),
                &[1; 32]
            )
            .unwrap(),
        ApprovalClaim::Invalid
    );
    assert!(store.mark_ticket_delivered(&sid(), &approval.id).is_err());
    store
        .append(
            &sid(),
            None,
            EventKind::TicketResolved {
                ticket_id: approval.id.clone(),
                resolution: json!({"approved": true}),
            },
        )
        .unwrap();
    assert_eq!(
        store
            .claim_approval(
                &sid(),
                &approval.id,
                "call-1",
                "mock",
                Some("child"),
                &[2; 32]
            )
            .unwrap(),
        ApprovalClaim::ArgsMismatch
    );
    assert_eq!(
        store
            .claim_approval(
                &sid(),
                &approval.id,
                "other-call",
                "mock",
                Some("child"),
                &[1; 32]
            )
            .unwrap(),
        ApprovalClaim::Invalid
    );
    assert_eq!(
        store.undelivered_tickets(&sid()).unwrap(),
        vec![approval.id.clone()]
    );
    assert!(store.mark_ticket_delivered(&sid(), &approval.id).unwrap());
    drop(store);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let writers: Vec<_> = (0..2)
        .map(|_| {
            let path = path.clone();
            let barrier = barrier.clone();
            let id = approval.id.clone();
            std::thread::spawn(move || {
                let mut store = Store::open(path).unwrap();
                barrier.wait();
                store
                    .claim_approval(&sid(), &id, "call-1", "mock", Some("child"), &[1; 32])
                    .unwrap()
            })
        })
        .collect();
    let claims: Vec<_> = writers
        .into_iter()
        .map(|writer| writer.join().unwrap())
        .collect();
    assert_eq!(
        claims
            .iter()
            .filter(|claim| **claim == ApprovalClaim::Claimed)
            .count(),
        1
    );
    assert_eq!(
        claims
            .iter()
            .filter(|claim| **claim == ApprovalClaim::AlreadyUsed)
            .count(),
        1
    );
    let mut recovered = Store::open(path).unwrap();
    assert_eq!(
        recovered
            .claim_approval(
                &sid(),
                &approval.id,
                "call-1",
                "mock",
                Some("child"),
                &[1; 32]
            )
            .unwrap(),
        ApprovalClaim::AlreadyUsed
    );
    assert!(!recovered
        .mark_ticket_delivered(&sid(), &approval.id)
        .unwrap());
    assert!(recovered.undelivered_tickets(&sid()).unwrap().is_empty());
}

#[test]
fn approval_claim_is_bound_to_parent_or_exact_child_origin() {
    use crate::ApprovalClaim;
    let mut store = Store::open(":memory:").unwrap();
    store
        .create_session(sid(), "Task", "model", PolicyMode::Guard, None)
        .unwrap();
    let mut parent = ticket(1, TicketDeferral::Approval);
    parent.origin.subagent_id = None;
    let mut child = ticket(2, TicketDeferral::Approval);
    child.origin.subagent_id = Some("child-a".into());
    for approval in [&parent, &child] {
        store.bind_approval(&sid(), &approval.id, &[1; 32]).unwrap();
        store
            .append(
                &sid(),
                None,
                EventKind::TicketOpened {
                    ticket: Box::new(approval.clone()),
                },
            )
            .unwrap();
        store
            .append(
                &sid(),
                None,
                EventKind::TicketResolved {
                    ticket_id: approval.id.clone(),
                    resolution: json!({"approved": true}),
                },
            )
            .unwrap();
    }
    let mut claim = |id: &TicketId, call: &str, origin: Option<&str>| {
        store
            .claim_approval(&sid(), id, call, "mock", origin, &[1; 32])
            .unwrap()
    };
    let child_call = child.tool_call_id.clone();
    let parent_call = parent.tool_call_id.clone();
    // Parent cannot spend a child's approval, a sibling cannot either, and a child cannot spend the parent's.
    assert_eq!(claim(&child.id, &child_call, None), ApprovalClaim::Invalid);
    assert_eq!(
        claim(&child.id, &child_call, Some("child-b")),
        ApprovalClaim::Invalid
    );
    assert_eq!(
        claim(&parent.id, &parent_call, Some("child-a")),
        ApprovalClaim::Invalid
    );
    assert_eq!(
        claim(&child.id, &child_call, Some("child-a")),
        ApprovalClaim::Claimed
    );
    assert_eq!(
        claim(&parent.id, &parent_call, None),
        ApprovalClaim::Claimed
    );
}

#[test]
fn legacy_approved_ticket_without_private_binding_fails_closed() {
    let mut store = Store::open(":memory:").unwrap();
    store
        .create_session(sid(), "Task", "model", PolicyMode::Guard, None)
        .unwrap();
    let ticket = ticket(1, TicketDeferral::Approval);
    store
        .append(
            &sid(),
            None,
            EventKind::TicketOpened {
                ticket: Box::new(ticket.clone()),
            },
        )
        .unwrap();
    store
        .append(
            &sid(),
            None,
            EventKind::TicketResolved {
                ticket_id: ticket.id.clone(),
                resolution: json!({"approved": true}),
            },
        )
        .unwrap();
    assert_eq!(
        store
            .claim_approval(
                &sid(),
                &ticket.id,
                "call-1",
                "mock",
                Some("child"),
                &[1; 32]
            )
            .unwrap(),
        crate::ApprovalClaim::Invalid
    );
}

#[test]
fn replay_keeps_an_injected_task_running_across_model_and_tool_boundaries() {
    use openside_proto::MessageOutcome;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.db");
    let mut store = Store::open(&path).unwrap();
    store
        .create_session(sid(), "Task", "model", PolicyMode::Guard, None)
        .unwrap();
    for event in [
        submit(mid(1), "inspect".into(), QueueMode::Queue),
        EventKind::MessageInjected { message_id: mid(1) },
        EventKind::TurnStarted {},
        EventKind::TurnEnded {},
        EventKind::ToolStarted {
            tool_call_id: "first".into(),
            name: "read".into(),
            args: json!({}),
        },
        EventKind::ToolEnded {
            tool_call_id: "first".into(),
            result: json!("read"),
            is_error: false,
        },
        EventKind::TurnStarted {},
        EventKind::TurnEnded {},
        EventKind::ToolStarted {
            tool_call_id: "second".into(),
            name: "read".into(),
            args: json!({}),
        },
    ] {
        store.append(&sid(), None, event).unwrap();
    }
    let active = store.session(&sid()).unwrap().clone();
    assert_eq!(active.session().status, SessionStatus::Running);
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.session(&sid()).unwrap(), &active);
    assert_eq!(store.list_sessions()[0].status, SessionStatus::Running);
    store
        .append(
            &sid(),
            None,
            EventKind::MessageCompleted {
                message_id: mid(1),
                outcome: MessageOutcome::Completed,
                reason: None,
            },
        )
        .unwrap();
    assert_eq!(store.session(&sid()).unwrap().status, SessionStatus::Idle);
    store.replay().unwrap();
    assert_eq!(store.session(&sid()).unwrap().status, SessionStatus::Idle);
}

#[test]
fn task_status_waits_for_all_injected_messages_and_deferral_keeps_human_waiting_distinct() {
    use openside_proto::MessageOutcome;
    let mut store = Store::open(":memory:").unwrap();
    store
        .create_session(sid(), "Task", "model", PolicyMode::Guard, None)
        .unwrap();
    for number in 1..=2 {
        store
            .append(
                &sid(),
                None,
                submit(mid(number), "work".into(), QueueMode::Queue),
            )
            .unwrap();
        store
            .append(
                &sid(),
                None,
                EventKind::MessageInjected {
                    message_id: mid(number),
                },
            )
            .unwrap();
        assert_eq!(
            store.session(&sid()).unwrap().status,
            SessionStatus::Running
        );
    }
    store
        .append(
            &sid(),
            None,
            EventKind::MessageCompleted {
                message_id: mid(1),
                outcome: MessageOutcome::Completed,
                reason: None,
            },
        )
        .unwrap();
    assert_eq!(
        store.session(&sid()).unwrap().status,
        SessionStatus::Running
    );
    store
        .append(
            &sid(),
            None,
            EventKind::TicketOpened {
                ticket: Box::new(ticket(1, TicketDeferral::External)),
            },
        )
        .unwrap();
    store
        .append(
            &sid(),
            None,
            EventKind::MessageCompleted {
                message_id: mid(2),
                outcome: MessageOutcome::Deferred,
                reason: None,
            },
        )
        .unwrap();
    assert_eq!(
        store.session(&sid()).unwrap().session().status,
        SessionStatus::WaitingHuman
    );
    store
        .append(
            &sid(),
            None,
            EventKind::TicketResolved {
                ticket_id: TicketId::new("ticket-1"),
                resolution: json!("answer"),
            },
        )
        .unwrap();
    for event in [EventKind::TurnStarted {}, EventKind::TurnEnded {}] {
        store.append(&sid(), None, event).unwrap();
    }
    assert_eq!(
        store.session(&sid()).unwrap().status,
        SessionStatus::Running
    );
    store
        .append(
            &sid(),
            None,
            EventKind::MessageCompleted {
                message_id: mid(2),
                outcome: MessageOutcome::Completed,
                reason: None,
            },
        )
        .unwrap();
    assert_eq!(store.session(&sid()).unwrap().status, SessionStatus::Idle);
}
