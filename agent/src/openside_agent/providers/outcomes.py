"""Credential health reports contain classifications, never provider response text."""

import math
from dataclasses import dataclass
from datetime import UTC, datetime
from email.utils import parsedate_to_datetime
from typing import Any, Literal, cast

OutcomeKind = Literal[
    "success", "auth", "rate_limit", "quota", "policy", "transient", "cancelled"
]
QUOTA_CODES = frozenset(
    {
        "insufficient_quota",
        "quota_exhausted",
        "quota_exceeded",
        "usage_limit_reached",
        "billing_hard_limit_reached",
        "credit_balance_too_low",
    }
)
AUTH_CODES = frozenset(
    {"invalid_api_key", "authentication_error", "invalid_token", "unauthorized"}
)
TRANSIENT_LIMIT_CODES = frozenset(
    {
        "concurrency_limit_exceeded",
        "requests_per_minute_exceeded",
        "tokens_per_minute_exceeded",
        "rpm_limit_exceeded",
        "tpm_limit_exceeded",
    }
)


@dataclass(frozen=True)
class ClassifiedOutcome:
    kind: OutcomeKind
    retry_after: float | None = None
    scope: Literal["account", "model"] = "account"

    @property
    def retry_after_ms(self) -> int | None:
        return (
            math.ceil(self.retry_after * 1000) if self.retry_after is not None else None
        )


def retry_after_seconds(value: str | None) -> float | None:
    if value is None:
        return None
    try:
        seconds = float(value)
    except ValueError:
        try:
            date = parsedate_to_datetime(value)
            if date.tzinfo is None:
                date = date.replace(tzinfo=UTC)
            seconds = (date - datetime.now(UTC)).total_seconds()
        except (ValueError, TypeError, OverflowError):
            return None
    return min(86400.0, max(0.0, seconds)) if math.isfinite(seconds) else None


def classify_response(
    status: int, retry_after: str | None = None, body: Any = None
) -> ClassifiedOutcome:
    if 200 <= status < 300:
        return ClassifiedOutcome("success")
    raw_error: Any = (
        cast(dict[str, Any], body).get("error", {}) if isinstance(body, dict) else {}
    )
    error: dict[str, Any] = (
        cast(dict[str, Any], raw_error) if isinstance(raw_error, dict) else {}
    )
    codes = {
        cast(str, error.get(name))
        for name in ("type", "code")
        if isinstance(error.get(name), str)
    }
    scope: Literal["account", "model"] = (
        "model" if error.get("scope") == "model" else "account"
    )
    delay = retry_after_seconds(retry_after)
    if status == 401 or (status == 403 and codes & AUTH_CODES):
        return ClassifiedOutcome("auth")
    if status in (402, 403, 429) and codes & QUOTA_CODES:
        return ClassifiedOutcome("quota", delay, scope)
    if status == 403:
        return ClassifiedOutcome("policy", delay, scope)
    if status == 429:
        if codes & TRANSIENT_LIMIT_CODES:
            return ClassifiedOutcome("transient")
        return ClassifiedOutcome("rate_limit", delay)
    return ClassifiedOutcome("transient")
