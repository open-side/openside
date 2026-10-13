import json
import logging

import pytest
from pydantic_ai.messages import ModelRequest, UserPromptPart
from pydantic_ai.models.function import FunctionModel
from runtime_helpers import fake_daemon, tool_spec

from openside_agent.protocol import ToolSpec
from openside_agent.runtime.prompt_epoch import (
    PromptEpoch,
    log_request_identity,
    stable_sections,
)
from openside_agent.runtime.state import SnapshotData


def tools(*names):
    return [ToolSpec.model_validate(tool_spec(name)) for name in names]


def reports(caplog):
    return [
        json.loads(record.message.split(" ", 1)[1])
        for record in caplog.records
        if record.message.startswith("prompt_sections ")
    ]


def test_canonical_tools_and_immutable_copies():
    source = tools("z", "a")
    epoch = PromptEpoch.assemble({"base_system": "fixed"}, source)
    assert (
        epoch.tools_json
        == PromptEpoch.assemble({"base_system": "fixed"}, source[::-1]).tools_json
    )
    source[0].description = "changed"
    epoch.tools[0].description = "changed"
    epoch.sections["base_system"] = "changed"
    assert [tool.name for tool in epoch.tools] == ["a", "z"]
    assert all(tool.description == "Daemon probe" for tool in epoch.tools)
    assert epoch.instructions == ["fixed"]
    with pytest.raises(ValueError, match="Duplicate"):
        PromptEpoch.assemble({}, tools("a", "a"))


def test_resume_preserves_epoch_until_explicit_transition():
    epoch = PromptEpoch.assemble({"base_system": "old"}, tools("a"))
    state = SnapshotData.capture(
        [],
        [],
        [],
        0,
        delegates=[],
        subagents=[],
        deferred_messages=[],
        prompt_epoch=epoch,
    )
    restored = SnapshotData.model_validate_json(state.data).prompt_epoch
    resumed = PromptEpoch.assemble({"base_system": "new"}, tools("b"), restored)
    assert resumed.model_dump() == epoch.model_dump()
    assert "new" not in resumed.tail_context({"base_system": "new"})
    transitioned = PromptEpoch.assemble(
        {"base_system": "new"}, tools("b"), resumed, transition=True
    )
    assert transitioned.id != resumed.id
    assert transitioned.instructions == ["new"]
    assert [tool.name for tool in transitioned.tools] == ["b"]


def test_section_hashes_and_tail_do_not_leak_content(caplog):
    caplog.set_level(logging.INFO)
    sections = stable_sections(
        ["secret-base", "<available_skills>secret-skill</available_skills>"],
        persona="secret-persona",
        profile="secret-profile",
    )
    epoch = PromptEpoch.assemble(sections, tools("secret-tool"))
    messages = [ModelRequest(parts=[UserPromptPart("secret-first")])]
    epoch.record_request(messages, provider="secret-provider", model="secret-model")
    epoch.record_request(
        messages,
        provider="secret-provider",
        model="secret-model",
        latest_context=epoch.tail_context(sections, "secret-tail"),
    )
    before, after = reports(caplog)
    assert after["causes"] == ["latest_context_changed"]
    for key in (
        "base_system",
        "persona",
        "profile",
        "skill_index",
        "tools",
        "first_message",
    ):
        assert before["sections"][key] == after["sections"][key]
    changed = PromptEpoch.assemble({**sections, "persona": "different"}, epoch.tools)
    changed.record_request(messages, provider="secret-provider", model="secret-model")
    third = reports(caplog)[-1]
    assert [
        key
        for key in before["sections"]
        if before["sections"][key] != third["sections"][key]
    ] == ["persona"]
    old = log_request_identity("secret-provider", "secret-model", "secret-account-a")
    log_request_identity("secret-provider", "secret-model", "secret-account-b", old)
    assert "account_boundary" in caplog.text
    assert "secret-" not in caplog.text


async def test_runtime_resumes_frozen_tools_and_instructions(caplog):
    seen = []

    async def respond(messages, info):
        seen.append((messages, info))
        yield "done"

    model = FunctionModel(stream_function=respond)
    async with fake_daemon(model=model) as (daemon, session):
        await daemon.open(tools=[tool_spec("z"), tool_spec("a")])
        await daemon.user()
        await daemon.until("settled")
        snapshot = daemon.snapshots()[-1]
        original = session.prompt_epoch.model_dump()
    async with fake_daemon(model=model) as (daemon, session):
        await daemon.send(
            "open",
            session="test-session",
            model={"id": "test:test", "fallback": []},
            system_prompt_parts=["New deployment must wait."],
            tools=[{**tool_spec("a"), "description": "new schema"}],
            resume_state=snapshot,
            config={"credentials": {}, "agent": {}},
        )
        await daemon.until("ready")
        await daemon.user("second", "next")
        await daemon.until("settled")
        assert session.prompt_epoch.model_dump() == original
    assert [tool.name for tool in seen[0][1].function_tools] == ["a", "z"]
    assert [tool.name for tool in seen[1][1].function_tools] == ["a", "z"]
    assert seen[0][1].instructions == seen[1][1].instructions
    assert "New deployment must wait." not in str(seen[1])
    assert "stable_sections_pending_next_epoch" in str(seen[1][0][-1])
    assert len(reports(caplog)) == 2


