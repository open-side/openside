"""A-3: the task goal stated at turn 1 survives compaction of a 200-turn session."""

import asyncio
from unittest.mock import AsyncMock

import pytest
from pydantic_ai import RunContext
from pydantic_ai.messages import (
    BinaryContent,
    ModelMessage,
    ModelRequest,
    ModelResponse,
    SystemPromptPart,
    TextPart,
    ToolCallPart,
    ToolReturnPart,
    UserPromptPart,
)
from pydantic_ai.models.function import AgentInfo, FunctionModel
from pydantic_ai.models.test import TestModel
from pydantic_ai.usage import RunUsage
from runtime_helpers import fake_daemon

from openside_agent.runtime.compaction import (
    IdleCompaction,
    ReportingCompaction,
    estimated_tokens,
)

GOAL = (
    "GOAL: migrate the billing database to region eu-west-3 before 2026-11-30; "
    "change ticket OPS-4471; never touch the invoices table."
)
FACTS = ("eu-west-3", "2026-11-30", "OPS-4471", "invoices table")
RECALL = "Remind me: what is the task goal, the deadline and the ticket?"
TURNS = 200
SUMMARY_PREFIX = "Summary of previous conversation:"


def user_texts(messages: list[ModelMessage]) -> list[str]:
    texts: list[str] = []
    for message in messages:
        if isinstance(message, ModelRequest):
            for part in message.parts:
                if isinstance(part, UserPromptPart):
                    content = part.content
                    texts.append(
                        content
                        if isinstance(content, str)
                        else " ".join(str(item) for item in content)
                    )
    return texts


class ScriptedRecall:
    """Main turns stream a reply; the summarizer gets a deliberately lossy summary."""

    def __init__(self) -> None:
        self.summaries = 0
        self.contexts: list[list[ModelMessage]] = []
        self.model = FunctionModel(self.summarize, stream_function=self.reply)

    async def summarize(
        self, messages: list[ModelMessage], info: AgentInfo
    ) -> ModelResponse:
        assert info.instructions and "summarization" in info.instructions
        self.summaries += 1
        # Drops the goal on purpose: only the pinned first message may carry it forward.
        return ModelResponse(parts=[TextPart("The user sent routine status updates.")])

    async def reply(self, messages: list[ModelMessage], info: AgentInfo):
        self.contexts.append(list(messages))
        texts = user_texts(messages)
        if texts and texts[-1] == RECALL:
            goal = next((text for text in texts if text.startswith("GOAL:")), None)
            yield f"The goal is: {goal}" if goal else "I no longer know the goal."
        else:
            yield "Noted."


async def test_goal_survives_compaction_of_a_200_turn_session():
    model = ScriptedRecall()
    async with fake_daemon(model=model.model) as (daemon, _):
        await daemon.open(agent={"compaction_max_messages": 100})
        for turn in range(1, TURNS + 1):
            if turn == 1:
                text = GOAL
            elif turn == TURNS:
                text = RECALL
            else:
                text = f"Status update {turn}: nothing new."
            await daemon.user(message_id=f"m{turn}", text=text)
            result = await daemon.until("message_result")
            assert result["message_id"] == f"m{turn}"
            assert result["outcome"] == "completed", result
            await daemon.until("settled")

        finals = [
            item["event"]["text"]
            for item in daemon.outputs
            if item["type"] == "event" and item["event"]["kind"] == "assistant.final"
        ]
        assert len(finals) == TURNS

    # 200 turns are 400 messages; 100/20 thresholds compact about every 40 turns.
    assert model.summaries >= 4, model.summaries
    # The daemon is told each time earlier messages left the model context, so it can
    # retire tool-result bases (browser snapshot ids) the model no longer sees.
    notices = [item for item in daemon.outputs if item["type"] == "context_compacted"]
    assert len(notices) == model.summaries, (len(notices), model.summaries)
    assert all("subagent" not in item for item in notices), notices
    compacted = [
        context
        for context in model.contexts
        if any(
            isinstance(part, SystemPromptPart)
            and part.content.startswith(SUMMARY_PREFIX)
            for message in context
            if isinstance(message, ModelRequest)
            for part in message.parts
        )
    ]
    assert len(compacted) > TURNS // 2
    assert max(len(context) for context in model.contexts) <= 101
    for context in compacted:
        assert GOAL in user_texts(context), (
            "goal missing from a post-compaction request"
        )
    summaries = [
        part.content
        for message in compacted[-1]
        if isinstance(message, ModelRequest)
        for part in message.parts
        if isinstance(part, SystemPromptPart)
    ]
    assert not any("eu-west-3" in summary for summary in summaries)

    recall = finals[-1]
    for fact in FACTS:
        assert fact in recall, recall


