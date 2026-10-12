"""Credential plugins return clients, never persist or report credential values."""

import os
import shutil
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Protocol, cast

import httpx
from pydantic_ai.models import Model, infer_model
from pydantic_ai.providers import Provider, infer_provider_class

from . import egress
from .broker import BrokerClient, Snapshot, broker_settings, response_json
from .config import env_names, number, section
from .credential_command import key_bytes
from .errors import AuthUnavailable
from .models import PROVIDER_URLS, SecretSafeModel, broker_model
from .pins import CredentialPins
from .selector import AccountSelector, eligible, matches_provider


@dataclass(frozen=True)
class Availability:
    available: bool
    reason: str
    next_step: str


class CredentialSource(Protocol):
    name: str

    def probe(self, provider: str) -> Availability: ...

    async def build(
        self, model_id: str, *, session_id: str, pins: CredentialPins
    ) -> Model | None: ...


def api_key_provider(provider: str, key: str) -> Provider[Any]:
    factory = cast(Callable[..., Provider[Any]], infer_provider_class(provider))
    return factory(api_key=key)


class EnvSource:
    name = "env"

    def __init__(self, credentials: Mapping[str, Any]) -> None:
        self._config = credentials

    def probe(self, provider: str) -> Availability:
        available = any(
            os.getenv(name, "").strip() for name in env_names(self._config, provider)
        )
        return Availability(
            available,
            "API-key environment configured"
            if available
            else "API-key environment unavailable",
            "Set a provider API-key environment variable or credentials.env",
        )

    async def build(
        self, model_id: str, *, session_id: str, pins: CredentialPins
    ) -> Model | None:
        provider = model_id.split(":", 1)[0]
        key = next(
            (
                os.environ[name].strip()
                for name in env_names(self._config, provider)
                if os.getenv(name, "").strip()
            ),
            None,
        )
        if key is None:
            return None
        try:
            instance = api_key_provider(provider, key)
            return SecretSafeModel(
                infer_model(model_id, provider_factory=lambda _: instance)
            )
        except Exception:
            raise AuthUnavailable(
                provider, ["env: provider client could not be initialized"]
            ) from None


class CommandSource:
    name = "command"

    def __init__(self, credentials: Mapping[str, Any]) -> None:
        self._config = section(credentials, "command")

    def _argv(self, provider: str) -> list[str] | None:
        value = self._config.get(provider)
        if (
            isinstance(value, list)
            and value
            and all(isinstance(arg, str) for arg in cast(list[Any], value))
        ):
            return cast(list[str], value)
        return None

    def probe(self, provider: str) -> Availability:
        argv = self._argv(provider)
        available = (
            os.name == "posix"
            and argv is not None
            and shutil.which(argv[0]) is not None
        )
        return Availability(
            available,
            "command configured; result unverified"
            if available
            else "credential command unavailable",
            "Configure credentials.command.<provider> as an executable argv list",
        )

    async def build(
        self, model_id: str, *, session_id: str, pins: CredentialPins
    ) -> Model | None:
        provider = model_id.split(":", 1)[0]
        argv = self._argv(provider)
        if argv is None:
            return None
        stdout = await key_bytes(
            argv, provider, number(self._config, "timeout_seconds", 10)
        )
        try:
            key = stdout.decode().strip()
            if not key:
                raise ValueError
            instance = api_key_provider(provider, key)
            return SecretSafeModel(
                infer_model(model_id, provider_factory=lambda _: instance)
            )
        except Exception:
            raise AuthUnavailable(
                provider, ["command: invalid credential or provider configuration"]
            ) from None


class CodexCliSource:
    name = "codex-cli"

    def __init__(self, credentials: Mapping[str, Any]) -> None:
        pass

    def probe(self, provider: str) -> Availability:
        path = Path(os.getenv("CODEX_HOME") or Path.home() / ".codex") / "auth.json"
        available = provider == "openai-codex" and path.is_file()
        return Availability(
            available,
            "Codex CLI auth file found; contents unverified"
            if available
            else "Codex CLI auth file unavailable",
            "Sign in with Codex CLI or configure CODEX_HOME",
        )

    async def build(
        self, model_id: str, *, session_id: str, pins: CredentialPins
    ) -> Model | None:
        provider = model_id.split(":", 1)[0]
        if not self.probe(provider).available:
            return None
        try:
            return SecretSafeModel(infer_model(model_id))
        except Exception:
            raise AuthUnavailable(
                provider, ["codex-cli: invalid or unreadable auth file"]
            ) from None


