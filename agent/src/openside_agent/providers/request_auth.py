"""Inject daemon leases without putting real credentials in persistent SDK clients."""

from collections.abc import AsyncGenerator, Callable, Mapping
from contextvars import ContextVar
from dataclasses import dataclass, field
from typing import Any, Literal, cast

import httpx2
from anthropic import AsyncAnthropic
from openai import AsyncOpenAI
from pydantic_ai.models import Model, infer_model
from pydantic_ai.providers import Provider, infer_provider_class
from pydantic_ai.providers.anthropic import AnthropicProvider
from pydantic_ai.providers.github_copilot import GitHubCopilotProvider
from pydantic_ai.providers.google import GoogleProvider
from pydantic_ai.providers.openai import OpenAIProvider
from pydantic_ai.providers.openai_codex import OpenAICodexProvider

from .anthropic import ClaudeCodeShaping
from .errors import AuthUnavailable
from .models import CODEX_URL, PROVIDER_URLS
from .outcomes import ClassifiedOutcome, classify_response
from .secret import CredentialSecret


@dataclass(repr=False)
class CredentialAttempt:
    provider: str
    source: str
    kind: Literal["api_key", "claude_code", "codex_cli", "bearer"]
    secret: CredentialSecret
    upstream_account_id: str | None = None
    outcome: ClassifiedOutcome = field(
        default_factory=lambda: ClassifiedOutcome("transient")
    )

    def __repr__(self) -> str:
        return "CredentialAttempt(credentials=**********)"


CURRENT_ATTEMPT: ContextVar[CredentialAttempt | None] = ContextVar(
    "model_credential_attempt", default=None
)
AUTH_HEADERS = (
    "authorization",
    "x-api-key",
    "x-goog-api-key",
    "chatgpt-account-id",
    "openai-organization",
    "openai-project",
)


class RequestCredentialAuth(httpx2.Auth):
    def __init__(
        self, provider: str, endpoint: str, credentials: Mapping[str, Any]
    ) -> None:
        self.provider = provider
        self.target = httpx2.URL(endpoint)
        self.shaping = (
            ClaudeCodeShaping(credentials) if provider == "anthropic" else None
        )

    def __repr__(self) -> str:
        return (
            f"RequestCredentialAuth(provider={self.provider!r}, credentials=**********)"
        )

    async def async_auth_flow(
        self, request: httpx2.Request
    ) -> AsyncGenerator[httpx2.Request, httpx2.Response]:
        attempt = CURRENT_ATTEMPT.get()
        if attempt is None or attempt.provider != self.provider:
            raise AuthUnavailable(self.provider, ["daemon credential request required"])
        if (request.url.scheme, request.url.host, request.url.port) != (
            self.target.scheme,
            self.target.host,
            self.target.port,
        ):
            raise AuthUnavailable(
                self.provider, ["refusing credentials to an unexpected origin"]
            )
        await request.aread()
        for header in AUTH_HEADERS:
            request.headers.pop(header, None)
        if (
            self.shaping is not None
            and attempt.kind == "claude_code"
            and request.url.path.endswith("/messages")
        ):
            request = self.shaping.apply(request)
        if self.provider == "openai-codex" and not attempt.upstream_account_id:
            raise AuthUnavailable(
                self.provider, ["daemon credential missing account metadata"]
            )
        try:
            if self.provider == "google" and attempt.kind == "api_key":
                request.headers["x-goog-api-key"] = attempt.secret.reveal()
            elif self.provider == "anthropic" and attempt.kind == "api_key":
                request.headers["x-api-key"] = attempt.secret.reveal()
            else:
                request.headers["authorization"] = f"Bearer {attempt.secret.reveal()}"
            if self.provider == "openai-codex":
                request.headers["chatgpt-account-id"] = cast(
                    str, attempt.upstream_account_id
                )
                request.headers["originator"] = "openside"
            response = yield request
            body = None
            if response.status_code >= 400:
                await response.aread()
                try:
                    body = response.json()
                except (ValueError, UnicodeDecodeError):
                    pass
            attempt.outcome = classify_response(
                response.status_code, response.headers.get("retry-after"), body
            )
        finally:
            for header in AUTH_HEADERS:
                request.headers.pop(header, None)


async def request_model(
    model_id: str,
    credentials: Mapping[str, Any],
    *,
    http_client: httpx2.AsyncClient | None = None,
) -> tuple[Model, httpx2.AsyncClient]:
    provider_name = model_id.split(":", 1)[0]
    http = http_client or httpx2.AsyncClient(trust_env=False, follow_redirects=False)
    provider: Provider[Any]
    try:
        if provider_name == "anthropic":
            provider = AnthropicProvider(
                anthropic_client=AsyncAnthropic(
                    api_key="daemon-managed",
                    webhook_key="",
                    max_retries=0,
                    http_client=http,
                )
            )
        elif provider_name == "openai-codex":
            provider = OpenAICodexProvider(
                openai_client=AsyncOpenAI(
                    api_key="daemon-managed",
                    base_url=CODEX_URL,
                    max_retries=0,
                    http_client=http,
                    admin_api_key="",
                    organization="",
                    project="",
                    webhook_secret="",
                )
            )
        elif provider_name == "github-copilot":
            provider = GitHubCopilotProvider(
                openai_client=AsyncOpenAI(
                    api_key="daemon-managed",
                    base_url=PROVIDER_URLS[provider_name],
                    max_retries=0,
                    http_client=http,
                    admin_api_key="",
                    organization="",
                    project="",
                    webhook_secret="",
                )
            )
        elif provider_name in ("openai", "openai-chat", "openai-responses"):
            provider = OpenAIProvider(
                openai_client=AsyncOpenAI(
                    api_key="daemon-managed",
                    max_retries=0,
                    http_client=http,
                    admin_api_key="",
                    organization="",
                    project="",
                    webhook_secret="",
                )
            )
        elif provider_name == "google":
            from google.genai.types import HttpRetryOptions

            provider = GoogleProvider(
                api_key="daemon-managed",
                http_client=http,
                retry_options=HttpRetryOptions(attempts=1),
            )
        else:
            factory = cast(
                Callable[..., Provider[Any]], infer_provider_class(provider_name)
            )
            provider = factory(api_key="daemon-managed", http_client=http)
            client = provider.client
            if hasattr(client, "max_retries"):
                client.max_retries = 0
        http.auth = RequestCredentialAuth(provider_name, provider.base_url, credentials)
        return infer_model(model_id, provider_factory=lambda _: provider), http
    except BaseException:
        await http.aclose()
        raise
