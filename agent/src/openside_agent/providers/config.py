"""Credential-source ordering is independent of model fallback ordering."""

from collections.abc import Mapping
from typing import Any, cast

DEFAULT_ORDERS: dict[str, list[str]] = {
    "anthropic": ["omp-broker", "env", "command"],
    "openai-codex": ["omp-broker", "codex-cli"],
}
ENV_NAMES: dict[str, list[str]] = {
    "anthropic": ["ANTHROPIC_API_KEY"],
    "openai": ["OPENAI_API_KEY"],
    "openai-chat": ["OPENAI_API_KEY"],
    "openai-responses": ["OPENAI_API_KEY"],
    "openrouter": ["OPENROUTER_API_KEY"],
    "google": ["GOOGLE_API_KEY", "GEMINI_API_KEY"],
    "github-copilot": [
        "GITHUB_COPILOT_API_KEY",
        "GITHUB_COPILOT_API_TOKEN",
        "COPILOT_GITHUB_TOKEN",
    ],
}


def section(config: Mapping[str, Any], name: str) -> Mapping[str, Any]:
    value = config.get(name, {})
    return cast(Mapping[str, Any], value) if isinstance(value, Mapping) else {}


def source_order(config: Mapping[str, Any], provider: str) -> list[str]:
    value = section(config, "order").get(provider)
    if value is None:
        return list(DEFAULT_ORDERS.get(provider, ["env", "command"]))
    if not isinstance(value, list) or not all(
        isinstance(item, str) for item in cast(list[Any], value)
    ):
        raise ValueError("credentials.order must contain lists of source names")
    return cast(list[str], value)


def env_names(config: Mapping[str, Any], provider: str) -> list[str]:
    value = section(config, "env").get(provider)
    if isinstance(value, str):
        return [value]
    if isinstance(value, list) and all(
        isinstance(item, str) for item in cast(list[Any], value)
    ):
        return cast(list[str], value)
    return ENV_NAMES.get(provider, [provider.upper().replace("-", "_") + "_API_KEY"])


def number(config: Mapping[str, Any], key: str, default: float) -> float:
    value = config.get(key, default)
    if isinstance(value, (int, float)) and value >= 0:
        return float(value)
    return default
