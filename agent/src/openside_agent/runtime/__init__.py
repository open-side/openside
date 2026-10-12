"""One session per connection; Rust owns execution, tickets and durable storage."""

from __future__ import annotations

import asyncio
import os
import time
from collections import OrderedDict, deque
from collections.abc import Awaitable, Callable
from contextlib import AsyncExitStack
from typing import Any, Literal, cast
from uuid import uuid4

from pydantic_ai import (
    Agent,
    CallDeferred,
    DeferredToolRequests,
    DeferredToolResults,
    RunContext,
    ToolApproved,
    ToolDenied,
)
from pydantic_ai.capabilities import AbstractCapability
from pydantic_ai.exceptions import RunCancelled
from pydantic_ai.messages import (
    ModelMessage,
    ModelRequest,
    ModelResponse,
    PartDeltaEvent,
    PartStartEvent,
    TextPart,
    TextPartDelta,
    ThinkingPart,
    ThinkingPartDelta,
    ToolCallPart,
    UserPromptPart,
)
from pydantic_ai.models import Model, ModelRequestContext
from pydantic_ai.run import AgentRun
from pydantic_ai.usage import RunUsage

from openside_agent import providers, research
from openside_agent.capabilities import adopted_capabilities
from openside_agent.content import user_prompt
from openside_agent.protocol import (
    MAX_USER_TEXT_BYTES,
    Abort,
    Approved,
    AssistantDelta,
    AssistantFinal,
    Bye,
    ContextCompacted,
    Deferred,
    DeferredCall,
    DiscardDeferred,
    Error,
    Event,
    EventBody,
    ExternalResult,
    JsonLineConnection,
    MessageAccepted,
    MessageResult,
    Open,
    Ready,
    Rejected,
    Settled,
    Shutdown,
    StateSnapshot,
    SubagentCancel,
    SubagentLifecycle,
    SubagentProgress,
    SubagentSteer,
    ThinkingDelta,
    TicketResolved,
    ToolCall,
    ToolOutcome,
    ToolResult,
    TurnEnded,
    TurnStarted,
    Usage,
    UserMessage,
)
from openside_agent.runtime.fork import fork_history
from openside_agent.runtime.state import (
    TERMINAL,
    PendingCall,
    PendingDelegate,
    SnapshotData,
    SnapshotTooLarge,
    SubagentState,
    SubagentStatus,
    dump_messages,
    load_messages,
)
from openside_agent.runtime.subagents import (
    SUBAGENT_INSTRUCTIONS,
    Admission,
    Child,
    ChildHooks,
)
from openside_agent.scripted import ScriptedModel
from openside_agent.skills import SKILL_TOOL, index_names, load_mentions
from openside_agent.tools import proxy_toolset

ABANDONED_RPC_LIMIT = 1024


class ToolGateError(Exception):
    pass


class SessionHooks(AbstractCapability[None]):
    def __init__(self, session: Session):
        self.session = session

    async def before_model_request(
        self, ctx: RunContext[None], request_context: ModelRequestContext
    ) -> ModelRequestContext:
        session = self.session
        actual = {
            tool.name
            for tool in request_context.model_request_parameters.declared_function_tools
        }
        if actual != session.expected_tools:
            raise ToolGateError()
        session.messages = ctx.messages
        accepted: list[str] = []
        if session.initial is not None:
            accepted.append(session.initial.text)
            await session.accept(session.initial)
            session.initial = None
        while session.steer_buffer:
            message = session.steer_buffer.popleft()
            injected = ModelRequest(
                parts=[UserPromptPart(user_prompt(message))],
                run_id=ctx.run_id,
                conversation_id=ctx.conversation_id,
            )
            ctx.messages.append(injected)
            request_context.messages = [*request_context.messages, injected]
            accepted.append(message.text)
            await session.accept(message)
        if session.skill_names:
            # Progressive disclosure: mentioned skills load through the daemon tool
            # before the model sees the message, so the transcript records it.
            loaded = await load_mentions(
                session.reverse_call,
                session.skill_names,
                accepted,
                run_id=ctx.run_id,
                conversation_id=ctx.conversation_id,
            )
            ctx.messages.extend(loaded)
            request_context.messages = [*request_context.messages, *loaded]
        session.steer_received.clear()
        await session.event(TurnStarted())
        session.turn_open = True
        return request_context

    async def after_model_request(
        self,
        ctx: RunContext[None],
        *,
        request_context: ModelRequestContext,
        response: ModelResponse,
    ) -> ModelResponse:
        await self.session.event(
            Usage(
                model=response.model_name or request_context.model.model_name,
                input_tokens=response.usage.input_tokens,
                output_tokens=response.usage.output_tokens,
            )
        )
        await self.session.event(TurnEnded())
        self.session.turn_open = False
        return response


