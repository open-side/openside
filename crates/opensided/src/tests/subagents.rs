use super::*;
use crate::tools::TicketRequest;

async fn start_child(daemon: &Daemon, session: &SessionId, id: &str) {
    for kind in [
        EventKind::SubagentStarted {
            subagent_id: id.into(),
            parent_tool_call_id: format!("delegate-{id}"),
            task: "work".into(),
            model: "test/model".into(),
            tools: vec!["write".into()],
            background: false,
        },
        EventKind::SubagentLifecycle {
            subagent_id: id.into(),
            status: "waiting_human".into(),
        },
    ] {
        daemon.shared.emit(session, None, kind).await.unwrap();
    }
}

async fn child_ticket(daemon: &Daemon, session: &SessionId, child: &str) -> Ticket {
    let call = ToolCall {
        subagent: Some(child.into()),
        ..call("ask_user", json!({}), &format!("call-{child}"))
    };
    let id = crate::tickets::open(
        &daemon.shared,
        session,
        &call,
        TicketDeferral::External,
        request(),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    daemon.shared.db.state(session).await.unwrap().tickets[&id].clone()
}

fn request() -> TicketRequest {
    TicketRequest {
        hire: None,
        kind: TicketKind::Info,
        summary: "Question".into(),
        instructions: None,
        options: Vec::new(),
        target: None,
        resume_when: None,
        otp: None,
        otp_register: None,
        login: None,
        choice: None,
        approval: None,
        form: None,
        pins: Vec::new(),
    }
}

async fn status(daemon: &Daemon, session: &SessionId, id: &str) -> (SubagentStatus, Vec<String>) {
    let child = crate::subagents::get(&daemon.shared, session, id)
        .await
        .unwrap()
        .unwrap();
    let rejected = daemon
        .shared
        .db
        .state(session)
        .await
        .unwrap()
        .tickets
        .values()
        .filter(|ticket| {
            ticket.origin.subagent_id.as_deref() == Some(id)
                && ticket.status == TicketStatus::Rejected
        })
        .map(|ticket| ticket.id.to_string())
        .collect();
    (child.status, rejected)
}

#[tokio::test]
async fn restore_fails_children_the_sidecar_snapshot_cannot_wake() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    start_child(&daemon, &session, "saved").await;
    start_child(&daemon, &session, "orphan").await;
    let saved = child_ticket(&daemon, &session, "saved").await;
    let orphan = child_ticket(&daemon, &session, "orphan").await;
    // Only the saved child's call is in the restored snapshot; the orphan's ticket was
    // opened after the last checkpoint.
    runtime.0.lock().ready_deferred.push(DeferredCall {
        ticket: saved.id.clone(),
        tool_call_id: saved.tool_call_id.clone(),
        kind: DeferKind::External,
    });
    daemon
        .shared
        .emit(
            &session,
            None,
            EventKind::TicketResolved {
                card: None,
                ticket_id: saved.id.clone(),
                resolution: json!({"answer":"blue"}),
            },
        )
        .await
        .unwrap();
    daemon
        .shared
        .command(&session, crate::engine::Action::Resume(saved.id.clone()))
        .await
        .unwrap();
    assert_eq!(
        status(&daemon, &session, "orphan").await,
        (SubagentStatus::Failed, vec![orphan.id.to_string()])
    );
    assert_eq!(
        status(&daemon, &session, "saved").await.0,
        SubagentStatus::WaitingHuman
    );
    assert_eq!(runtime.0.lock().resolved.len(), 1, "saved child resumes");
    assert_eq!(runtime.0.lock().resolved[0].0, saved.id);
    daemon.shutdown().await;
}

#[tokio::test]
async fn cancelled_child_cannot_open_or_keep_tickets() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    start_child(&daemon, &session, "child").await;
    let first = child_ticket(&daemon, &session, "child").await;
    assert!(crate::subagents::cancel(&daemon.shared, &session, "child")
        .await
        .unwrap());
    assert!(!crate::subagents::cancel(&daemon.shared, &session, "child")
        .await
        .unwrap());
    assert_eq!(
        status(&daemon, &session, "child").await,
        (SubagentStatus::Cancelled, vec![first.id.to_string()])
    );
    // A call admitted before the cancel that reaches ticket creation afterwards is refused
    // under the same gate, so no pending ticket can outlive its child.
    let late = ToolCall {
        subagent: Some("child".into()),
        ..call("ask_user", json!({}), "call-late")
    };
    let refused = crate::tickets::open(
        &daemon.shared,
        &session,
        &late,
        TicketDeferral::External,
        request(),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(refused.unwrap_err().to_string(), "subagent_finished");
    assert!(daemon
        .shared
        .db
        .state(&session)
        .await
        .unwrap()
        .tickets
        .values()
        .all(|ticket| ticket.status != TicketStatus::Pending));
    daemon.shutdown().await;
}

