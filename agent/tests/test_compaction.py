"""A-3: the task goal stated at turn 1 survives compaction of a 200-turn session."""

from pydantic_ai.messages import (
    ModelMessage,
    ModelRequest,
    ModelResponse,
    SystemPromptPart,
    TextPart,
    UserPromptPart,
)
from pydantic_ai.models.function import AgentInfo, FunctionModel
from pydantic_ai.models.test import TestModel
from runtime_helpers import fake_daemon

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
        await daemon.open()
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
