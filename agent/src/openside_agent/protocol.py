"""Versioned JSONL wire contract; see docs/specs/sidecar-protocol.md."""

from __future__ import annotations

import asyncio
from typing import Annotated, Any, Literal, cast

from pydantic import (
    BaseModel,
    BeforeValidator,
    ConfigDict,
    Field,
    TypeAdapter,
    ValidationError,
)

MAX_LINE_BYTES = 4 * 1024 * 1024
MAX_USER_TEXT_BYTES = MAX_LINE_BYTES // 4
STATE_FORMAT = "openside-agent-state/1"


class WireModel(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)


class Envelope(WireModel):
    v: Literal[1] = 1


class ToolSpec(WireModel):
    name: str
    description: str
    parameters: dict[str, Any]
    approval: Literal["read", "write", "exec", "sensitive"]
    deferrable: bool


class ModelSpec(WireModel):
    id: str
    fallback: list[str]


class OpenConfig(WireModel):
    credentials: dict[str, Any]
    agent: dict[str, Any]


class AgentState(WireModel):
    format: Literal["openside-agent-state/1"] = STATE_FORMAT
    data: str


class Open(Envelope):
    type: Literal["open"] = "open"
    session: str
    model: ModelSpec
    system_prompt_parts: list[str]
    tools: list[ToolSpec]
    resume_state: AgentState | None = None
    config: OpenConfig


class ContentPart(WireModel):
    """Attachment or binary tool-result content (PP-14).

    `data_base64` carries the bytes inline; `artifact_ref` names the daemon-held copy.
    A part with only `artifact_ref` was not inlined (too large or unsupported).
    """

    type: Literal["text", "image", "audio", "file"]
    mime: str
    data_base64: str | None = None
    artifact_ref: str | None = None
    name: str | None = None


class UserMessage(Envelope):
    type: Literal["user_message"] = "user_message"
    message_id: str
    text: str
    mode: Literal["queue", "steer", "now"]
    parts: list[ContentPart] = Field(default_factory=list[ContentPart])


class Abort(Envelope):
    type: Literal["abort"] = "abort"


class Shutdown(Envelope):
    type: Literal["shutdown"] = "shutdown"


class SubagentCancel(Envelope):
    type: Literal["subagent_cancel"] = "subagent_cancel"
    subagent_id: str


class SubagentSteer(Envelope):
    type: Literal["subagent_steer"] = "subagent_steer"
    subagent_id: str
    text: str
    control_id: str


class DiscardDeferred(Envelope):
    type: Literal["discard_deferred"] = "discard_deferred"


class Done(WireModel):
    status: Literal["done"] = "done"
    for_model: Any
    parts: list[ContentPart] = Field(default_factory=list[ContentPart])


class DeferredOutcome(WireModel):
    status: Literal["deferred"] = "deferred"
    ticket_id: str
    kind: Literal["approval", "external"]


class Denied(WireModel):
    status: Literal["denied"] = "denied"
    reason: str
    hint: str | None = None


class ToolError(WireModel):
    status: Literal["error"] = "error"
    message: str


ToolOutcome = Annotated[
    Done | DeferredOutcome | Denied | ToolError, Field(discriminator="status")
]


class ToolResult(Envelope):
    type: Literal["tool_result"] = "tool_result"
    rpc_id: str
    outcome: ToolOutcome


class Approved(WireModel):
    status: Literal["approved"] = "approved"


class Rejected(WireModel):
    status: Literal["denied"] = "denied"
    reason: str


class ExternalResult(WireModel):
    status: Literal["result"] = "result"
    for_model: Any


Resolution = Annotated[
    Approved | Rejected | ExternalResult, Field(discriminator="status")
]


class TicketResolved(Envelope):
    type: Literal["ticket_resolved"] = "ticket_resolved"
    ticket_id: str
    tool_call_id: str
    resolution: Resolution


class Versions(WireModel):
    python: str
    pydantic_ai: str
    pydantic_ai_harness: str
    openside_agent: str


class Hello(Envelope):
    type: Literal["hello"] = "hello"
    protocol: Literal[1] = 1
    nonce: str
    pid: int
    versions: Versions


class DeferredCall(WireModel):
    tool_call_id: str
    ticket_id: str
    kind: Literal["approval", "external"]


class Ready(Envelope):
    type: Literal["ready"] = "ready"
    tools: list[str]
    pending_deferred: list[DeferredCall]
    startup_ms: int


class TurnStarted(WireModel):
    kind: Literal["turn.started"] = "turn.started"


class TurnEnded(WireModel):
    kind: Literal["turn.ended"] = "turn.ended"


class AssistantDelta(WireModel):
    kind: Literal["assistant.delta"] = "assistant.delta"
    text: str


class AssistantFinal(WireModel):
    kind: Literal["assistant.final"] = "assistant.final"
    text: str


class ThinkingDelta(WireModel):
    kind: Literal["thinking.delta"] = "thinking.delta"
    text: str


class SubagentLifecycle(WireModel):
    kind: Literal["subagent.lifecycle"] = "subagent.lifecycle"
    subagent_id: str
    status: str
    # Final answer, only with status "completed".
    output: str | None = None


