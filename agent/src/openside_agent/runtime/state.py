"""Opaque run-end snapshots; credential policy remains entirely in the daemon."""

import json
from typing import Any, Literal, cast

from pydantic import BaseModel, ConfigDict, Field, model_validator
from pydantic_ai.messages import ModelMessage, ModelMessagesTypeAdapter

from openside_agent.protocol import (
    MAX_LINE_BYTES,
    AgentState,
    ContentPart,
    DeferredCall,
    Resolution,
    StateSnapshot,
    UserMessage,
)
from openside_agent.runtime.prompt_epoch import PromptEpoch

SubagentStatus = Literal["running", "waiting_human", "completed", "failed", "cancelled"]
TERMINAL: frozenset[str] = frozenset({"completed", "failed", "cancelled"})


class PendingCall(DeferredCall):
    resolution: Resolution | None = None

    def wire(self) -> DeferredCall:
        return DeferredCall(
            tool_call_id=self.tool_call_id, ticket_id=self.ticket_id, kind=self.kind
        )


class PendingDelegate(BaseModel):
    """A parent `delegate` call suspended until its child reaches a terminal state.

    Unlike ticket resolutions, a stored child result is sidecar-internal and may be
    reused after restore: the daemon never delivers it.
    """

    model_config = ConfigDict(extra="forbid")
    tool_call_id: str
    subagent_id: str
    result: Any = None


# Base64 bytes of inline media kept in one snapshot, across the parent history, every
# child history and queued steer messages. Snapshots travel in one wire line (4 MiB), so
# older media is replaced by a note, newest first kept. Observation screenshots have no
# artifact copy; attachments and delivered screenshots remain in daemon storage.
SNAPSHOT_MEDIA_BUDGET = 1024 * 1024


class SnapshotTooLarge(Exception):
    """The retained continuation cannot fit a single protocol checkpoint."""


def _shed(dumped: list[Any], budget: int) -> int:
    """Keeps the newest binary items of one dumped history within `budget`; returns the
    budget left."""
    for message in reversed(dumped):
        for part in reversed(message.get("parts") or []):
            content = part.get("content")
            if part.get("part_kind") != "user-prompt" or not isinstance(content, list):
                continue
            items = cast(list[Any], content)
            for index in reversed(range(len(items))):
                item: Any = items[index]
                if not isinstance(item, dict):
                    continue
                entry = cast(dict[str, Any], item)
                if entry.get("kind") != "binary":
                    continue
                size = len(cast(str, entry["data"]))
                if size <= budget:
                    budget -= size
                    continue
                label = f"{entry.get('media_type')} {entry.get('identifier')}"
                items[index] = f"[{label}: omitted from saved conversation state]"
    return budget


def dump_messages(messages: list[ModelMessage]) -> list[Any]:
    dumped: list[Any] = json.loads(ModelMessagesTypeAdapter.dump_json(messages))
    _shed(dumped, SNAPSHOT_MEDIA_BUDGET)
    return dumped


def _shed_snapshot(data: "SnapshotData", budget: int) -> None:
    # Queued steer messages are not delivered yet, so their media is kept first; a part
    # that does not fit keeps its artifact reference and reaches the model as a note.
    steer: list[UserMessage] = []
    for message in data.steer_buffer:
        parts: list[ContentPart] = []
        for part in message.parts:
            size = len(part.data_base64 or "")
            if size <= budget:
                budget -= size
                parts.append(part)
            else:
                parts.append(part.model_copy(update={"data_base64": None}))
        steer.append(message.model_copy(update={"parts": parts}))
    data.steer_buffer = steer
    budget = _shed(data.messages, budget)
    for child in data.subagents:
        budget = _shed(child.messages, budget)


def load_messages(messages: list[Any]) -> list[ModelMessage]:
    return ModelMessagesTypeAdapter.validate_json(json.dumps(messages))


class SubagentState(BaseModel):
    """Child conversation persisted only at the child's own run-end points."""

    model_config = ConfigDict(extra="forbid")
    id: str
    parent_tool_call_id: str
    task: str
    model: str
    tools: list[str]
    status: SubagentStatus
    messages: list[Any] = Field(default_factory=list[Any])
    pending_deferred: list[PendingCall] = Field(default_factory=list[PendingCall])
    steer_buffer: list[str] = Field(default_factory=list[str])
    script_cursor: int = Field(default=0, ge=0)
    result: Any = None
    # A Task started by the front agent: nobody awaits it; its result reaches the
    # conversation as a daemon-queued `[task report]` message.
    background: bool = False
    prompt_epoch: PromptEpoch | None = None


