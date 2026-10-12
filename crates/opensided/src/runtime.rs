use std::sync::Arc;

use futures_util::future::BoxFuture;
use openside_core::{
    AgentRuntime, AgentSession, ContentPart, HostToolHandler, MessageId, QueueMode, RuntimeError,
    RuntimeEvent, SessionInfo, SessionSpec, SubagentControl, TicketId, TicketResolution,
};

// The core traits use return-position futures. Erasure lets actors own heterogeneous
// runtimes without test branches or changing the runtime-independent core contract.
pub(crate) trait Runtime: Send + Sync {
    fn open(
        &self,
        spec: SessionSpec,
        host: Arc<dyn HostToolHandler>,
    ) -> BoxFuture<'_, Result<Box<dyn Session>, RuntimeError>>;
}

pub(crate) trait Session: Send {
    fn info(&self) -> &SessionInfo;
    fn send(
        &mut self,
        id: MessageId,
        text: String,
        parts: Vec<ContentPart>,
        mode: QueueMode,
    ) -> BoxFuture<'_, Result<(), RuntimeError>>;
    fn abort(&mut self) -> BoxFuture<'_, Result<(), RuntimeError>>;
    fn resolve(
        &mut self,
        id: TicketId,
        call: String,
        resolution: TicketResolution,
    ) -> BoxFuture<'_, Result<(), RuntimeError>>;
    fn control(
        &mut self,
        subagent: String,
        control: SubagentControl,
    ) -> BoxFuture<'_, Result<(), RuntimeError>>;
    fn discard(&mut self) -> BoxFuture<'_, Result<(), RuntimeError>>;
    fn next(&mut self) -> BoxFuture<'_, Option<RuntimeEvent>>;
    fn shutdown(&mut self) -> BoxFuture<'_, Result<(), RuntimeError>>;
}

impl<R: AgentRuntime> Runtime for R
where
    R::Session: 'static,
{
    fn open(
        &self,
        spec: SessionSpec,
        host: Arc<dyn HostToolHandler>,
    ) -> BoxFuture<'_, Result<Box<dyn Session>, RuntimeError>> {
        Box::pin(async move {
            let session = self.open_session(spec, host).await?;
            Ok(Box::new(session) as Box<dyn Session>)
        })
    }
}

impl<S: AgentSession> Session for S {
    fn info(&self) -> &SessionInfo {
        AgentSession::info(self)
    }
    fn send(
        &mut self,
        id: MessageId,
        text: String,
        parts: Vec<ContentPart>,
        mode: QueueMode,
    ) -> BoxFuture<'_, Result<(), RuntimeError>> {
        Box::pin(AgentSession::send_with_parts(self, id, text, parts, mode))
    }
    fn abort(&mut self) -> BoxFuture<'_, Result<(), RuntimeError>> {
        Box::pin(AgentSession::abort(self))
    }
    fn resolve(
        &mut self,
        id: TicketId,
        call: String,
        resolution: TicketResolution,
    ) -> BoxFuture<'_, Result<(), RuntimeError>> {
        Box::pin(AgentSession::resolve_ticket(self, id, call, resolution))
    }
    fn control(
        &mut self,
        subagent: String,
        control: SubagentControl,
    ) -> BoxFuture<'_, Result<(), RuntimeError>> {
        Box::pin(AgentSession::control_subagent(self, subagent, control))
    }
    fn discard(&mut self) -> BoxFuture<'_, Result<(), RuntimeError>> {
        Box::pin(AgentSession::discard_deferred(self))
    }
    fn next(&mut self) -> BoxFuture<'_, Option<RuntimeEvent>> {
        Box::pin(AgentSession::next_event(self))
    }
    fn shutdown(&mut self) -> BoxFuture<'_, Result<(), RuntimeError>> {
        Box::pin(AgentSession::shutdown(self))
    }
}
