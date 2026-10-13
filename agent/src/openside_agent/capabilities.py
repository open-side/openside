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
from typing import Any

from pydantic_ai import RunContext
from pydantic_ai.capabilities import AbstractCapability
from pydantic_ai.messages import ModelRequest, UserPromptPart
from pydantic_ai_harness.guardrails import InputGuardrail
from pydantic_ai_harness.guardrails.detectors import redact_secrets
from pydantic_ai_harness.prompt_injection_defender import PromptInjectionDefender
from pydantic_ai_harness.repair_tool_arguments import RepairToolArguments
from pydantic_ai_harness.spend import SpendLimits
from pydantic_ai_harness.system_reminders import SystemReminders
from pydantic_ai_harness.warn_on_cache_busts import WarnOnCacheBusts

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
    from openside_agent.runtime.compaction import (
        DEFAULT_MAX_TOKENS,
        ReportingCompaction,
    )

    maximum: Any = config.get("compaction_max_messages")
    keep: Any = config.get("compaction_keep_messages", 20)
    tokens: Any = config.get("compaction_max_tokens", DEFAULT_MAX_TOKENS)
    if (
        type(keep) is not int
        or keep < 2
        or (maximum is not None and (type(maximum) is not int or maximum <= keep))
        or type(tokens) is not int
        or tokens <= 0
    ):
        raise ValueError("Invalid compaction thresholds")
    capabilities: list[AbstractCapability[None]] = [
        # The first user message states the task goal (A-3); keeping it verbatim means a
        # lossy summary cannot drop the goal. Covered by tests/test_compaction.py.
        ReportingCompaction(
            model=None,
            max_messages=maximum,
            keep_messages=keep,
            max_tokens=tokens,
            tool_return_max_chars=None,
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
