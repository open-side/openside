use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use openside_core::{
    AgentSession, ContentPart, DeferredCall, HostToolHandler, MessageId, QueueMode, RuntimeError,
    RuntimeEvent, SessionInfo, SessionSpec, SubagentControl, TicketId, TicketResolution, ToolCall,
    ToolOutcome,
};
use serde_json::{Map, Value};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch, Mutex};
use tokio::task::{JoinError, JoinHandle, JoinSet};
use zeroize::Zeroizing;

use crate::credential_rpc;
use crate::framing::Lines;
use crate::protocol::{
    self, DaemonMessage, Envelope, ErrorCode, Event, Outcome, Resolution, SidecarMessage,
};
use crate::{MAX_LINE_BYTES, MAX_USER_TEXT_BYTES};

type Writer = Arc<Mutex<Box<dyn AsyncWrite + Unpin + Send>>>;

/// Host RPC calls run concurrently for one sidecar; tools and credential calls share the cap.
pub(crate) const MAX_IN_FLIGHT_CALLS: usize = 32;
/// Runtime events buffered ahead of the consumer before the reader stops reading the socket.
pub(crate) const EVENT_CAPACITY: usize = 256;
/// Completed `rpc_id`s remembered for duplicate detection, beyond the in-flight ones.
pub(crate) const RECENT_RPC_IDS: usize = 1024;

/// Duplicate-`rpc_id` detection with memory bounded by the in-flight cap plus a recent window.
#[derive(Default)]
struct RpcIds {
    in_flight: HashSet<String>,
    recent: HashSet<String>,
    recent_order: VecDeque<String>,
}

impl RpcIds {
    fn seen(&self, id: &str) -> bool {
        self.in_flight.contains(id) || self.recent.contains(id)
    }

    fn start(&mut self, id: String) {
        self.in_flight.insert(id);
    }

    fn complete(&mut self, id: String) {
        self.in_flight.remove(&id);
        self.remember(id);
    }

    fn remember(&mut self, id: String) {
        if self.recent_order.len() == RECENT_RPC_IDS {
            if let Some(oldest) = self.recent_order.pop_front() {
                self.recent.remove(&oldest);
            }
        }
        self.recent.insert(id.clone());
        self.recent_order.push_back(id);
    }
}

type CallResult = Result<(String, Result<(), RuntimeError>), JoinError>;

fn finish_call(finished: Option<CallResult>, ids: &mut RpcIds) -> Result<(), RuntimeError> {
    match finished {
        Some(Ok((id, Ok(())))) => {
            ids.complete(id);
            Ok(())
        }
        _ => Err(RuntimeError::Unavailable("host RPC response failed".into())),
    }
}

/// A ready JSONL connection independent of process spawning and Unix transports.
///
/// The reader runs continuously, including when the caller is not polling events.
/// Host tools execute concurrently and are cancelled when the connection closes.
pub struct Connection {
    info: SessionInfo,
    writer: Option<Writer>,
    events: mpsc::Receiver<RuntimeEvent>,
    /// Events pulled off the channel while shutdown waits, so the reader can reach `bye`.
    backlog: VecDeque<RuntimeEvent>,
    bye: watch::Receiver<bool>,
    driver: JoinHandle<()>,
    steer_ack: watch::Receiver<Option<(String, bool)>>,
    next_control_id: u64,
}

