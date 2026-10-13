"""Budgeted history compaction; daemon spill storage is deliberately independent."""

from __future__ import annotations

import asyncio
import hashlib
import logging
from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field, fields, is_dataclass, replace
from typing import cast

from pydantic_ai import RunContext
from pydantic_ai.messages import (
    BinaryContent,
    ImageUrl,
    ModelMessage,
    ModelMessagesTypeAdapter,
    ModelRequest,
)
from pydantic_ai.models import ModelRequestContext
from pydantic_ai_harness.compaction import SummarizingCompaction

DEFAULT_MAX_TOKENS = 64_000
# Encoded image size is not its token cost; reserve a stable vision allowance.
MEDIA_TOKENS = 1600
logger = logging.getLogger(__name__)


def history_bytes(messages: list[ModelMessage]) -> bytes:
    return ModelMessagesTypeAdapter.dump_json(messages)


def estimated_tokens(messages: list[ModelMessage]) -> int:
    """Estimate text at UTF-8 bytes/3 without serializing binary media payloads."""

    def size(value: object) -> int:
        if isinstance(value, (BinaryContent, ImageUrl)):
            return MEDIA_TOKENS * 3
        if isinstance(value, str):
            return len(value.encode("utf-8")) + 2
        if isinstance(value, bytes):
            return MEDIA_TOKENS * 3
        if is_dataclass(value) and not isinstance(value, type):
            return sum(
                size(item.name) + size(getattr(value, item.name))
                for item in fields(value)
            )
        if isinstance(value, dict):
            entries = cast("dict[object, object]", value)
            return sum(size(key) + size(item) for key, item in entries.items())
        if isinstance(value, (list, tuple)):
            items = cast("list[object] | tuple[object, ...]", value)
            return sum(size(item) + 1 for item in items)
        return 4

    # Replay retains old instructions; providers send only the current block.
    instructions = next(
        (
            message.instructions
            for message in reversed(messages)
            if isinstance(message, ModelRequest) and message.instructions is not None
        ),
        None,
    )
    return (
        sum(size(message.parts) for message in messages) + size(instructions) + 2
    ) // 3


@dataclass
class ReportingCompaction(SummarizingCompaction[None]):
    on_compaction: Callable[[], Awaitable[None]] | None = field(
        default=None, kw_only=True
    )
    last_context: RunContext[None] | None = field(default=None, init=False, repr=False)

    def needed(self, messages: list[ModelMessage]) -> bool:
        return (
            self.max_tokens is not None and estimated_tokens(messages) > self.max_tokens
        ) or (self.max_messages is not None and len(messages) > self.max_messages)

    async def compact(
        self, messages: list[ModelMessage], ctx: RunContext[None]
    ) -> list[ModelMessage]:
        compacted = await self.summarize_history(messages, ctx)
        if compacted is not messages and self.on_compaction is not None:
            await self.on_compaction()
        return compacted

    async def summarize_history(
        self, messages: list[ModelMessage], ctx: RunContext[None]
    ) -> list[ModelMessage]:
        # A large old result must not hide behind the normal twenty-message tail.
        keep = self.keep_messages
        if (
            self.max_tokens is not None
            and estimated_tokens(messages[-keep:]) > self.max_tokens
        ):
            keep = 2
        compactor = replace(self, keep_messages=keep)
        return await SummarizingCompaction[None].compact(compactor, messages, ctx)

    async def before_model_request(
        self, ctx: RunContext[None], request_context: ModelRequestContext
    ) -> ModelRequestContext:
        self.last_context = replace(ctx, model=request_context.model)
        messages = list(request_context.messages)
        if not self.needed(messages):
            return request_context
        compacted = await self.compact(messages, self.last_context)
        ctx.messages[:] = compacted
        return replace(request_context, messages=compacted)

    async def idle_summary(self, messages: list[ModelMessage]) -> list[ModelMessage]:
        assert self.last_context is not None
        # Bypass the notice until the caller commits the compare-and-set.
        return await self.summarize_history(messages, self.last_context)


class IdleCompaction:
    """Cancel only speculative summarization, never a running agent or host tool."""

    def __init__(self) -> None:
        self.generation = 0
        self.task: asyncio.Task[None] | None = None

    def cancel(self) -> None:
        self.generation += 1
        if self.task is not None:
            self.task.cancel()
            self.task = None

    def schedule(
        self,
        capability: ReportingCompaction,
        messages: list[ModelMessage],
        epoch: object,
        current: Callable[[], tuple[object, list[ModelMessage], bool]],
        commit: Callable[[list[ModelMessage]], Awaitable[None]],
    ) -> None:
        self.cancel()
        if capability.last_context is None or not capability.needed(messages):
            return
        if current()[2]:
            return
        generation = self.generation
        fingerprint = hashlib.sha256(history_bytes(messages)).digest()

        def valid() -> bool:
            live_epoch, live_messages, wake_imminent = current()
            return (
                generation == self.generation
                and live_epoch == epoch
                and live_messages is messages
                and not wake_imminent
                and hashlib.sha256(history_bytes(live_messages)).digest() == fingerprint
            )

        async def run() -> None:
            try:
                await asyncio.sleep(0.25)
                if not valid():
                    return
                compacted = await capability.idle_summary(list(messages))
                if valid() and compacted != messages:
                    # The callback assigns history before its first await.
                    self.task = None
                    await commit(compacted)
            except asyncio.CancelledError:
                pass
            except Exception:
                # Do not log provider errors that can contain prompt or credential data.
                logger.warning("Idle compaction failed")

        self.task = asyncio.create_task(run())
