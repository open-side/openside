"""Source diagnostics do not acquire credentials or independently decide readiness."""

from collections.abc import Mapping
from typing import Any, cast

from .config import section
from .subscriptions import statuses

SOURCE_NAMES = (
    "omp-broker",
    "vault",
    "env",
    "command",
    "claude-code",
    "codex-cli",
    "builtin",
)


def diagnostic_sources(
    credentials: Mapping[str, Any], providers: list[str]
) -> list[dict[str, Any]]:
    raw_metadata = section(credentials, "_diagnostics").get("accounts")
    metadata = (
        cast(dict[str, Any], raw_metadata) if isinstance(raw_metadata, dict) else None
    )
    rows: list[dict[str, Any]] = []
    if metadata is not None and isinstance(metadata.get("sources"), list):
        for raw_source in cast(list[Any], metadata["sources"]):
            if not isinstance(raw_source, dict):
                continue
            source = cast(dict[str, Any], raw_source)
            if source.get("name") not in SOURCE_NAMES:
                continue
            for provider in providers:
                accounts = source.get("accounts", [])
                relevant = (
                    [
                        cast(dict[str, Any], account)
                        for account in cast(list[Any], accounts)
                        if isinstance(account, dict)
                        and cast(dict[str, Any], account).get("provider") == provider
                    ]
                    if isinstance(accounts, list)
                    else []
                )
                rows.append(
                    {
                        "provider": provider,
                        "source": source["name"],
                        "available": any(
                            account.get("available") is True for account in relevant
                        ),
                        "reason": "Daemon account metadata available"
                        if relevant
                        else "No daemon account metadata for provider",
                        "next_step": (
                            "Manage the provider account pool in Models & accounts"
                        ),
                    }
                )
        return rows
    for provider in providers:
        rows.append(
            {
                "provider": provider,
                "source": "daemon",
                "available": False,
                "reason": "Daemon credential metadata required; readiness unknown",
                "next_step": "Run openside doctor against the daemon",
            }
        )
    for status in statuses():
        rows.append(
            {
                "provider": status["provider"],
                "source": status["id"],
                "available": False,
                "reason": "CLI installed; login readiness unknown"
                if status["installed"]
                else "CLI not installed; login readiness unknown",
                "next_step": "Manage the subscription account in Models & accounts",
            }
        )
    return rows