impl Connection {
    /// Authenticate hello, send open, and validate ready against the exact tool gate.
    ///
    /// The caller supplies a deadline covering this handshake. Only non-secret
    /// request-shaping configuration crosses open; model attempts resolve credentials later.
    pub async fn open<S>(
        stream: S,
        nonce: &str,
        spec: SessionSpec,
        tools: Arc<dyn HostToolHandler>,
    ) -> Result<Self, RuntimeError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let expected: HashSet<String> = spec.tools.iter().map(|tool| tool.name.clone()).collect();
        let open = open_message(spec)?;
        let (read, write) = tokio::io::split(stream);
        let writer: Writer = Arc::new(Mutex::new(Box::new(write)));
        let mut lines = Lines::new(read, MAX_LINE_BYTES);
        loop {
            match read_message(&mut lines, &writer).await? {
                Some(SidecarMessage::Hello {
                    protocol,
                    nonce: received,
                    versions,
                    ..
                }) if received == nonce => {
                    crate::version::check_protocol(protocol)?;
                    crate::version::check_agent_version(&versions.openside_agent)?;
                    break;
                }
                Some(SidecarMessage::Hello { .. }) => {
                    return Err(RuntimeError::Unavailable(
                        "sidecar hello authentication failed".into(),
                    ));
                }
                Some(SidecarMessage::Error {
                    code,
                    message,
                    fatal,
                    ..
                }) => {
                    if fatal || matches!(code, ErrorCode::AuthUnavailable) {
                        return Err(runtime_error(code, message));
                    }
                }
                Some(_) => protocol_error(&writer, "expected hello").await?,
                None => return Err(disconnected()),
            }
        }
        write_message(&writer, open).await?;
        let (events_tx, events) = mpsc::channel(EVENT_CAPACITY);
        let (ready_tx, ready_rx) = oneshot::channel();
        let (bye_tx, bye) = watch::channel(false);
        let (steer_tx, steer_ack) = watch::channel(None);
        let driver_writer = writer.clone();
        let driver = tokio::spawn(async move {
            drive(
                lines,
                driver_writer,
                tools,
                expected,
                DriveChannels {
                    events: events_tx,
                    ready: ready_tx,
                    bye: bye_tx,
                    steer: steer_tx,
                },
            )
            .await;
        });
        // Construct before awaiting readiness so cancelling open aborts the driver too.
        let mut connection = Self {
            info: SessionInfo {
                tools: Vec::new(),
                pending_deferred: Vec::new(),
                startup_ms: 0,
            },
            writer: Some(writer),
            events,
            backlog: VecDeque::new(),
            bye,
            driver,
            steer_ack,
            next_control_id: 0,
        };
        connection.info = ready_rx.await.map_err(|_| disconnected())??;
        Ok(connection)
    }

    /// Validated tool names, restored deferred calls, and startup latency.
    pub fn info(&self) -> &SessionInfo {
        &self.info
    }

    /// Send a daemon-owned message without waiting for a model response.
    pub async fn send(
        &mut self,
        id: MessageId,
        text: String,
        mode: QueueMode,
    ) -> Result<(), RuntimeError> {
        self.send_with_parts(id, text, Vec::new(), mode).await
    }

    /// Send a message whose content parts are inlined as base64 on the wire.
    pub async fn send_with_parts(
        &mut self,
        id: MessageId,
        text: String,
        parts: Vec<ContentPart>,
        mode: QueueMode,
    ) -> Result<(), RuntimeError> {
        if text.len() > MAX_USER_TEXT_BYTES {
            return Err(RuntimeError::Rejected("message text exceeds 1 MiB".into()));
        }
        let mode = match mode {
            QueueMode::Queue => protocol::MessageMode::Queue,
            QueueMode::Steer => protocol::MessageMode::Steer,
        };
        self.write(DaemonMessage::UserMessage {
            message_id: id,
            text,
            mode,
            parts: parts.into_iter().map(wire_part).collect(),
        })
        .await
    }

    /// Request cancellation; the sidecar treats cancellation while idle as harmless.
    pub async fn abort(&mut self) -> Result<(), RuntimeError> {
        self.write(DaemonMessage::Abort).await
    }

    /// Resolve a deferred call using model-safe data only.
    pub async fn resolve_ticket(
        &mut self,
        ticket: TicketId,
        tool_call_id: String,
        resolution: TicketResolution,
    ) -> Result<(), RuntimeError> {
        let resolution = match resolution {
            TicketResolution::Approved => Resolution::Approved,
            TicketResolution::Denied { reason } => Resolution::Denied { reason },
            TicketResolution::Result { for_model_json } => Resolution::Result {
                for_model: serde_json::from_str(&for_model_json).map_err(|_| {
                    RuntimeError::Rejected("ticket result is not valid JSON".into())
                })?,
            },
        };
        self.write(DaemonMessage::TicketResolved {
            ticket_id: ticket,
            tool_call_id,
            resolution,
        })
        .await
    }

    /// Steering succeeds only after the destination acknowledges lifecycle admission.
    pub async fn control_subagent(
        &mut self,
        subagent_id: String,
        control: SubagentControl,
    ) -> Result<(), RuntimeError> {
        if matches!(&control, SubagentControl::Steer { text } if text.len() > MAX_USER_TEXT_BYTES) {
            return Err(RuntimeError::Rejected("message text exceeds 1 MiB".into()));
        }
        let SubagentControl::Steer { text } = control else {
            return self
                .write(DaemonMessage::SubagentCancel { subagent_id })
                .await;
        };
        self.next_control_id += 1;
        let control_id = format!("steer-{}", self.next_control_id);
        self.write(DaemonMessage::SubagentSteer {
            subagent_id,
            text,
            control_id: control_id.clone(),
        })
        .await?;
        let acknowledgement = async {
            loop {
                if let Some((id, delivered)) = self.steer_ack.borrow_and_update().clone() {
                    if id == control_id {
                        return if delivered {
                            Ok(())
                        } else {
                            Err(RuntimeError::Rejected("subagent_finished".into()))
                        };
                    }
                }
                tokio::select! {
                    result = self.steer_ack.changed() => result.map_err(|_| disconnected())?,
                    event = self.events.recv() => {
                        self.backlog.push_back(event.ok_or_else(disconnected)?);
                    }
                }
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(30), acknowledgement)
            .await
            .map_err(|_| RuntimeError::Unavailable("steer acknowledgement timed out".into()))?
    }

    /// Drop the restored continuation; its terminal output is already durable.
    pub async fn discard_deferred(&mut self) -> Result<(), RuntimeError> {
        self.write(DaemonMessage::DiscardDeferred).await
    }

    /// Receive normalized observations; EOF and fatal errors terminate the stream.
    ///
    /// The reader awaits channel capacity, so a consumer that stops polling stalls the sidecar
    /// instead of growing daemon memory.
    pub async fn next_event(&mut self) -> Option<RuntimeEvent> {
        if let Some(event) = self.backlog.pop_front() {
            return Some(event);
        }
        self.events.recv().await
    }

    /// Request shutdown and wait for bye; process supervision supplies the deadline.
    pub async fn shutdown(&mut self) -> Result<(), RuntimeError> {
        self.request_shutdown().await?;
        let Self {
            events,
            backlog,
            bye,
            ..
        } = self;
        let mut open = true;
        while !*bye.borrow() {
            tokio::select! {
                changed = bye.changed() => changed.map_err(|_| disconnected())?,
                event = events.recv(), if open => match event {
                    Some(event) => stash(backlog, event),
                    None => open = false,
                },
            }
        }
        Ok(())
    }

    /// Send `shutdown` while draining events.
    ///
    /// The write can wait on the writer lock behind a tool result that the sidecar is not
    /// reading because it is itself blocked on a full event queue; draining breaks that cycle.
    pub(crate) async fn request_shutdown(&mut self) -> Result<(), RuntimeError> {
        let Self {
            writer,
            events,
            backlog,
            ..
        } = self;
        let write = write_message(
            writer.as_ref().ok_or_else(disconnected)?,
            DaemonMessage::Shutdown,
        );
        tokio::pin!(write);
        let mut open = true;
        loop {
            tokio::select! {
                result = &mut write => return result,
                event = events.recv(), if open => match event {
                    Some(event) => stash(backlog, event),
                    None => open = false,
                },
            }
        }
    }

    #[cfg(unix)]
    pub(crate) async fn wait_closed(&mut self) {
        let Self {
            events,
            backlog,
            driver,
            ..
        } = self;
        let mut open = true;
        loop {
            tokio::select! {
                _ = &mut *driver => break,
                event = events.recv(), if open => match event {
                    Some(event) => stash(backlog, event),
                    None => open = false,
                },
            }
        }
    }

    #[cfg(unix)]
    pub(crate) fn try_next_event(&mut self) -> Option<RuntimeEvent> {
        self.backlog
            .pop_front()
            .or_else(|| self.events.try_recv().ok())
    }

    #[cfg(unix)]
    pub(crate) fn set_startup_ms(&mut self, startup_ms: u64) {
        self.info.startup_ms = startup_ms;
    }

    pub(crate) fn close(&mut self) {
        self.driver.abort();
        self.writer.take();
        self.events.close();
    }

    async fn write(&self, message: DaemonMessage) -> Result<(), RuntimeError> {
        write_message(self.writer.as_ref().ok_or_else(disconnected)?, message).await
    }
}