class Session:
    def __init__(self, connection: JsonLineConnection, *, model: Model | None = None):
        self.connection = connection
        self.model_override = model
        self.agent: Agent[None, str | DeferredToolRequests] | None = None
        self.spec: Open | None = None
        self.expected_tools: set[str] = set()
        self.skill_names: list[str] = []
        self.messages: list[ModelMessage] = []
        self.pins = providers.CredentialPins()
        self.pending: dict[str, PendingCall] = {}
        self.pending_delegates: dict[str, PendingDelegate] = {}
        self.children: dict[str, Child] = {}
        self.retries = 3
        self.steer_buffer: deque[UserMessage] = deque()
        self.queue: deque[UserMessage] = deque()
        self.steer_received = asyncio.Event()
        self.scripted: ScriptedModel | None = None
        self.initial: UserMessage | None = None
        self.run_messages: list[str] = []
        self.deferred_messages: list[str] = []
        self.run_task: asyncio.Task[None] | None = None
        self.run: AgentRun[None, str | DeferredToolRequests] | None = None
        self.abort_requested = False
        self.turn_open = False
        self.shutting_down = False
        self.fatal = False
        self.rpc: dict[str, asyncio.Future[ToolOutcome]] = {}
        # Cancelled runs abandon RPCs the daemon still answers once; drop that reply.
        self.abandoned: OrderedDict[str, None] = OrderedDict()
        self.stack = AsyncExitStack()

    async def event(self, event: EventBody) -> None:
        await self.connection.send(Event(event=event))

    def compaction_reporter(
        self, subagent: str | None
    ) -> Callable[[], Awaitable[None]]:
        """Tells the daemon this agent's earlier messages left its model context.

        The daemon retires tool-result bases (e.g. browser snapshot ids) that the model
        may no longer see; it must hear this before the next request can name one.
        """

        async def report() -> None:
            await self.connection.send(ContextCompacted(subagent=subagent))

        return report

    async def error(
        self,
        code: Literal[
            "protocol",
            "bad_request",
            "stale_ticket",
            "model_error",
            "auth_unavailable",
            "tool_gate",
            "internal",
        ],
        message: str,
        *,
        fatal: bool = False,
    ) -> None:
        await self.connection.send(Error(code=code, message=message, fatal=fatal))
        if fatal:
            self.fatal = True
            self.connection.writer.close()

    async def accept(self, message: UserMessage) -> None:
        if message.message_id not in self.run_messages:
            self.run_messages.append(message.message_id)
        await self.connection.send(MessageAccepted(message_id=message.message_id))

    async def open(self, spec: Open) -> None:
        start = time.monotonic()
        self.spec = spec
        self.expected_tools = {tool.name for tool in spec.tools}
        self.skill_names = (
            index_names(spec.system_prompt_parts)
            if SKILL_TOOL in self.expected_tools
            else []
        )
        if len(self.expected_tools) != len(spec.tools):
            await self.error("tool_gate", "Duplicate daemon tool names", fatal=True)
            return
        cursor = 0
        if spec.resume_state is not None:
            state = SnapshotData.model_validate_json(spec.resume_state.data)
            self.messages = state.restore_messages()
            self.pins = providers.CredentialPins.from_json(state.pins)
            self.pending = {item.tool_call_id: item for item in state.pending_deferred}
            self.pending_delegates = {
                item.tool_call_id: item for item in state.pending_delegates
            }
            self.children = {item.id: Child(item) for item in state.subagents}
            # Saved resolutions need the daemon's durable delivery check before reuse.
            for pending in self.pending.values():
                pending.resolution = None
            for child in self.children.values():
                for pending in child.pending.values():
                    pending.resolution = None
                if child.state.status not in TERMINAL and not child.pending:
                    # Nothing can wake a child without tickets; failing it keeps
                    # its parent's delegate call from waiting forever.
                    child.state.status = "failed"
                    child.state.result = {
                        "status": "failed",
                        "subagent_id": child.id,
                        "message": "Subagent run was interrupted",
                    }
            for item in self.pending_delegates.values():
                if item.result is None:
                    item.result = self.child_result(item.subagent_id)
            self.steer_buffer.extend(state.steer_buffer)
            self.deferred_messages = list(state.deferred_messages)
            cursor = state.script_cursor
        network_allow = spec.config.agent.get("network_allow", [])
        if not isinstance(network_allow, list) or not all(
            isinstance(entry, str) for entry in cast(list[Any], network_allow)
        ):
            raise ValueError("Invalid network allow-list")
        providers.egress.install(cast(list[str], network_allow))
        script_path = os.getenv("OPENSIDE_AGENT_SCRIPT")
        if script_path:
            self.scripted = ScriptedModel.from_file(
                script_path, self.steer_received, cursor
            )
            model = self.scripted.model
        elif self.model_override is not None:
            model = self.model_override
        else:
            model = await providers.build_model(
                spec.model.id,
                spec.model.fallback,
                spec.config.credentials,
                session_id=spec.session,
                pins=self.pins,
            )
        retries = spec.config.agent.get("tool_retries", 3)
        if type(retries) is not int or retries < 0:
            raise ValueError("Invalid tool retry limit")
        self.retries = retries
        profile = spec.config.agent.get("profile")
        capabilities = [
            SessionHooks(self),
            *adopted_capabilities(spec.config.agent, self.compaction_reporter(None)),
            *research.capabilities(profile),
        ]
        self.agent = Agent(
            model,
            output_type=[str, DeferredToolRequests],
            deps_type=type(None),
            instructions=spec.system_prompt_parts,
            toolsets=[
                proxy_toolset(
                    spec.tools,
                    self.reverse_call,
                    retries=retries,
                    continuations={"delegate": self.delegate},
                )
            ],
            capabilities=capabilities,
            model_settings=research.model_settings(profile),
            retries=retries,
        )
        await self.stack.enter_async_context(self.agent)
        # Public toolsets include capability contributions, not just daemon proxies.
        # Capability-owned wrappers need their registry even for preflight listing.
        ctx = RunContext(
            deps=None,
            model=model,
            usage=RunUsage(),
            agent=self.agent,
            capabilities={str(index): cap for index, cap in enumerate(capabilities)},
        )
        exposed: list[str] = []
        for toolset in self.agent.toolsets:
            exposed.extend((await toolset.get_tools(ctx)).keys())
        if set(exposed) != self.expected_tools or len(exposed) != len(set(exposed)):
            await self.error(
                "tool_gate", "Exposed tools differ from daemon tools", fatal=True
            )
            return
        await self.connection.send(
            Ready(
                tools=sorted(exposed),
                pending_deferred=self.deferred_wire(),
                startup_ms=int((time.monotonic() - start) * 1000),
            )
        )
        self.resume_if_ready()

    async def reverse_call(
        self,
        tool_call_id: str,
        name: str,
        args: dict[str, object],
        approval_ticket: str | None,
        subagent: str | None = None,
    ) -> ToolOutcome:
        rpc_id = uuid4().hex
        future: asyncio.Future[ToolOutcome] = asyncio.get_running_loop().create_future()
        self.rpc[rpc_id] = future
        try:
            await self.connection.send(
                ToolCall(
                    rpc_id=rpc_id,
                    tool_call_id=tool_call_id,
                    name=name,
                    args=args,
                    subagent=subagent,
                    approval_ticket=approval_ticket,
                )
            )
            return await future
        finally:
            self.rpc.pop(rpc_id, None)
            # Cancelling the awaiting task also cancels the future itself.
            if future.cancelled() or not future.done():
                self.abandoned[rpc_id] = None
                while len(self.abandoned) > ABANDONED_RPC_LIMIT:
                    self.abandoned.popitem(last=False)

    def start(
        self, message: UserMessage | None, results: DeferredToolResults | None = None
    ) -> None:
        self.initial = message
        # A message-less resume finishes the messages whose run deferred.
        self.run_messages = (
            [message.message_id]
            if message is not None
            else list(self.deferred_messages)
        )
        self.abort_requested = False
        self.run_task = asyncio.create_task(self.drive(message, results))

    def waiting(self) -> bool:
        return bool(self.pending or self.pending_delegates)

    def children_active(self) -> bool:
        return any(child.active for child in self.children.values())

    def start_next(self, *, allow_steer: bool) -> bool:
        if self.shutting_down or self.fatal or self.waiting():
            return False
        if allow_steer and self.steer_buffer:
            self.start(self.steer_buffer.popleft())
            return True
        if self.queue:
            self.start(self.queue.popleft())
            return True
        return False

    def deferred_wire(self) -> list[DeferredCall]:
        """Daemon-visible deferred calls: parent tickets plus waiting children's."""
        calls = [item.wire() for item in self.pending.values()]
        for child in self.children.values():
            if child.state.status not in TERMINAL:
                calls.extend(item.wire() for item in child.pending.values())
        return calls

    async def snapshot(self) -> None:
        cursor = self.scripted.cursor if self.scripted is not None else 0
        try:
            state = SnapshotData.capture(
                self.messages,
                list(self.pending.values()),
                list(self.steer_buffer),
                self.pins,
                cursor,
                delegates=list(self.pending_delegates.values()),
                subagents=[child.capture() for child in self.children.values()],
                deferred_messages=self.deferred_messages,
            )
        except SnapshotTooLarge as exc:
            await self.error("internal", str(exc))
            return
        await self.connection.send(StateSnapshot(state=state))

    async def stream_event(self, event: object, held: list[str] | None = None) -> None:
        if isinstance(event, PartStartEvent):
            if isinstance(event.part, TextPart) and event.part.content:
                await self.text_delta(event.part.content, held)
            elif isinstance(event.part, ThinkingPart) and event.part.content:
                await self.event(ThinkingDelta(text=event.part.content))
        elif isinstance(event, PartDeltaEvent):
            if isinstance(event.delta, TextPartDelta):
                await self.text_delta(event.delta.content_delta, held)
            elif (
                isinstance(event.delta, ThinkingPartDelta) and event.delta.content_delta
            ):
                await self.event(ThinkingDelta(text=event.delta.content_delta))

    async def text_delta(self, text: str, held: list[str] | None) -> None:
        if held is None:
            await self.event(AssistantDelta(text=text))
        else:
            held.append(text)

    async def drive(
        self,
        message: UserMessage | None,
        results: DeferredToolResults | None,
    ) -> None:
        assert self.agent is not None and self.spec is not None
        profile = self.spec.config.agent.get("profile")
        outcome: Literal["completed", "aborted", "failed", "deferred"] = "completed"
        reason: str | None = None
        try:
            async with self.agent.iter(
                user_prompt(message) if message is not None else None,
                message_history=self.messages,
                deferred_tool_results=results,
                conversation_id=self.spec.session,
            ) as run:
                self.run = run
                if self.abort_requested:
                    run.cancel()
                async for node in run:
                    if Agent.is_model_request_node(node):
                        held: list[str] | None = (
                            [] if research.holds_final_text(profile) else None
                        )
                        async with node.stream(run.ctx) as stream:
                            async for event in stream:
                                await self.stream_event(event, held)
                        # Text beside tool calls is narration, never the final answer.
                        if held and any(
                            isinstance(part, ToolCallPart)
                            for part in stream.response.parts
                        ):
                            await self.event(AssistantDelta(text="".join(held)))
                result = run.result
                assert result is not None
                self.messages = result.all_messages()
                if isinstance(result.output, DeferredToolRequests):
                    outcome = "deferred"
                    pending: dict[str, PendingCall] = {}
                    delegates: dict[str, PendingDelegate] = {}
                    groups: list[
                        tuple[Literal["external", "approval"], list[ToolCallPart]]
                    ] = [
                        ("external", result.output.calls),
                        ("approval", result.output.approvals),
                    ]
                    for kind, calls in groups:
                        for call in calls:
                            metadata = result.output.metadata[call.tool_call_id]
                            subagent_id = metadata.get("subagent_id")
                            if kind == "external" and isinstance(subagent_id, str):
                                delegates[call.tool_call_id] = PendingDelegate(
                                    tool_call_id=call.tool_call_id,
                                    subagent_id=subagent_id,
                                    result=self.child_result(subagent_id),
                                )
                                continue
                            pending[call.tool_call_id] = PendingCall(
                                tool_call_id=call.tool_call_id,
                                ticket_id=metadata["ticket_id"],
                                kind=kind,
                            )
                    self.pending = pending
                    self.pending_delegates = delegates
                    wire = self.deferred_wire()
                    if wire:
                        await self.connection.send(Deferred(calls=wire))
                else:
                    await self.event(AssistantFinal(text=result.output))
        except RunCancelled as exc:
            self.messages = exc.all_messages()
            outcome = "aborted"
        except providers.AuthUnavailable:
            await self.error(
                "auth_unavailable",
                "Model credentials unavailable; check credential configuration",
                fatal=True,
            )
            return
        except ToolGateError:
            await self.error(
                "tool_gate", "Exposed tools differ from daemon tools", fatal=True
            )
            return
        except Exception:
            outcome, reason = "failed", "Model run failed"
            await self.error(
                "model_error", "Model run failed; check provider configuration"
            )
        finally:
            self.run = None
            self.initial = None
            if not self.fatal:
                if self.turn_open:
                    await self.event(TurnEnded())
                    self.turn_open = False
                carried = set(self.deferred_messages)
                for message_id in self.run_messages:
                    # The daemon accepts deferred -> terminal, not a repeated deferred.
                    if outcome == "deferred" and message_id in carried:
                        continue
                    await self.connection.send(
                        MessageResult(
                            message_id=message_id, outcome=outcome, reason=reason
                        )
                    )
                self.deferred_messages = (
                    list(self.run_messages) if outcome == "deferred" else []
                )
                await self.snapshot()
            self.run_task = None
        if not self.fatal and not self.resume_if_ready():
            if (
                not self.start_next(allow_steer=outcome == "completed")
                and not self.children_active()
            ):
                await self.connection.send(Settled())

    async def resolve(self, message: TicketResolved) -> None:
        owner: Child | None = None
        pending = self.pending.get(message.tool_call_id)
        if pending is None or pending.ticket_id != message.ticket_id:
            pending = None
            for child in self.children.values():
                candidate = child.pending.get(message.tool_call_id)
                if (
                    candidate is not None
                    and candidate.ticket_id == message.ticket_id
                    and child.state.status not in TERMINAL
                ):
                    owner, pending = child, candidate
                    break
        if pending is None or pending.resolution is not None:
            await self.error(
                "stale_ticket", "Unknown or already resolved deferred call"
            )
            return
        if (
            pending.kind == "approval"
            and isinstance(message.resolution, ExternalResult)
        ) or (
            pending.kind == "external"
            and not isinstance(message.resolution, ExternalResult)
        ):
            await self.error("bad_request", "Ticket resolution kind does not match")
            return
        pending.resolution = message.resolution
        if owner is None:
            self.resume_if_ready()
        else:
            self.child_resume_if_ready(owner)

    @staticmethod
    def deferred_results(pending: dict[str, PendingCall]) -> DeferredToolResults:
        results = DeferredToolResults()
        for call_id, item in pending.items():
            resolution = item.resolution
            if isinstance(resolution, Approved):
                results.approvals[call_id] = ToolApproved()
                results.metadata[call_id] = {"ticket_id": item.ticket_id}
            elif isinstance(resolution, Rejected):
                results.approvals[call_id] = ToolDenied(resolution.reason)
            elif isinstance(resolution, ExternalResult):
                results.calls[call_id] = resolution.for_model
        return results

    def resume_if_ready(self) -> bool:
        # A resolution can arrive immediately after deferred, before its snapshot.
        if (
            self.shutting_down
            or self.fatal
            or self.run_task is not None
            or not self.waiting()
        ):
            return False
        if any(item.resolution is None for item in self.pending.values()) or any(
            item.result is None for item in self.pending_delegates.values()
        ):
            return False
        results = self.deferred_results(self.pending)
        for call_id, item in self.pending_delegates.items():
            results.calls[call_id] = item.result
        self.pending.clear()
        self.pending_delegates.clear()
        self.start(None, results)
        return True

    def child_result(self, subagent_id: str) -> Any:
        child = self.children.get(subagent_id)
        if child is None:
            return {
                "status": "failed",
                "subagent_id": subagent_id,
                "message": "Subagent state was lost",
            }
        return child.state.result if child.state.status in TERMINAL else None

    async def delegate(
        self, ctx: RunContext[None], args: dict[str, Any], admitted: Any
    ) -> Any:
        """Run the child the daemon admitted; its deferral defers this parent call."""
        assert ctx.tool_call_id is not None
        admission = Admission.model_validate(admitted)
        task = str(args.get("task", ""))
        history = fork_history(ctx.messages) if admission.context == "fork" else []
        child = Child(
            SubagentState(
                id=admission.subagent_id,
                parent_tool_call_id=ctx.tool_call_id,
                task=task,
                model=admission.model,
                tools=admission.tools,
                status="running",
                messages=dump_messages(history),
            )
        )
        self.children[child.id] = child
        child.waiter = asyncio.get_running_loop().create_future()
        self.start_child(child, task, None)
        try:
            status = await asyncio.shield(child.waiter)
        except asyncio.CancelledError:
            # The parent run was cancelled; never leave its child running unowned.
            child.request_cancel()
            if child.task is not None:
                await asyncio.wait([child.task])
            raise
        finally:
            child.waiter = None
        if status == "waiting_human":
            raise CallDeferred(metadata={"subagent_id": child.id})
        return child.state.result

    async def child_agent(
        self, child: Child
    ) -> Agent[None, str | DeferredToolRequests]:
        if child.agent is not None:
            return child.agent
        assert self.spec is not None
        spec = self.spec
        allowed = set(child.state.tools)
        model: Model
        if self.scripted is not None:
            child.scripted = self.scripted.for_subagent(
                child.state.parent_tool_call_id,
                child.steer_received,
                child.state.script_cursor,
            )
            model = child.scripted.model
        elif self.model_override is not None:
            model = self.model_override
        else:
            model = await providers.build_model(
                child.state.model,
                [],
                spec.config.credentials,
                session_id=spec.session,
                pins=self.pins,
            )

        async def call(
            tool_call_id: str, name: str, args: dict[str, Any], ticket: str | None
        ) -> ToolOutcome:
            return await self.reverse_call(
                tool_call_id, name, args, ticket, subagent=child.id
            )

        agent = Agent(
            model,
            output_type=[str, DeferredToolRequests],
            deps_type=type(None),
            instructions=[*spec.system_prompt_parts, SUBAGENT_INSTRUCTIONS],
            toolsets=[
                proxy_toolset(
                    [tool for tool in spec.tools if tool.name in allowed],
                    call,
                    retries=self.retries,
                )
            ],
            capabilities=[
                ChildHooks(self, child),
                *adopted_capabilities(
                    spec.config.agent, self.compaction_reporter(child.id)
                ),
            ],
            retries=self.retries,
        )
        await self.stack.enter_async_context(agent)
        child.agent = agent
        return agent

    def start_child(
        self, child: Child, prompt: str | None, results: DeferredToolResults | None
    ) -> None:
        child.active = True
        child.cancel_requested = False
        child.task = asyncio.create_task(self.drive_child(child, prompt, results))

    def child_resume_if_ready(self, child: Child) -> bool:
        if self.shutting_down or self.fatal or child.active or not child.pending:
            return False
        if any(item.resolution is None for item in child.pending.values()):
            return False
        # child.pending stays until the run ends: a snapshot taken meanwhile must
        # still show this child waiting on its (now delivered) tickets.
        self.start_child(child, None, self.deferred_results(child.pending))
        return True

    async def drive_child(
        self,
        child: Child,
        prompt: str | None,
        results: DeferredToolResults | None,
    ) -> None:
        assert self.spec is not None
        status: SubagentStatus = "failed"
        output = ""
        messages = child.state.messages
        pending: dict[str, PendingCall] = {}
        try:
            await self.event(SubagentLifecycle(subagent_id=child.id, status="running"))
            agent = await self.child_agent(child)
            while True:
                async with agent.iter(
                    prompt,
                    message_history=load_messages(messages),
                    deferred_tool_results=results,
                    conversation_id=f"{self.spec.session}/{child.id}",
                ) as run:
                    child.run = run
                    if child.cancel_requested:
                        run.cancel()
                    async for node in run:
                        if Agent.is_model_request_node(node):
                            async with node.stream(run.ctx) as stream:
                                async for _ in stream:
                                    pass
                    result = run.result
                    assert result is not None
                child.run = None
                messages = dump_messages(result.all_messages())
                if isinstance(result.output, DeferredToolRequests):
                    groups: list[
                        tuple[Literal["external", "approval"], list[ToolCallPart]]
                    ] = [
                        ("external", result.output.calls),
                        ("approval", result.output.approvals),
                    ]
                    for kind, calls in groups:
                        for call in calls:
                            pending[call.tool_call_id] = PendingCall(
                                tool_call_id=call.tool_call_id,
                                ticket_id=result.output.metadata[call.tool_call_id][
                                    "ticket_id"
                                ],
                                kind=kind,
                            )
                    status = "waiting_human"
                    break
                # A steer that arrived during the final model request still reaches
                # the child instead of being dropped with the finished run.
                if child.state.steer_buffer and not child.cancel_requested:
                    prompt, results = child.state.steer_buffer.pop(0), None
                    continue
                status, output = "completed", result.output
                break
        except RunCancelled as exc:
            if self.shutting_down and child.state.status == "waiting_human":
                # Shutdown keeps the last consistent state; the restored child
                # waits for the daemon to redeliver or interrupt its tickets.
                return
            messages = dump_messages(exc.all_messages())
            status = "cancelled"
        except Exception:
            status = "failed"
        finally:
            child.run = None
            child.task = None
            child.active = False
        child.state.messages = messages
        child.pending = pending
        child.state.status = status
        if status == "completed":
            child.state.result = {
                "status": "completed",
                "subagent_id": child.id,
                "output": output,
            }
        elif status == "failed":
            child.state.result = {
                "status": "failed",
                "subagent_id": child.id,
                "message": "Subagent run failed",
            }
        elif status == "cancelled":
            child.state.result = {"status": "cancelled", "subagent_id": child.id}
        await self.event(SubagentLifecycle(subagent_id=child.id, status=status))
        await self.child_stopped(child)

    async def child_stopped(self, child: Child) -> None:
        if child.waiter is not None and not child.waiter.done():
            child.waiter.set_result(child.state.status)
            return
        if self.shutting_down or self.fatal:
            return
        if child.state.status in TERMINAL:
            for item in self.pending_delegates.values():
                if item.subagent_id == child.id:
                    item.result = child.state.result
        elif child.pending:
            await self.connection.send(
                Deferred(calls=[item.wire() for item in child.pending.values()])
            )
        if self.run_task is not None:
            return
        # Persist the child's outcome before the parent consumes it, so a crash
        # during the parent's resumption never replays the child's run.
        await self.snapshot()
        if self.resume_if_ready():
            return
        if not self.children_active() and not self.start_next(allow_steer=True):
            await self.connection.send(Settled())

    async def cancel_subagent(self, subagent_id: str) -> None:
        child = self.children.get(subagent_id)
        if child is None or child.state.status in TERMINAL:
            return
        if child.active:
            child.request_cancel()
            return
        child.pending.clear()
        child.state.status = "cancelled"
        child.state.result = {"status": "cancelled", "subagent_id": child.id}
        await self.event(SubagentLifecycle(subagent_id=child.id, status="cancelled"))
        await self.child_stopped(child)

    async def steer_subagent(self, message: SubagentSteer) -> None:
        if len(message.text.encode("utf-8")) > MAX_USER_TEXT_BYTES:
            await self.error("bad_request", "Message text exceeds 1 MiB")
            return
        child = self.children.get(message.subagent_id)
        if child is None or child.state.status in TERMINAL:
            return
        child.state.steer_buffer.append(message.text)
        child.steer_received.set()
        await self.event(
            SubagentProgress(subagent_id=child.id, progress={"kind": "steer_queued"})
        )
        if not child.active and self.run_task is None:
            await self.snapshot()

    async def discard_deferred(self) -> None:
        """Drop a restored continuation whose terminal output is already durable."""
        if self.run_task is not None:
            return
        self.pending.clear()
        self.pending_delegates.clear()
        for child in self.children.values():
            if child.state.status not in TERMINAL:
                child.pending.clear()
                child.state.status = "cancelled"
                child.state.result = {"status": "cancelled", "subagent_id": child.id}
        self.deferred_messages = []
        # The unanswered batch cannot be replayed to a model; keep the answered prefix.
        kept = fork_history(self.messages)
        dropped = len(kept) != len(self.messages)
        self.messages = kept
        if dropped:
            # The model loses those messages like a compaction would: the daemon must
            # stop honoring tool-result bases that only existed in them.
            await self.compaction_reporter(None)()
        await self.snapshot()
        self.start_next(allow_steer=True)

    async def stop_children(self) -> None:
        tasks = [
            child.task
            for child in self.children.values()
            if child.active and child.task is not None
        ]
        for child in self.children.values():
            if child.active:
                child.request_cancel()
        if tasks:
            await asyncio.wait(tasks)

    async def abort(self) -> None:
        if self.run_task is not None:
            self.abort_requested = True
            if self.run is not None:
                self.run.cancel()
            await self.run_task

    async def serve(self) -> None:
        try:
            while not self.fatal:
                message = await self.connection.receive()
                if message is None:
                    break
                if isinstance(message, Error) and message.fatal:
                    self.fatal = True
                    break
                if isinstance(message, Shutdown):
                    self.shutting_down = True
                    await self.abort()
                    await self.stop_children()
                    if self.spec is not None:
                        await self.snapshot()
                    await self.connection.send(Bye())
                    break
                if isinstance(message, Open):
                    if self.spec is not None:
                        await self.error("bad_request", "Session is already open")
                        continue
                    try:
                        await self.open(message)
                    except providers.AuthUnavailable:
                        await self.error(
                            "auth_unavailable",
                            "Model credentials unavailable; "
                            "check credential configuration",
                            fatal=True,
                        )
                    except Exception:
                        await self.error(
                            "bad_request",
                            "Unable to open session; check configuration and snapshot",
                            fatal=True,
                        )
                    continue
                if isinstance(message, Error):
                    continue
                if isinstance(message, Abort):
                    await self.abort()
                    continue
                if self.agent is None:
                    await self.error("bad_request", "Open a session first")
                elif isinstance(message, UserMessage):
                    if len(message.text.encode("utf-8")) > MAX_USER_TEXT_BYTES:
                        await self.connection.send(
                            Error(
                                code="bad_request",
                                message="Message text exceeds 1 MiB",
                                fatal=False,
                                message_id=message.message_id,
                            )
                        )
                        continue
                    if self.run_task is None and not self.waiting():
                        self.start(message)
                    elif message.mode == "queue":
                        self.queue.append(message)
                    else:
                        self.steer_buffer.append(message)
                        self.steer_received.set()
                elif isinstance(message, ToolResult):
                    future = self.rpc.get(message.rpc_id)
                    if message.rpc_id in self.abandoned:
                        del self.abandoned[message.rpc_id]
                    elif future is None or future.done():
                        await self.error("bad_request", "Unknown reverse RPC response")
                    else:
                        future.set_result(message.outcome)
                elif isinstance(message, SubagentCancel):
                    await self.cancel_subagent(message.subagent_id)
                elif isinstance(message, SubagentSteer):
                    await self.steer_subagent(message)
                elif isinstance(message, DiscardDeferred):
                    await self.discard_deferred()
                else:
                    await self.resolve(message)
        finally:
            self.shutting_down = True
            try:
                await self.abort()
                await self.stop_children()
            finally:
                try:
                    await self.stack.aclose()
                finally:
                    await self.connection.close()