class SubagentProgress(WireModel):
    kind: Literal["subagent.progress"] = "subagent.progress"
    subagent_id: str
    progress: Any


class Usage(WireModel):
    kind: Literal["usage"] = "usage"
    model: str
    input_tokens: int
    output_tokens: int


EventBody = Annotated[
    TurnStarted
    | TurnEnded
    | AssistantDelta
    | AssistantFinal
    | ThinkingDelta
    | SubagentLifecycle
    | SubagentProgress
    | Usage,
    Field(discriminator="kind"),
]


class Event(Envelope):
    type: Literal["event"] = "event"
    event: EventBody


class MessageAccepted(Envelope):
    type: Literal["message_accepted"] = "message_accepted"
    message_id: str


class MessageResult(Envelope):
    type: Literal["message_result"] = "message_result"
    message_id: str
    outcome: Literal["completed", "aborted", "failed", "deferred"]
    reason: str | None = None


class ToolCall(Envelope):
    type: Literal["tool_call"] = "tool_call"
    rpc_id: str
    tool_call_id: str
    name: str
    args: dict[str, Any]
    subagent: str | None = None
    approval_ticket: str | None = None


class Deferred(Envelope):
    type: Literal["deferred"] = "deferred"
    calls: list[DeferredCall]


class StateSnapshot(Envelope):
    type: Literal["state_snapshot"] = "state_snapshot"
    state: AgentState


class Settled(Envelope):
    type: Literal["settled"] = "settled"


class ContextCompacted(Envelope):
    """Earlier messages left one agent's model context; absent subagent = the parent."""

    type: Literal["context_compacted"] = "context_compacted"
    subagent: str | None = None


class Error(Envelope):
    type: Literal["error"] = "error"
    code: Literal[
        "protocol",
        "bad_request",
        "stale_ticket",
        "model_error",
        "auth_unavailable",
        "tool_gate",
        "internal",
    ]
    message: str
    fatal: bool
    message_id: str | None = None


class SubagentSteerResult(Envelope):
    type: Literal["subagent_steer_result"] = "subagent_steer_result"
    control_id: str
    delivered: bool


class Bye(Envelope):
    type: Literal["bye"] = "bye"


def require_version(value: Any) -> Any:
    if not isinstance(value, dict):
        raise ValueError("Protocol envelope must be an object")
    envelope = cast(dict[str, Any], value)
    if type(envelope.get("v")) is not int or envelope["v"] != 1:
        raise ValueError("Protocol version 1 is required")
    return envelope


Inbound = Annotated[
    Open
    | UserMessage
    | Abort
    | ToolResult
    | TicketResolved
    | Shutdown
    | Error
    | SubagentCancel
    | SubagentSteer
    | DiscardDeferred,
    Field(discriminator="type"),
]
Outbound = Annotated[
    Hello
    | Ready
    | Event
    | MessageAccepted
    | MessageResult
    | ToolCall
    | Deferred
    | StateSnapshot
    | Settled
    | ContextCompacted
    | Error
    | SubagentSteerResult
    | Bye,
    Field(discriminator="type"),
]
INBOUND_ADAPTER: TypeAdapter[Inbound] = TypeAdapter(
    Annotated[Inbound, BeforeValidator(require_version)]
)
OUTBOUND_ADAPTER: TypeAdapter[Outbound] = TypeAdapter(Outbound)


class JsonLineConnection:
    def __init__(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter):
        self.reader = reader
        self.writer = writer
        self._write_lock = asyncio.Lock()
        self._buffer = bytearray()
        self._discarding = False

    async def send(self, message: Envelope) -> None:
        payload = message.model_dump_json(exclude_none=True).encode("utf-8")
        if len(payload) > MAX_LINE_BYTES:
            raise ValueError("Outgoing protocol line exceeds the size limit")
        async with self._write_lock:
            self.writer.write(payload + b"\n")
            await self.writer.drain()

    async def protocol_error(self) -> None:
        await self.send(
            Error(
                code="protocol",
                message="Invalid protocol message discarded",
                fatal=False,
            )
        )

    async def receive(self) -> Inbound | None:
        while True:
            newline = self._buffer.find(b"\n")
            if newline >= 0:
                line = bytes(self._buffer[:newline])
                del self._buffer[: newline + 1]
                if self._discarding:
                    self._discarding = False
                    continue
                if len(line) > MAX_LINE_BYTES:
                    await self.protocol_error()
                    continue
                try:
                    return INBOUND_ADAPTER.validate_json(line)
                except (ValidationError, ValueError, RecursionError):
                    await self.protocol_error()
                    continue
            if len(self._buffer) > MAX_LINE_BYTES:
                if not self._discarding:
                    await self.protocol_error()
                self._discarding = True
                self._buffer.clear()
            chunk = await self.reader.read(65536)
            if not chunk:
                if self._buffer and not self._discarding:
                    await self.protocol_error()
                return None
            self._buffer.extend(chunk)

    async def close(self) -> None:
        self.writer.close()
        await self.writer.wait_closed()