/// Shutdown drains a stalled reader into a bounded buffer; the oldest events are dropped so
/// the final snapshot, which arrives last, survives a flooding sidecar.
fn stash(backlog: &mut VecDeque<RuntimeEvent>, event: RuntimeEvent) {
    if backlog.len() == EVENT_CAPACITY {
        backlog.pop_front();
    }
    backlog.push_back(event);
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.close();
    }
}

impl AgentSession for Connection {
    fn info(&self) -> &SessionInfo {
        Connection::info(self)
    }
    async fn send(
        &mut self,
        id: MessageId,
        text: String,
        mode: QueueMode,
    ) -> Result<(), RuntimeError> {
        Connection::send(self, id, text, mode).await
    }
    async fn send_with_parts(
        &mut self,
        id: MessageId,
        text: String,
        parts: Vec<ContentPart>,
        mode: QueueMode,
    ) -> Result<(), RuntimeError> {
        Connection::send_with_parts(self, id, text, parts, mode).await
    }
    async fn abort(&mut self) -> Result<(), RuntimeError> {
        Connection::abort(self).await
    }
    async fn resolve_ticket(
        &mut self,
        ticket: TicketId,
        tool_call_id: String,
        resolution: TicketResolution,
    ) -> Result<(), RuntimeError> {
        Connection::resolve_ticket(self, ticket, tool_call_id, resolution).await
    }
    async fn control_subagent(
        &mut self,
        subagent_id: String,
        control: SubagentControl,
    ) -> Result<(), RuntimeError> {
        Connection::control_subagent(self, subagent_id, control).await
    }
    async fn discard_deferred(&mut self) -> Result<(), RuntimeError> {
        Connection::discard_deferred(self).await
    }
    async fn next_event(&mut self) -> Option<RuntimeEvent> {
        Connection::next_event(self).await
    }
    async fn shutdown(&mut self) -> Result<(), RuntimeError> {
        Connection::shutdown(self).await
    }
}

