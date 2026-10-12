"""S-9 adoptions for every profile: input secret redaction and request re-anchoring."""

from pydantic_ai.messages import ModelMessage, ModelRequest, UserPromptPart
from pydantic_ai.models.function import AgentInfo, DeltaToolCall, FunctionModel
from runtime_helpers import fake_daemon, tool_spec

from openside_agent.capabilities import REANCHOR_EVERY

# Shaped like an AWS access key id; not a real credential.
FAKE_KEY = "AKIA" + "Q" * 16
TASK = f"Upload the report with key {FAKE_KEY} and summarize it."


def prompts(messages: list[ModelMessage]) -> list[str]:
    return [
        part.content
        if isinstance(part.content, str)
        else "".join(item for item in part.content if isinstance(item, str))
        for message in messages
        if isinstance(message, ModelRequest)
        for part in message.parts
        if isinstance(part, UserPromptPart)
    ]


class Looping:
    """Calls the probe tool `calls` times, then answers; records every request."""

    def __init__(self, calls: int) -> None:
        self.calls = calls
        self.requests: list[list[ModelMessage]] = []
        self.model = FunctionModel(stream_function=self.stream)

    async def stream(self, messages: list[ModelMessage], info: AgentInfo):
        self.requests.append(list(messages))
        if len(self.requests) <= self.calls:
            yield {
                0: DeltaToolCall(
                    name="probe", json_args="{}", tool_call_id=f"c{len(self.requests)}"
                )
            }
        else:
            yield "done"


async def run(calls: int) -> Looping:
    model = Looping(calls)
    async with fake_daemon(model=model.model) as (daemon, _):
        await daemon.open(tools=[tool_spec()])
        await daemon.user(text=TASK)
        for _ in range(calls):
            call = await daemon.until("tool_call")
            await daemon.result(call, {"status": "done", "for_model": "ok"})
        await daemon.until("event", kind="assistant.final")
        await daemon.until("settled")
    return model


async def test_user_prompt_secrets_never_reach_the_model():
    model = await run(0)
    sent = "\n".join(prompts(model.requests[0]))
    assert FAKE_KEY not in sent
    assert "Upload the report with key" in sent and "redacted" in sent


async def test_long_runs_are_reanchored_on_the_current_request():
    model = await run(REANCHOR_EVERY + 1)
    reminded = [
        index
        for index, messages in enumerate(model.requests, start=1)
        if any("<system-reminder>" in text for text in prompts(messages))
    ]
    assert reminded == [REANCHOR_EVERY]
    reminder = next(
        text
        for text in prompts(model.requests[REANCHOR_EVERY - 1])
        if "<system-reminder>" in text
    )
    assert "Upload the report" in reminder and FAKE_KEY not in reminder
