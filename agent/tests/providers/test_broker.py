import json
import time

import httpx
import pytest

from openside_agent.providers import (
    AuthUnavailable,
    CredentialPins,
    build_model,
    doctor_report,
)
from openside_agent.providers.broker import BrokerClient, broker_settings
from openside_agent.providers.selector import AccountSelector

from .conftest import ACCESS_CANARY, BROKER_CANARY, FakeBroker, credential, model_id


async def test_snapshot_ttl_and_masking(fake_broker: FakeBroker):
    client = BrokerClient(fake_broker.url, BROKER_CANARY)
    try:
        first = await client.snapshot()
        assert await client.snapshot() is first
        assert fake_broker.requests.count(("GET", "/v1/snapshot")) == 1
        assert ACCESS_CANARY not in repr(first)
        assert BROKER_CANARY not in repr(client)
        assert "refresh" not in type(first).model_fields
        await client.snapshot(force=True)
        assert fake_broker.requests.count(("GET", "/v1/snapshot")) == 2
    finally:
        await client.aclose()


async def test_selection_quota_pin_and_serialization(fake_broker: FakeBroker):
    fake_broker.entries = [credential(cid=1), credential(cid=2)]
    fake_broker.reports = [
        {
            "provider": "openai-codex",
            "metadata": {"credentialId": cid},
            "limits": [{"scope": {}, "amount": {"remainingFraction": remaining}}],
        }
        for cid, remaining in [(1, 0.2), (2, 0.8)]
    ]
    client = BrokerClient(fake_broker.url, BROKER_CANARY, snapshot_ttl_seconds=0)
    pins = CredentialPins()
    selector = AccountSelector(client, "openai-codex", "session", pins)
    try:
        assert (await selector.select()).id == 2
        fake_broker.reports[0]["limits"][0]["amount"]["remainingFraction"] = 1
        assert (await selector.select()).id == 2
        serialized = pins.to_json()
        assert ACCESS_CANARY not in serialized and "synthetic-account" not in serialized
        assert CredentialPins.from_json(serialized) == pins
        restored = AccountSelector(
            client, "openai-codex", "session", CredentialPins.from_json(serialized)
        )
        assert (await restored.select()).id == 2
    finally:
        await client.aclose()


async def test_anthropic_pin_idle_expiry(fake_broker: FakeBroker):
    fake_broker.entries = [credential("anthropic", 1), credential("anthropic", 2)]
    now = time.time()
    pins = CredentialPins()
    pins.set("session", "anthropic", 1, now)
    client = BrokerClient(fake_broker.url, BROKER_CANARY, snapshot_ttl_seconds=0)
    selector = AccountSelector(
        client, "anthropic", "session", pins, clock=lambda: now + 3599
    )
    try:
        assert (await selector.select(touch=False)).id == 1
        selector = AccountSelector(
            client, "anthropic", "session", pins, clock=lambda: now + 3600
        )
        assert (await selector.select()).id == 2
    finally:
        await client.aclose()


async def test_expired_block_is_eligible(fake_broker: FakeBroker):
    fake_broker.entries[0]["blocks"] = [
        {"providerKey": "openai-codex", "blockScope": "chat", "blockedUntilMs": 1}
    ]
    client = BrokerClient(fake_broker.url, BROKER_CANARY)
    try:
        assert (
            await AccountSelector(
                client, "openai-codex", "s", CredentialPins()
            ).select()
        ).id == 1
    finally:
        await client.aclose()


async def test_block_records_wire_contract(fake_broker: FakeBroker):
    client = BrokerClient(fake_broker.url, BROKER_CANARY, snapshot_ttl_seconds=0)
    selector = AccountSelector(client, "openai-codex", "s", CredentialPins())
    try:
        selected = await selector.select()
        await selector.block(selected)
        assert fake_broker.blocks[0][0] == 1
        assert set(fake_broker.blocks[0][1]) == {
            "providerKey",
            "blockScope",
            "blockedUntilMs",
        }
        with pytest.raises(AuthUnavailable):
            await selector.select()
    finally:
        await client.aclose()


async def test_sse_snapshot_upsert_and_remove(fake_broker: FakeBroker):
    new = credential(cid=2)
    fake_broker.events = [
        {
            "kind": "entry",
            "entry": new,
            "generation": 2,
            "serverNowMs": time.time() * 1000,
        },
        {
            "kind": "removed",
            "id": 1,
            "generation": 3,
            "serverNowMs": time.time() * 1000,
        },
    ]
    client = BrokerClient(fake_broker.url, BROKER_CANARY)
    observed = []
    try:
        with pytest.raises(AuthUnavailable, match="stream unavailable"):
            async for snapshot in client.snapshot_stream():
                observed.append([entry.id for entry in snapshot.credentials])
        assert observed == [[1], [1, 2], [2]]
        assert (await client.snapshot()).credentials[0].id == 1
    finally:
        await client.aclose()


async def test_all_blocked_never_falls_back(
    fake_broker: FakeBroker, monkeypatch: pytest.MonkeyPatch
):
    fake_broker.entries = [credential("anthropic")]
    fake_broker.entries[0]["blocks"] = [
        {
            "providerKey": "anthropic",
            "blockScope": "",
            "blockedUntilMs": (time.time() + 60) * 1000,
        }
    ]
    monkeypatch.setenv("ANTHROPIC_API_KEY", ACCESS_CANARY)
    with pytest.raises(AuthUnavailable, match="blocked") as error:
        await build_model(
            model_id("anthropic"), [], fake_broker.config(), session_id="s"
        )
    assert ACCESS_CANARY not in str(error.value)
    assert (
        doctor_report(fake_broker.config(), {"default": model_id("anthropic")})[
            "models"
        ]["providers_ok"]
        is False
    )


async def test_broker_down_never_falls_back(
    fake_broker: FakeBroker, monkeypatch: pytest.MonkeyPatch
):
    fake_broker.status = 503
    monkeypatch.setenv("ANTHROPIC_API_KEY", ACCESS_CANARY)
    with pytest.raises(AuthUnavailable) as error:
        await build_model(
            model_id("anthropic"), [], fake_broker.config(), session_id="s"
        )
    assert ACCESS_CANARY not in str(error.value)
    assert BROKER_CANARY not in str(error.value)


async def test_connect_error_masks_tokens():
    async def fail(request: httpx.Request):
        raise httpx.ConnectError(BROKER_CANARY, request=request)

    async with httpx.AsyncClient(transport=httpx.MockTransport(fail)) as http:
        client = BrokerClient("http://127.0.0.1:1", BROKER_CANARY, http_client=http)
        with pytest.raises(AuthUnavailable) as error:
            await client.snapshot()
        assert BROKER_CANARY not in str(error.value)


@pytest.mark.parametrize(
    "value",
    [
        "{} garbage",
        "[]",
        '{"s":{"anthropic":{"credential_id":1,"last_request_at":0,"token":"canary"}}}',
        '{"s":{"p":{"credential_id":1,"last_request_at":NaN}}}',
    ],
)
def test_pin_parser_rejects_extra_or_invalid_fields(value: str):
    with pytest.raises(ValueError) as error:
        CredentialPins.from_json(value)
    assert "canary" not in str(error.value)


def test_no_token_is_quietly_unavailable():
    _, token, reason = broker_settings({})
    assert token is None
    assert "unavailable" in reason
    report = doctor_report({}, {"default": model_id("anthropic")})
    assert report["models"]["providers_ok"] is False
    assert report["credentials"][0]["available"] is False
    assert BROKER_CANARY not in json.dumps(report)
