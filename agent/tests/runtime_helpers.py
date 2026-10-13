import asyncio
import json
import socket
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Any

import pytest
from pydantic_ai.models import Model
from pydantic_ai.models.test import TestModel

from openside_agent.protocol import OUTBOUND_ADAPTER, JsonLineConnection
from openside_agent.runtime import Session


class FakeDaemon:
    def __init__(self, reader, writer):
        self.reader = reader
        self.writer = writer
        self.outputs = []

    async def send(self, type: str, **fields: Any):
        self.writer.write(json.dumps({"v": 1, "type": type, **fields}).encode() + b"\n")
        await self.writer.drain()

    async def receive(self):
        line = await asyncio.wait_for(self.reader.readline(), 10)
        assert line, "Sidecar closed before expected output"
        OUTBOUND_ADAPTER.validate_json(line)
        value = json.loads(line)
        self.outputs.append(value)
        return value

    async def until(self, type: str, *, kind: str | None = None):
        while True:
            message = await self.receive()
            if message["type"] == type and (
                kind is None or message["event"]["kind"] == kind
            ):
                return message
            assert message["type"] != "error" or not message["fatal"], message

    async def open(self, *, tools=None, resume_state=None, agent=None, team=None):
        model = TestModel()
        await self.send(
            "open",
            session="test-session",
            model={"id": f"{model.system}:{model.model_name}", "fallback": []},
            system_prompt_parts=["Use daemon tools only."],
            tools=tools or [],
            resume_state=resume_state,
            config={"credentials": {}, "agent": agent or {}},
            team=team,
        )
        return await self.until("ready")

    async def user(self, message_id="message-1", text="start", mode="now"):
        await self.send("user_message", message_id=message_id, text=text, mode=mode)

    async def result(self, call, outcome):
        await self.send("tool_result", rpc_id=call["rpc_id"], outcome=outcome)

    def snapshots(self):
        return [
            item["state"] for item in self.outputs if item["type"] == "state_snapshot"
        ]


@asynccontextmanager
async def fake_daemon(*, model: Model | None = None):
    left, right = socket.socketpair()
    reader, writer = await asyncio.open_connection(sock=left, limit=5 * 1024 * 1024)
    peer_reader, peer_writer = await asyncio.open_connection(
        sock=right, limit=5 * 1024 * 1024
    )
    session = Session(JsonLineConnection(reader, writer), model=model)
    task = asyncio.create_task(session.serve())
    daemon = FakeDaemon(peer_reader, peer_writer)
    try:
        yield daemon, session
    finally:
        if not task.done() and not session.fatal and not session.shutting_down:
            await daemon.send("shutdown")
            await daemon.until("bye")
        await asyncio.wait_for(task, 10)
        peer_writer.close()
        await peer_writer.wait_closed()


def tool_spec(name="probe"):
    return {
        "name": name,
        "description": "Daemon probe",
        "parameters": {
            "type": "object",
            "properties": {"value": {"type": "string"}},
            "additionalProperties": False,
        },
        "approval": "read",
        "deferrable": True,
    }


def tool_response(name="probe", id="call-1"):
    return {"parts": [{"type": "tool_call", "name": name, "args": {}, "id": id}]}


def text_response(text="finished"):
    return {"parts": [{"type": "text", "text": text}]}


def install_script(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, responses, delay_ms=0
):
    script = tmp_path / "script.json"
    script.write_text(json.dumps({"responses": responses, "delay_ms": delay_ms}))
    monkeypatch.setenv("OPENSIDE_AGENT_SCRIPT", str(script))
    return script