class SnapshotData(BaseModel):
    model_config = ConfigDict(extra="forbid")
    messages: list[Any]
    pending_deferred: list[PendingCall] = Field(default_factory=list[PendingCall])
    pending_delegates: list[PendingDelegate] = Field(
        default_factory=list[PendingDelegate]
    )
    subagents: list[SubagentState] = Field(default_factory=list[SubagentState])
    steer_buffer: list[UserMessage] = Field(default_factory=list[UserMessage])
    # Messages whose run ended deferred; the resumed run reports their final outcome.
    deferred_messages: list[str] = Field(default_factory=list[str])
    script_cursor: int = Field(default=0, ge=0)
    title_attempted: bool = False
    prompt_epoch: PromptEpoch | None = None

    @model_validator(mode="before")
    @classmethod
    def ignore_retired_pins(cls, value: Any) -> Any:
        if isinstance(value, dict):
            data = cast(dict[str, Any], value)
            if "pins" in data:
                return {key: item for key, item in data.items() if key != "pins"}
            return data
        return value

    @classmethod
    def capture(
        cls,
        messages: list[ModelMessage],
        pending: list[PendingCall],
        steer: list[UserMessage],
        cursor: int,
        *,
        delegates: list[PendingDelegate],
        subagents: list[SubagentState],
        deferred_messages: list[str],
        title_attempted: bool = False,
        prompt_epoch: PromptEpoch | None = None,
    ) -> AgentState:
        data = cls(
            messages=dump_messages(messages),
            pending_deferred=pending,
            pending_delegates=delegates,
            subagents=[child.model_copy(deep=True) for child in subagents],
            steer_buffer=[message.model_copy(deep=True) for message in steer],
            deferred_messages=deferred_messages,
            script_cursor=cursor,
            title_attempted=title_attempted,
            prompt_epoch=prompt_epoch,
        )
        _shed_snapshot(data, SNAPSHOT_MEDIA_BUDGET)
        if not _fits(data):
            _shed_snapshot(data, 0)
        histories = [
            (
                data.messages,
                {item.tool_call_id for item in data.pending_deferred}
                | {item.tool_call_id for item in data.pending_delegates},
            ),
            *(
                (child.messages, {item.tool_call_id for item in child.pending_deferred})
                for child in data.subagents
            ),
        ]
        # A pending call needs its original turn after restart. Only completed
        # prefixes are candidates, even when a protected history is the largest.
        while not _fits(data):
            candidates = [
                (history, end)
                for history, pending_ids in histories
                if (end := _completed_prefix_end(history, pending_ids)) > 0
            ]
            if not candidates:
                break
            history, end = max(candidates, key=lambda item: len(json.dumps(item[0])))
            _drop_oldest_turn(history, end)
        if not _fits(data):
            raise SnapshotTooLarge(
                "Snapshot exceeds 4 MiB; pending continuation cannot be checkpointed"
            )
        return AgentState(data=data.model_dump_json())

    def restore_messages(self) -> list[ModelMessage]:
        return load_messages(self.messages)


def _fits(data: SnapshotData) -> bool:
    """Measures the exact line JsonLineConnection would send for this snapshot."""
    envelope = StateSnapshot(state=AgentState(data=data.model_dump_json()))
    return (
        len(envelope.model_dump_json(exclude_none=True).encode("utf-8"))
        <= MAX_LINE_BYTES
    )


def _parts(message: Any) -> list[dict[str, Any]]:
    return cast(list[dict[str, Any]], cast(dict[str, Any], message).get("parts") or [])


def _completed_prefix_end(history: list[Any], pending_ids: set[str]) -> int:
    if len(history) <= 1:
        return 0
    end = next(
        (
            index
            for index in range(1, len(history))
            if any(
                part.get("part_kind") == "user-prompt"
                for part in _parts(history[index])
            )
        ),
        len(history),
    )
    if any(
        part.get("part_kind") == "tool-call" and part.get("tool_call_id") in pending_ids
        for message in history[:end]
        for part in _parts(message)
    ):
        return 0
    return end


def _drop_oldest_turn(history: list[Any], end: int) -> None:
    del history[:end]
    if history:
        first = cast(dict[str, Any], history[0])
        first["parts"] = [
            part for part in _parts(first) if part.get("part_kind") != "tool-return"
        ]