class BuiltinSource:
    name = "builtin"

    def __init__(self, credentials: Mapping[str, Any]) -> None:
        pass

    def probe(self, provider: str) -> Availability:
        available = any(os.getenv(name, "").strip() for name in env_names({}, provider))
        if provider == "openai-codex":
            return CodexCliSource({}).probe(provider)
        return Availability(
            available,
            "built-in environment configured"
            if available
            else "built-in authentication unverified",
            "Configure the Pydantic AI provider's documented environment",
        )

    async def build(
        self, model_id: str, *, session_id: str, pins: CredentialPins
    ) -> Model | None:
        try:
            return SecretSafeModel(infer_model(model_id))
        except Exception:
            return None


class OmpBrokerSource:
    name = "omp-broker"

    def __init__(self, credentials: Mapping[str, Any]) -> None:
        self._config = credentials

    def probe(self, provider: str) -> Availability:
        url, token, reason = broker_settings(self._config)
        next_step = (
            "Configure OPENSIDE_BROKER_URL and OPENSIDE_BROKER_TOKEN "
            "or credentials.omp_broker.token_file"
        )
        if token is None or provider not in PROVIDER_URLS:
            return Availability(
                False,
                reason if token is None else "broker provider unsupported",
                next_step,
            )
        try:
            with httpx.Client(
                timeout=number(
                    section(self._config, "omp_broker"), "timeout_seconds", 10
                ),
                trust_env=False,
            ) as http:
                with http.stream(
                    "GET",
                    url + "/v1/snapshot",
                    headers={
                        "Authorization": f"Bearer {token.get_secret_value()}",
                        "OMP-Auth-Broker-Capabilities": "codex-meter-block-scopes",
                    },
                ) as response:
                    snapshot = Snapshot.parse(response_json(response))
                present = any(
                    matches_provider(entry, provider) for entry in snapshot.credentials
                )
                available = any(
                    matches_provider(entry, provider)
                    and eligible(entry, snapshot.server_now_ms)
                    and entry.credential.expires > snapshot.server_now_ms
                    for entry in snapshot.credentials
                )
        except (httpx.HTTPError, ValueError, OSError):
            return Availability(
                False, "broker unavailable or invalid snapshot", next_step
            )
        return Availability(
            available,
            "broker OAuth account available"
            if available
            else (
                "no usable broker OAuth account"
                if present
                else "broker provider absent"
            ),
            next_step,
        )

    async def build(
        self, model_id: str, *, session_id: str, pins: CredentialPins
    ) -> Model | None:
        provider = model_id.split(":", 1)[0]
        if provider not in PROVIDER_URLS:
            return None
        broker = BrokerClient.from_config(self._config)
        if broker is None:
            return None
        egress.allow_endpoint(broker_settings(self._config)[0])
        egress.allow_endpoint(PROVIDER_URLS[provider])
        try:
            snapshot = await broker.snapshot()
            if not any(
                matches_provider(entry, provider) for entry in snapshot.credentials
            ):
                await broker.aclose()
                return None
            selector = AccountSelector(broker, provider, session_id, pins)
            await selector.select(touch=False)
            return broker_model(model_id, selector, self._config)
        except Exception as error:
            await broker.aclose()
            if isinstance(error, AuthUnavailable):
                raise AuthUnavailable(
                    provider,
                    [
                        "omp-broker: configured broker unavailable "
                        "or all accounts blocked"
                    ],
                ) from None
            raise AuthUnavailable(
                provider, ["omp-broker: provider initialization failed"]
            ) from None


SOURCE_TYPES: dict[str, Callable[[Mapping[str, Any]], CredentialSource]] = {
    "omp-broker": OmpBrokerSource,
    "env": EnvSource,
    "command": CommandSource,
    "codex-cli": CodexCliSource,
    "builtin": BuiltinSource,
}
