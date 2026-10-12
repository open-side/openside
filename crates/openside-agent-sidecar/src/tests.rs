use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use openside_core::{
    AgentState, ApprovalTier, DeferKind, HostToolHandler, MessageId, QueueMode, RunOutcome,
    RuntimeError, RuntimeEvent, SessionId, SessionSpec, TicketId, TicketResolution, ToolCall,
    ToolOutcome, ToolSpec,
};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, Semaphore};

use crate::connection::{EVENT_CAPACITY, MAX_IN_FLIGHT_CALLS, RECENT_RPC_IDS};
use crate::{Connection, MAX_LINE_BYTES};

const NONCE: &str = "0123456789abcdef0123456789abcdef";
const TEST_TIMEOUT: Duration = Duration::from_secs(4);

pub(crate) fn spec() -> SessionSpec {
    SessionSpec {
        session: SessionId::new("session-test"),
        model: format!("function:{}", std::process::id()),
        fallback_models: Vec::new(),
        system_prompt_parts: vec!["Follow the daemon tool gate.".into()],
        tools: vec![ToolSpec {
            name: "echo".into(),
            description: "Return model-safe data.".into(),
            parameters_json: r#"{"type":"object","properties":{"text":{"type":"string"}}}"#.into(),
            approval: ApprovalTier::Read,
            deferrable: true,
        }],
        resume_state: Some(AgentState {
            format: "openside-agent-state/1".into(),
            data: "opaque-state".into(),
        }),
        credentials_json: r#"{"order":{"test":["env"]}}"#.into(),
        agent_json: r#"{"tool_retries":2}"#.into(),
    }
}

pub(crate) struct Echo;
impl HostToolHandler for Echo {
    fn call(&self, call: ToolCall) -> Pin<Box<dyn Future<Output = ToolOutcome> + Send + '_>> {
        Box::pin(async move {
            ToolOutcome::Done {
                for_model_json: call.arguments_json,
            }
        })
    }
}

pub(crate) struct Peer {
    read: BufReader<OwnedReadHalf>,
    write: OwnedWriteHalf,
}

impl Peer {
    pub(crate) fn new(stream: UnixStream) -> Self {
        let (read, write) = stream.into_split();
        Self {
            read: BufReader::new(read),
            write,
        }
    }

    pub(crate) async fn send(&mut self, value: Value) {
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        self.write.write_all(&bytes).await.unwrap();
    }

    async fn raw(&mut self, bytes: &[u8]) {
        self.write.write_all(bytes).await.unwrap();
    }

