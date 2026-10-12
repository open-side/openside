"""SDK injection opts out of Pydantic AI's Codex OAuth refresh lifecycle.

Installed source: pydantic_ai/providers/openai_codex.py:520-536 documents
openai_client injection; models/wrapper.py:44-97 defines lifecycle delegation.
Application auth re-reads broker snapshots only, including stale/401 recovery.
"""

import asyncio
import time
from collections.abc import AsyncGenerator, Mapping
from contextlib import asynccontextmanager
from types import TracebackType
from typing import Any, Self

import httpx2
from anthropic import AsyncAnthropic
from openai import AsyncOpenAI
from pydantic_ai import RunContext
from pydantic_ai.exceptions import ModelAPIError, ModelHTTPError
from pydantic_ai.messages import ModelMessage, ModelResponse
from pydantic_ai.models import (
    Model,
    ModelRequestParameters,
    StreamedResponse,
    infer_model,
)
from pydantic_ai.models.wrapper import WrapperModel
from pydantic_ai.providers.anthropic import AnthropicProvider
from pydantic_ai.providers.github_copilot import GitHubCopilotProvider
from pydantic_ai.providers.openai_codex import (
    OpenAICodexCredentials,
    OpenAICodexProvider,
)
from pydantic_ai.settings import ModelSettings

from .anthropic import ClaudeCodeShaping
from .broker import BrokerClient, BrokerCredential
from .config import section
from .errors import AuthUnavailable
from .selector import AccountSelector

CODEX_URL = "https://chatgpt.com/backend-api/codex"
PROVIDER_URLS = {
    "openai-codex": CODEX_URL,
    "anthropic": "https://api.anthropic.com",
    "github-copilot": "https://api.githubcopilot.com",
}


class CodexBrokerSource:
    def __init__(self, selector: AccountSelector) -> None:
        self.selector = selector

    async def load(self) -> OpenAICodexCredentials:
        entry = await self.selector.select(force=True)
        if not entry.credential.account_id:
            raise AuthUnavailable(
                "openai-codex", ["omp-broker: missing account metadata"]
            )
        return OpenAICodexCredentials(
            access_token=entry.credential.access.get_secret_value(),
            refresh_token="broker-owned-refresh-not-permitted",
            account_id=entry.credential.account_id,
        )

    async def save(self, credentials: OpenAICodexCredentials) -> None:
        raise AuthUnavailable("openai-codex", ["broker credentials are read-only"])


class BrokerOnlyAuth(httpx2.Auth):
    def __init__(
        self,
        selector: AccountSelector,
        *,
        shaping: ClaudeCodeShaping | None = None,
    ) -> None:
        self.selector = selector
        self.shaping = shaping
        self._lock = asyncio.Lock()
        self._current: BrokerCredential | None = None

    def __repr__(self) -> str:
        return f"BrokerOnlyAuth(provider={self.selector.provider!r}, token=**********)"

    async def prepare(self) -> BrokerCredential:
        async with self._lock:
            current = await self.selector.select()
            if current.credential.expires <= time.time() * 1000 + 30_000:
                current = await self.selector.select(force=True)
                if current.credential.expires <= time.time() * 1000 + 30_000:
                    raise AuthUnavailable(
                        self.selector.provider,
                        ["omp-broker: token stale; broker must refresh"],
                    )
            self._current = current
            return current

    async def recover_401(self, rejected: BrokerCredential) -> BrokerCredential:
        async with self._lock:
            candidate = self._current
            if candidate is None or candidate == rejected:
                candidate = await self.selector.select(force=True)
            if (
                candidate == rejected
                or candidate.credential.expires <= time.time() * 1000 + 30_000
            ):
                raise AuthUnavailable(
                    self.selector.provider,
                    ["omp-broker: rejected credentials unchanged"],
                )
            self._current = candidate
            return candidate

    def inject(
        self, request: httpx2.Request, entry: BrokerCredential
    ) -> httpx2.Request:
        request.headers["Authorization"] = (
            f"Bearer {entry.credential.access.get_secret_value()}"
        )
        request.headers.pop("x-api-key", None)
        if self.selector.provider == "openai-codex":
            if not entry.credential.account_id:
                raise AuthUnavailable(
                    "openai-codex", ["omp-broker: missing account metadata"]
                )
            request.headers["chatgpt-account-id"] = entry.credential.account_id
            request.headers["originator"] = "openside"
        if self.shaping is not None and request.url.path.endswith("/messages"):
            request = self.shaping.apply(request)
        return request

    async def async_auth_flow(
        self, request: httpx2.Request
    ) -> AsyncGenerator[httpx2.Request, httpx2.Response]:
        target = httpx2.URL(PROVIDER_URLS[self.selector.provider])
        if (request.url.scheme, request.url.host, request.url.port) != (
            target.scheme,
            target.host,
            target.port,
        ):
            raise AuthUnavailable(
                self.selector.provider, ["refusing credentials to an unexpected origin"]
            )
        await request.aread()
        used = await self.prepare()
        retried_401 = False
        retried_version = False
        tried: set[int] = set()
        while True:
            tried.add(used.id)
            request = self.inject(request, used)
            response = yield request
            if response.status_code < 400:
                return
            await response.aread()
            if (
                self.shaping is not None
                and "claude_code_version_too_old" in response.text
            ):
                if not retried_version and self.shaping.adopt_required_version(
                    response
                ):
                    retried_version = True
                    await response.aclose()
                    continue
                return
            if response.status_code == 401:
                if retried_401:
                    raise AuthUnavailable(
                        self.selector.provider,
                        ["omp-broker: replacement credential rejected"],
                    )
                used = await self.recover_401(used)
                retried_401 = True
                await response.aclose()
                continue
            if response.status_code == 429 or "rate_limit" in response.text:
                retry_after = response.headers.get("retry-after", "60")
                try:
                    delay = min(86400, max(1, float(retry_after)))
                except ValueError:
                    delay = 60
                await self.selector.block(used, retry_after_seconds=delay)
                used = await self.selector.select(force=True)
                if used.id in tried:
                    raise AuthUnavailable(
                        self.selector.provider,
                        ["omp-broker: all attempted accounts rate limited"],
                    )
                await response.aclose()
                continue
            return


