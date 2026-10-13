"""Credential-free current SDK model index; the daemon owns account availability."""

import re
from collections.abc import Mapping
from typing import Any, NamedTuple, cast

from pydantic_ai.exceptions import UserError
from pydantic_ai.providers import infer_provider_class


class ModelFamily(NamedTuple):
    providers: tuple[str, ...]
    pattern: str
    current: tuple[str, ...]
    tier: str
    strengths: str


# This is the sole policy table for generations, role hints and routing guidance.
FAMILIES = (
    ModelFamily(
        ("anthropic",),
        r"^claude-opus-",
        ("claude-opus-5-5",),
        "frontier",
        "hard reasoning, complex coding, architecture",
    ),
    ModelFamily(
        ("anthropic",),
        r"^claude-sonnet-",
        ("claude-sonnet-5-5",),
        "reasoning",
        "multi-step research, coding",
    ),
    ModelFamily(
        ("anthropic",),
        r"^claude-haiku-",
        ("claude-haiku-5-5",),
        "fast",
        "quick lookups, simple edits",
    ),
    ModelFamily(
        ("openai", "openai-codex"),
        r"(?:^|-)sol(?:$|-)",
        ("gpt-6.1-sol",),
        "reasoning",
        "multi-step research, coding",
    ),
    ModelFamily(
        ("openai", "openai-codex"),
        r"(?:^|-)astra(?:$|-)",
        ("gpt-6-astra",),
        "frontier",
        "hard reasoning, complex coding, architecture",
    ),
    ModelFamily(
        ("openai", "openai-codex"),
        r"(?:^|-)luna(?:$|-)",
        ("gpt-6-luna",),
        "fast",
        "quick lookups, simple edits",
    ),
    ModelFamily(
        ("google",),
        r"(?:^|-)flash(?:$|-)",
        ("gemini-3.8-flash",),
        "fast",
        "quick lookups, simple browsing",
    ),
    ModelFamily(
        ("google",),
        r"(?:^|-)pro(?:$|-)",
        ("gemini-3.1-pro-preview",),
        "reasoning",
        "multi-step research, coding",
    ),
    ModelFamily(
        (),
        r"(?:^|[-_.:/])(mini|nano|lite)(?:$|[-_.:/])",
        (),
        "fast",
        "quick lookups, simple edits",
    ),
    ModelFamily(
        (),
        r"^(?:gpt-[456](?:[.]\d+)?|o[134](?:-preview)?|deepseek-(?:r1|v3)|deepseek-reasoner)$",
        (),
        "reasoning",
        "multi-step research, coding",
    ),
)
CURATED: dict[str, list[str]] = {
    provider: [
        name
        for family in FAMILIES
        if provider in family.providers
        for name in family.current
    ]
    for provider in ("anthropic", "openai", "google", "openai-codex", "github-copilot")
}


def metadata(model_id: str) -> tuple[str, str, str | None]:
    name = model_id.rsplit(":", 1)[-1].lower()
    family = next(
        (family for family in FAMILIES if re.search(family.pattern, name)), None
    )
    if family is None:
        return (
            "reasoning",
            "Explicitly configured model; capabilities not curated",
            None,
        )
    return (
        family.tier,
        family.strengths,
        "fast" if family.tier == "fast" else "reasoning",
    )


async def catalog_report(
    models: Mapping[str, Any] | None = None, provider_names: list[str] | None = None
) -> dict[str, Any]:
    namespaces = set(CURATED)
    namespaces.update(
        name
        for name in cast(list[object], provider_names or [])
        if isinstance(name, str)
    )
    configured: dict[str, set[str]] = {}
    if models is not None:
        selectors: list[Any] = [models.get("default"), models.get("front")]
        fallback = models.get("fallback", [])
        if isinstance(fallback, list):
            selectors.extend(cast(list[Any], fallback))
        for selector in selectors:
            if isinstance(selector, str) and ":" in selector:
                provider, name = selector.split(":", 1)
                namespaces.add(provider)
                if name:
                    configured.setdefault(provider, set()).add(name)
    providers: list[dict[str, Any]] = []
    for provider in sorted(namespaces):
        current = set(CURATED.get(provider, []))
        names = sorted(current | configured.get(provider, set()))
        try:
            infer_provider_class(provider)
        except (ImportError, ValueError, UserError):
            continue
        entries: list[dict[str, Any]] = []
        for name in names:
            tier, strengths, role = metadata(name)
            entries.append(
                {
                    "id": f"{provider}:{name}",
                    "label": name,
                    "recommended_for": role,
                    "in_catalog": name in current,
                    "tier": tier,
                    "strengths": strengths,
                }
            )
        providers.append({"provider": provider, "models": entries})
    return {"providers": providers}
