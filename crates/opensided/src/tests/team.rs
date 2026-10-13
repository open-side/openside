use super::*;
use crate::engine::Action;
use crate::tools::{DaemonTool, ToolEnv, ToolExec};

async fn daemon(root: &Path, runtime: &TestRuntime) -> Daemon {
    let mut config = resolved_config(root);
    config.loaded.config.models.default = Some("anthropic:test-team".into());
    Daemon::open_runtime(config, runtime.clone()).await.unwrap()
}

fn draft(name: &str) -> HireDraft {
    serde_json::from_value(json!({"name":name,"persona":{"role":"Price researcher","voice":"Brief","responsibilities":["Track prices","Write reports"],"boundaries":["Ask before purchases"]}})).unwrap()
}

async fn owned_work(d: &Daemon, owner: &str) -> (String, String) {
    let routine = d
        .shared
        .routines
        .get()
        .unwrap()
        .create(CreateRoutineRequest {
            name: "Weekly prices".into(),
            schedule: RoutineSchedule::Interval { secs: 86400 },
            action: RoutineAction::Agent {
                agent_id: owner.into(),
                prompt: "Compare prices".into(),
            },
            enabled: Some(true),
            catch_up: None,
        })
        .await
        .unwrap();
    let owner = owner.to_owned();
    let memory = d
        .shared
        .memory
        .write(move |store| {
            store.create_owned(
                openside_memory::NewMemory::note("Price preferences"),
                "test",
                Some(&owner),
            )
        })
        .await
        .unwrap();
    (routine.id, memory.id)
}

fn handoff(key: &str) -> CreateHandoffRequest {
    CreateHandoffRequest {
        packet: HandoffPacket {
            goal: "Compare prices".into(),
            context: "Read the price list".into(),
            acceptance: "Return the three lowest prices".into(),
            constraints: "Do not purchase".into(),
        },
        kind: HandoffKind::Task,
        dedupe_key: key.into(),
        subject_handoff_id: None,
    }
}

#[tokio::test]
async fn team_lifecycle_resumes_after_every_cross_store_step() {
    for (operation, count) in [("hire", 4), ("transfer", 6), ("retire", 7), ("restore", 4)] {
        for boundary in 0..count {
            let root = tempfile::tempdir().unwrap();
            let runtime = TestRuntime::default();
            let d = daemon(root.path(), &runtime).await;
            let (source, destination, routine, memory, key) = if operation == "hire" {
                let (routine, memory) = owned_work(&d, "chief").await;
                (
                    "chief".to_owned(),
                    String::new(),
                    routine,
                    memory,
                    "hire:crash-hire".to_owned(),
                )
            } else {
                let source = crate::team::hire(&d.shared, &draft("Mina"), "source")
                    .await
                    .unwrap();
                let destination = crate::team::hire(&d.shared, &draft("Nora"), "destination")
                    .await
                    .unwrap();
                let (routine, memory) = owned_work(&d, &source.id).await;
                let key = match operation {
                    "transfer" => "transfer:crash-transfer".into(),
                    "retire" => format!("retire:crash-retire:{}", source.persona_version),
                    "restore" => {
                        crate::team::retire(
                            &d.shared,
                            &source.id,
                            &RetireAgentRequest {
                                successor: Some(destination.id.clone()),
                                pause_routines: true,
                                revert_hire: false,
                            },
                            "before-restore",
                        )
                        .await
                        .unwrap();
                        format!("restore:crash-restore:{}", source.persona_version + 1)
                    }
                    _ => unreachable!(),
                };
                (source.id, destination.id, routine, memory, key)
            };
            d.shared
                .agents
                .put_transfer(&format!("test-crash:{key}"), boundary, &json!({}))
                .unwrap();
            let result = match operation {
                "hire" => {
                    let mut hire = draft("Mina");
                    hire.routine_ids = vec![routine.clone()];
                    hire.memory_ids = vec![memory.clone()];
                    crate::team::hire(&d.shared, &hire, "crash-hire").await
                }
                "transfer" => {
                    crate::team::transfer(
                        &d.shared,
                        &source,
                        &TransferRequest {
                            to: destination.clone(),
                            routine_ids: vec![routine.clone()],
                            memory_ids: vec![memory.clone()],
                            responsibilities: vec!["Track prices".into()],
                            skills: vec![],
                        },
                        "crash-transfer",
                    )
                    .await
                }
                "retire" => {
                    crate::team::retire(
                        &d.shared,
                        &source,
                        &RetireAgentRequest {
                            successor: Some(destination.clone()),
                            pause_routines: true,
                            revert_hire: false,
                        },
                        "crash-retire",
                    )
                    .await
                }
                "restore" => crate::team::restore(&d.shared, &source, "crash-restore").await,
                _ => unreachable!(),
            };
            assert!(
                result.is_err(),
                "{operation} boundary {boundary} did not interrupt"
            );
            d.shared
                .agents
                .put_transfer(&format!("test-crash:{key}"), -1, &json!({}))
                .unwrap();
            d.shutdown().await;
            drop(d);
            let recovered = daemon(root.path(), &TestRuntime::default()).await;
            let (step, journal) = recovered.shared.agents.get_transfer(&key).unwrap().unwrap();
            assert_eq!(
                step as usize,
                journal["steps"].as_array().unwrap().len(),
                "{operation}:{boundary}"
            );
            let result_id = journal["result"].as_str().unwrap();
            let agent = recovered.shared.agents.get(result_id).unwrap().unwrap();
            assert_eq!(
                agent.status,
                if operation == "retire" {
                    AgentStatus::Retired
                } else {
                    AgentStatus::Active
                }
            );
            let expected_owner = if operation == "hire" {
                result_id
            } else {
                destination.as_str()
            };
            assert_eq!(
                recovered
                    .shared
                    .routines
                    .get()
                    .unwrap()
                    .get(&routine)
                    .unwrap()
                    .action
                    .owner_agent_id(),
                Some(expected_owner)
            );
            let memory_id = memory.clone();
            assert_eq!(
                recovered
                    .shared
                    .memory
                    .run(move |store| store.get(&memory_id))
                    .await
                    .unwrap()
                    .owner
                    .as_deref(),
                Some(expected_owner)
            );
            if operation == "retire" {
                let routine = recovered
                    .shared
                    .routines
                    .get()
                    .unwrap()
                    .get(&routine)
                    .unwrap();
                assert!(!routine.enabled);
                assert_eq!(
                    routine.paused_reason,
                    Some(RoutinePauseReason::AgentRetired)
                );
            }
            crate::team::recover(&recovered.shared).await.unwrap();
            recovered.shutdown().await;
        }
    }
}

