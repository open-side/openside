"""Daemon-owned tool schemas and reverse RPC; no local execution tools.

Tool.from_schema intentionally skips Python-side schema validation (installed
pydantic_ai/tools.py:491-523): the daemon validates its own canonical schema.
"""

from collections.abc import Awaitable, Callable, Mapping
from typing import Any, cast

from pydantic_ai import ApprovalRequired, CallDeferred, ModelRetry, RunContext, Tool
from pydantic_ai.messages import ToolReturn
from pydantic_ai.toolsets import FunctionToolset

from openside_agent.content import user_content
from openside_agent.protocol import (
    DeferredOutcome,
    Denied,
    Done,
    ToolError,
    ToolOutcome,
    ToolSpec,
)

ReverseCall = Callable[[str, str, dict[str, Any], str | None], Awaitable[ToolOutcome]]
# Runs after the daemon admitted a call (policy, audit) with its `for_model` value.
Continuation = Callable[[RunContext[None], dict[str, Any], Any], Awaitable[Any]]


def proxy_toolset(
    specs: list[ToolSpec],
    call: ReverseCall,
    *,
    retries: int = 3,
    continuations: Mapping[str, Continuation] | None = None,
) -> FunctionToolset[None]:
    toolset = FunctionToolset[None](max_retries=retries, id="daemon-proxy")
    after = continuations or {}

    def make_proxy(name: str) -> Callable[..., Awaitable[Any]]:
        async def proxy(ctx: RunContext[None], **args: Any) -> Any:
            if ctx.tool_call_id is None:
                raise RuntimeError("Tool call context has no call identifier")
            ticket: str | None = None
            if ctx.tool_call_approved:
                metadata: Any = ctx.tool_call_metadata
                if isinstance(metadata, dict):
                    value: Any = cast(dict[str, Any], metadata).get("ticket_id")
                    if isinstance(value, str):
                        ticket = value
            outcome = await call(ctx.tool_call_id, name, args, ticket)
            if isinstance(outcome, Done):
                continuation = after.get(name)
                value = (
                    await continuation(ctx, args, outcome.for_model)
                    if continuation is not None
                    else outcome.for_model
                )
                if outcome.parts:
                    # Binary results (e.g. screenshots) reach vision models as user
                    # content next to the JSON return value.
                    return ToolReturn(
                        return_value=value, content=user_content(outcome.parts)
                    )
                return value
            if isinstance(outcome, DeferredOutcome):
                metadata = {"ticket_id": outcome.ticket_id}
                if outcome.kind == "approval":
                    raise ApprovalRequired(metadata=metadata)
                raise CallDeferred(metadata=metadata)
            if isinstance(outcome, Denied):
                reason = outcome.reason
                if outcome.hint:
                    reason += "\n" + outcome.hint
                raise ModelRetry(reason)
            assert isinstance(outcome, ToolError)
            raise ModelRetry(outcome.message)

        return proxy

    for spec in specs:
        toolset.add_tool(
            Tool.from_schema(
                make_proxy(spec.name),
                name=spec.name,
                description=spec.description,
                json_schema=spec.parameters,
                takes_ctx=True,
            )
        )
    return toolset