async def test_compaction_notice_names_the_agent_it_belongs_to():
    async with fake_daemon(model=TestModel()) as (daemon, session):
        await daemon.open()
        await session.compaction_reporter("sub-1")()
        await session.compaction_reporter(None)()
        child = await daemon.until("context_compacted")
        parent = await daemon.until("context_compacted")
    assert child["subagent"] == "sub-1"
    assert "subagent" not in parent


def context() -> RunContext[None]:
    return RunContext(deps=None, model=TestModel(), usage=RunUsage())


def large_history() -> list[ModelMessage]:
    return [
        ModelRequest(parts=[UserPromptPart("Keep the goal.")]),
        ModelResponse(parts=[ToolCallPart("probe", {}, "call")]),
        ModelRequest(parts=[ToolReturnPart("probe", "x" * 20000, "call")]),
        ModelResponse(parts=[TextPart("Evidence read.")]),
        ModelRequest(parts=[UserPromptPart("Continue.")]),
        ModelResponse(parts=[TextPart("Latest answer verbatim.")]),
    ]


async def test_large_single_result_compacts_and_keeps_final_messages(monkeypatch):
    messages = large_history()
    cap = ReportingCompaction(max_tokens=1000, keep_messages=20)
    summarize = AsyncMock(return_value="Evidence summarized.")
    monkeypatch.setattr(ReportingCompaction, "_summarize", summarize)
    assert cap.needed(messages)
    compacted = await cap.compact(messages, context())
    assert compacted[-2:] == messages[-2:]
    assert len(compacted) < len(messages)
    assert not any(
        isinstance(part, (ToolCallPart, ToolReturnPart))
        for message in compacted
        for part in message.parts
    )
    assert summarize.await_args is not None
    assert messages[2] in summarize.await_args.args[0]


async def test_tool_result_tail_keeps_its_matching_call(monkeypatch):
    messages = large_history()[:3]
    cap = ReportingCompaction(max_tokens=1000, keep_messages=2)
    monkeypatch.setattr(
        ReportingCompaction, "_summarize", AsyncMock(return_value="Goal summary.")
    )
    compacted = await cap.compact(messages, context())
    assert messages[1] in compacted and messages[2] in compacted


def test_short_hundred_message_chat_does_not_trigger_default_budget():
    messages: list[ModelMessage] = [
        ModelRequest(parts=[UserPromptPart("Hello")]) for _ in range(100)
    ]
    assert not ReportingCompaction(max_tokens=64000).needed(messages)
    assert estimated_tokens(large_history()) > 1000


def test_large_screenshots_use_a_bounded_media_estimate():
    def history(payload: bytes) -> list[ModelMessage]:
        return [
            ModelRequest(
                parts=[
                    UserPromptPart(
                        [
                            "Inspect this page",
                            BinaryContent(payload, media_type="image/png"),
                        ]
                    )
                ]
            )
            for _ in range(5)
        ]

    small = history(b"small")
    large = history(b"x" * 2_000_000)
    assert estimated_tokens(large) == estimated_tokens(small)
    assert estimated_tokens(large) >= 5 * 1600
    assert not ReportingCompaction(max_tokens=64000).needed(large)
    assert ReportingCompaction(max_tokens=64000).needed(
        [*large, ModelRequest(parts=[UserPromptPart("x" * 200_000)])]
    )


