import copy
import json
import os
import re
from pathlib import Path
from typing import Any

import httpx
import pytest
from jsonschema import Draft202012Validator

from openside_agent.providers.accounts import usage_windows
from openside_agent.providers.broker import (
    BrokerClient,
    BrokerCredential,
    Snapshot,
    response_json,
)
from openside_agent.providers.selector import remaining_quota

from .conftest import ACCESS_CANARY, BROKER_CANARY, FakeBroker, credential

SCHEMA = json.loads(
    (Path(__file__).parents[3] / "docs/specs/omp-broker-wire.schema.json").read_text()
)


def validator(kind: str) -> Draft202012Validator:
    return Draft202012Validator(
        {
            "$schema": SCHEMA["$schema"],
            "$ref": f"#/$defs/{kind}",
            "$defs": SCHEMA["$defs"],
        }
    )


def conforms(kind: str, payload: Any) -> bool:
    # Never format jsonschema errors: their instances can contain access credentials.
    return validator(kind).is_valid(payload)


def report() -> dict[str, Any]:
    return {
        "provider": "openai-codex",
        "fetchedAt": 0,
        "metadata": {"credentialId": 1},
        "limits": [
            {
                "id": "quota",
                "label": "Quota",
                "scope": {"provider": "openai-codex"},
                "window": {"id": "5h", "label": "Quota", "resetsAt": 1000},
                "amount": {
                    "usedFraction": 0.25,
                    "remainingFraction": 0.75,
                    "unit": "percent",
                },
            }
        ],
    }


async def test_fake_broker_requests_responses_and_parser_match_pinned_wire(
    fake_broker: FakeBroker,
):
    Draft202012Validator.check_schema(SCHEMA)
    fake_broker.reports = [report()]
    async with httpx.AsyncClient() as http:
        broker = BrokerClient(fake_broker.url, BROKER_CANARY, http_client=http)
        snapshot = await broker.snapshot()
        usage = await broker.usage()
        await broker.block(1, "openai-codex", blocked_until_ms=1000, block_scope="chat")
    assert conforms("snapshot", fake_broker.snapshot())
    assert conforms("usage", {"generatedAt": 0, "reports": usage})
    assert conforms("block_request", fake_broker.blocks[0][1])
    assert conforms("block_response", {"ok": True})
    assert snapshot.credentials[0].id == 1
    assert remaining_quota(snapshot.credentials[0], usage) == 0.75
    assert usage_windows(snapshot.credentials[0], usage)[0]["used_percent"] == 25


@pytest.mark.parametrize(
    "path",
    [
        ("generation",),
        ("serverNowMs",),
        ("credentials",),
        ("credentials", 0, "id"),
        ("credentials", 0, "provider"),
        ("credentials", 0, "credential", "type"),
        ("credentials", 0, "credential", "access"),
        ("credentials", 0, "credential", "expires"),
    ],
)
@pytest.mark.parametrize("change", ["removed", "renamed", "type"])
def test_breaking_snapshot_upgrade_fails_without_displaying_values(path, change):
    payload = FakeBroker().snapshot()
    current = payload
    for key in path[:-1]:
        current = current[key]
    key = path[-1]
    value = current.pop(key)
    if change == "renamed":
        current["renamed"] = value
    elif change == "type":
        current[key] = {"invalid": True}
    assert not conforms("snapshot", payload)
    with pytest.raises((ValueError, TypeError)):
        Snapshot.parse(payload)


@pytest.mark.parametrize(
    "kind,payload,key",
    [
        ("usage", {"reports": [report()]}, "reports"),
        (
            "block_request",
            {
                "providerKey": "openai-codex",
                "blockScope": "chat",
                "blockedUntilMs": 1000,
            },
            "blockScope",
        ),
        ("block_response", {"ok": True}, "ok"),
    ],
)
@pytest.mark.parametrize("change", ["removed", "renamed", "type"])
def test_breaking_usage_or_block_upgrade_fails(kind, payload, key, change):
    payload = copy.deepcopy(payload)
    value = payload.pop(key)
    if change == "renamed":
        payload["renamed"] = value
    elif change == "type":
        payload[key] = {"invalid": True}
    assert not conforms(kind, payload)


