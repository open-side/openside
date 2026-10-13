"""Offline FunctionModel script format from sidecar-protocol.md §9."""

from __future__ import annotations

import asyncio
import hashlib
import re
from collections.abc import AsyncIterator
from pathlib import Path
from typing import Annotated, Any, Literal, cast

from pydantic import BaseModel, ConfigDict, Field
from pydantic_ai.messages import (
    BinaryContent,
    ModelMessage,
    ModelRequest,
    ModelResponse,
    TextPart,
    ToolCallPart,
    ToolReturnPart,
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
    # echo: reply with the latest user prompt text, so offline tests can prove which
    # instruction (e.g. a steer) reached this agent.
    type: Literal["text", "tool_call", "wait_steer", "media_digest", "echo"]
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
    title: str = "Discuss the request"
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
        self.title_model = FunctionModel(self.respond_title)

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

    async def respond_title(
        self, messages: list[ModelMessage], info: AgentInfo
    ) -> ModelResponse:
        return ModelResponse(parts=[TextPart(self.script.title)])

    async def respond(
        self, messages: list[ModelMessage], info: AgentInfo
    ) -> ModelResponse:
        response = await self.next_response()
        parts: list[TextPart | ToolCallPart] = []
        for part in response.parts:
            if part.type in ("text", "media_digest", "echo"):
                parts.append(TextPart(part_text(part, messages)))
            elif part.type == "tool_call":
                args = resolve_refs(part.args, messages)
                if part.id is None:
                    parts.append(ToolCallPart(part.name, args))
                else:
                    parts.append(ToolCallPart(part.name, args, tool_call_id=part.id))
        return ModelResponse(parts=parts or [TextPart("")])

    async def stream(
        self, messages: list[ModelMessage], info: AgentInfo
    ) -> AsyncIterator[str | DeltaToolCalls]:
        response = await self.next_response()
        emitted = False
        for index, part in enumerate(response.parts):
            if part.type in ("text", "media_digest", "echo"):
                text = part_text(part, messages)
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
                        json_args=json.dumps(resolve_refs(part.args, messages)),
                        tool_call_id=part.id,
                    )
                }
        if not emitted:
            yield ""


REF = re.compile(r"\{\{result:([^.}]+)\.([^}]+)\}\}")


def part_text(part: ScriptPart, messages: list[ModelMessage]) -> str:
    if part.type == "media_digest":
        return media_digest(messages)
    if part.type == "echo":
        return last_user_text(messages)
    return part.text


def last_user_text(messages: list[ModelMessage]) -> str:
    for message in reversed(messages):
        if isinstance(message, ModelRequest):
            for part in reversed(message.parts):
                if isinstance(part, UserPromptPart) and isinstance(part.content, str):
                    return part.content
                if isinstance(part, UserPromptPart) and isinstance(part.content, list):
                    texts = [item for item in part.content if isinstance(item, str)]
                    if texts:
                        return (
                            texts[1]
                            if len(texts) > 1 and '"provenance"' in texts[0]
                            else texts[0]
                        )
    return ""


def resolve_refs(args: dict[str, Any], messages: list[ModelMessage]) -> dict[str, Any]:
    """Resolve result paths without hard-coded daemon IDs."""

    def value(match: re.Match[str]) -> str:
        for message in messages:
            if isinstance(message, ModelRequest):
                for part in message.parts:
                    if isinstance(
                        part, ToolReturnPart
                    ) and part.tool_call_id == match.group(1):
                        content: object = part.content
                        for key in match.group(2).split("."):
                            if isinstance(content, dict):
                                content = cast(dict[str, Any], content).get(key)
                            elif isinstance(content, list) and key.isdecimal():
                                items = cast(list[Any], content)
                                index = int(key)
                                content = items[index] if index < len(items) else None
                            else:
                                return ""
                        return "" if content is None else str(content)
        return ""

    def resolve(item: Any) -> Any:
        if isinstance(item, str):
            if item == "{{user_text}}":
                return last_user_text(messages)
            if item == "{{media_digest}}":
                return media_digest(messages)
            return REF.sub(value, item)
        if isinstance(item, list):
            return [resolve(child) for child in cast(list[Any], item)]
        if isinstance(item, dict):
            return {
                key: resolve(child) for key, child in cast(dict[str, Any], item).items()
            }
        return item

    return {key: resolve(item) for key, item in args.items()}


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