fn open_message(spec: SessionSpec) -> Result<DaemonMessage, RuntimeError> {
    if spec.team.as_ref().is_some_and(|team| {
        team.persona.chars().count() > 1500 || team.roster.chars().count() > 2000
    }) {
        return Err(RuntimeError::Rejected(
            "team prompt exceeds character limits".into(),
        ));
    }
    let tools = spec
        .tools
        .into_iter()
        .map(|tool| {
            let parameters = serde_json::from_str::<Map<String, Value>>(&tool.parameters_json)
                .map_err(|_| {
                    RuntimeError::Rejected("tool parameters must be a JSON Schema object".into())
                })?;
            Ok(protocol::Tool {
                name: tool.name,
                description: tool.description,
                parameters,
                approval: tool.approval,
                deferrable: tool.deferrable,
            })
        })
        .collect::<Result<Vec<_>, RuntimeError>>()?;
    let credentials_json = Zeroizing::new(spec.credentials_json);
    let credentials =
        serde_json::from_str::<Map<String, Value>>(&credentials_json).map_err(|_| {
            RuntimeError::Rejected("credential configuration must be a JSON object".into())
        })?;
    let agent = serde_json::from_str::<Map<String, Value>>(&spec.agent_json)
        .map_err(|_| RuntimeError::Rejected("agent configuration must be a JSON object".into()))?;
    Ok(DaemonMessage::Open {
        session: spec.session,
        model: protocol::Model {
            id: spec.model,
            fallback: spec.fallback_models,
        },
        system_prompt_parts: spec.system_prompt_parts,
        tools,
        resume_state: spec.resume_state.map(|state| protocol::State {
            format: state.format,
            data: state.data,
        }),
        config: protocol::Config { credentials, agent },
        team: spec.team,
    })
}

/// The exact `open` message, as JSON, that a sidecar receives for `spec`. Profile snapshots and
/// the token budget measure this, so they cannot drift from the bytes on the wire.
pub fn open_wire(spec: SessionSpec) -> Result<Value, RuntimeError> {
    serde_json::to_value(Envelope::new(open_message(spec)?))
        .map_err(|_| RuntimeError::Rejected("could not encode sidecar message".into()))
}

pub(crate) fn disconnected() -> RuntimeError {
    RuntimeError::Unavailable("sidecar connection closed".into())
}

async fn write_message(writer: &Writer, message: DaemonMessage) -> Result<(), RuntimeError> {
    let mut bytes = Zeroizing::new(
        serde_json::to_vec(&Envelope::new(message))
            .map_err(|_| RuntimeError::Rejected("could not encode sidecar message".into()))?,
    );
    if bytes.len() > MAX_LINE_BYTES {
        return Err(RuntimeError::Rejected(
            "sidecar message exceeds 4 MiB".into(),
        ));
    }
    bytes.push(b'\n');
    writer
        .lock()
        .await
        .write_all(&bytes)
        .await
        .map_err(|_| disconnected())
}

