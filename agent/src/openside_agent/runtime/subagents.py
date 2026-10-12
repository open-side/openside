"""Sidecar-owned child runs for the daemon-admitted `delegate` tool.

The harness `SubAgents` capability cannot surface a child's deferred calls to its
parent (docs/spikes/S-9-harness-maturity.md), so children are plain agents owned by
the session: their tickets belong to the parent session, the parent `delegate` call
stays deferred until the child is terminal, and both conversations live in the
same run-end snapshot (ADR-020).
"""

from __future__ import annotations

import asyncio
from typing import TYPE_CHECKING, Any, Literal

from pydantic import BaseModel, ConfigDict
from pydantic_ai import Agent, DeferredToolRequests, RunContext
from pydantic_ai.capabilities import AbstractCapability
from pydantic_ai.messages import (
    ModelRequest,
    ModelResponse,
    TextPart,
    ToolCallPart,
    UserPromptPart,
)
from pydantic_ai.models import ModelRequestContext
from pydantic_ai.run import AgentRun

from openside_agent.protocol import SubagentProgress, Usage
from openside_agent.runtime.state import PendingCall, SubagentState
from openside_agent.scripted import ScriptedModel

if TYPE_CHECKING:
    from openside_agent.runtime import Session

SUBAGENT_INSTRUCTIONS = (
    "You are a subagent working on one delegated task. Use only the tools you are "
    "given and finish with a concise result for the agent that delegated the task."
)
PROGRESS_TEXT_LIMIT = 500


class Admission(BaseModel):
    """The daemon's `delegate` tool result: the admitted child configuration."""

    model_config = ConfigDict(extra="ignore")
    subagent_id: str
    model: str
    tools: list[str]
    context: Literal["fresh", "fork"] = "fresh"


class ChildGateError(Exception):
    pass


class Child:
    def __init__(self, state: SubagentState):
        self.state = state
        self.pending: dict[str, PendingCall] = {
            item.tool_call_id: item for item in state.pending_deferred
        }
        self.agent: Agent[None, str | DeferredToolRequests] | None = None
        self.scripted: ScriptedModel | None = None
        self.task: asyncio.Task[None] | None = None
        self.run: AgentRun[None, str | DeferredToolRequests] | None = None
        # Cleared before a run's terminal callbacks, so a parent finishing at the
        # same moment never mistakes a stopping child for active work.
        self.active = False
        self.cancel_requested = False
        self.steer_received = asyncio.Event()
        # Set while the delegating parent tool call awaits this child's first stop.
        self.waiter: asyncio.Future[str] | None = None

    @property
    def id(self) -> str:
        return self.state.id

    def capture(self) -> SubagentState:
        cursor = (
            self.scripted.cursor
            if self.scripted is not None
            else self.state.script_cursor
        )
        return self.state.model_copy(
            update={
                "pending_deferred": list(self.pending.values()),
                "script_cursor": cursor,
            },
            deep=True,
        )

    def request_cancel(self) -> None:
        self.cancel_requested = True
        if self.run is not None:
            self.run.cancel()


class ChildHooks(AbstractCapability[None]):
    def __init__(self, session: Session, child: Child):
        self.session = session
        self.child = child

    async def before_model_request(
        self, ctx: RunContext[None], request_context: ModelRequestContext
    ) -> ModelRequestContext:
        child = self.child
        declared = {
            tool.name
            for tool in request_context.model_request_parameters.declared_function_tools
        }
        if declared != set(child.state.tools):
            raise ChildGateError()
        while child.state.steer_buffer:
            text = child.state.steer_buffer.pop(0)
            injected = ModelRequest(
                parts=[UserPromptPart(text)],
                run_id=ctx.run_id,
                conversation_id=ctx.conversation_id,
            )
            ctx.messages.append(injected)
            request_context.messages = [*request_context.messages, injected]
            await self.session.event(
                SubagentProgress(
                    subagent_id=child.id, progress={"kind": "steer_injected"}
                )
            )
        child.steer_received.clear()
        await self.session.event(
            SubagentProgress(subagent_id=child.id, progress={"kind": "model_request"})
        )
        return request_context

    async def after_model_request(
        self,
        ctx: RunContext[None],
        *,
        request_context: ModelRequestContext,
        response: ModelResponse,
    ) -> ModelResponse:
        progress: dict[str, Any] = {
            "kind": "model_response",
            "tool_calls": [
                part.tool_name
                for part in response.parts
                if isinstance(part, ToolCallPart)
            ],
        }
        text = "".join(
            part.content for part in response.parts if isinstance(part, TextPart)
        )
        if text:
            progress["text"] = text[:PROGRESS_TEXT_LIMIT]
        await self.session.event(
            Usage(
                model=response.model_name or request_context.model.model_name,
                input_tokens=response.usage.input_tokens,
                output_tokens=response.usage.output_tokens,
            )
        )
        await self.session.event(
            SubagentProgress(subagent_id=self.child.id, progress=progress)
        )
        return response
