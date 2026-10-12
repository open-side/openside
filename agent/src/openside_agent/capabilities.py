"""Tool-free harness adoption from docs/spikes/S-9-harness-maturity.md.

Compaction uses the session model; injection defense is local pattern-only.
Repair and cache warnings add no tools. Spend is observation-only, with its
get_spend tool explicitly disabled (H/spend/_capability.py:271-277). The input
guardrail redacts vendor keys and private keys from the user prompt before it reaches
the provider (deterministic, not parallel). System reminders re-anchor long runs on the
current request. Planning is profile-specific (openside_agent.research) because it
needs a plan source.

The upstream Subagents capability cannot propagate child deferred results to
parent suspension (S-9 §1); runtime.subagents replaces it (ADR-020).
Skills, memory, ask_user, tool_output_limits and code_mode add tools outside the
daemon's schema gate. Step persistence duplicates daemon storage and omits
pending deferred state. Shell, filesystem, playwright, browser_use and bubblewrap
execute outside daemon policy. Background tools turn deferred exceptions into
failure strings. None of those capabilities are registered here.

Session.open compares the tool names every toolset exposes with the daemon's list and
refuses the session on any difference; that is the gate for capability-added tools.
"""

from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
from typing import Any

from pydantic_ai import RunContext
from pydantic_ai.capabilities import AbstractCapability
from pydantic_ai.messages import ModelMessage, ModelRequest, UserPromptPart
from pydantic_ai_harness.compaction import SummarizingCompaction
from pydantic_ai_harness.guardrails import InputGuardrail
from pydantic_ai_harness.guardrails.detectors import redact_secrets
from pydantic_ai_harness.prompt_injection_defender import PromptInjectionDefender
from pydantic_ai_harness.repair_tool_arguments import RepairToolArguments
from pydantic_ai_harness.spend import SpendLimits
from pydantic_ai_harness.system_reminders import SystemReminders
from pydantic_ai_harness.warn_on_cache_busts import WarnOnCacheBusts


@dataclass
class ReportingCompaction(SummarizingCompaction[None]):
    """SummarizingCompaction that tells the daemon when it dropped earlier messages.

    Daemon tools can hand the model a base it later refers to by id (browser snapshot
    diffs); once compaction summarizes that result away the daemon must stop honoring it
    afterwards.
    """

    on_compaction: Callable[[], Awaitable[None]] | None = field(
        default=None, kw_only=True
    )

    async def compact(
        self, messages: list[ModelMessage], ctx: RunContext[None]
    ) -> list[ModelMessage]:
        compacted = await super().compact(messages, ctx)
        # `compact` returns its input object when nothing was old enough to summarize.
        if compacted is not messages and self.on_compaction is not None:
            await self.on_compaction()
        return compacted


REANCHOR_EVERY = 5
REANCHOR_MAX_CHARS = 500


def current_request_anchor(ctx: RunContext[None]) -> str | None:
    """Every REANCHOR_EVERY model requests in one run, restate the request being served.

    Harness GoalReanchor restates the conversation's first message, which in a
    long-lived session is usually an older, finished task.
    """
    if ctx.run_step == 0 or ctx.run_step % REANCHOR_EVERY:
        return None
    for message in reversed(ctx.messages):
        if isinstance(message, ModelRequest):
            for part in message.parts:
                if isinstance(part, UserPromptPart) and isinstance(part.content, str):
                    request = part.content[:REANCHOR_MAX_CHARS]
                    return (
                        "<system-reminder>\nThe request you are working on: "
                        f'"{request}". Check that your next step advances it; '
                        "if it is done, answer.\n</system-reminder>"
                    )
    return None


def adopted_capabilities(
    config: dict[str, Any],
    on_compaction: Callable[[], Awaitable[None]] | None = None,
) -> list[AbstractCapability[None]]:
    maximum: Any = config.get("compaction_max_messages", 100)
    keep: Any = config.get("compaction_keep_messages", 20)
    if type(maximum) is not int or type(keep) is not int or not 0 < keep < maximum:
        raise ValueError("Invalid compaction message thresholds")
    capabilities: list[AbstractCapability[None]] = [
        # The first user message states the task goal (A-3); keeping it verbatim means a
        # lossy summary cannot drop the goal. Covered by tests/test_compaction.py.
        ReportingCompaction(
            model=None,
            max_messages=maximum,
            keep_messages=keep,
            preserve_first_user_message=True,
            on_compaction=on_compaction,
        ),
        InputGuardrail[None](guard=redact_secrets, parallel=False),
        SystemReminders[None](dynamic_reminders=(current_request_anchor,)),
        PromptInjectionDefender[None](block_high_risk=True, semantic_detection=False),
        RepairToolArguments[None](),
        WarnOnCacheBusts[None](),
        SpendLimits[None](expose_tools=False),
    ]
    if any(capability.get_toolset() is not None for capability in capabilities):
        raise ValueError("Adopted harness capability unexpectedly exposes tools")
    return capabilities