async fn protocol_error(writer: &Writer, message: &str) -> Result<(), RuntimeError> {
    write_message(
        writer,
        DaemonMessage::Error {
            code: ErrorCode::Protocol,
            message: message.into(),
            fatal: false,
            message_id: None,
        },
    )
    .await
}

async fn read_message<R: AsyncRead + Unpin>(
    lines: &mut Lines<R>,
    writer: &Writer,
) -> Result<Option<SidecarMessage>, RuntimeError> {
    loop {
        let Some(frame) = lines.next().await.map_err(|_| disconnected())? else {
            return Ok(None);
        };
        if !frame.terminated {
            return Ok(None);
        }
        if !frame.oversized {
            if let Ok(envelope) = serde_json::from_slice::<Envelope<SidecarMessage>>(&frame.bytes) {
                if envelope.v == 1 {
                    return Ok(Some(envelope.message));
                }
            }
        }
        // Never echo malformed input or serde diagnostics: they may contain secrets.
        protocol_error(
            writer,
            if frame.oversized {
                "line exceeds 4 MiB"
            } else {
                "invalid protocol message"
            },
        )
        .await?;
    }
}

struct DriveChannels {
    events: mpsc::Sender<RuntimeEvent>,
    ready: oneshot::Sender<Result<SessionInfo, RuntimeError>>,
    bye: watch::Sender<bool>,
    steer: watch::Sender<Option<(String, bool)>>,
}

