"""Ultrabrowse research mode (A-9, docs/specs/ultrabrowse.md).

The daemon narrows tools and prompt for the `ultrabrowse` profile; this module adds what
only the agent loop can do:

- higher reasoning: provider-neutral `ModelSettings(thinking=...)`;
- a planning step: browser tools are refused until the current turn states a `Plan:`,
  and that plan is re-injected on every later request through harness `Planning`;
- enforced citations: an `OutputGuardrail` sends answers with uncited claims back to
  the model, then annotates them with `[citation needed]` once retries are spent.
"""

from __future__ import annotations

import re
from collections.abc import Sequence
from typing import Any

from pydantic_ai import ModelRetry, RunContext
from pydantic_ai.capabilities import AbstractCapability, WrapModelRequestHandler
from pydantic_ai.messages import (
    ModelMessage,
    ModelRequest,
    ModelResponse,
    TextPart,
    ToolCallPart,
    ToolReturnPart,
    UserPromptPart,
)
from pydantic_ai.models import ModelRequestContext
from pydantic_ai.settings import ModelSettings
from pydantic_ai.tools import ToolDefinition
from pydantic_ai_harness.guardrails import GuardrailResult, OutputGuardrail
from pydantic_ai_harness.planning import InMemoryPlanStore, PlanItem, Planning

from openside_agent import citations

ULTRABROWSE = "ultrabrowse"
# Thinking effort per profile; other profiles keep the provider's default behavior.
THINKING: dict[str, Any] = {ULTRABROWSE: "high"}
# Results whose `url` field is the page the browser tool loaded or acted on.
VISITING_TOOLS = frozenset(
    {"browser_open_tab", "browser_navigate", "browser_act", "browser_repl"}
)
CITATION_RETRIES = 2
PLAN_REQUIRED = (
    "Before using browser tools, write a line `Plan:` followed by numbered research "
    "steps, then browse."
)
_PLAN_HEADING = re.compile(
    r"^\s*(?:#{1,6}\s*)?\**plan\**\s*:?\**\s*(.*)$", re.IGNORECASE
)
_PLAN_ITEM = re.compile(r"^\s*(?:\d+[.)]|[-*+])\s+(.+)$")


def is_research(profile: object) -> bool:
    return profile == ULTRABROWSE


def model_settings(profile: object) -> ModelSettings | None:
    thinking = THINKING.get(profile) if isinstance(profile, str) else None
    return ModelSettings(thinking=thinking) if thinking is not None else None


def current_turn(messages: Sequence[ModelMessage]) -> Sequence[ModelMessage]:
    """Messages since the latest user prompt (a new or steered task)."""
    for index in range(len(messages) - 1, -1, -1):
        message = messages[index]
        if isinstance(message, ModelRequest) and any(
            isinstance(part, UserPromptPart) for part in message.parts
        ):
            return messages[index + 1 :]
    return messages


def parse_plan(text: str) -> list[str]:
    """Steps of the last `Plan:` block in `text`."""
    steps: list[str] = []
    collecting = False
    for line in text.splitlines():
        heading = _PLAN_HEADING.match(line)
        if heading:
            steps = [heading.group(1).strip()] if heading.group(1).strip() else []
            collecting = True
            continue
        if not collecting:
            continue
        item = _PLAN_ITEM.match(line)
        if item:
            steps.append(item.group(1).strip())
        elif line.strip():
            collecting = False
    return steps


def plan_steps(messages: Sequence[ModelMessage]) -> list[str]:
    """The plan the model stated in the current turn; empty when it has not planned."""
    steps: list[str] = []
    for message in current_turn(messages):
        if isinstance(message, ModelResponse):
            for part in message.parts:
                if isinstance(part, TextPart):
                    steps = parse_plan(part.content) or steps
    return steps


def visited_sources(messages: Sequence[ModelMessage]) -> set[str]:
    """Pages the daemon's browser tools reported loading in this conversation."""
    found: set[str] = set()
    for message in messages:
        if isinstance(message, ModelRequest):
            for part in message.parts:
                if (
                    isinstance(part, ToolReturnPart)
                    and part.tool_name in VISITING_TOOLS
                ):
                    found |= citations.result_urls(part.content)
    return found


def retry_message(report: citations.Report, visited: set[str]) -> str:
    claims = "\n".join(f"- {claim.text}" for claim in report.uncited)
    pages = "\n".join(f"- {url}" for url in sorted(visited)) or "- (none yet)"
    return (
        "These factual sentences have no link to a page you opened with a browser "
        "tool:\n"
        f"{claims}\n"
        "Pages opened in this session:\n"
        f"{pages}\n"
        "Rewrite the answer: cite each fact with a Markdown link to one of these "
        "pages, browse for a source first, or drop the fact or mark it "
        f"{citations.MARKER}."
    )


def citation_guard(ctx: RunContext[None], output: object) -> GuardrailResult:
    if not isinstance(output, str):
        return GuardrailResult.allow()
    visited = visited_sources(ctx.messages)
    report = citations.check(output, visited)
    if not report.uncited:
        return GuardrailResult.allow()
    if ctx.retry < min(CITATION_RETRIES, ctx.max_retries):
        return GuardrailResult.retry(retry_message(report, visited))
    # Retries are spent: deliver the answer, visibly marking what stayed unsupported.
    return GuardrailResult.replace(citations.annotate(output, report))


class PlanFirst(AbstractCapability[None]):
    """Requires a stated plan before browsing and feeds it to the shared plan store.

    The plan is re-derived from history on every request, so it survives sidecar
    restarts and deferred resumes without its own persistence.
    """

    def __init__(self, store: InMemoryPlanStore) -> None:
        self.store = store

    async def wrap_model_request(
        self,
        ctx: RunContext[None],
        *,
        request_context: ModelRequestContext,
        handler: WrapModelRequestHandler,
    ) -> ModelResponse:
        # Listed before Planning, so this wrap runs first and the store is current when
        # Planning reads it for its reminder.
        steps = plan_steps(request_context.messages)
        await self.store.set_items([PlanItem(content=step) for step in steps])
        return await handler(request_context)

    async def before_tool_execute(
        self,
        ctx: RunContext[None],
        *,
        call: ToolCallPart,
        tool_def: ToolDefinition,
        args: Any,
    ) -> Any:
        if call.tool_name.startswith("browser_") and not plan_steps(ctx.messages):
            raise ModelRetry(PLAN_REQUIRED)
        return args


def capabilities(profile: object) -> list[AbstractCapability[None]]:
    if not is_research(profile):
        return []
    store = InMemoryPlanStore()
    return [
        PlanFirst(store),
        # tools=() keeps the plan tools off the model's tool list (daemon tools only);
        # inject=True appends the current plan to each request without persisting it.
        Planning[None](tools=(), inject=True, store=store),
        OutputGuardrail[None](guard=citation_guard),
    ]


def holds_final_text(profile: object) -> bool:
    """Whether text deltas wait until the response proves not to be the final answer.

    A final answer can be sent back by the citation guard, so ultrabrowse only streams
    text that accompanies tool calls; the answer arrives once, as assistant.final.
    """
    return is_research(profile)
