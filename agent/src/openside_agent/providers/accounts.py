"""Daemon-supplied public metadata; standalone account readiness is unknown."""

from collections.abc import Mapping
from typing import Any, cast

from .config import section
from .subscriptions import statuses


async def accounts_report(
    credentials: Mapping[str, Any], models: Mapping[str, Any]
) -> dict[str, Any]:
    raw_metadata = section(credentials, "_diagnostics").get("accounts")
    metadata = (
        cast(dict[str, Any], raw_metadata) if isinstance(raw_metadata, dict) else None
    )
    if metadata is not None and isinstance(metadata.get("sources"), list):
        return {
            "sources": metadata["sources"],
            "cached_for_ms": metadata.get("cached_for_ms", 0),
            "truncated": metadata.get("truncated", False),
        }
    return {
        "sources": [
            {
                "name": status["id"],
                "kind": status["id"],
                "available": False,
                "reason": "CLI installed; login readiness unknown"
                if status["installed"]
                else "CLI not installed; login readiness unknown",
                "accounts": [],
            }
            for status in statuses()
        ],
        "cached_for_ms": 0,
        "truncated": False,
    }