    pub(crate) async fn recv(&mut self) -> Value {
        let mut line = String::new();
        let count = tokio::time::timeout(TEST_TIMEOUT, self.read.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        assert_ne!(count, 0, "unexpected peer EOF");
        serde_json::from_str(&line).unwrap()
    }

    pub(crate) async fn hello(&mut self, nonce: &str) {
        self.send(json!({"v":1,"type":"hello","protocol":1,"nonce":nonce,"pid":std::process::id(),
            "versions":{"python":"test","pydantic_ai":"test","pydantic_ai_harness":"test","openside_agent":"0.1.0"}})).await;
    }

    pub(crate) async fn ready(&mut self) {
        self.send(
            json!({"v":1,"type":"ready","tools":["echo"],"pending_deferred":[],"startup_ms":7}),
        )
        .await;
    }
}

async fn connected(handler: Arc<dyn HostToolHandler>) -> (Connection, Peer) {
    let (host, peer) = UnixStream::pair().unwrap();
    let opening = Connection::open(host, NONCE, spec(), handler);
    let handshake = async {
        let mut peer = Peer::new(peer);
        peer.hello(NONCE).await;
        assert_eq!(peer.recv().await["type"], "open");
        peer.ready().await;
        peer
    };
    let (connection, peer) =
        tokio::time::timeout(TEST_TIMEOUT, async { tokio::join!(opening, handshake) })
            .await
            .unwrap();
    (connection.unwrap(), peer)
}

async fn event(connection: &mut Connection) -> RuntimeEvent {
    tokio::time::timeout(TEST_TIMEOUT, connection.next_event())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn hello_open_and_ready_preserve_the_authoritative_fields() {
    let (host, peer) = UnixStream::pair().unwrap();
    let handshake = tokio::spawn(async move {
        let mut peer = Peer::new(peer);
        peer.hello(NONCE).await;
        let open = peer.recv().await;
        assert_eq!(open["v"], 1);
        assert_eq!(open["session"], "session-test");
        assert_eq!(open["model"]["id"], spec().model);
        assert_eq!(open["model"]["fallback"], json!([]));
        assert_eq!(
            open["system_prompt_parts"],
            json!(["Follow the daemon tool gate."])
        );
        assert_eq!(open["tools"][0]["parameters"]["type"], "object");
        assert_eq!(open["tools"][0]["approval"], "read");
        assert_eq!(open["tools"][0]["deferrable"], true);
        assert_eq!(open["resume_state"]["data"], "opaque-state");
        assert_eq!(
            open["config"]["credentials"],
            json!({"order":{"test":["env"]}})
        );
        assert_eq!(open["config"]["agent"], json!({"tool_retries":2}));
        peer.send(json!({"v":1,"type":"ready","tools":["echo"],"startup_ms":9,
            "pending_deferred":[{"tool_call_id":"restored","ticket_id":"ticket-restored","kind":"external"}]})).await;
        peer
    });
    let connection = Connection::open(host, NONCE, spec(), Arc::new(Echo))
        .await
        .unwrap();
    assert_eq!(connection.info().tools, ["echo"]);
    assert_eq!(connection.info().startup_ms, 9);
    assert_eq!(
        connection.info().pending_deferred[0].ticket.as_str(),
        "ticket-restored"
    );
    assert_eq!(
        connection.info().pending_deferred[0].kind,
        DeferKind::External
    );
    let _peer = handshake.await.unwrap();
}

#[tokio::test]
async fn nonce_hello_protocol_and_agent_version_mismatch_are_unavailable() {
    for (nonce, protocol, agent) in [
        ("wrong-nonce", 1, "0.1.0"),
        (NONCE, 2, "0.1.0"),
        (NONCE, 1, "0.2.0"),
        (NONCE, 1, "0.0.9"),
        (NONCE, 1, "test"),
    ] {
        let (host, peer) = UnixStream::pair().unwrap();
        let mut peer = Peer::new(peer);
        peer.send(json!({"v":1,"type":"hello","protocol":protocol,"nonce":nonce,"pid":1,
            "versions":{"python":"test","pydantic_ai":"test","pydantic_ai_harness":"test","openside_agent":agent}})).await;
        let result = Connection::open(host, NONCE, spec(), Arc::new(Echo)).await;
        let Err(RuntimeError::Unavailable(message)) = result else {
            panic!("{nonce} {protocol} {agent} must be refused");
        };
        if agent != "0.1.0" {
            assert!(message.contains(">=0.1.0,<0.2.0"), "{message}");
        }
    }
}

#[tokio::test]
async fn ready_gate_mismatch_sends_a_fatal_tool_gate_error() {
    let (host, peer) = UnixStream::pair().unwrap();
    let handshake = tokio::spawn(async move {
        let mut peer = Peer::new(peer);
        peer.hello(NONCE).await;
        peer.recv().await;
        peer.send(
            json!({"v":1,"type":"ready","tools":["extra"],"pending_deferred":[],"startup_ms":0}),
        )
        .await;
        let error = peer.recv().await;
        assert_eq!(error["code"], "tool_gate");
        assert_eq!(error["fatal"], true);
    });
    let result = Connection::open(host, NONCE, spec(), Arc::new(Echo)).await;
    assert!(
        matches!(result, Err(RuntimeError::Rejected(message)) if message.starts_with("tool_gate:"))
    );
    handshake.await.unwrap();
}

#[tokio::test]
async fn malformed_input_during_both_handshakes_is_nonfatal() {
    let (host, peer) = UnixStream::pair().unwrap();
    let handshake = tokio::spawn(async move {
        let mut peer = Peer::new(peer);
        peer.raw(b"garbage\n").await;
        assert_eq!(peer.recv().await["code"], "protocol");
        peer.hello(NONCE).await;
        peer.recv().await;
        peer.raw(
            b"{\"v\":2,\"type\":\"ready\",\"tools\":[],\"pending_deferred\":[],\"startup_ms\":0}\n",
        )
        .await;
        assert_eq!(peer.recv().await["code"], "protocol");
        peer.ready().await;
        peer
    });
    let connection = Connection::open(host, NONCE, spec(), Arc::new(Echo))
        .await
        .unwrap();
    assert_eq!(connection.info().tools, ["echo"]);
    let _peer = handshake.await.unwrap();
}

#[tokio::test]
async fn every_observation_maps_to_the_runtime_boundary() {
    let (mut connection, mut peer) = connected(Arc::new(Echo)).await;
    let observations = [
        (json!({"kind":"turn.started"}), RuntimeEvent::TurnStarted),
        (json!({"kind":"turn.ended"}), RuntimeEvent::TurnEnded),
        (
            json!({"kind":"assistant.delta","text":"a"}),
            RuntimeEvent::AssistantDelta { text: "a".into() },
        ),
        (
            json!({"kind":"assistant.final","text":"b"}),
            RuntimeEvent::AssistantFinal { text: "b".into() },
        ),
        (
            json!({"kind":"thinking.delta","text":"c"}),
            RuntimeEvent::ThinkingDelta { text: "c".into() },
        ),
        (
            json!({"kind":"subagent.lifecycle","subagent_id":"child","status":"running"}),
            RuntimeEvent::SubagentLifecycle {
                subagent_id: "child".into(),
                status: "running".into(),
            },
        ),
        (
            json!({"kind":"subagent.progress","subagent_id":"child","progress":{"step":2}}),
            RuntimeEvent::SubagentProgress {
                subagent_id: "child".into(),
                progress_json: r#"{"step":2}"#.into(),
            },
        ),
        (
            json!({"kind":"usage","model":spec().model,"input_tokens":13,"output_tokens":4}),
            RuntimeEvent::Usage {
                model: spec().model,
                input_tokens: 13,
                output_tokens: 4,
            },
        ),
    ];
    for (observation, expected) in observations {
        peer.send(json!({"v":1,"type":"event","event":observation}))
            .await;
        assert_eq!(event(&mut connection).await, expected);
    }
    peer.send(json!({"v":1,"type":"message_accepted","message_id":"message"}))
        .await;
    assert_eq!(
        event(&mut connection).await,
        RuntimeEvent::MessageAccepted {
            id: MessageId::new("message")
        }
    );
    for (status, outcome) in [
        ("completed", RunOutcome::Completed),
        ("aborted", RunOutcome::Aborted),
        ("failed", RunOutcome::Failed),
        ("deferred", RunOutcome::Deferred),
    ] {
        peer.send(json!({"v":1,"type":"message_result","message_id":"message","outcome":status,"reason":"detail"})).await;
        assert_eq!(
            event(&mut connection).await,
            RuntimeEvent::MessageResult {
                id: MessageId::new("message"),
                outcome,
                reason: Some("detail".into())
            }
        );
    }
    peer.send(json!({"v":1,"type":"deferred","calls":[{"tool_call_id":"call","ticket_id":"ticket","kind":"approval"}]})).await;
    let RuntimeEvent::Deferred { calls } = event(&mut connection).await else {
        panic!("expected deferred");
    };
    assert_eq!(calls[0].tool_call_id, "call");
    assert_eq!(calls[0].ticket.as_str(), "ticket");
    assert_eq!(calls[0].kind, DeferKind::Approval);
    peer.send(json!({"v":1,"type":"state_snapshot","state":{"format":"openside-agent-state/1","data":"opaque"}})).await;
    assert_eq!(
        event(&mut connection).await,
        RuntimeEvent::StateSnapshot {
            state: AgentState {
                format: "openside-agent-state/1".into(),
                data: "opaque".into()
            }
        }
    );
    peer.send(json!({"v":1,"type":"settled"})).await;
    assert_eq!(event(&mut connection).await, RuntimeEvent::Settled);
}

#[tokio::test]
async fn messages_abort_and_all_ticket_resolutions_serialize_exactly() {
    let (mut connection, mut peer) = connected(Arc::new(Echo)).await;
    for (mode, wire) in [(QueueMode::Queue, "queue"), (QueueMode::Steer, "steer")] {
        connection
            .send(MessageId::new("message"), "text".into(), mode)
            .await
            .unwrap();
        assert_eq!(
            peer.recv().await,
            json!({"v":1,"type":"user_message","message_id":"message","text":"text","mode":wire})
        );
    }
    connection.abort().await.unwrap();
    assert_eq!(peer.recv().await, json!({"v":1,"type":"abort"}));
    for (resolution, wire) in [
        (TicketResolution::Approved, json!({"status":"approved"})),
        (
            TicketResolution::Denied {
                reason: "expired".into(),
            },
            json!({"status":"denied","reason":"expired"}),
        ),
        (
            TicketResolution::Result {
                for_model_json: r#"{"status":"filled"}"#.into(),
            },
            json!({"status":"result","for_model":{"status":"filled"}}),
        ),
    ] {
        connection
            .resolve_ticket(TicketId::new("ticket"), "original-call".into(), resolution)
            .await
            .unwrap();
        assert_eq!(
            peer.recv().await,
            json!({"v":1,"type":"ticket_resolved","ticket_id":"ticket","tool_call_id":"original-call","resolution":wire})
        );
    }
    connection
        .control_subagent("child".into(), openside_core::SubagentControl::Cancel)
        .await
        .unwrap();
    assert_eq!(
        peer.recv().await,
        json!({"v":1,"type":"subagent_cancel","subagent_id":"child"})
    );
    connection
        .control_subagent(
            "child".into(),
            openside_core::SubagentControl::Steer {
                text: "focus".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        peer.recv().await,
        json!({"v":1,"type":"subagent_steer","subagent_id":"child","text":"focus"})
    );
    connection.discard_deferred().await.unwrap();
    assert_eq!(peer.recv().await, json!({"v":1,"type":"discard_deferred"}));
    let goodbye = tokio::spawn(async move {
        assert_eq!(peer.recv().await, json!({"v":1,"type":"shutdown"}));
        peer.send(json!({"v":1,"type":"bye"})).await;
    });
    tokio::time::timeout(TEST_TIMEOUT, connection.shutdown())
        .await
        .unwrap()
        .unwrap();
    goodbye.await.unwrap();
    assert!(connection.next_event().await.is_none());
}

struct BlockingTools {
    started: mpsc::UnboundedSender<ToolCall>,
    release: Arc<Semaphore>,
}
impl HostToolHandler for BlockingTools {
    fn call(&self, call: ToolCall) -> Pin<Box<dyn Future<Output = ToolOutcome> + Send + '_>> {
        Box::pin(async move {
            self.started.send(call.clone()).unwrap();
            self.release.acquire().await.unwrap().forget();
            ToolOutcome::Done {
                for_model_json: json!({"call":call.tool_call_id}).to_string(),
            }
        })
    }
}

#[tokio::test]
async fn concurrent_tools_do_not_block_events_and_correlate_by_rpc_id() {
    let (started, mut calls) = mpsc::unbounded_channel();
    let release = Arc::new(Semaphore::new(0));
    let handler = Arc::new(BlockingTools {
        started,
        release: release.clone(),
    });
    let (mut connection, mut peer) = connected(handler).await;
    for id in ["a", "b"] {
        peer.send(
            json!({"v":1,"type":"tool_call","rpc_id":format!("rpc-{id}"),"tool_call_id":id,
            "name":"echo","args":{"id":id},"subagent":"child","approval_ticket":"approved-ticket"}),
        )
        .await;
    }
    for _ in 0..2 {
        let call = tokio::time::timeout(TEST_TIMEOUT, calls.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(call.subagent.as_deref(), Some("child"));
        assert_eq!(call.approval_ticket, Some(TicketId::new("approved-ticket")));
    }
    peer.send(
        json!({"v":1,"type":"event","event":{"kind":"assistant.delta","text":"while-tools-block"}}),
    )
    .await;
    assert_eq!(
        event(&mut connection).await,
        RuntimeEvent::AssistantDelta {
            text: "while-tools-block".into()
        }
    );
    release.add_permits(2);
    let mut ids = Vec::new();
    for _ in 0..2 {
        let result = peer.recv().await;
        let call = result["outcome"]["for_model"]["call"].as_str().unwrap();
        assert_eq!(result["rpc_id"], format!("rpc-{call}"));
        assert_eq!(result["outcome"]["status"], "done");
        ids.push(call.to_owned());
    }
    ids.sort();
    assert_eq!(ids, ["a", "b"]);
}

struct Outcomes;
impl HostToolHandler for Outcomes {
    fn call(&self, call: ToolCall) -> Pin<Box<dyn Future<Output = ToolOutcome> + Send + '_>> {
        Box::pin(async move {
            match call.tool_call_id.as_str() {
                "approval" => ToolOutcome::Deferred {
                    ticket: TicketId::new("ticket"),
                    kind: DeferKind::Approval,
                },
                "external" => ToolOutcome::Deferred {
                    ticket: TicketId::new("ticket"),
                    kind: DeferKind::External,
                },
                "denied" => ToolOutcome::Denied {
                    reason: "policy".into(),
                    hint: Some("read instead".into()),
                },
                "failed" => ToolOutcome::Failed {
                    message: "sanitized failure".into(),
                },
                "invalid-json" => ToolOutcome::Done {
                    for_model_json: "not-json-sensitive-canary".into(),
                },
                _ => ToolOutcome::Done {
                    for_model_json: "null".into(),
                },
            }
        })
    }
}

#[tokio::test]
async fn every_host_outcome_has_the_specified_wire_shape() {
    let (_connection, mut peer) = connected(Arc::new(Outcomes)).await;
    for (id, expected) in [
        (
            "approval",
            json!({"status":"deferred","ticket_id":"ticket","kind":"approval"}),
        ),
        (
            "external",
            json!({"status":"deferred","ticket_id":"ticket","kind":"external"}),
        ),
        (
            "denied",
            json!({"status":"denied","reason":"policy","hint":"read instead"}),
        ),
        (
            "failed",
            json!({"status":"error","message":"sanitized failure"}),
        ),
        (
            "invalid-json",
            json!({"status":"error","message":"host tool returned invalid JSON"}),
        ),
        ("null", json!({"status":"done","for_model":null})),
    ] {
        peer.send(
            json!({"v":1,"type":"tool_call","rpc_id":id,"tool_call_id":id,"name":"echo","args":{}}),
        )
        .await;
        assert_eq!(
            peer.recv().await,
            json!({"v":1,"type":"tool_result","rpc_id":id,"outcome":expected})
        );
    }
}

#[tokio::test]
async fn duplicate_rpc_ids_get_no_second_tool_result() {
    let (_connection, mut peer) = connected(Arc::new(Echo)).await;
    let call = json!({"v":1,"type":"tool_call","rpc_id":"once","tool_call_id":"call","name":"echo","args":{}});
    peer.send(call.clone()).await;
    assert_eq!(peer.recv().await["type"], "tool_result");
    peer.send(call).await;
    assert_eq!(peer.recv().await["code"], "protocol");
    peer.send(json!({"v":1,"type":"tool_call","rpc_id":"next","tool_call_id":"call-next","name":"echo","args":{}})).await;
    assert_eq!(peer.recv().await["rpc_id"], "next");
}

#[tokio::test]
async fn tool_calls_beyond_the_in_flight_cap_are_rejected_without_spawning() {
    let (started, mut calls) = mpsc::unbounded_channel();
    let release = Arc::new(Semaphore::new(0));
    let handler = Arc::new(BlockingTools {
        started,
        release: release.clone(),
    });
    let (_connection, mut peer) = connected(handler).await;
    let rejected = 10;
    for index in 0..MAX_IN_FLIGHT_CALLS + rejected {
        peer.send(
            json!({"v":1,"type":"tool_call","rpc_id":format!("rpc-{index}"),
            "tool_call_id":format!("call-{index}"),"name":"echo","args":{}}),
        )
        .await;
    }
    for index in 0..rejected {
        let result = peer.recv().await;
        assert_eq!(result["type"], "tool_result");
        assert_eq!(
            result["rpc_id"],
            format!("rpc-{}", MAX_IN_FLIGHT_CALLS + index)
        );
        assert_eq!(
            result["outcome"],
            json!({"status":"error","message":"too many concurrent tool calls"})
        );
    }
    let mut started_ids = Vec::new();
    for _ in 0..MAX_IN_FLIGHT_CALLS {
        let call = tokio::time::timeout(TEST_TIMEOUT, calls.recv())
            .await
            .unwrap()
            .unwrap();
        started_ids.push(call.tool_call_id);
    }
    started_ids.sort();
    let mut expected: Vec<_> = (0..MAX_IN_FLIGHT_CALLS)
        .map(|index| format!("call-{index}"))
        .collect();
    expected.sort();
    assert_eq!(started_ids, expected);
    assert!(
        calls.try_recv().is_err(),
        "a rejected call must not reach the host"
    );

    release.add_permits(MAX_IN_FLIGHT_CALLS);
    for _ in 0..MAX_IN_FLIGHT_CALLS {
        assert_eq!(peer.recv().await["outcome"]["status"], "done");
    }
    release.add_permits(1);
    peer.send(json!({"v":1,"type":"tool_call","rpc_id":"after","tool_call_id":"call-after","name":"echo","args":{}}))
        .await;
    let result = peer.recv().await;
    assert_eq!(result["rpc_id"], "after");
    assert_eq!(result["outcome"]["status"], "done");
}

#[tokio::test]
async fn in_flight_duplicate_rpc_id_is_rejected_while_the_call_runs() {
    let (started, mut calls) = mpsc::unbounded_channel();
    let release = Arc::new(Semaphore::new(0));
    let handler = Arc::new(BlockingTools {
        started,
        release: release.clone(),
    });
    let (_connection, mut peer) = connected(handler).await;
    let call = json!({"v":1,"type":"tool_call","rpc_id":"busy","tool_call_id":"call","name":"echo","args":{}});
    peer.send(call.clone()).await;
    tokio::time::timeout(TEST_TIMEOUT, calls.recv())
        .await
        .unwrap()
        .unwrap();
    peer.send(call).await;
    assert_eq!(peer.recv().await["code"], "protocol");
    assert!(calls.try_recv().is_err());
    release.add_permits(1);
    assert_eq!(peer.recv().await["rpc_id"], "busy");
}

#[tokio::test]
async fn completed_rpc_ids_are_remembered_only_for_a_bounded_window() {
    let (_connection, mut peer) = connected(Arc::new(Echo)).await;
    for index in 0..RECENT_RPC_IDS + 10 {
        peer.send(
            json!({"v":1,"type":"tool_call","rpc_id":format!("rpc-{index}"),
            "tool_call_id":"call","name":"echo","args":{}}),
        )
        .await;
        assert_eq!(peer.recv().await["rpc_id"], format!("rpc-{index}"));
    }
    let latest = format!("rpc-{}", RECENT_RPC_IDS + 9);
    peer.send(json!({"v":1,"type":"tool_call","rpc_id":latest,"tool_call_id":"call","name":"echo","args":{}}))
        .await;
    assert_eq!(peer.recv().await["code"], "protocol");
    peer.send(json!({"v":1,"type":"tool_call","rpc_id":"rpc-0","tool_call_id":"call","name":"echo","args":{}}))
        .await;
    assert_eq!(peer.recv().await["rpc_id"], "rpc-0");
}

#[tokio::test]
async fn event_flood_stalls_the_sidecar_instead_of_growing_the_queue() {
    let (host, peer) = tokio::io::duplex(512);
    let opening = tokio::spawn(Connection::open(host, NONCE, spec(), Arc::new(Echo)));
    let (read, mut write) = tokio::io::split(peer);
    let mut read = BufReader::new(read);
    write
        .write_all(
            format!(
                "{}\n",
                json!({"v":1,"type":"hello","protocol":1,"nonce":NONCE,"pid":1,
                "versions":{"python":"test","pydantic_ai":"test","pydantic_ai_harness":"test","openside_agent":"0.1.0"}})
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut line = String::new();
    read.read_line(&mut line).await.unwrap();
    assert!(line.contains(r#""type":"open""#));
    write
        .write_all(
            format!(
                "{}\n",
                json!({"v":1,"type":"ready","tools":["echo"],"pending_deferred":[],"startup_ms":0})
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut connection = tokio::time::timeout(TEST_TIMEOUT, opening)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    let total = EVENT_CAPACITY * 8;
    let written = Arc::new(AtomicUsize::new(0));
    let progress = written.clone();
    let flood = tokio::spawn(async move {
        for index in 0..total {
            let line = format!(
                "{}\n",
                json!({"v":1,"type":"event","event":{"kind":"assistant.delta","text":index.to_string()}})
            );
            write.write_all(line.as_bytes()).await.unwrap();
            progress.fetch_add(1, Ordering::SeqCst);
        }
        write
    });
    // The consumer is idle: wait until the peer stops making progress.
    let mut previous = usize::MAX;
    let mut stable = 0;
    while stable < 5 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let now = written.load(Ordering::SeqCst);
        stable = if now == previous { stable + 1 } else { 0 };
        previous = now;
    }
    // Channel capacity plus what the 512-byte pipe and the reader's buffer can hold.
    assert!(
        previous <= EVENT_CAPACITY + 64,
        "reader kept accepting events with no consumer: {previous}"
    );
    assert!(previous < total);

    for index in 0..total {
        assert_eq!(
            event(&mut connection).await,
            RuntimeEvent::AssistantDelta {
                text: index.to_string()
            }
        );
    }
    drop(flood.await.unwrap());
}

#[tokio::test]
async fn shutdown_reaches_bye_behind_an_event_backlog_and_keeps_the_final_event() {
    let (mut connection, mut peer) = connected(Arc::new(Echo)).await;
    let total = EVENT_CAPACITY * 3;
    // The reader stalls on the full channel, so the peer's own writes can block until
    // shutdown starts draining; send from a separate task.
    let flood = tokio::spawn(async move {
        for index in 0..total {
            peer.send(json!({"v":1,"type":"event","event":{"kind":"assistant.delta","text":index.to_string()}}))
                .await;
        }
        peer.send(json!({"v":1,"type":"bye"})).await;
        peer
    });
    tokio::time::timeout(TEST_TIMEOUT, connection.shutdown())
        .await
        .unwrap()
        .unwrap();
    let mut last = None;
    while let Some(event) = tokio::time::timeout(TEST_TIMEOUT, connection.next_event())
        .await
        .unwrap()
    {
        last = Some(event);
    }
    assert_eq!(
        last,
        Some(RuntimeEvent::AssistantDelta {
            text: (total - 1).to_string()
        })
    );
    drop(flood.await.unwrap());
}

struct LargeResult;
impl HostToolHandler for LargeResult {
    fn call(&self, _call: ToolCall) -> Pin<Box<dyn Future<Output = ToolOutcome> + Send + '_>> {
        Box::pin(async {
            ToolOutcome::Done {
                for_model_json: json!({"blob":"x".repeat(16 * 1024)}).to_string(),
            }
        })
    }
}

#[tokio::test]
async fn shutdown_does_not_deadlock_behind_a_blocked_tool_result_write() {
    let (host, peer) = tokio::io::duplex(512);
    let opening = tokio::spawn(Connection::open(host, NONCE, spec(), Arc::new(LargeResult)));
    let (read, mut write) = tokio::io::split(peer);
    let mut read = BufReader::new(read);
    let hello = json!({"v":1,"type":"hello","protocol":1,"nonce":NONCE,"pid":1,
        "versions":{"python":"test","pydantic_ai":"test","pydantic_ai_harness":"test","openside_agent":"0.1.0"}});
    write
        .write_all(format!("{hello}\n").as_bytes())
        .await
        .unwrap();
    let mut line = String::new();
    read.read_line(&mut line).await.unwrap();
    assert!(line.contains(r#""type":"open""#));
    let ready = json!({"v":1,"type":"ready","tools":["echo"],"pending_deferred":[],"startup_ms":0});
    write
        .write_all(format!("{ready}\n").as_bytes())
        .await
        .unwrap();
    let mut connection = tokio::time::timeout(TEST_TIMEOUT, opening)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    // The peer is single-threaded like a blocked sidecar: it writes everything before it reads,
    // so the tool result stays stuck in the writer while the event queue backs up.
    let peer = tokio::spawn(async move {
        let call = json!({"v":1,"type":"tool_call","rpc_id":"big","tool_call_id":"call","name":"echo","args":{}});
        write
            .write_all(format!("{call}\n").as_bytes())
            .await
            .unwrap();
        for index in 0..EVENT_CAPACITY * 3 {
            let event = json!({"v":1,"type":"event","event":{"kind":"assistant.delta","text":index.to_string()}});
            write
                .write_all(format!("{event}\n").as_bytes())
                .await
                .unwrap();
        }
        loop {
            line.clear();
            assert_ne!(read.read_line(&mut line).await.unwrap(), 0);
            if line.contains(r#""type":"shutdown""#) {
                break;
            }
        }
        let last = json!({"v":1,"type":"event","event":{"kind":"assistant.delta","text":"final"}});
        write
            .write_all(format!("{last}\n{}\n", json!({"v":1,"type":"bye"})).as_bytes())
            .await
            .unwrap();
        (read, write)
    });
    // Let the reader stall on the full queue and the result write block on the writer.
    tokio::time::sleep(Duration::from_millis(200)).await;
    tokio::time::timeout(TEST_TIMEOUT, connection.shutdown())
        .await
        .expect("shutdown must not wait on a reader stalled by its own event queue")
        .unwrap();
    let mut last = None;
    while let Some(event) = tokio::time::timeout(TEST_TIMEOUT, connection.next_event())
        .await
        .unwrap()
    {
        last = Some(event);
    }
    assert_eq!(
        last,
        Some(RuntimeEvent::AssistantDelta {
            text: "final".into()
        })
    );
    drop(peer.await.unwrap());
}

struct CountCalls(AtomicUsize);
impl HostToolHandler for CountCalls {
    fn call(&self, _call: ToolCall) -> Pin<Box<dyn Future<Output = ToolOutcome> + Send + '_>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            ToolOutcome::Done {
                for_model_json: "null".into(),
            }
        })
    }
}

#[tokio::test]
async fn a_nonexposed_tool_never_reaches_the_host() {
    let calls = Arc::new(CountCalls(AtomicUsize::new(0)));
    let (_connection, mut peer) = connected(calls.clone()).await;
    peer.send(json!({"v":1,"type":"tool_call","rpc_id":"hidden","tool_call_id":"call","name":"not-exposed","args":{}})).await;
    assert_eq!(peer.recv().await["outcome"]["status"], "denied");
    assert_eq!(calls.0.load(Ordering::SeqCst), 0);
}

struct OrderLog(parking_lot::Mutex<Vec<String>>);
impl HostToolHandler for OrderLog {
    fn call(&self, call: ToolCall) -> Pin<Box<dyn Future<Output = ToolOutcome> + Send + '_>> {
        self.0.lock().push(format!("call:{}", call.tool_call_id));
        Box::pin(async {
            ToolOutcome::Done {
                for_model_json: "null".into(),
            }
        })
    }

    fn context_compacted(&self, subagent: Option<String>) {
        self.0
            .lock()
            .push(format!("compacted:{}", subagent.unwrap_or_default()));
    }
}

#[tokio::test]
async fn context_compaction_reaches_the_host_before_any_later_tool_call() {
    let log = Arc::new(OrderLog(parking_lot::Mutex::new(Vec::new())));
    let (_connection, mut peer) = connected(log.clone()).await;
    for (index, message) in [
        json!({"v":1,"type":"context_compacted"}),
        json!({"v":1,"type":"tool_call","rpc_id":"r1","tool_call_id":"after-parent","name":"echo","args":{}}),
        json!({"v":1,"type":"context_compacted","subagent":"sub-1"}),
        json!({"v":1,"type":"tool_call","rpc_id":"r2","tool_call_id":"after-child","name":"echo","args":{}}),
    ]
    .into_iter()
    .enumerate()
    {
        peer.send(message).await;
        if index % 2 == 1 {
            assert_eq!(peer.recv().await["outcome"]["status"], "done");
        }
    }
    assert_eq!(
        *log.0.lock(),
        [
            "compacted:",
            "call:after-parent",
            "compacted:sub-1",
            "call:after-child"
        ]
    );
}

#[tokio::test]
async fn malformed_lines_are_discarded_without_closing_the_connection() {
    let (mut connection, mut peer) = connected(Arc::new(Echo)).await;
    for bytes in [
        b"not JSON\n".as_slice(),
        b"[]\n",
        b"null\n",
        b"\xff\xfe\n",
        b"\n",
        b"{\"v\":1,\"type\":\"unknown\"}\n",
        b"{\"v\":2,\"type\":\"settled\"}\n",
        b"{\"type\":\"settled\"}\n",
        b"{\"v\":1,\"type\":\"tool_call\",\"rpc_id\":\"missing-fields\"}\n",
        b"{\"v\":1,\"type\":\"event\",\"event\":{\"kind\":\"tool.started\"}}\n",
        b"{\"v\":1,\"type\":\"event\",\"event\":{\"kind\":\"assistant.delta\",\"text\":4}}\n",
        b"{\"v\":1,\"type\":\"error\",\"code\":\"internal\",\"message\":\"sensitive-canary\"}\n",
    ] {
        peer.raw(bytes).await;
        let error = peer.recv().await;
        assert_eq!(error["v"], 1);
        assert_eq!(error["code"], "protocol");
        assert_eq!(error["fatal"], false);
        assert!(!error.to_string().contains("sensitive-canary"));
    }
    peer.send(json!({"v":1,"type":"settled"})).await;
    assert_eq!(event(&mut connection).await, RuntimeEvent::Settled);
}

#[tokio::test]
async fn oversized_line_discards_the_entire_frame_and_preserves_the_next_line() {
    let (mut connection, mut peer) = connected(Arc::new(Echo)).await;
    let mut oversized = vec![b'x'; MAX_LINE_BYTES + 8192];
    oversized.push(b'\n');
    peer.raw(&oversized).await;
    let error = peer.recv().await;
    assert_eq!(error["code"], "protocol");
    assert_eq!(error["fatal"], false);
    peer.send(json!({"v":1,"type":"settled"})).await;
    assert_eq!(event(&mut connection).await, RuntimeEvent::Settled);
}

#[tokio::test]
async fn exact_line_limit_is_accepted() {
    let (mut connection, mut peer) = connected(Arc::new(Echo)).await;
    let overhead = serde_json::to_vec(
        &json!({"v":1,"type":"event","event":{"kind":"assistant.final","text":""}}),
    )
    .unwrap()
    .len();
    let text = "x".repeat(MAX_LINE_BYTES - overhead);
    let bytes = serde_json::to_vec(
        &json!({"v":1,"type":"event","event":{"kind":"assistant.final","text":text}}),
    )
    .unwrap();
    assert_eq!(bytes.len(), MAX_LINE_BYTES);
    peer.raw(&bytes).await;
    peer.raw(b"\n").await;
    assert!(
        matches!(event(&mut connection).await, RuntimeEvent::AssistantFinal { text } if text.len() == MAX_LINE_BYTES - overhead)
    );
}

#[tokio::test]
async fn authentication_failure_during_open_is_explicit_and_fatal() {
    let (host, peer) = UnixStream::pair().unwrap();
    let handshake = tokio::spawn(async move {
        let mut peer = Peer::new(peer);
        peer.hello(NONCE).await;
        peer.recv().await;
        peer.send(json!({"v":1,"type":"error","code":"auth_unavailable","message":"configure a credential source","fatal":true})).await;
        peer
    });
    let result = Connection::open(host, NONCE, spec(), Arc::new(Echo)).await;
    assert!(
        matches!(result, Err(RuntimeError::AuthUnavailable(message)) if message == "configure a credential source")
    );
    let _peer = handshake.await.unwrap();
}

#[tokio::test]
async fn late_authentication_failure_emits_an_error_then_eof_even_if_flag_is_false() {
    let (mut connection, mut peer) = connected(Arc::new(Echo)).await;
    peer.send(json!({"v":1,"type":"error","code":"auth_unavailable","message":"all sources unavailable","fatal":false})).await;
    assert_eq!(
        event(&mut connection).await,
        RuntimeEvent::Error {
            error: RuntimeError::AuthUnavailable("all sources unavailable".into())
        }
    );
    assert!(tokio::time::timeout(TEST_TIMEOUT, connection.next_event())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn nonfatal_errors_keep_reading_and_other_fatal_errors_close() {
    let (mut connection, mut peer) = connected(Arc::new(Echo)).await;
    peer.send(json!({"v":1,"type":"error","code":"stale_ticket","message":"already applied","fatal":false})).await;
    assert_eq!(
        event(&mut connection).await,
        RuntimeEvent::Error {
            error: RuntimeError::Rejected("already applied".into())
        }
    );
    peer.send(json!({"v":1,"type":"settled"})).await;
    assert_eq!(event(&mut connection).await, RuntimeEvent::Settled);
    peer.send(
        json!({"v":1,"type":"error","code":"internal","message":"sanitized failure","fatal":true}),
    )
    .await;
    assert_eq!(
        event(&mut connection).await,
        RuntimeEvent::Error {
            error: RuntimeError::Rejected("sanitized failure".into())
        }
    );
    assert!(tokio::time::timeout(TEST_TIMEOUT, connection.next_event())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn eof_is_not_a_synthetic_runtime_error() {
    let (mut connection, peer) = connected(Arc::new(Echo)).await;
    drop(peer);
    assert!(tokio::time::timeout(TEST_TIMEOUT, connection.next_event())
        .await
        .unwrap()
        .is_none());
}

struct DropFlag(Arc<AtomicBool>);
impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
struct CancellableTool {
    started: Semaphore,
    dropped: Arc<AtomicBool>,
}
impl HostToolHandler for CancellableTool {
    fn call(&self, _call: ToolCall) -> Pin<Box<dyn Future<Output = ToolOutcome> + Send + '_>> {
        Box::pin(async move {
            let _guard = DropFlag(self.dropped.clone());
            self.started.add_permits(1);
            std::future::pending().await
        })
    }
}

#[tokio::test]
async fn dropping_a_connection_cancels_outstanding_host_calls() {
    let dropped = Arc::new(AtomicBool::new(false));
    let handler = Arc::new(CancellableTool {
        started: Semaphore::new(0),
        dropped: dropped.clone(),
    });
    let (connection, mut peer) = connected(handler.clone()).await;
    peer.send(json!({"v":1,"type":"tool_call","rpc_id":"blocked","tool_call_id":"call","name":"echo","args":{}})).await;
    tokio::time::timeout(TEST_TIMEOUT, handler.started.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    drop(connection);
    tokio::time::timeout(TEST_TIMEOUT, async {
        while !dropped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn invalid_outgoing_json_is_rejected_without_disclosing_input() {
    for (credentials, parameters) in [("[]", "{}"), ("{}", "[]"), ("sensitive-canary", "{}")] {
        let (host, _peer) = UnixStream::pair().unwrap();
        let mut request = spec();
        request.credentials_json = credentials.into();
        request.tools[0].parameters_json = parameters.into();
        let error = Connection::open(host, NONCE, request, Arc::new(Echo))
            .await
            .err()
            .unwrap();
        assert!(matches!(error, RuntimeError::Rejected(_)));
        assert!(!error.to_string().contains("sensitive-canary"));
    }
    let (mut connection, mut peer) = connected(Arc::new(Echo)).await;
    let error = connection
        .resolve_ticket(
            TicketId::new("ticket"),
            "call".into(),
            TicketResolution::Result {
                for_model_json: "sensitive-canary".into(),
            },
        )
        .await
        .unwrap_err();
    assert!(!error.to_string().contains("sensitive-canary"));
    connection.abort().await.unwrap();
    assert_eq!(peer.recv().await["type"], "abort");
}

#[tokio::test]
async fn oversized_outbound_message_does_not_corrupt_the_stream() {
    let (mut connection, mut peer) = connected(Arc::new(Echo)).await;
    let result = connection
        .send(
            MessageId::new("huge"),
            "x".repeat(MAX_LINE_BYTES),
            QueueMode::Queue,
        )
        .await;
    assert!(matches!(result, Err(RuntimeError::Rejected(_))));
    connection.abort().await.unwrap();
    assert_eq!(peer.recv().await, json!({"v":1,"type":"abort"}));
}

#[tokio::test]
async fn an_unterminated_frame_at_eof_is_not_a_message() {
    let (mut connection, mut peer) = connected(Arc::new(Echo)).await;
    peer.raw(b"{\"v\":1,\"type\":\"settled\"}").await;
    drop(peer);
    assert!(tokio::time::timeout(TEST_TIMEOUT, connection.next_event())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn no_resume_state_omits_the_optional_wire_field() {
    let (host, peer) = UnixStream::pair().unwrap();
    let mut request = spec();
    request.resume_state = None;
    let handshake = tokio::spawn(async move {
        let mut peer = Peer::new(peer);
        peer.hello(NONCE).await;
        assert!(peer.recv().await.get("resume_state").is_none());
        peer.ready().await;
        peer
    });
    let _connection = Connection::open(host, NONCE, request, Arc::new(Echo))
        .await
        .unwrap();
    let _peer = handshake.await.unwrap();
}

#[tokio::test]
async fn text_ingress_cap_is_utf8_bounded_and_rejection_preserves_connection() {
    use crate::MAX_USER_TEXT_BYTES;
    use openside_core::SubagentControl;
    let (mut connection, mut peer) = connected(Arc::new(Echo)).await;
    let under = format!("{}x", "é".repeat(MAX_USER_TEXT_BYTES / 2 - 1));
    let over = format!("{under}xx");
    assert_eq!(under.len(), MAX_USER_TEXT_BYTES - 1);
    for result in [
        connection
            .send(MessageId::new("over"), over.clone(), QueueMode::Steer)
            .await,
        connection
            .control_subagent("child".into(), SubagentControl::Steer { text: over })
            .await,
    ] {
        assert!(matches!(result, Err(RuntimeError::Rejected(_))));
    }
    let (sent, message) = tokio::join!(
        connection.send(MessageId::new("under"), under.clone(), QueueMode::Steer),
        peer.recv(),
    );
    sent.unwrap();
    assert_eq!(message["type"], "user_message");
    assert_eq!(message["text"], under);
    let (sent, message) = tokio::join!(
        connection.control_subagent(
            "child".into(),
            SubagentControl::Steer {
                text: under.clone()
            }
        ),
        peer.recv(),
    );
    sent.unwrap();
    assert_eq!(message["type"], "subagent_steer");
    assert_eq!(message["text"], under);
}

#[tokio::test]
async fn largest_allowed_text_and_attachments_fit_the_same_wire_line() {
    use crate::MAX_USER_TEXT_BYTES;
    use base64::Engine as _;
    use openside_core::{ContentKind, ContentPart};
    let (mut connection, mut peer) = connected(Arc::new(Echo)).await;
    let text = "x".repeat(MAX_USER_TEXT_BYTES);
    let data = vec![b'a'; 2 * 1024 * 1024];
    let expected = base64::engine::general_purpose::STANDARD.encode(&data);
    let parts = vec![ContentPart {
        kind: ContentKind::Text,
        mime: "text/plain".into(),
        data: Some(data),
        artifact_ref: Some("attachments/maximum/1-note.txt".into()),
        name: Some("note.txt".into()),
    }];
    let (sent, message) = tokio::join!(
        connection.send_with_parts(
            MessageId::new("maximum"),
            text.clone(),
            parts,
            QueueMode::Queue
        ),
        peer.recv(),
    );
    sent.unwrap();
    assert!(serde_json::to_vec(&message).unwrap().len() <= MAX_LINE_BYTES);
    assert_eq!(message["text"], text);
    assert_eq!(message["parts"][0]["data_base64"], expected);
}
