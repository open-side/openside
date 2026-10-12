"""Pluggable model credentials and token-free diagnostics.

Source order is evaluated at build time. Missing broker tokens or an absent
provider allow the next source. A configured broker that is unreachable or has
only blocked accounts fails closed. Once selected, a broker model never
silently degrades to local credentials, including through model fallbacks:
AuthUnavailable is deliberately not a FallbackModel fallback condition.
"""

from collections.abc import Mapping
from contextlib import AsyncExitStack
from typing import Any, cast

from pydantic_ai.models import Model
from pydantic_ai.models.fallback import FallbackModel
from pydantic_ai.models.wrapper import WrapperModel

from . import egress
from .config import source_order
from .errors import AuthUnavailable
from .pins import CredentialPins
from .sources import SOURCE_TYPES, CredentialSource

__all__ = [
    "AuthUnavailable",
    "CredentialPins",
    "CredentialSource",
    "build_model",
    "doctor_report",
]


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
    session_id: str,
    pins: CredentialPins | None = None,
) -> Model:
    pins = pins if pins is not None else CredentialPins()
    chain: list[Model] = []
    reasons: list[str] = []
    async with AsyncExitStack() as resources:
        for identifier in [model_id, *fallback]:
            provider = _provider(identifier)
            try:
                order = source_order(credentials, provider)
            except ValueError:
                raise AuthUnavailable(
                    provider, ["invalid credentials.order configuration"]
                ) from None
            for name in order:
                factory = SOURCE_TYPES.get(name)
                if factory is None:
                    reasons.append(f"{provider}: unknown credential source")
                    continue
                source = factory(credentials)
                try:
                    model = await source.build(
                        identifier, session_id=session_id, pins=pins
                    )
                except AuthUnavailable as error:
                    if name == "omp-broker":
                        raise
                    reasons.extend(
                        f"{provider}/{name}: {reason}" for reason in error.reasons
                    )
                    continue
                if model is not None:
                    resources.push_async_callback(_close_candidate, model)
                    chain.append(model)
                    break
                reasons.append(f"{provider}/{name}: unavailable")
        if not chain:
            raise AuthUnavailable(_provider(model_id), reasons)
        for model in chain:
            for url in _endpoints(model):
                egress.allow_endpoint(url)
        for identifier in [model_id, *fallback]:
            for url in egress.AUXILIARY_ENDPOINTS.get(_provider(identifier), ()):
                egress.allow_endpoint(url)
        result = chain[0] if len(chain) == 1 else FallbackModel(chain[0], *chain[1:])
        resources.pop_all()
        return result


def _endpoints(model: Model) -> list[str]:
    """Base URLs of a built model and everything it wraps."""
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
    fallback = models.get("fallback", [])
    fallback = cast(list[Any], fallback) if isinstance(fallback, list) else []
    identifiers = ([default] if isinstance(default, str) else []) + [
        item for item in fallback if isinstance(item, str)
    ]
    providers: list[str] = []
    valid = bool(default)
    for identifier in identifiers:
        try:
            provider = _provider(identifier)
        except AuthUnavailable:
            valid = False
            continue
        if provider not in providers:
            providers.append(provider)
    rows: list[dict[str, Any]] = []
    providers_ok = valid
    for provider in providers:
        try:
            order = source_order(credentials, provider)
        except ValueError:
            order = []
        available = False
        for name in order:
            factory = SOURCE_TYPES.get(name)
            if factory is None:
                rows.append(
                    {
                        "provider": provider,
                        "source": "unknown",
                        "available": False,
                        "reason": "unknown credential source",
                        "next_step": (
                            "Use omp-broker, env, command, codex-cli or builtin"
                        ),
                    }
                )
                continue
            result = factory(credentials).probe(provider)
            rows.append(
                {
                    "provider": provider,
                    "source": name,
                    "available": result.available,
                    "reason": result.reason,
                    "next_step": result.next_step,
                }
            )
            if (
                not available
                and name == "omp-broker"
                and not result.available
                and result.reason
                in (
                    "broker unavailable or invalid snapshot",
                    "no usable broker OAuth account",
                )
            ):
                available = False
                break
            available |= result.available
        providers_ok &= available
    return {
        "credentials": rows,
        "models": {
            "default": default if isinstance(default, str) else None,
            "fallback": fallback,
            "providers_ok": providers_ok,
        },
    }
