import asyncio

from pydantic_ai import Agent
from pydantic_ai.messages import ModelResponse, TextPart, ToolCallPart
from pydantic_ai.models import ModelRequestParameters
from pydantic_ai.models.function import AgentInfo, FunctionModel
from runtime_helpers import text_response, tool_response

from openside_agent.scripted import Script, ScriptedModel


async def test_nonstream_cursor_and_exhaustion():
    script = Script.model_validate(
        {"responses": [text_response("first"), text_response("second")]}
    )
    model = ScriptedModel(script, asyncio.Event())
    first = await Agent(model.model).run("start")
    assert first.output == "first" and model.cursor == 1
    restored = ScriptedModel(script, asyncio.Event(), cursor=model.cursor)
    assert (await Agent(restored.model).run("continued")).output == "second"
    assert (await Agent(restored.model).run("exhausted")).output == "[script exhausted]"
    assert restored.cursor == 2


async def test_wait_steer_gates_script_response():
    event = asyncio.Event()
    script = Script.model_validate(
        {
            "responses": [
                {
                    "parts": [
                        {"type": "wait_steer"},
                        {"type": "text", "text": "released"},
                    ]
                }
            ],
            "delay_ms": 10000,
        }
    )
    model = ScriptedModel(script, event)
    started = asyncio.Event()

    async def observed(messages, info):
        started.set()
        return await model.respond(messages, info)

    task = asyncio.create_task(Agent(FunctionModel(observed)).run("start"))
    await asyncio.wait_for(started.wait(), 5)
    assert not task.done()
    event.set()
    assert (await asyncio.wait_for(task, 5)).output == "released"


async def test_nonstream_tool_call_keeps_supplied_id():
    script = Script.model_validate({"responses": [tool_response("probe", "stable-id")]})
    model = ScriptedModel(script, asyncio.Event())
    response: ModelResponse = await model.respond(
        [],
        AgentInfo(
            function_tools=[],
            allow_text_output=True,
            output_tools=[],
            model_settings=None,
            model_request_parameters=ModelRequestParameters(),
            instructions=None,
        ),
    )
    assert isinstance(response.parts[0], ToolCallPart)
    assert response.parts[0].tool_call_id == "stable-id"
    assert not any(isinstance(part, TextPart) for part in response.parts)


async def test_subagent_script_keyed_by_delegate_call_with_own_cursor():
    script = Script.model_validate(
        {
            "responses": [text_response("parent")],
            "subagents": {
                "del-1": [text_response("child one"), text_response("child two")]
            },
        }
    )
    parent = ScriptedModel(script, asyncio.Event())
    child = parent.for_subagent("del-1", asyncio.Event(), cursor=1)
    assert (await Agent(child.model).run("go")).output == "child two"
    assert child.cursor == 2 and parent.cursor == 0
    unknown = parent.for_subagent("del-2", asyncio.Event())
    assert (await Agent(unknown.model).run("go")).output == "[script exhausted]"


async def test_wait_steer_timeout_overrides_script_delay():
    script = Script.model_validate(
        {
            "responses": [
                {
                    "parts": [
                        {"type": "wait_steer", "timeout_ms": 10},
                        {"type": "text", "text": "timed out"},
                    ]
                }
            ],
            "delay_ms": 60000,
        }
    )
    model = ScriptedModel(script, asyncio.Event())
    result = await asyncio.wait_for(Agent(model.model).run("go"), 5)
    assert result.output == "timed out"


async def test_result_refs_follow_nested_fields_and_list_indexes():
    script = Script.model_validate(
        {
            "responses": [
                tool_response("tabs", "list"),
                {
                    "parts": [
                        {
                            "type": "tool_call",
                            "name": "read_tab",
                            "args": {"tab_id": "{{result:list.0.tab_id}}"},
                            "id": "read",
                        }
                    ]
                },
                text_response("done"),
            ]
        }
    )
    model = ScriptedModel(script, asyncio.Event())
    agent = Agent(model.model)
    seen = []

    @agent.tool_plain
    def tabs() -> list[dict[str, str]]:
        return [{"tab_id": "assigned-tab"}]

    @agent.tool_plain
    def read_tab(tab_id: str) -> str:
        seen.append(tab_id)
        return "read"

    assert (await agent.run("read it")).output == "done"
    assert seen == ["assigned-tab"]
