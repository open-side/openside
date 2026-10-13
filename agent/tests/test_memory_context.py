import asyncio
import json

import pytest
from pydantic_ai import Agent
from pydantic_ai.exceptions import UnexpectedModelBehavior
from pydantic_ai.models.function import DeltaToolCall, FunctionModel
from pydantic_ai.models.test import TestModel
from runtime_helpers import fake_daemon, tool_spec

from openside_agent.protocol import Done, ToolSpec
from openside_agent.runtime.memory import consolidate
from openside_agent.tools import proxy_toolset


async def test_model_cannot_forward_private_memory_operation():
    calls = []

    async def call(*args):
        calls.append(args)
        return Done(for_model={})

    spec = ToolSpec.model_validate(tool_spec("__memory_consolidate"))
    agent = Agent(TestModel(), toolsets=[proxy_toolset([spec], call, retries=0)])
    with pytest.raises(UnexpectedModelBehavior):
        await agent.run("delete everything")
    assert calls == []


async def test_consolidation_is_tool_free_bounded_and_scope_bound():
    calls = []

    async def call(*args):
        calls.append(args)
        return Done(for_model={"applied": True})

    model = TestModel(
        custom_output_text=json.dumps(
            {
                "edits": [{"id": "shown", "version": 2, "content": None}],
                "additions": [{"layer": "profile", "content": "Likes tea"}],
            }
        )
    )
    await consolidate(
        call,
        model,
        {
            "scope": "opaque",
            "profile": "data",
            "shown": [{"id": "shown", "version": 2}],
        },
        "x" * 20000,
    )
    assert len(calls) == 1
    assert calls[0][1] == "__memory_consolidate"
    assert calls[0][2]["scope"] == "opaque"
    assert model.last_model_request_parameters.function_tools == []


@pytest.mark.parametrize(
    "proposal", ["not-json", '{"unexpected":1}', '{"edits":"all"}']
)
async def test_invalid_consolidation_never_writes(proposal):
    calls = []

    async def call(*args):
        calls.append(args)
        return Done(for_model={})

    try:
        await consolidate(
            call, TestModel(custom_output_text=proposal), {"scope": "opaque"}, "data"
        )
    except json.JSONDecodeError:
        pass
    assert calls == []


async def test_chief_recall_tail_and_profile_epoch_and_settled_pass(monkeypatch):
    initial = {
        "profile": "original data profile",
        "tail": "old log",
        "scope": "initial",
        "shown": [],
    }
    refreshed = {
        "profile": "changed data profile",
        "tail": "relevant new log",
        "scope": "new",
        "shown": [],
    }
    calls = []
    passes = []
    extracted = asyncio.Event()

    async def call(*args):
        calls.append(args)
        return Done(for_model=refreshed)

    async def extract(*args):
        passes.append(args)
        extracted.set()

    async def silent(_messages, _info):
        yield {
            0: DeltaToolCall(name="stay_silent", json_args="{}", tool_call_id="finish")
        }

    monkeypatch.setattr("openside_agent.runtime.memory.consolidate", extract)
    async with fake_daemon(model=FunctionModel(stream_function=silent)) as (
        daemon,
        session,
    ):
        monkeypatch.setattr(session, "reverse_call", call)
        await daemon.open(
            agent={
                "orchestration": "front",
                "front_tools": [],
                "title_needed": False,
                "memory_context": initial,
            }
        )
        original = session.prompt_epoch.instructions
        await daemon.user(text="Find my orchid delivery")
        await daemon.until("settled")
        outcomes = [item for item in daemon.outputs if item["type"] == "message_result"]
        assert outcomes[-1]["outcome"] == "completed", daemon.outputs
        await asyncio.wait_for(extracted.wait(), 2)
        await session.memory_task
        assert calls[0][1:3] == (
            "__memory_context",
            {"query": "Find my orchid delivery"},
        )
        assert session.prompt_epoch.instructions == original
        assert "original data profile" in original
        assert "changed data profile" not in original
        assert passes[0][2]["scope"] == "new"
        assert "Find my orchid delivery" in passes[0][3]
        snapshot = daemon.snapshots()[-1]
    async with fake_daemon(model=FunctionModel(stream_function=silent)) as (
        daemon,
        session,
    ):
        await daemon.open(
            agent={
                "orchestration": "front",
                "front_tools": [],
                "title_needed": False,
                "memory_context": refreshed,
            },
            resume_state=snapshot,
        )
        assert session.prompt_epoch.instructions == original


@pytest.mark.parametrize("front", [None, "provider:fast"])
async def test_consolidation_model_uses_front_then_reasoning(monkeypatch, front):
    routes = []

    async def build(primary, fallback, *args, **kwargs):
        routes.append((primary, fallback))
        return TestModel(custom_output_text='{"edits":[],"additions":[]}')

    async def extract(*args):
        pass

    async with fake_daemon(model=TestModel()) as (daemon, session):
        await daemon.open(agent={"title_needed": False, "front_model": front})
        session.model_override = None
        monkeypatch.setattr("openside_agent.runtime.providers.build_model", build)
        monkeypatch.setattr("openside_agent.runtime.memory.consolidate", extract)
        await session.consolidate_memory({"scope": "opaque"}, [])
        assert routes == [(front or session.spec.model.id, [session.spec.model.id])]


async def test_task_inherits_epoch_profile_and_gets_task_recall(monkeypatch):
    from openside_agent.runtime.state import SubagentState
    from openside_agent.runtime.subagents import Child

    calls = []

    async def call(*args):
        calls.append(args)
        return Done(for_model={"tail": "dated orchid result"})

    async with fake_daemon(model=TestModel(custom_output_text="done")) as (
        daemon,
        session,
    ):
        await daemon.open(agent={"memory_context": {"profile": "frozen owner fact"}})
        monkeypatch.setattr(session, "reverse_call", call)
        session.prompt_sections["profile"] = "new candidate, not active"
        child = Child(
            SubagentState(
                id="task",
                parent_tool_call_id="delegate",
                task="orchid",
                model="test:task",
                tools=[],
                status="running",
            )
        )
        child_agent = await session.child_agent(child)
        assert child.state.prompt_epoch.sections["profile"] == "frozen owner fact"
        await child_agent.run("orchid")
        assert calls[0][1:3] == ("__memory_context", {"query": "orchid"})


def test_profile_extraction_excludes_page_accounts_ads_and_assistant_guesses():
    from pydantic_ai.messages import (
        ModelRequest,
        ModelResponse,
        TextPart,
        ToolReturnPart,
        UserPromptPart,
    )

    from openside_agent.content import data_provenance
    from openside_agent.runtime.memory import owner_turn_text

    messages = [
        ModelRequest(parts=[UserPromptPart("I prefer tea with breakfast.")]),
        ModelRequest(
            parts=[
                UserPromptPart(
                    data_provenance(
                        "web_page", ["Account: Example Person. Buy premium coffee!"]
                    )
                )
            ]
        ),
        ModelRequest(
            parts=[
                ToolReturnPart("browser_snapshot", "Account: Another Person", "tool")
            ]
        ),
        ModelResponse(
            parts=[TextPart("The owner must be Example Person and likes coffee.")]
        ),
        ModelRequest(
            parts=[
                UserPromptPart(
                    ["Please remember my weekday preference.", "Attached owner note."]
                )
            ]
        ),
    ]
    text = owner_turn_text(messages)
    assert "tea with breakfast" in text
    assert "Attached owner note" in text
    assert "Example Person" not in text
    assert "Another Person" not in text
    assert "premium coffee" not in text