def sanitized_error(error: Exception, model_name: str) -> Exception:
    cause: BaseException | None = error
    seen: set[int] = set()
    while cause is not None and id(cause) not in seen:
        seen.add(id(cause))
        if isinstance(cause, AuthUnavailable):
            return cause
        cause = cause.__cause__
    if isinstance(error, ModelHTTPError):
        return ModelHTTPError(
            error.status_code, model_name, body="provider rejected request"
        )
    return ModelAPIError(
        model_name, "Provider request failed; response details redacted"
    )


class SecretSafeModel(WrapperModel):
    def __repr__(self) -> str:
        return f"SecretSafeModel(provider={self.system!r}, credentials=**********)"

    async def request(
        self,
        messages: list[ModelMessage],
        model_settings: ModelSettings | None,
        model_request_parameters: ModelRequestParameters,
    ) -> ModelResponse:
        try:
            return await self.wrapped.request(
                messages, model_settings, model_request_parameters
            )
        except Exception as error:
            raise sanitized_error(error, self.model_name) from None

    @asynccontextmanager
    async def request_stream(
        self,
        messages: list[ModelMessage],
        model_settings: ModelSettings | None,
        model_request_parameters: ModelRequestParameters,
        run_context: RunContext[Any] | None = None,
    ) -> AsyncGenerator[StreamedResponse]:
        started = False
        try:
            async with self.wrapped.request_stream(
                messages, model_settings, model_request_parameters, run_context
            ) as stream:
                started = True
                yield stream
        except Exception as error:
            # Never retry a stream after emitting a partial result.
            if started:
                raise ModelAPIError(
                    self.model_name, "Provider stream interrupted; details redacted"
                ) from None
            raise sanitized_error(error, self.model_name) from None


class BrokerModel(SecretSafeModel):
    def __init__(
        self,
        wrapped: Model,
        broker: BrokerClient,
        http: httpx2.AsyncClient,
        *,
        sse: bool = False,
    ) -> None:
        super().__init__(wrapped)
        self.broker = broker
        self.http = http
        self.sse = sse
        self._stream_task: asyncio.Task[None] | None = None
        self._entered = 0

    def __repr__(self) -> str:
        return f"BrokerModel(provider={self.system!r}, credentials=**********)"

    async def _watch(self) -> None:
        try:
            async for _ in self.broker.snapshot_stream():
                pass
        except AuthUnavailable:
            # SSE is optional; a disconnected stream falls back to TTL polling.
            pass

    async def __aenter__(self) -> Self:
        self._entered += 1
        if self.sse and self._stream_task is None:
            self._stream_task = asyncio.create_task(self._watch())
        await self.wrapped.__aenter__()
        return self

    async def aclose(self) -> None:
        if self._stream_task is not None:
            self._stream_task.cancel()
            try:
                await self._stream_task
            except asyncio.CancelledError:
                pass
            self._stream_task = None
        await self.http.aclose()
        await self.broker.aclose()

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc_val: BaseException | None,
        exc_tb: TracebackType | None,
    ) -> bool | None:
        result = await self.wrapped.__aexit__(exc_type, exc_val, exc_tb)
        self._entered -= 1
        if self._entered <= 0:
            await self.aclose()
        return result


def broker_model(
    model_id: str,
    selector: AccountSelector,
    credentials: Mapping[str, Any],
    *,
    http_client: httpx2.AsyncClient | None = None,
) -> BrokerModel:
    provider_name = selector.provider
    shaping = ClaudeCodeShaping(credentials) if provider_name == "anthropic" else None
    http = http_client or httpx2.AsyncClient(trust_env=False)
    http.auth = BrokerOnlyAuth(selector, shaping=shaping)
    if provider_name == "anthropic":
        client = AsyncAnthropic(
            api_key="", auth_token="broker-managed", max_retries=0, http_client=http
        )
        provider = AnthropicProvider(anthropic_client=client)
    elif provider_name == "openai-codex":
        client = AsyncOpenAI(
            api_key="broker-managed",
            base_url=CODEX_URL,
            max_retries=0,
            http_client=http,
        )
        provider = OpenAICodexProvider(openai_client=client)
    else:
        client = AsyncOpenAI(
            api_key="broker-managed",
            base_url=PROVIDER_URLS[provider_name],
            max_retries=0,
            http_client=http,
        )
        provider = GitHubCopilotProvider(openai_client=client)
    model = infer_model(model_id, provider_factory=lambda _: provider)
    settings = section(credentials, "omp_broker")
    return BrokerModel(
        model,
        selector.broker,
        http,
        sse=settings.get("sse") is True,
    )