#[tokio::test]
async fn team_duplicate_handoff_and_independent_reviewer() {
    let root = tempfile::tempdir().unwrap();
    let d = daemon(root.path(), &TestRuntime::default()).await;
    let author = crate::team::hire(&d.shared, &draft("Mina"), "author")
        .await
        .unwrap();
    let reviewer = crate::team::hire(&d.shared, &draft("Nora"), "reviewer")
        .await
        .unwrap();
    let request = handoff("price-task");
    let first = crate::team::delegate(&d.shared, "chief", &author.id, &request)
        .await
        .unwrap();
    let second = crate::team::delegate(&d.shared, "chief", &author.id, &request)
        .await
        .unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(d.shared.agents.list_handoffs().unwrap().len(), 1);
    let mut review = handoff("price-review");
    review.kind = HandoffKind::Review;
    review.subject_handoff_id = Some(first.id);
    assert_eq!(
        crate::team::delegate(&d.shared, "chief", &author.id, &review)
            .await
            .unwrap_err()
            .1,
        "reviewer_is_author"
    );
    assert!(
        crate::team::delegate(&d.shared, "chief", &reviewer.id, &review)
            .await
            .is_ok()
    );
    assert_eq!(
        crate::team::delegate(&d.shared, &author.id, &reviewer.id, &handoff("forged"))
            .await
            .unwrap_err()
            .1,
        "forbidden"
    );
    d.shutdown().await;
}