async fn drive<R: AsyncRead + Unpin>(
    mut lines: Lines<R>,
    writer: Writer,
    tools: Arc<dyn HostToolHandler>,
    expected: HashSet<String>,
    channels: DriveChannels,
) {
    let DriveChannels {
        events,
        ready,
        bye,
        steer,
    } = channels;
    let mut ready = Some(ready);
    let mut calls = JoinSet::new();
    let mut ids = RpcIds::default();
    loop {
        let incoming = {
            // Keep writes of malformed-line replies alive across tool completions.
            let incoming = read_message(&mut lines, &writer);
            tokio::pin!(incoming);
            loop {
                tokio::select! {
                    result = &mut incoming => break result,
                    finished = calls.join_next(), if !calls.is_empty() => {
                        if let Err(error) = finish_call(finished, &mut ids) {
                            break Err(error);
                        }
                    }
                }
            }
        };
        let message = match incoming {
            Ok(Some(message)) => message,
            Ok(None) => break,
            Err(error) => {
                let _ = events.send(RuntimeEvent::Error { error }).await;
                break;
            }
        };
        match message {
            SidecarMessage::Ready {
                tools,
                pending_deferred,
                startup_ms,
            } if ready.is_some() => {
                let received: HashSet<_> = tools.iter().map(String::as_str).collect();
                if received.len() != expected.len()
                    || received.iter().any(|name| !expected.contains(*name))
                {
                    let error = RuntimeError::Rejected(
                        "tool_gate: exposed tools differ from open.tools".into(),
                    );
                    let _ = write_message(
                        &writer,
                        DaemonMessage::Error {
                            code: ErrorCode::ToolGate,
                            message: "exposed tools differ from open.tools".into(),
                            fatal: true,
                            message_id: None,
                        },
                    )
                    .await;
                    if let Some(sender) = ready.take() {
                        let _ = sender.send(Err(error));
                    }
                    break;
                }
                if let Some(sender) = ready.take() {
                    let _ = sender.send(Ok(SessionInfo {
                        tools,
                        pending_deferred: pending_deferred.into_iter().map(deferred_call).collect(),
                        startup_ms,
                    }));
                }
            }
            message @ (SidecarMessage::CredentialRequest { .. }
            | SidecarMessage::CredentialReport { .. }) => {
                let Some((id, request_id)) = credential_rpc::correlation(&message) else {
                    if protocol_error(&writer, "invalid credential correlation id")
                        .await
                        .is_err()
                    {
                        break;
                    }
                    continue;
                };
                let id = id.to_owned();
                let request_id = request_id.to_owned();
                let mut reap_failed = None;
                while let Some(finished) = calls.try_join_next() {
                    if let Err(error) = finish_call(Some(finished), &mut ids) {
                        reap_failed = Some(error);
                        break;
                    }
                }
                if let Some(error) = reap_failed {
                    let _ = events.send(RuntimeEvent::Error { error }).await;
                    break;
                }
                if ids.seen(&id) {
                    if protocol_error(&writer, "duplicate credential rpc id")
                        .await
                        .is_err()
                    {
                        break;
                    }
                    continue;
                }
                if calls.len() >= MAX_IN_FLIGHT_CALLS {
                    ids.remember(id.clone());
                    let mut error = openside_core::CredentialError::unavailable();
                    error.code = openside_core::CredentialErrorCode::ProviderUnavailable;
                    error.reason = "Too many concurrent host RPC calls".into();
                    if write_message(&writer, credential_rpc::denial(id, request_id, error))
                        .await
                        .is_err()
                    {
                        break;
                    }
                    continue;
                }
                ids.start(id.clone());
                let handler = tools.clone();
                let response_writer = writer.clone();
                calls.spawn(async move {
                    let response = credential_rpc::reply(handler.as_ref(), message).await;
                    let result = write_message(&response_writer, response).await;
                    (id, result)
                });
            }
            SidecarMessage::ToolCall {
                rpc_id,
                tool_call_id,
                name,
                args,
                subagent,
                approval_ticket,
            } => {
                // Reap finished calls first so the in-flight cap and id sets are current.
                let mut reap_failed = None;
                while let Some(finished) = calls.try_join_next() {
                    if let Err(error) = finish_call(Some(finished), &mut ids) {
                        reap_failed = Some(error);
                        break;
                    }
                }
                if let Some(error) = reap_failed {
                    let _ = events.send(RuntimeEvent::Error { error }).await;
                    break;
                }
                if ids.seen(&rpc_id) {
                    if protocol_error(&writer, "duplicate rpc_id").await.is_err() {
                        break;
                    }
                    continue;
                }
                if calls.len() >= MAX_IN_FLIGHT_CALLS {
                    ids.remember(rpc_id.clone());
                    let rejected = DaemonMessage::ToolResult {
                        rpc_id,
                        outcome: Outcome::Error {
                            message: "too many concurrent tool calls".into(),
                        },
                    };
                    if write_message(&writer, rejected).await.is_err() {
                        break;
                    }
                    continue;
                }
                ids.start(rpc_id.clone());
                let call = ToolCall {
                    tool_call_id,
                    name,
                    arguments_json: Value::Object(args).to_string(),
                    subagent,
                    approval_ticket,
                };
                let exposed = expected.contains(&call.name);
                let handler = tools.clone();
                let response_writer = writer.clone();
                calls.spawn(async move {
                    let outcome = if exposed {
                        handler.call(call).await
                    } else {
                        ToolOutcome::Denied {
                            reason: "tool is not exposed in this session".into(),
                            hint: None,
                        }
                    };
                    let result = write_message(
                        &response_writer,
                        DaemonMessage::ToolResult {
                            rpc_id: rpc_id.clone(),
                            outcome: tool_outcome(outcome),
                        },
                    )
                    .await;
                    (rpc_id, result)
                });
            }
            SidecarMessage::SubagentSteerResult {
                control_id,
                delivered,
            } => {
                let _ = steer.send(Some((control_id, delivered)));
            }
            SidecarMessage::Error {
                code,
                message,
                fatal,
                ..
            } => {
                let fatal = fatal || matches!(code, ErrorCode::AuthUnavailable);
                let error = runtime_error(code, message);
                if fatal {
                    if let Some(sender) = ready.take() {
                        let _ = sender.send(Err(error.clone()));
                    }
                }
                let _ = events.send(RuntimeEvent::Error { error }).await;
                if fatal {
                    break;
                }
            }
            // Applied here, not through the event channel: a tool call that follows on the wire
            // must already see the new context epoch.
            SidecarMessage::ContextCompacted { subagent } => tools.context_compacted(subagent),
            SidecarMessage::Bye => {
                let _ = bye.send(true);
                break;
            }
            SidecarMessage::Hello { .. } | SidecarMessage::Ready { .. } => {
                if protocol_error(&writer, "unexpected handshake message")
                    .await
                    .is_err()
                {
                    break;
                }
            }
            other => {
                if let Some(event) = runtime_event(other) {
                    let _ = events.send(event).await;
                }
            }
        }
    }
    if let Some(sender) = ready {
        let _ = sender.send(Err(disconnected()));
    }
    calls.abort_all();
    // Cancelled tools must release their writer locks before closing the transport.
    while calls.join_next().await.is_some() {}
    let _ = writer.lock().await.shutdown().await;
}

