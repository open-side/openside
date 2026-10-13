"""Model requests replay only when daemon policy authorizes an account rotation."""

import asyncio
from collections.abc import AsyncGenerator, Awaitable, Callable
from contextlib import asynccontextmanager
from dataclasses import dataclass
from types import TracebackType
from typing import Any, Protocol, Self
from uuid import uuid4

import httpx2
from pydantic_ai import RunContext
from pydantic_ai.exceptions import ModelAPIError, ModelHTTPError
from pydantic_ai.messages import ModelMessage, ModelResponse
from pydantic_ai.models import (
    Model,
    ModelRequestContext,
    ModelRequestParameters,
    StreamedResponse,
)
from pydantic_ai.models.wrapper import WrapperModel
from pydantic_ai.settings import ModelSettings, merge_model_settings
from pydantic_ai.usage import RequestUsage

from .errors import AuthUnavailable
from .models import sanitized_error
from .outcomes import ClassifiedOutcome, classify_response
from .request_auth import CURRENT_ATTEMPT, CredentialAttempt


@dataclass(repr=False)
class CredentialLease:
    lease_id: str
    account_id: str
    credential: CredentialAttempt

    def __repr__(self) -> str:
        return "CredentialLease(credentials=**********)"


class CredentialRpc(Protocol):
    async def acquire(
        self,
        provider: str,
        model: str,
        request_id: str,
        attempt: int,
        subagent: str | None,
    ) -> CredentialLease: ...

    async def report(
        self,
        lease: CredentialLease,
        outcome: ClassifiedOutcome,
        request_id: str,
        *,
        stream_started: bool = False,
        usage: RequestUsage | None = None,
        wait: bool = True,
    ) -> bool: ...