async fn finals(daemon: &Daemon, session: &SessionId) -> usize {
    let mut count = 0;
    let mut after = 0;
    loop {
        let page = daemon.shared.db.events(session, after).await.unwrap();
        if page.is_empty() {
            return count;
        }
        for event in page {
            after = event.seq;
            count += usize::from(matches!(event.kind, EventKind::AssistantFinal { .. }));
        }
    }
}

#[tokio::test]
async fn startup_alone_fails_an_orphan_waiting_child_and_frees_the_queue() {
    let root = tempfile::tempdir().unwrap();
    let first = daemon(root.path(), &TestRuntime::default()).await;
    let session = create_session(&first, "session", PolicyMode::Guard, None).await;
    start_child(&first, &session, "orphan").await;
    let orphan = child_ticket(&first, &session, "orphan").await;
    first.shutdown().await;
    drop(first);
    // The restored snapshot predates the child: no deferred calls at all.
    let runtime = TestRuntime::default();
    let second = daemon(root.path(), &runtime).await;
    assert_eq!(
        status(&second, &session, "orphan").await,
        (SubagentStatus::Failed, vec![orphan.id.to_string()])
    );
    let child = crate::subagents::get(&second.shared, &session, "orphan")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(child.reason.as_deref(), Some("not_recoverable"));
    second.shutdown().await;
}

#[tokio::test]
async fn startup_closes_tickets_left_by_a_half_recorded_child_end() {
    let root = tempfile::tempdir().unwrap();
    let first = daemon(root.path(), &TestRuntime::default()).await;
    let session = create_session(&first, "session", PolicyMode::Guard, None).await;
    start_child(&first, &session, "child").await;
    let ticket = child_ticket(&first, &session, "child").await;
    // Crash after the terminal event but before the ticket rejections.
    first
        .shared
        .emit(
            &session,
            None,
            EventKind::SubagentEnded {
                subagent_id: "child".into(),
                status: SubagentStatus::Cancelled,
                reason: Some("cancelled".into()),
                output: None,
            },
        )
        .await
        .unwrap();
    first.shutdown().await;
    drop(first);
    let second = daemon(root.path(), &TestRuntime::default()).await;
    assert_eq!(
        status(&second, &session, "child").await,
        (SubagentStatus::Cancelled, vec![ticket.id.to_string()])
    );
    second.shutdown().await;
}

#[tokio::test]
async fn restore_discards_a_continuation_whose_final_is_already_durable() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    let ticket = opened(
        &daemon,
        &session,
        TicketKind::Info,
        "ticket",
        Utc::now() + chrono::Duration::hours(1),
    )
    .await;
    // The resumed run finished and its final is durable; the crash lost only the
    // snapshot, so the restored sidecar still reports the call as deferred.
    for kind in [
        EventKind::TicketResolved {
            card: None,
            ticket_id: ticket.id.clone(),
            resolution: json!({"answer":"blue"}),
        },
        EventKind::AssistantFinal {
            text: "done".into(),
        },
    ] {
        daemon.shared.emit(&session, None, kind).await.unwrap();
    }
    runtime.0.lock().ready_deferred.push(DeferredCall {
        ticket: ticket.id.clone(),
        tool_call_id: ticket.tool_call_id.clone(),
        kind: DeferKind::External,
    });
    daemon
        .shared
        .command(&session, crate::engine::Action::Recover)
        .await
        .unwrap();
    assert_eq!(runtime.0.lock().discarded, 1);
    assert!(
        runtime.0.lock().resolved.is_empty(),
        "continuation is not rerun"
    );
    assert_eq!(finals(&daemon, &session).await, 1);
    daemon.shutdown().await;
}

#[tokio::test]
async fn restore_still_resumes_a_continuation_without_a_durable_final() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let daemon = daemon(root.path(), &runtime).await;
    let session = create_session(&daemon, "session", PolicyMode::Guard, None).await;
    daemon
        .shared
        .emit(
            &session,
            None,
            EventKind::AssistantFinal {
                text: "earlier".into(),
            },
        )
        .await
        .unwrap();
    let ticket = opened(
        &daemon,
        &session,
        TicketKind::Info,
        "ticket",
        Utc::now() + chrono::Duration::hours(1),
    )
    .await;
    daemon
        .shared
        .emit(
            &session,
            None,
            EventKind::TicketResolved {
                card: None,
                ticket_id: ticket.id.clone(),
                resolution: json!({"answer":"blue"}),
            },
        )
        .await
        .unwrap();
    runtime.0.lock().ready_deferred.push(DeferredCall {
        ticket: ticket.id.clone(),
        tool_call_id: ticket.tool_call_id.clone(),
        kind: DeferKind::External,
    });
    daemon
        .shared
        .command(&session, crate::engine::Action::Recover)
        .await
        .unwrap();
    assert_eq!(runtime.0.lock().discarded, 0);
    assert_eq!(runtime.0.lock().resolved.len(), 1);
    daemon.shutdown().await;
}
