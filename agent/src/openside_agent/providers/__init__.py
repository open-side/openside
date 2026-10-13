"""Daemon-leased model credentials and credential-free sidecar diagnostics."""

from collections.abc import Mapping
from contextlib import AsyncExitStack
from typing import TYPE_CHECKING, Any, cast

from pydantic_ai.models import Model
from pydantic_ai.models.fallback import FallbackModel
from pydantic_ai.models.wrapper import WrapperModel

from . import egress
from .errors import AuthUnavailable
from .sources import diagnostic_sources

if TYPE_CHECKING:
    from .request_model import CredentialRpc

__all__ = ["AuthUnavailable", "build_model", "doctor_report"]


def _provider(model_id: str) -> str:
    provider, separator, name = model_id.partition(":")
    if not separator or not provider or not name:
        raise AuthUnavailable(
            "model", ["expected an explicit provider:model identifier"]
        )
    return provider


async def _close_candidate(model: Model) -> None:
    async with model:
        pass


async def build_model(
    model_id: str,
    fallback: list[str],
    credentials: Mapping[str, Any],
    *,
    credential_rpc: "CredentialRpc",
    subagent: str | None = None,
) -> Model:
    from .request_auth import request_model as build_request_model
    from .request_model import RequestCredentialModel

    chain: list[Model] = []
    async with AsyncExitStack() as resources:
        for identifier in [model_id, *fallback]:
            provider = _provider(identifier)
            try:
                wrapped, http = await build_request_model(identifier, credentials)
            except Exception:
                raise AuthUnavailable(
                    provider,
                    ["Provider SDK could not be initialized"],
                    reason="provider_unavailable",
                ) from None
            model = RequestCredentialModel(
                wrapped, http, credential_rpc, model_id=identifier, subagent=subagent
            )
            resources.push_async_callback(_close_candidate, model)
            chain.append(model)
            for url in _endpoints(model):
                egress.allow_endpoint(url)
        result = chain[0] if len(chain) == 1 else FallbackModel(chain[0], *chain[1:])
        resources.pop_all()
        return result


def _endpoints(model: Model) -> list[str]:
    if isinstance(model, FallbackModel):
        return [url for inner in model.models for url in _endpoints(inner)]
    urls = [model.base_url] if model.base_url else []
    if isinstance(model, WrapperModel):
        urls.extend(_endpoints(model.wrapped))
    return urls


def doctor_report(
    credentials: Mapping[str, Any], models: Mapping[str, Any]
) -> dict[str, Any]:
    default = models.get("default")
    configured_fallback = models.get("fallback", [])
    fallback = (
        cast(list[Any], configured_fallback)
        if isinstance(configured_fallback, list)
        else []
    )
    identifiers = ([default] if isinstance(default, str) else []) + [
        value for value in fallback if isinstance(value, str)
    ]
    providers = list(
        dict.fromkeys(value.split(":", 1)[0] for value in identifiers if ":" in value)
    )
    rows = diagnostic_sources(credentials, providers)
    return {
        "credentials": rows,
        "models": {
            "default": default if isinstance(default, str) else None,
            "fallback": fallback,
            "providers_ok": bool(providers)
            and all(
                any(row["provider"] == provider and row["available"] for row in rows)
                for provider in providers
            ),
        },
    }
