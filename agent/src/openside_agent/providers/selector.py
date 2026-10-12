"""Broker-owned eligibility with token-free, session-local affinity.

The omp contract keeps a present, unblocked session pin even when another
account has more quota. Anthropic's pin expires after one idle hour. Without a
usable pin, prefer remaining quota, then token expiry, then the stable account
id; a scoped rate-limit block releases the pin before the retry. Affinity
snapshots therefore contain identifiers/timestamps only, never credentials.
"""

import asyncio
import time
from collections.abc import Callable
from typing import Any, cast

from .broker import BrokerClient, BrokerCredential, Snapshot
from .errors import AuthUnavailable
from .pins import CredentialPins


def matches_provider(entry: BrokerCredential, provider: str) -> bool:
    return entry.provider == provider or (
        provider == "github-copilot" and entry.provider in ("copilot", "github-copilot")
    )


def eligible(entry: BrokerCredential, now_ms: float) -> bool:
    return not any(block.blocked_until_ms > now_ms for block in entry.blocks)


def remaining_quota(entry: BrokerCredential, reports: list[dict[str, Any]]) -> float:
    fractions: list[float] = []
    for report in reports:
        if not matches_provider(entry, str(report.get("provider", ""))):
            continue
        raw_metadata = report.get("metadata", {})
        metadata = (
            cast(dict[str, Any], raw_metadata) if isinstance(raw_metadata, dict) else {}
        )
        account_id = entry.credential.account_id
        attributed = (
            metadata.get("credentialId") == entry.id
            or (account_id is not None and metadata.get("accountId") == account_id)
            or (
                entry.credential.email is not None
                and metadata.get("email") == entry.credential.email
            )
        )
        raw_limits = report.get("limits", [])
        if not isinstance(raw_limits, list):
            continue
        for raw_limit in cast(list[Any], raw_limits):
            if not isinstance(raw_limit, dict):
                continue
            limit = cast(dict[str, Any], raw_limit)
            raw_scope = limit.get("scope", {})
            scope = (
                cast(dict[str, Any], raw_scope) if isinstance(raw_scope, dict) else {}
            )
            org = scope.get("orgId", metadata.get("orgId"))
            if entry.credential.org_id is not None and org != entry.credential.org_id:
                continue
            scope_match = (
                account_id is not None and scope.get("accountId") == account_id
            )
            if not attributed and not scope_match:
                continue
            raw_amount = limit.get("amount", {})
            if not isinstance(raw_amount, dict):
                continue
            amount = cast(dict[str, Any], raw_amount)
            remaining = amount.get("remainingFraction")
            used = amount.get("usedFraction")
            total = amount.get("limit")
            if isinstance(remaining, (int, float)):
                fractions.append(float(remaining))
            elif isinstance(used, (int, float)):
                fractions.append(1 - float(used))
            elif isinstance(total, (int, float)) and total > 0:
                left = amount.get("remaining")
                consumed = amount.get("used")
                if isinstance(left, (int, float)):
                    fractions.append(left / total)
                elif isinstance(consumed, (int, float)):
                    fractions.append(1 - consumed / total)
    return min(fractions) if fractions else -1.0


class AccountSelector:
    def __init__(
        self,
        broker: BrokerClient,
        provider: str,
        session_id: str,
        pins: CredentialPins,
        *,
        clock: Callable[[], float] = time.time,
    ) -> None:
        self.broker = broker
        self.provider = provider
        self.session_id = session_id
        self.pins = pins
        self._clock = clock
        self._lock = asyncio.Lock()
        self._blocked: dict[int, float] = {}

    def __repr__(self) -> str:
        return f"AccountSelector(provider={self.provider!r})"

    def candidates(self, snapshot: Snapshot) -> list[BrokerCredential]:
        now_ms = self._clock() * 1000
        present = {entry.id for entry in snapshot.credentials}
        self._blocked = {
            cid: until
            for cid, until in self._blocked.items()
            if until > now_ms and cid in present
        }
        return [
            entry
            for entry in snapshot.credentials
            if matches_provider(entry, self.provider)
            and eligible(entry, max(now_ms, snapshot.server_now_ms))
            and self._blocked.get(entry.id, 0) <= now_ms
        ]

    async def select(
        self, *, force: bool = False, touch: bool = True
    ) -> BrokerCredential:
        async with self._lock:
            snapshot = await self.broker.snapshot(force=force)
            candidates = self.candidates(snapshot)
            if not candidates:
                raise AuthUnavailable(
                    self.provider, ["omp-broker: no unblocked OAuth account"]
                )
            now = self._clock()
            pin = self.pins.get(self.session_id, self.provider)
            if (
                pin is not None
                and self.provider == "anthropic"
                and now - pin.last_request_at >= 3600
            ):
                self.pins.release(self.session_id, self.provider)
                pin = None
            chosen = next(
                (
                    entry
                    for entry in candidates
                    if pin is not None and entry.id == pin.credential_id
                ),
                None,
            )
            if chosen is None:
                reports = await self.broker.usage()
                chosen = max(
                    candidates,
                    key=lambda entry: (
                        remaining_quota(entry, reports),
                        entry.credential.expires,
                        -entry.id,
                    ),
                )
            if touch:
                self.pins.set(self.session_id, self.provider, chosen.id, now)
            return chosen

    async def block(
        self, entry: BrokerCredential, *, retry_after_seconds: float = 60
    ) -> None:
        until = (self._clock() + max(1, retry_after_seconds)) * 1000
        async with self._lock:
            self._blocked[entry.id] = until
            self.pins.release(self.session_id, self.provider)
            await self.broker.block(entry.id, entry.provider, blocked_until_ms=until)
