"""Offline FunctionModel script format from sidecar-protocol.md §9."""

from __future__ import annotations

import asyncio
import hashlib
import re
from collections.abc import AsyncIterator
from pathlib import Path
from typing import Annotated, Any, Literal

from pydantic import BaseModel, ConfigDict, Field
from pydantic_ai.messages import (
    BinaryContent,
    ModelMessage,
    ModelRequest,
    ModelResponse,
    TextPart,
    ToolCallPart,
    UserPromptPart,
)
from pydantic_ai.models.function import (
    AgentInfo,
    DeltaToolCall,
    DeltaToolCalls,
    FunctionModel,
)


class ScriptPart(BaseModel):
    model_config = ConfigDict(extra="forbid")
    # media_digest: reply with `mime:sha256` of every binary input part the model
    # received, so offline tests can prove attachments and tool images reached it.
    type: Literal["text", "tool_call", "wait_steer", "media_digest"]
    text: str = ""
    name: str = ""
    args: dict[str, Any] = Field(default_factory=dict)
    id: str | None = None
    # wait_steer only: bound for this wait instead of the script-wide delay_ms,
    # so one steered turn does not slow every other scripted response.
    timeout_ms: Annotated[int, Field(ge=0)] | None = None


class ScriptResponse(BaseModel):
    parts: list[ScriptPart]


class Script(BaseModel):
    responses: list[ScriptResponse]
    delay_ms: Annotated[int, Field(ge=0)] = 0
    # Child turns keyed by the parent's `delegate` tool_call id, which is stable
    # across restarts, unlike creation order of concurrently delegated children.
    subagents: dict[str, list[ScriptResponse]] = Field(
        default_factory=dict[str, list[ScriptResponse]]
    )


class ScriptedModel:
    def __init__(self, script: Script, steer_received: asyncio.Event, cursor: int = 0):
        self.script = script
        self.steer_received = steer_received
        self.cursor = cursor
        self.model = FunctionModel(self.respond, stream_function=self.stream)

    @classmethod
    def from_file(
        cls, path: str, steer_received: asyncio.Event, cursor: int = 0
    ) -> ScriptedModel:
        return cls(
            Script.model_validate_json(Path(path).read_bytes()), steer_received, cursor
        )

    def for_subagent(
        self, parent_tool_call_id: str, steer_received: asyncio.Event, cursor: int = 0
    ) -> ScriptedModel:
        script = Script(
            responses=self.script.subagents.get(parent_tool_call_id, []),
            delay_ms=self.script.delay_ms,
        )
        return ScriptedModel(script, steer_received, cursor)

    async def next_response(self) -> ScriptResponse:
        if self.cursor >= len(self.script.responses):
            return ScriptResponse(
                parts=[ScriptPart(type="text", text="[script exhausted]")]
            )
        response = self.script.responses[self.cursor]
        self.cursor += 1
        delay = self.script.delay_ms / 1000
        waits = [part for part in response.parts if part.type == "wait_steer"]
        if waits:
            timeout = waits[0].timeout_ms
            try:
                await asyncio.wait_for(
                    self.steer_received.wait(),
                    timeout=delay if timeout is None else timeout / 1000,
                )
            except TimeoutError:
                pass
        elif delay:
            await asyncio.sleep(delay)
        return response

    async def respond(
        self, messages: list[ModelMessage], info: AgentInfo
    ) -> ModelResponse:
        response = await self.next_response()
        parts: list[TextPart | ToolCallPart] = []
        for part in response.parts:
            if part.type == "text":
                parts.append(TextPart(part.text))
            elif part.type == "media_digest":
                parts.append(TextPart(media_digest(messages)))
            elif part.type == "tool_call":
                if part.id is None:
                    parts.append(ToolCallPart(part.name, part.args))
                else:
                    parts.append(
                        ToolCallPart(part.name, part.args, tool_call_id=part.id)
                    )
        return ModelResponse(parts=parts or [TextPart("")])

    async def stream(
        self, messages: list[ModelMessage], info: AgentInfo
    ) -> AsyncIterator[str | DeltaToolCalls]:
        response = await self.next_response()
        emitted = False
        for index, part in enumerate(response.parts):
            if part.type in ("text", "media_digest"):
                text = part.text if part.type == "text" else media_digest(messages)
                for word in re.findall(r"\S+\s*|\s+", text):
                    emitted = True
                    yield word
                    await asyncio.sleep(0)
            elif part.type == "tool_call":
                import json

                emitted = True
                yield {
                    index: DeltaToolCall(
                        name=part.name,
                        json_args=json.dumps(part.args),
                        tool_call_id=part.id,
                    )
                }
        if not emitted:
            yield ""


def media_digest(messages: list[ModelMessage]) -> str:
    digests = [
        f"{item.media_type}:{hashlib.sha256(item.data).hexdigest()}"
        for message in messages
        if isinstance(message, ModelRequest)
        for part in message.parts
        if isinstance(part, UserPromptPart) and not isinstance(part.content, str)
        for item in part.content
        if isinstance(item, BinaryContent)
    ]
    return "media " + (" ".join(digests) if digests else "none")