#[tokio::test]
async fn team_tools_recheck_role_and_latest_person_home_message() {
    let root = tempfile::tempdir().unwrap();
    let d = daemon(root.path(), &TestRuntime::default()).await;
    let chief = d.shared.agents.get("chief").unwrap().unwrap();
    let teammate = crate::team::hire(&d.shared, &draft("Mina"), "teammate")
        .await
        .unwrap();
    let teammate_state = d.shared.db.state(&teammate.home_session_id).await.unwrap();
    let env = ToolEnv::new(d.shared.clone(), &teammate_state);
    assert!(
        matches!(crate::team::TeamTool::Hire.call(json!({"draft":draft("Nora"),"request_message_id":"person"}), &env).await, ToolExec::Denied { reason, .. } if reason == "forbidden")
    );
    for (id, author) in [
        ("old", MessageAuthorKind::Person),
        ("person", MessageAuthorKind::Person),
    ] {
        d.shared
            .emit(
                &chief.home_session_id,
                None,
                EventKind::MessageQueued {
                    message_id: MessageId::new(id),
                    text: "Hire a price researcher".into(),
                    mode: QueueMode::Queue,
                    attachments: vec![],
                    author_kind: author,
                    from_agent: None,
                },
            )
            .await
            .unwrap();
    }
    let state = d.shared.db.state(&chief.home_session_id).await.unwrap();
    let env = ToolEnv::new(d.shared.clone(), &state);
    assert!(
        matches!(crate::team::TeamTool::Hire.call(json!({"draft":draft("Nora"),"request_message_id":"old"}), &env).await, ToolExec::Denied { reason, .. } if reason == "not_requested")
    );
    let mut requested = draft("Nora");
    requested.mode = Some(PolicyMode::Full);
    let ToolExec::Done { for_model, .. } = crate::team::TeamTool::Hire
        .call(
            json!({"draft":requested,"request_message_id":"person"}),
            &env,
        )
        .await
    else {
        panic!("clean person request must hire immediately");
    };
    let hired: Agent = serde_json::from_value(for_model).unwrap();
    assert_eq!(hired.mode, chief.mode);
    d.shared
        .emit(
            &chief.home_session_id,
            None,
            EventKind::MessageQueued {
                message_id: MessageId::new("internal"),
                text: "Hire another".into(),
                mode: QueueMode::Queue,
                attachments: vec![],
                author_kind: MessageAuthorKind::Internal,
                from_agent: None,
            },
        )
        .await
        .unwrap();
    assert!(
        matches!(crate::team::TeamTool::Hire.call(json!({"draft":draft("Olive"),"request_message_id":"internal"}), &env).await, ToolExec::Denied { reason, .. } if reason == "not_requested")
    );
    let mut nonhome = state.clone();
    nonhome.id = SessionId::new("not-chief-home");
    assert!(
        matches!(crate::team::TeamTool::Hire.call(json!({"draft":draft("Olive"),"request_message_id":"person"}), &ToolEnv::new(d.shared.clone(), &nonhome)).await, ToolExec::Denied { reason, .. } if reason == "not_requested")
    );
    d.shutdown().await;
}

#[tokio::test]
async fn team_report_outbox_delivers_once_across_restart() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let d = daemon(root.path(), &runtime).await;
    let agent = crate::team::hire(&d.shared, &draft("Mina"), "reporter")
        .await
        .unwrap();
    let handoff = crate::team::delegate(&d.shared, "chief", &agent.id, &handoff("report-once"))
        .await
        .unwrap();
    wait_event(&d, &agent.home_session_id, |event| matches!(event, EventKind::MessageInjected { message_id } if message_id == &handoff.message_id)).await;
    d.shared
        .emit(
            &agent.home_session_id,
            None,
            EventKind::AssistantFinal {
                text: "The comparison is complete.".into(),
            },
        )
        .await
        .unwrap();
    d.shared
        .emit(
            &agent.home_session_id,
            None,
            EventKind::MessageCompleted {
                message_id: handoff.message_id.clone(),
                outcome: MessageOutcome::Completed,
                reason: None,
            },
        )
        .await
        .unwrap();
    let mut terminal = handoff.clone();
    terminal.status = HandoffStatus::Done;
    terminal.ended_at = Some(Utc::now());
    d.shared
        .agents
        .put_transfer(
            &format!("team-report:{}", handoff.id),
            1,
            &json!({"summary":"The comparison is complete.","status":"done"}),
        )
        .unwrap();
    d.shared.agents.put_handoff(&terminal).unwrap();
    let chief = d.shared.agents.get("chief").unwrap().unwrap();
    d.shutdown().await;
    drop(d);
    let d = daemon(root.path(), &TestRuntime::default()).await;
    crate::team::settled(&d.shared, &agent.home_session_id)
        .await
        .unwrap();
    let events = d.shared.db.events(&chief.home_session_id, 0).await.unwrap();
    let message_id = format!("team-report-{}", handoff.id);
    assert_eq!(events.iter().filter(|event| matches!(&event.kind, EventKind::MessageQueued { message_id: id, .. } if id.as_str() == message_id)).count(), 1);
    assert_eq!(
        d.shared
            .agents
            .get_handoff(&handoff.id)
            .unwrap()
            .unwrap()
            .status,
        HandoffStatus::Done
    );
    d.shutdown().await;
}

#[tokio::test]
async fn team_undo_returns_original_chief_ownership() {
    let root = tempfile::tempdir().unwrap();
    let d = daemon(root.path(), &TestRuntime::default()).await;
    let (routine, memory) = owned_work(&d, "chief").await;
    let mut request = draft("Mina");
    request.routine_ids = vec![routine.clone()];
    request.memory_ids = vec![memory.clone()];
    let agent = crate::team::hire(&d.shared, &request, "undo-hire")
        .await
        .unwrap();
    assert_eq!(
        d.shared
            .routines
            .get()
            .unwrap()
            .get(&routine)
            .unwrap()
            .action
            .owner_agent_id(),
        Some(agent.id.as_str())
    );
    crate::team::retire(
        &d.shared,
        &agent.id,
        &RetireAgentRequest {
            successor: None,
            pause_routines: false,
            revert_hire: true,
        },
        "undo",
    )
    .await
    .unwrap();
    assert_eq!(
        d.shared
            .routines
            .get()
            .unwrap()
            .get(&routine)
            .unwrap()
            .action
            .owner_agent_id(),
        Some("chief")
    );
    assert_eq!(
        d.shared
            .memory
            .run(move |store| store.get(&memory))
            .await
            .unwrap()
            .owner
            .as_deref(),
        Some("chief")
    );
    d.shutdown().await;
}

