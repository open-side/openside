"""Bounded public account metadata; credential values never cross this boundary."""

import math
import re
from collections.abc import Mapping
from typing import Any, cast

from .broker import BrokerClient, BrokerCredential
from .config import section, source_order
from .errors import AuthUnavailable
from .models import PROVIDER_URLS
from .selector import matches_provider
from .sources import SOURCE_TYPES

MAX_SOURCES = 32
MAX_ACCOUNTS = 128
MAX_MODELS = 64
MAX_WINDOWS = 16
CACHE_MS = 5000


def finite(value: Any) -> float | None:
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        result = float(value)
        if math.isfinite(result):
            return result
    return None


def usage_windows(
    entry: BrokerCredential, reports: list[dict[str, Any]]
) -> list[dict[str, Any]]:
    windows: list[dict[str, Any]] = []
    for report in reports:
        if not matches_provider(entry, str(report.get("provider", ""))):
            continue
        metadata = section(report, "metadata")
        attributed = (
            metadata.get("credentialId") == entry.id
            or (
                entry.credential.account_id is not None
                and metadata.get("accountId") == entry.credential.account_id
            )
            or (
                entry.credential.email is not None
                and metadata.get("email") == entry.credential.email
            )
        )
        limits = report.get("limits", [])
        if not isinstance(limits, list):
            continue
        for raw in cast(list[Any], limits):
            if not isinstance(raw, dict):
                continue
            limit = cast(dict[str, Any], raw)
            scope = section(limit, "scope")
            if (
                entry.credential.org_id is not None
                and scope.get("orgId", metadata.get("orgId")) != entry.credential.org_id
            ):
                continue
            if not attributed and not (
                entry.credential.account_id is not None
                and scope.get("accountId") == entry.credential.account_id
            ):
                continue
            amount = section(limit, "amount")
            used = finite(amount.get("usedFraction"))
            remaining = finite(amount.get("remainingFraction"))
            total = finite(amount.get("limit"))
            consumed = finite(amount.get("used"))
            left = finite(amount.get("remaining"))
            if used is None and total is not None and total > 0:
                if consumed is not None:
                    used = consumed / total
                elif left is not None:
                    used = 1 - left / total
            if (
                used is None
                and amount.get("unit") == "percent"
                and consumed is not None
            ):
                used = consumed / 100
            if used is None and remaining is not None:
                used = 1 - remaining
            # Broker names and metadata may contain identifiers or arbitrary text.
            window = section(limit, "window")
            window_id = window.get("id", "")
            name = (
                window_id
                if isinstance(window_id, str)
                and re.fullmatch(
                    r"(?:[0-9]{1,4}[smhdw]|daily|weekly|monthly)", window_id
                )
                else f"Window {len(windows) + 1}"
            )
            windows.append(
                {
                    "name": name,
                    "used_percent": min(100, max(0, used * 100))
                    if used is not None
                    else None,
                    "reset_at_ms": finite(window.get("resetsAt")),
                }
            )
            if len(windows) > MAX_WINDOWS:
                return windows
    return windows


async def accounts_report(
    credentials: Mapping[str, Any], models: Mapping[str, Any]
) -> dict[str, Any]:
    identifiers: list[str] = []
    default = models.get("default")
    if isinstance(default, str) and ":" in default:
        identifiers.append(default)
    fallback = models.get("fallback", [])
    if isinstance(fallback, list):
        identifiers.extend(
            item
            for item in cast(list[Any], fallback)
            if isinstance(item, str) and ":" in item
        )
    by_provider: dict[str, list[str]] = {}
    truncated = len(identifiers) > MAX_MODELS
    for identifier in identifiers[:MAX_MODELS]:
        if len(identifier) > 256:
            truncated = True
            continue
        provider = identifier.split(":", 1)[0]
        by_provider.setdefault(provider, []).append(identifier)
    for key in ("order", "env", "command"):
        for provider in section(credentials, key):
            if provider != "timeout_seconds" and len(provider) <= 64:
                by_provider.setdefault(provider, [])
    requested: dict[str, list[str]] = {}
    for provider in by_provider:
        try:
            order = source_order(credentials, provider)
        except ValueError:
            order = []
        for name in order:
            if name in SOURCE_TYPES:
                requested.setdefault(name, []).append(provider)
    if "omp_broker" in credentials:
        requested.setdefault("omp-broker", [])
    sources: list[dict[str, Any]] = []
    for name, providers in list(requested.items())[:MAX_SOURCES]:
        accounts: list[dict[str, Any]] = []
        available = False
        reason = "credential source unavailable"
        if name == "omp-broker":
            broker = BrokerClient.from_config(credentials)
            if broker is not None:
                try:
                    snapshot = await broker.snapshot()
                    reports = await broker.usage()
                    for entry in snapshot.credentials[:MAX_ACCOUNTS]:
                        provider = (
                            "github-copilot"
                            if entry.provider == "copilot"
                            else entry.provider
                            if entry.provider in PROVIDER_URLS
                            else "unknown"
                        )
                        blocks = [
                            block.blocked_until_ms
                            for block in entry.blocks
                            if block.blocked_until_ms > snapshot.server_now_ms
                        ]
                        blocked = max(blocks) if blocks else None
                        available |= (
                            provider != "unknown"
                            and blocked is None
                            and entry.credential.expires > snapshot.server_now_ms
                        )
                        windows = usage_windows(entry, reports)
                        truncated |= len(windows) > MAX_WINDOWS
                        accounts.append(
                            {
                                "id": f"broker:{entry.id}",
                                "label": f"Broker account {entry.id}",
                                "provider": provider,
                                "models": by_provider.get(provider, []),
                                "usage_windows": windows[:MAX_WINDOWS],
                                "blocked_until_ms": blocked,
                            }
                        )
                    truncated |= len(snapshot.credentials) > MAX_ACCOUNTS
                    reason = (
                        "broker account available"
                        if available
                        else "no usable broker OAuth account"
                    )
                except (AuthUnavailable, ValueError):
                    reason = "broker unavailable or invalid snapshot"
                finally:
                    await broker.aclose()
        else:
            source = SOURCE_TYPES[name](credentials)
            for provider in providers[:MAX_ACCOUNTS]:
                probe = source.probe(provider)
                available |= probe.available
                if probe.available:
                    accounts.append(
                        {
                            "id": f"{name}:{provider}",
                            "label": f"{name} account",
                            "provider": provider,
                            "models": by_provider[provider],
                            "usage_windows": [],
                            "blocked_until_ms": None,
                        }
                    )
            reason = (
                "credential source configured; result unverified"
                if available
                else "credential source unavailable"
            )
            truncated |= len(providers) > MAX_ACCOUNTS
        sources.append(
            {
                "name": name,
                "kind": name,
                "available": available,
                "reason": reason,
                "accounts": accounts,
            }
        )
    return {
        "sources": sources,
        "cached_for_ms": CACHE_MS,
        "truncated": truncated or len(requested) > MAX_SOURCES,
    }