fn runtime_error(code: ErrorCode, message: String) -> RuntimeError {
    match code {
        ErrorCode::AuthUnavailable => RuntimeError::AuthUnavailable(message),
        ErrorCode::ToolGate => RuntimeError::Rejected(format!("tool_gate: {message}")),
        _ => RuntimeError::Rejected(message),
    }
}

fn deferred_call(call: protocol::Deferred) -> DeferredCall {
    DeferredCall {
        tool_call_id: call.tool_call_id,
        ticket: call.ticket_id,
        kind: call.kind,
    }
}

fn tool_outcome(outcome: ToolOutcome) -> Outcome {
    let (for_model_json, parts) = match outcome {
        ToolOutcome::Done { for_model_json } => (for_model_json, Vec::new()),
        ToolOutcome::DoneWithParts {
            for_model_json,
            parts,
        } => (for_model_json, parts),
        ToolOutcome::Deferred { ticket, kind } => {
            return Outcome::Deferred {
                ticket_id: ticket,
                kind,
            }
        }
        ToolOutcome::Denied { reason, hint } => return Outcome::Denied { reason, hint },
        ToolOutcome::Failed { message } => return Outcome::Error { message },
    };
    match serde_json::from_str(&for_model_json) {
        Ok(for_model) => Outcome::Done {
            for_model,
            parts: parts.into_iter().map(wire_part).collect(),
        },
        Err(_) => Outcome::Error {
            message: "host tool returned invalid JSON".into(),
        },
    }
}

fn wire_part(part: ContentPart) -> protocol::Part {
    use base64::Engine as _;
    protocol::Part {
        kind: part.kind,
        mime: part.mime,
        data_base64: part
            .data
            .map(|data| base64::engine::general_purpose::STANDARD.encode(data)),
        artifact_ref: part.artifact_ref,
        name: part.name,
    }
}

fn runtime_event(message: SidecarMessage) -> Option<RuntimeEvent> {
    Some(match message {
        SidecarMessage::Event { event } => match event {
            Event::TurnStarted => RuntimeEvent::TurnStarted,
            Event::TurnEnded => RuntimeEvent::TurnEnded,
            Event::AssistantDelta { text } => RuntimeEvent::AssistantDelta { text },
            Event::AssistantFinal { text } => RuntimeEvent::AssistantFinal { text },
            Event::SessionTitleProposed { title } => RuntimeEvent::SessionTitleProposed { title },
            Event::ThinkingDelta { text } => RuntimeEvent::ThinkingDelta { text },
            Event::SubagentLifecycle {
                subagent_id,
                status,
                output,
            } => RuntimeEvent::SubagentLifecycle {
                subagent_id,
                status,
                output,
            },
            Event::SubagentProgress {
                subagent_id,
                progress,
            } => RuntimeEvent::SubagentProgress {
                subagent_id,
                progress_json: progress.to_string(),
            },
            Event::Usage {
                model,
                input_tokens,
                output_tokens,
            } => RuntimeEvent::Usage {
                model,
                input_tokens,
                output_tokens,
            },
        },
        SidecarMessage::MessageAccepted { message_id } => {
            RuntimeEvent::MessageAccepted { id: message_id }
        }
        SidecarMessage::MessageResult {
            message_id,
            outcome,
            reason,
        } => RuntimeEvent::MessageResult {
            id: message_id,
            outcome: match outcome {
                protocol::RunOutcome::Completed => openside_core::RunOutcome::Completed,
                protocol::RunOutcome::Aborted => openside_core::RunOutcome::Aborted,
                protocol::RunOutcome::Failed => openside_core::RunOutcome::Failed,
                protocol::RunOutcome::Deferred => openside_core::RunOutcome::Deferred,
            },
            reason,
        },
        SidecarMessage::Deferred { calls } => RuntimeEvent::Deferred {
            calls: calls.into_iter().map(deferred_call).collect(),
        },
        SidecarMessage::StateSnapshot { state } => RuntimeEvent::StateSnapshot {
            state: openside_core::AgentState {
                format: state.format,
                data: state.data,
            },
        },
        SidecarMessage::Settled => RuntimeEvent::Settled,
        _ => return None,
    })
}