#[tokio::test]
async fn team_persona_reopens_warm_runtime_and_skill_grants_are_enforced() {
    let root = tempfile::tempdir().unwrap();
    let runtime = TestRuntime::default();
    let d = daemon(root.path(), &runtime).await;
    let mut request = draft("Mina");
    request.skills = Some(vec!["prices".into()]);
    let mut agent = crate::team::hire(&d.shared, &request, "persona")
        .await
        .unwrap();
    d.shared
        .command(&agent.home_session_id, Action::Recover)
        .await
        .unwrap();
    let first = runtime
        .0
        .lock()
        .opened
        .last()
        .unwrap()
        .team
        .clone()
        .unwrap();
    agent.persona.voice = "Answer with short tables".into();
    agent.persona_version += 1;
    d.shared.agents.put(&agent, "test_persona").unwrap();
    d.shared
        .command(&agent.home_session_id, Action::Recover)
        .await
        .unwrap();
    let second = runtime
        .0
        .lock()
        .opened
        .last()
        .unwrap()
        .team
        .clone()
        .unwrap();
    assert_ne!(first.persona, second.persona);
    assert!(second.persona.contains("Answer with short tables"));
    let state = d.shared.db.state(&agent.home_session_id).await.unwrap();
    let env = ToolEnv::new(d.shared.clone(), &state);
    assert!(
        matches!(crate::skills::SkillTool.call(json!({"name":"ungranted"}), &env).await, ToolExec::Denied { reason, .. } if reason == "skill_not_granted")
    );
    d.shutdown().await;
}

#[tokio::test]
async fn team_settle_event_delivers_report_without_polling() {
    let root = tempfile::tempdir().unwrap();
    let d = daemon(root.path(), &TestRuntime::default()).await;
    let agent = crate::team::hire(&d.shared, &draft("Mina"), "listener")
        .await
        .unwrap();
    let task = crate::team::delegate(&d.shared, "chief", &agent.id, &handoff("listener"))
        .await
        .unwrap();
    wait_event(&d, &agent.home_session_id, |event| matches!(event, EventKind::MessageInjected { message_id } if message_id == &task.message_id)).await;
    d.shared
        .emit(
            &agent.home_session_id,
            None,
            EventKind::MessageCompleted {
                message_id: task.message_id,
                outcome: MessageOutcome::Completed,
                reason: None,
            },
        )
        .await
        .unwrap();
    d.shared
        .emit(&agent.home_session_id, None, EventKind::SessionSettled {})
        .await
        .unwrap();
    let chief = d.shared.agents.get("chief").unwrap().unwrap();
    let expected = format!("team-report-{}", task.id);
    wait_event(&d, &chief.home_session_id, |event| matches!(event, EventKind::MessageQueued { message_id, .. } if message_id.as_str() == expected)).await;
    assert_eq!(
        d.shared
            .agents
            .get_handoff(&task.id)
            .unwrap()
            .unwrap()
            .status,
        HandoffStatus::Done
    );
    d.shutdown().await;
}

#[tokio::test]
async fn team_memory_write_rechecks_retirement_after_waiting_for_owner_gate() {
    let root = tempfile::tempdir().unwrap();
    let d = daemon(root.path(), &TestRuntime::default()).await;
    let mut agent = crate::team::hire(&d.shared, &draft("Mina"), "memory-race")
        .await
        .unwrap();
    let state = d.shared.db.state(&agent.home_session_id).await.unwrap();
    let env = ToolEnv::new(d.shared.clone(), &state);
    let gate = d.shared.agents.gate.lock().await;
    let mut write = Box::pin(
        crate::tools::memory::MemoryWrite.call(json!({"content":"Late private note"}), &env),
    );
    assert!(futures_util::poll!(&mut write).is_pending());
    agent.status = AgentStatus::Retired;
    d.shared.agents.put(&agent, "test_retirement").unwrap();
    drop(gate);
    assert!(matches!(write.await, ToolExec::Denied { reason, .. } if reason == "agent_retired"));
    let owner = agent.id.clone();
    assert!(d
        .shared
        .memory
        .run(move |store| store.owned_ids(&owner))
        .await
        .unwrap()
        .is_empty());
    d.shutdown().await;
}