def test_additive_upstream_fields_are_allowed_and_api_keys_remain_filtered():
    payload = FakeBroker().snapshot()
    payload["newUpstreamField"] = "test-extension"
    payload["credentials"].append(
        {
            "id": 2,
            "provider": "test-provider",
            "credential": {"type": "api_key", "key": ACCESS_CANARY},
        }
    )
    assert conforms("snapshot", payload)
    assert len(Snapshot.parse(payload).credentials) == 1
    invalid = credential()
    invalid["credential"]["access"] = 7
    assert not conforms("entry", invalid)
    assert BrokerCredential.model_validate(credential()).id == 1


def safe_shape(value: Any, field: str = "") -> Any:
    keys = {
        "generation",
        "serverNowMs",
        "credentials",
        "id",
        "provider",
        "credential",
        "type",
        "key",
        "access",
        "expires",
        "accountId",
        "email",
        "orgId",
        "blocks",
        "providerKey",
        "blockScope",
        "blockedUntilMs",
        "updatedAtMs",
        "reports",
        "limits",
        "scope",
        "amount",
        "window",
        "metadata",
        "credentialId",
        "label",
        "resetsAt",
        "used",
        "limit",
        "remaining",
        "usedFraction",
        "remainingFraction",
        "unit",
        "ok",
    }
    if isinstance(value, dict):
        return {
            key: safe_shape(item, key) for key, item in value.items() if key in keys
        }
    if isinstance(value, list):
        return [safe_shape(item, field) for item in value]
    if isinstance(value, str):
        allowed = {
            "type": {"oauth", "api_key"},
            "unit": {
                "percent",
                "tokens",
                "requests",
                "credits",
                "usd",
                "minutes",
                "bytes",
                "unknown",
            },
        }
        if value in allowed.get(field, set()):
            return value
        return "<redacted>" if value else ""
    if isinstance(value, bool) or value is None:
        return value
    if isinstance(value, (int, float)):
        return 0 if isinstance(value, int) else 0.0
    return None


def check_live_shapes(url: str, token: str) -> bool:
    try:
        with httpx.Client(timeout=10, trust_env=False) as http:
            for path, kind in [("/v1/snapshot", "snapshot"), ("/v1/usage", "usage")]:
                with http.stream(
                    "GET",
                    url.rstrip("/") + path,
                    headers={
                        "Authorization": f"Bearer {token}",
                        "OMP-Auth-Broker-Capabilities": "codex-meter-block-scopes",
                    },
                ) as response:
                    payload = safe_shape(response_json(response))
                if not conforms(kind, payload):
                    return False
                if kind == "snapshot":
                    Snapshot.parse(payload)
        return True
    except Exception:
        return False


def test_live_check_is_get_only_and_redacts_before_reporting(
    monkeypatch: pytest.MonkeyPatch,
):
    seen = []

    def wire(request: httpx.Request):
        seen.append((request.method, request.url.path))
        value = (
            FakeBroker().snapshot()
            if request.url.path == "/v1/snapshot"
            else {"reports": [report()]}
        )
        return httpx.Response(200, json=value)

    original = httpx.Client
    monkeypatch.setattr(
        httpx,
        "Client",
        lambda **kwargs: original(transport=httpx.MockTransport(wire), **kwargs),
    )
    assert check_live_shapes("http://127.0.0.1:1", BROKER_CANARY)
    assert seen == [("GET", "/v1/snapshot"), ("GET", "/v1/usage")]
    assert ACCESS_CANARY not in json.dumps(safe_shape(FakeBroker().snapshot()))
    assert BROKER_CANARY not in json.dumps(safe_shape(FakeBroker().snapshot()))


@pytest.mark.live
@pytest.mark.live_broker
def test_live_broker_upgrade_wire_shapes():
    url = os.getenv("OPENSIDE_LIVE_BROKER_URL")
    name = os.getenv("OPENSIDE_LIVE_BROKER_TOKEN_ENV")
    if not url or not name:
        pytest.skip(
            "Set explicit OPENSIDE_LIVE_BROKER_URL and "
            "OPENSIDE_LIVE_BROKER_TOKEN_ENV to opt in; no defaults are used"
        )
    if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]{0,127}", name):
        pytest.fail("Invalid broker token environment reference", pytrace=False)
    token = os.getenv(name)
    if not token:
        pytest.fail("Broker token environment reference is unavailable", pytrace=False)
    valid = check_live_shapes(url, token)
    del token
    if not valid:
        pytest.fail(
            "Broker wire shape unavailable or incompatible; values redacted",
            pytrace=False,
        )