class RequestCredentialModel(WrapperModel):
    def __init__(
        self,
        wrapped: Model,
        http: httpx2.AsyncClient,
        rpc: CredentialRpc,
        *,
        model_id: str,
        subagent: str | None = None,
        max_attempts: int = 64,
    ) -> None:
        super().__init__(wrapped)
        self.http = http
        self.rpc = rpc
        self.identifier = model_id
        self.provider_name = model_id.split(":", 1)[0]
        self.subagent = subagent
        self.max_attempts = max_attempts
        self._entered = 0
        self._last_account_digest: str | None = None

    def __repr__(self) -> str:
        return (
            f"RequestCredentialModel(provider={self.provider_name!r}, "
            "credentials=**********)"
        )

    @property
    def settings(self) -> ModelSettings | None:
        if self.provider_name in {
            "anthropic",
            "openai",
            "openai-chat",
            "openai-responses",
            "openai-codex",
        }:
            return merge_model_settings(
                ModelSettings(cache=True), self.wrapped.settings
            )
        return self.wrapped.settings

    def _log_identity(self, lease: CredentialLease) -> None:
        from openside_agent.runtime.prompt_epoch import log_request_identity

        self._last_account_digest = log_request_identity(
            self.provider_name,
            self.identifier,
            lease.account_id,
            self._last_account_digest,
        )

    async def __aenter__(self) -> Self:
        await self.wrapped.__aenter__()
        self._entered += 1
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc_val: BaseException | None,
        exc_tb: TracebackType | None,
    ) -> bool | None:
        try:
            return await self.wrapped.__aexit__(exc_type, exc_val, exc_tb)
        finally:
            self._entered -= 1
            if self._entered <= 0:
                await self.http.aclose()

    async def _report(
        self,
        lease: CredentialLease,
        request_id: str,
        *,
        failed: bool = False,
        cancelled: bool = False,
        stream_started: bool = False,
        usage: RequestUsage | None = None,
    ) -> bool:
        outcome = lease.credential.outcome
        if cancelled:
            outcome = ClassifiedOutcome("cancelled")
        elif not failed:
            outcome = ClassifiedOutcome("success")
        elif failed and outcome.kind == "success":
            outcome = ClassifiedOutcome("transient")
        return await self.rpc.report(
            lease,
            outcome,
            request_id,
            stream_started=stream_started,
            usage=usage,
            wait=not cancelled,
        )

    def _exhausted(self) -> AuthUnavailable:
        return AuthUnavailable(
            self.provider_name,
            ["daemon credential rotation attempt limit reached"],
            reason="attempt_limit",
        )

    def _classify_error(self, lease: CredentialLease, error: Exception) -> None:
        if isinstance(error, ModelHTTPError) and lease.credential.outcome.kind in (
            "success",
            "transient",
        ):
            lease.credential.outcome = classify_response(
                error.status_code, body=error.body
            )

    async def _execute[T](self, operation: Callable[[], Awaitable[T]]) -> T:
        request_id = str(uuid4())
        for attempt in range(self.max_attempts):
            lease = await self.rpc.acquire(
                self.provider_name, self.identifier, request_id, attempt, self.subagent
            )
            self._log_identity(lease)
            marker = CURRENT_ATTEMPT.set(lease.credential)
            try:
                try:
                    result = await operation()
                except Exception as error:
                    self._classify_error(lease, error)
                    retry = await self._report(lease, request_id, failed=True)
                    if retry:
                        continue
                    raise sanitized_error(error, self.model_name) from None
                except asyncio.CancelledError:
                    await self._report(lease, request_id, cancelled=True)
                    raise
                usage = (
                    result.usage
                    if isinstance(result, ModelResponse)
                    else result
                    if isinstance(result, RequestUsage)
                    else None
                )
                await self._report(lease, request_id, usage=usage)
                return result
            finally:
                CURRENT_ATTEMPT.reset(marker)
                lease.credential.secret.clear()
                lease.credential.upstream_account_id = None
        raise self._exhausted()

    async def request(
        self,
        messages: list[ModelMessage],
        model_settings: ModelSettings | None,
        model_request_parameters: ModelRequestParameters,
    ) -> ModelResponse:
        return await self._execute(
            lambda: self.wrapped.request(
                messages,
                merge_model_settings(self.settings, model_settings),
                model_request_parameters,
            )
        )

    async def count_tokens(
        self,
        messages: list[ModelMessage],
        model_settings: ModelSettings | None,
        model_request_parameters: ModelRequestParameters,
    ) -> RequestUsage:
        if type(self.wrapped).count_tokens is Model.count_tokens:
            return await self.wrapped.count_tokens(
                messages, model_settings, model_request_parameters
            )
        return await self._execute(
            lambda: self.wrapped.count_tokens(
                messages,
                merge_model_settings(self.settings, model_settings),
                model_request_parameters,
            )
        )

    async def compact_messages(
        self, request_context: ModelRequestContext, *, instructions: str | None = None
    ) -> ModelResponse:
        if type(self.wrapped).compact_messages is Model.compact_messages:
            return await self.wrapped.compact_messages(
                request_context, instructions=instructions
            )
        return await self._execute(
            lambda: self.wrapped.compact_messages(
                request_context, instructions=instructions
            )
        )

    async def cancel_suspended_response(self, response: ModelResponse) -> None:
        if (
            type(self.wrapped).cancel_suspended_response
            is Model.cancel_suspended_response
        ):
            return
        await self._execute(lambda: self.wrapped.cancel_suspended_response(response))

    @asynccontextmanager
    async def request_stream(
        self,
        messages: list[ModelMessage],
        model_settings: ModelSettings | None,
        model_request_parameters: ModelRequestParameters,
        run_context: RunContext[Any] | None = None,
    ) -> AsyncGenerator[StreamedResponse]:
        request_id = str(uuid4())
        for attempt in range(self.max_attempts):
            lease = await self.rpc.acquire(
                self.provider_name, self.identifier, request_id, attempt, self.subagent
            )
            self._log_identity(lease)
            marker = CURRENT_ATTEMPT.set(lease.credential)
            started = False
            stream: StreamedResponse | None = None
            try:
                try:
                    async with self.wrapped.request_stream(
                        messages,
                        merge_model_settings(self.settings, model_settings),
                        model_request_parameters,
                        run_context,
                    ) as stream:
                        started = True
                        yield stream
                except Exception as error:
                    self._classify_error(lease, error)
                    retry = await self._report(
                        lease,
                        request_id,
                        failed=True,
                        stream_started=started,
                        usage=stream.usage if stream is not None else None,
                    )
                    if retry and not started:
                        continue
                    if started:
                        raise ModelAPIError(
                            self.model_name,
                            "Provider stream interrupted; details redacted",
                        ) from None
                    raise sanitized_error(error, self.model_name) from None
                except asyncio.CancelledError:
                    await self._report(
                        lease,
                        request_id,
                        cancelled=True,
                        stream_started=started,
                        usage=stream.usage if stream is not None else None,
                    )
                    raise
                await self._report(
                    lease, request_id, stream_started=started, usage=stream.usage
                )
                return
            finally:
                CURRENT_ATTEMPT.reset(marker)
                lease.credential.secret.clear()
                lease.credential.upstream_account_id = None
        raise self._exhausted()
