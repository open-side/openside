"""Parent-context copy for forked subagents (acceptance A-5 ⑤, PP-24 ③)."""

from __future__ import annotations

import copy
from collections.abc import Sequence

from pydantic_ai.messages import (
    ModelMessage,
    ModelRequest,
    ModelResponse,
    RetryPromptPart,
    ToolCallPart,
    ToolReturnPart,
)


def answer_id(part: object) -> str | None:
    """Tool call answered by a request part, if any."""
    if isinstance(part, ToolReturnPart):
        return part.tool_call_id
    # A retry without a tool name answers output validation, not a tool call.
    if isinstance(part, RetryPromptPart) and part.tool_name is not None:
        return part.tool_call_id
    return None


def fork_history(messages: Sequence[ModelMessage]) -> list[ModelMessage]:
    """Copy history up to the first tool-call batch lacking a return for every call.

    The delegating call itself is always such a batch: its return is the child's
    result, so a child must never inherit it. Returns without a kept call are
    dropped as well, so a provider never sees an orphaned tool result.
    """
    cut = len(messages)
    # Truncating can strand an earlier call whose only return came after the cut,
    # so shrink until every kept call is answered inside the kept prefix.
    while True:
        answered: set[str] = set()
        incomplete: int | None = None
        for index in range(cut - 1, -1, -1):
            message = messages[index]
            if isinstance(message, ModelRequest):
                answered.update(
                    answer
                    for part in message.parts
                    if (answer := answer_id(part)) is not None
                )
            elif any(
                isinstance(part, ToolCallPart) and part.tool_call_id not in answered
                for part in message.parts
            ):
                incomplete = index
        if incomplete is None:
            break
        cut = incomplete
    issued: set[str] = set()
    kept: list[ModelMessage] = []
    for message in messages[:cut]:
        if isinstance(message, ModelResponse):
            issued.update(
                part.tool_call_id
                for part in message.parts
                if isinstance(part, ToolCallPart)
            )
            kept.append(copy.deepcopy(message))
            continue
        request = copy.deepcopy(message)
        request.parts = [
            part
            for part in request.parts
            if (answer := answer_id(part)) is None or answer in issued
        ]
        if request.parts:
            kept.append(request)
    return kept
