import asyncio
import json
import random

import pytest
from pydantic_ai import Agent, RunContext
from pydantic_ai.models.test import TestModel
from pydantic_ai.usage import RunUsage
from runtime_helpers import fake_daemon, tool_spec

from openside_agent.capabilities import adopted_capabilities
from openside_agent.protocol import MAX_LINE_BYTES


@pytest.mark.parametrize("index", range(5))
async def test_adopted_capability_adds_zero_tools(index):
    capability = adopted_capabilities({})[index]
    assert capability.get_toolset() is None
    model = TestModel()
    agent = Agent(model, capabilities=[capability])
    context = RunContext(deps=None, model=model, usage=RunUsage(), agent=agent)
    actual = [
        name for toolset in agent.toolsets for name in await toolset.get_tools(context)
    ]
    assert actual == []
    assert (await agent.run("probe")).output


async def test_gate_uses_exposed_toolsets():
    async with fake_daemon(model=TestModel()) as (daemon, _):
        specs = [tool_spec("one"), tool_spec("two")]
        ready = await daemon.open(tools=specs)
        assert ready["tools"] == ["one", "two"]
        await daemon.send("open", session="ignored")
        assert (await daemon.until("error"))["code"] == "protocol"


async def test_gate_rejects_added_capability_tool(monkeypatch):
    from pydantic_ai.capabilities import AbstractCapability
    from pydantic_ai.toolsets import FunctionToolset

    class RogueCapability(AbstractCapability):
        def get_toolset(self):
            tools = FunctionToolset(id="rogue")

            @tools.tool_plain
            def extra() -> str:
                return "extra"

            return tools

    monkeypatch.setattr(
        "openside_agent.runtime.adopted_capabilities",
        lambda *_: [RogueCapability()],
    )
    async with fake_daemon(model=TestModel()) as (daemon, _):
        model = TestModel()
        await daemon.send(
            "open",
            session="gate",
            model={"id": f"{model.system}:{model.model_name}", "fallback": []},
            system_prompt_parts=[],
            tools=[],
            config={"credentials": {}, "agent": {}},
        )
        error = await daemon.receive()
        assert error["code"] == "tool_gate" and error["fatal"]


async def test_protocol_fuzz_survives_and_recovers():
    rng = random.Random(19)
    malformed = [
        b"{bad json}",
        b"[]",
        b'{"v":2,"type":"abort"}',
        b'{"v":1,"type":"unknown"}',
        b'{"v":1,"type":"user_message"}',
        b'{"type":"abort"}',
        b'{"v":true,"type":"abort"}',
        bytes(rng.randrange(128, 256) for _ in range(100)),
        b"x" * (MAX_LINE_BYTES + 100000),
    ]
    async with fake_daemon(model=TestModel()) as (daemon, _):

        async def write_lines():
            for line in malformed:
                daemon.writer.write(line + b"\n")
                await daemon.writer.drain()
            await daemon.send("abort")

        sender = asyncio.create_task(write_lines())
        for _ in malformed:
            error = await daemon.receive()
            assert error["type"] == "error" and error["code"] == "protocol"
            assert not error["fatal"]
        await sender
        await daemon.open()
        await daemon.user()
        await daemon.until("settled")
        assert any(item["type"] == "message_result" for item in daemon.outputs)


async def test_writer_serializes_concurrent_messages():
    async with fake_daemon(model=TestModel()) as (daemon, session):
        from openside_agent.protocol import MessageAccepted

        await asyncio.gather(
            *(
                session.connection.send(MessageAccepted(message_id=str(i)))
                for i in range(100)
            )
        )
        values = [await daemon.receive() for _ in range(100)]
        assert {value["message_id"] for value in values} == {str(i) for i in range(100)}
        assert all(json.dumps(value) for value in values)


async def test_received_fatal_error_closes_without_more_output(tmp_path, monkeypatch):
    from runtime_helpers import install_script, tool_response

    install_script(tmp_path, monkeypatch, [tool_response()])
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=[tool_spec()])
        await daemon.user()
        await daemon.until("tool_call")
        await daemon.send(
            "error", code="internal", message="Daemon stopped", fatal=True
        )
        assert await asyncio.wait_for(daemon.reader.read(), 5) == b""
