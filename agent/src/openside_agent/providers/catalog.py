"""Credential-free curated SDK model index; the daemon owns account availability."""

from collections.abc import Mapping
from typing import Any, cast

from pydantic_ai.exceptions import UserError
from pydantic_ai.providers import infer_provider_class

# Only chat/tool-capable identifiers supported by the pinned SDKs belong here.
CURATED: dict[str, list[str]] = {
    "anthropic": [
        "claude-opus-5-5",
        "claude-sonnet-5-5",
        "claude-haiku-5-5",
        "claude-opus-4-8",
        "claude-sonnet-4-6",
    ],
    "openai": ["gpt-6.1-sol", "gpt-6-astra", "gpt-6-luna", "gpt-5.4-mini"],
    "google": ["gemini-3.8-flash", "gemini-3.1-pro-preview", "gemini-2.5-pro"],
    "openai-codex": ["gpt-6.1-sol", "gpt-6-astra", "gpt-6-luna", "gpt-5.4-mini"],
    "github-copilot": [],
}


async def catalog_report(
    models: Mapping[str, Any] | None = None, provider_names: list[str] | None = None
) -> dict[str, Any]:
    namespaces = set(CURATED)
    namespaces.update(
        name
        for name in cast(list[object], provider_names or [])
        if isinstance(name, str)
    )
    if models is not None:
        selectors: list[Any] = [models.get("default"), models.get("front")]
        fallback = models.get("fallback", [])
        if isinstance(fallback, list):
            selectors.extend(cast(list[Any], fallback))
        for selector in selectors:
            if isinstance(selector, str) and ":" in selector:
                namespaces.add(selector.split(":", 1)[0])
    providers: list[dict[str, Any]] = []
    for provider in sorted(namespaces):
        names = CURATED.get(provider, [])
        try:
            infer_provider_class(provider)
        except (ImportError, ValueError, UserError):
            continue
        providers.append(
            {
                "provider": provider,
                "models": [
                    {"id": f"{provider}:{name}", "label": name} for name in names
                ],
            }
        )
    return {"providers": providers}
