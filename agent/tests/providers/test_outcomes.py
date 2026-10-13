from datetime import UTC, datetime, timedelta
from email.utils import format_datetime

import pytest

from openside_agent.providers.outcomes import classify_response, retry_after_seconds


@pytest.mark.parametrize(
    "status,body,expected",
    [
        (200, None, "success"),
        (401, {"error": "synthetic-secret-canary"}, "auth"),
        (403, {"error": {"type": "authentication_error"}}, "auth"),
        (403, {"error": {"type": "permission_error"}}, "policy"),
        (429, {"error": {"code": "insufficient_quota"}}, "quota"),
        (429, {"error": "synthetic-secret-canary"}, "rate_limit"),
        (429, {"error": {"code": "rpm_limit_exceeded"}}, "transient"),
        (500, None, "transient"),
        (
            400,
            {"error": {"message": "rate_limit synthetic-secret-canary"}},
            "transient",
        ),
    ],
)
def test_response_classification_is_structured_and_secret_free(status, body, expected):
    outcome = classify_response(status, "31", body)
    assert outcome.kind == expected
    assert outcome.retry_after == (
        31 if expected in ("rate_limit", "quota", "policy") else None
    )
    assert "synthetic-secret-canary" not in repr(outcome)


@pytest.mark.parametrize(
    "value,expected",
    [
        (None, None),
        ("bad", None),
        ("nan", None),
        ("inf", None),
        ("-1", 0),
        ("0.5", 0.5),
        ("99999999999", 86400),
    ],
)
def test_retry_after_is_bounded_and_finite(value, expected):
    assert retry_after_seconds(value) == expected


def test_retry_after_accepts_http_date():
    value = format_datetime(datetime.now(UTC) + timedelta(seconds=60))
    assert 58 <= retry_after_seconds(value) <= 60


def test_model_scoped_policy_and_unknown_reset_remain_distinct():
    outcome = classify_response(
        403, None, {"error": {"scope": "model", "type": "permission_error"}}
    )
    assert outcome.kind == "policy" and outcome.scope == "model"
    assert outcome.retry_after_ms is None
    assert classify_response(429).retry_after_ms is None
