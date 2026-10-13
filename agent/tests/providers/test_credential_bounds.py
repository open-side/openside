import json

import pytest
from pydantic import ValidationError
from pydantic_ai.models.test import TestModel
from runtime_helpers import fake_daemon

from openside_agent.protocol import INBOUND_ADAPTER, OUTBOUND_ADAPTER

CANARY = "synthetic-private-wire-secret-canary"


def response(**fields):
    return {
        "v": 1,
        "type": "credential_response",
        "id": "rpc1",
        "request_id": "model1",
        "lease_id": "lease1",
        "generation": 1,
        "account": {"source": "vault", "account_id": "opaque1"},
        "auth": {"kind": "api_key", "value": CANARY},
        **fields,
    }


@pytest.mark.parametrize("attempt", [-1, 64, True])
def test_request_attempt_is_zero_based_and_bounded(attempt):
    with pytest.raises(ValidationError):
        OUTBOUND_ADAPTER.validate_python(
            {
                "v": 1,
                "type": "credential_request",
                "id": "rpc1",
                "request_id": "model1",
                "provider": "anthropic",
                "model": "anthropic:test",
                "attempt": attempt,
            }
        )


@pytest.mark.parametrize(
    "auth",
    [
        {
            "kind": "api_key",
            "value": CANARY,
            "refresh_token": "synthetic-refresh-canary",
        },
        {"kind": "api_key", "value": CANARY, "credential_home": "/fake/private-home"},
        {"kind": "api_key", "value": "x" * 16385},
    ],
)
async def test_invalid_private_response_is_rejected_without_echo(auth):
    async with fake_daemon(model=TestModel()) as (daemon, _):
        daemon.writer.write(json.dumps(response(auth=auth)).encode() + b"\n")
        await daemon.writer.drain()
        error = await daemon.receive()
        assert error["code"] == "protocol" and not error["fatal"]
        assert CANARY not in json.dumps(error)
        assert "synthetic-refresh-canary" not in json.dumps(error)
        await daemon.open()


def test_private_snapshot_repr_is_redacted():
    parsed = INBOUND_ADAPTER.validate_json(json.dumps(response()))
    assert CANARY not in repr(parsed) + repr(parsed.auth)
    assert parsed.auth.value.get_secret_value() == CANARY