def test_replayed_instructions_count_only_the_current_block():
    messages = [
        ModelRequest(parts=[UserPromptPart("Hello")], instructions="policy " * 2000)
        for _ in range(100)
    ]
    current_only = [
        ModelRequest(
            parts=message.parts,
            instructions=message.instructions if index == 99 else None,
        )
        for index, message in enumerate(messages)
    ]
    assert estimated_tokens(messages) == estimated_tokens(current_only)
    assert not ReportingCompaction(max_tokens=64000).needed(messages)
    messages[-1].instructions = "x" * 200_000
    assert ReportingCompaction(max_tokens=64000).needed(messages)


@pytest.mark.parametrize("change", ["owner", "wake", "epoch", "history"])
async def test_idle_summary_never_commits_stale_history(change, monkeypatch):
    messages = large_history()
    cap = ReportingCompaction(max_tokens=1000)
    cap.last_context = context()
    started, release = asyncio.Event(), asyncio.Event()

    async def summarize(_self, _messages):
        started.set()
        try:
            await release.wait()
        except asyncio.CancelledError:
            # A provider may finish even after its request was cancelled.
            await release.wait()
        return [ModelRequest(parts=[UserPromptPart("summary")])]

    monkeypatch.setattr(ReportingCompaction, "idle_summary", summarize)
    controller = IdleCompaction()
    epoch, wake = "epoch-1", False
    commit = AsyncMock()
    controller.schedule(cap, messages, epoch, lambda: (epoch, messages, wake), commit)
    task = controller.task
    await asyncio.wait_for(started.wait(), 2)
    if change == "owner":
        controller.cancel()
    elif change == "wake":
        wake = True
    elif change == "epoch":
        epoch = "epoch-2"
    else:
        messages.append(ModelRequest(parts=[UserPromptPart("new owner message")]))
    release.set()
    assert task is not None
    await task
    commit.assert_not_awaited()


async def test_idle_summary_commits_once_and_skips_imminent_wake(monkeypatch):
    messages = large_history()
    cap = ReportingCompaction(max_tokens=1000)
    cap.last_context = context()
    summary = [messages[0], messages[-1]]
    monkeypatch.setattr(
        ReportingCompaction, "idle_summary", AsyncMock(return_value=summary)
    )
    controller = IdleCompaction()
    commit = AsyncMock()
    controller.schedule(
        cap, messages, "epoch", lambda: ("epoch", messages, True), commit
    )
    assert controller.task is None
    controller.schedule(
        cap, messages, "epoch", lambda: ("epoch", messages, False), commit
    )
    assert controller.task is not None
    await controller.task
    commit.assert_awaited_once_with(summary)


async def test_session_idle_commit_invalidates_bases_and_snapshots(monkeypatch):
    async with fake_daemon(model=TestModel()) as (daemon, session):
        await daemon.open(agent={"compaction_max_tokens": 1000})
        await daemon.user()
        await daemon.until("settled")
        session.messages = large_history()
        monkeypatch.setattr(
            ReportingCompaction,
            "idle_summary",
            AsyncMock(return_value=[session.messages[0], session.messages[-1]]),
        )
        session.schedule_idle_compaction()
        await daemon.until("context_compacted")
        await daemon.until("state_snapshot")
        assert len(session.messages) == 2


async def test_session_owner_input_cancels_idle_summary(monkeypatch):
    started, cancelled = asyncio.Event(), asyncio.Event()

    async def summarize(_self, _messages):
        started.set()
        try:
            await asyncio.Event().wait()
        except asyncio.CancelledError:
            cancelled.set()
            raise

    async with fake_daemon(model=TestModel()) as (daemon, session):
        await daemon.open(agent={"compaction_max_tokens": 1000})
        await daemon.user()
        await daemon.until("settled")
        session.messages = large_history()
        monkeypatch.setattr(ReportingCompaction, "idle_summary", summarize)
        session.schedule_idle_compaction()
        await asyncio.wait_for(started.wait(), 2)
        # A larger runtime budget avoids a foreground summary in this cancellation test.
        assert session.compaction is not None
        session.compaction.max_tokens = 64000
        await daemon.user(message_id="owner-update", text="Owner update")
        await daemon.until("settled")
        await asyncio.wait_for(cancelled.wait(), 2)
        assert "Owner update" in user_texts(session.messages)
        assert not any(item["type"] == "context_compacted" for item in daemon.outputs)
