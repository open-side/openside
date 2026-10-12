"""`/name` skill mentions: progressive disclosure without a model round trip.

The daemon lists the enabled skills in a system-prompt block and owns the `skill`
tool that returns a skill's instructions. When a user message mentions `/name` for
a skill in that list, the sidecar calls the same daemon tool *before* the model
request and puts the call and its result into the history. The model starts with
the instructions loaded and the session transcript shows the load. A skill that is
no longer loadable (disabled or removed since the session opened) is skipped
silently: the daemon already said no.
"""

from __future__ import annotations

import re
from collections.abc import Awaitable, Callable, Iterable
from typing import Any
from uuid import uuid4

from pydantic_ai.messages import (
    ModelMessage,
    ModelRequest,
    ModelResponse,
    ToolCallPart,
    ToolReturnPart,
)

from openside_agent.protocol import Done, ToolOutcome

SKILL_TOOL = "skill"
# Must match `INDEX_OPEN`/`INDEX_CLOSE` in `crates/opensided/src/skills.rs`.
INDEX_OPEN = "<available_skills>"
INDEX_CLOSE = "</available_skills>"
MAX_MENTIONS = 5

_ENTRY = re.compile(r"^- ([a-z0-9]+(?:-[a-z0-9]+)*):")
# A mention starts a word and ends before whitespace or sentence punctuation; a
# following `/` or `.name` means it is a path or file name, not a mention.
_MENTION = re.compile(
    r"(?:^|(?<=[\s(\[{\"']))/([a-z0-9]+(?:-[a-z0-9]+)*)(?=$|[\s)\]}\"',;!?]|[.:](?:$|\s))"
)

ReverseCall = Callable[[str, str, dict[str, Any], str | None], Awaitable[ToolOutcome]]


def index_names(prompt_parts: Iterable[str]) -> list[str]:
    """Skill names listed in the daemon's `<available_skills>` block."""
    names: list[str] = []
    for part in prompt_parts:
        start = part.find(INDEX_OPEN)
        end = part.find(INDEX_CLOSE)
        if start < 0 or end < start:
            continue
        for line in part[start + len(INDEX_OPEN) : end].splitlines():
            entry = _ENTRY.match(line.strip())
            if entry and entry.group(1) not in names:
                names.append(entry.group(1))
    return names


def mentioned(text: str, names: Iterable[str]) -> list[str]:
    """Known skill names mentioned as `/name`, in order, without repeats."""
    known = set(names)
    found: list[str] = []
    for match in _MENTION.finditer(text):
        name = match.group(1)
        if name in known and name not in found:
            found.append(name)
    return found[:MAX_MENTIONS]


async def load_mentions(
    call: ReverseCall,
    names: Iterable[str],
    texts: Iterable[str],
    *,
    run_id: str | None = None,
    conversation_id: str | None = None,
) -> list[ModelMessage]:
    """History messages that load every skill mentioned in `texts`: one assistant turn
    with the `skill` tool calls, then one request with their results."""
    wanted: list[str] = []
    for text in texts:
        for name in mentioned(text, names):
            if name not in wanted:
                wanted.append(name)
    calls: list[ToolCallPart] = []
    returns: list[ToolReturnPart] = []
    for name in wanted[:MAX_MENTIONS]:
        call_id = f"skill-mention-{uuid4().hex[:12]}"
        outcome = await call(call_id, SKILL_TOOL, {"name": name}, None)
        if not isinstance(outcome, Done):
            continue
        calls.append(
            ToolCallPart(
                tool_name=SKILL_TOOL, args={"name": name}, tool_call_id=call_id
            )
        )
        returns.append(
            ToolReturnPart(
                tool_name=SKILL_TOOL, content=outcome.for_model, tool_call_id=call_id
            )
        )
    if not calls:
        return []
    return [
        ModelResponse(parts=calls, run_id=run_id, conversation_id=conversation_id),
        ModelRequest(parts=returns, run_id=run_id, conversation_id=conversation_id),
    ]