@pytest.mark.parametrize("provider", ["anthropic", "openai"])
async def test_provider_requests_enable_stable_prefix_cache(provider):
    import httpx2
    from anthropic import AsyncAnthropic
    from openai import AsyncOpenAI
    from pydantic_ai.messages import InstructionPart
    from pydantic_ai.models import ModelRequestParameters
    from pydantic_ai.models.anthropic import AnthropicModel
    from pydantic_ai.models.openai import OpenAIResponsesModel
    from pydantic_ai.providers.anthropic import AnthropicProvider
    from pydantic_ai.providers.openai import OpenAIProvider
    from pydantic_ai.tools import ToolDefinition

    from openside_agent.providers.request_auth import CredentialAttempt
    from openside_agent.providers.request_model import (
        CredentialLease,
        RequestCredentialModel,
    )
    from openside_agent.providers.secret import CredentialSecret

    captured = []

    def wire(request):
        captured.append(json.loads(request.content))
        if provider == "anthropic":
            return httpx2.Response(
                200,
                json={
                    "id": "msg_test",
                    "type": "message",
                    "role": "assistant",
                    "model": "claude-sonnet-4-5",
                    "content": [{"type": "text", "text": "ok"}],
                    "stop_reason": "end_turn",
                    "stop_sequence": None,
                    "usage": {
                        "input_tokens": 100,
                        "output_tokens": 1,
                        "cache_read_input_tokens": 50,
                        "cache_creation_input_tokens": 0,
                    },
                },
            )
        return httpx2.Response(
            200,
            json={
                "id": "resp_test",
                "object": "response",
                "created_at": 1,
                "status": "completed",
                "model": "gpt-5.6",
                "output": [
                    {
                        "id": "msg_test",
                        "type": "message",
                        "status": "completed",
                        "role": "assistant",
                        "content": [
                            {"type": "output_text", "text": "ok", "annotations": []}
                        ],
                    }
                ],
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 1,
                    "total_tokens": 101,
                    "input_tokens_details": {"cached_tokens": 50},
                },
            },
        )

    class Rpc:
        async def acquire(self, *args):
            return CredentialLease(
                "lease",
                "account",
                CredentialAttempt(
                    provider, "vault", "api_key", CredentialSecret("fake-key")
                ),
            )

        async def report(self, *args, **kwargs):
            return False

    http = httpx2.AsyncClient(transport=httpx2.MockTransport(wire))
    if provider == "anthropic":
        wrapped = AnthropicModel(
            "claude-sonnet-4-5",
            provider=AnthropicProvider(
                anthropic_client=AsyncAnthropic(api_key="fake-key", http_client=http)
            ),
        )
    else:
        wrapped = OpenAIResponsesModel(
            "gpt-5.6",
            provider=OpenAIProvider(
                openai_client=AsyncOpenAI(api_key="fake-key", http_client=http)
            ),
        )
    model = RequestCredentialModel(
        wrapped, http, Rpc(), model_id=f"{provider}:{wrapped.model_name}"
    )
    params = ModelRequestParameters(
        instruction_parts=[InstructionPart(content="stable instructions")],
        function_tools=[
            ToolDefinition(
                name="probe",
                parameters_json_schema={"type": "object", "properties": {}},
            )
        ],
    )
    async with model:
        for _ in range(2):
            response = await model.request(
                [ModelRequest(parts=[UserPromptPart("same prompt")])],
                {"max_tokens": 16},
                params,
            )
            assert response.usage.cache_read_tokens == 50
    assert captured[0] == captured[1]
    if provider == "anthropic":
        assert captured[0]["system"][-1]["cache_control"]["type"] == "ephemeral"
        assert captured[0]["tools"][-1]["cache_control"]["type"] == "ephemeral"
    else:
        assert "prompt_cache_breakpoint" in json.dumps(captured[0])


async def test_child_epoch_keeps_original_grants_and_logs_requests(caplog):
    from pydantic_ai.messages import ModelResponse, TextPart

    from openside_agent.runtime.state import SubagentState
    from openside_agent.runtime.subagents import Child

    def respond(messages, info):
        return ModelResponse(parts=[TextPart("done")])

    async with fake_daemon(model=FunctionModel(respond)) as (daemon, session):
        await daemon.open(tools=[tool_spec("probe"), tool_spec("read")])
        child = Child(
            SubagentState(
                id="child",
                parent_tool_call_id="call",
                task="work",
                model="test:test",
                tools=["probe"],
                status="running",
            )
        )
        agent = await session.child_agent(child)
        await agent.run("child task")
        assert child.state.tools == ["probe"]
        assert [tool.name for tool in child.state.prompt_epoch.tools] == [
            "probe",
            "read",
        ]
        restored = Child(
            SubagentState.model_validate_json(child.capture().model_dump_json())
        )
        original = restored.state.prompt_epoch.model_dump()
        await session.child_agent(restored)
        assert restored.state.prompt_epoch.model_dump() == original
    assert reports(caplog)[-1]["epoch"] == original["id"]
