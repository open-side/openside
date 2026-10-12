import asyncio
import json
import time

import httpx2
import pytest
from pydantic_ai import Agent
from pydantic_ai.exceptions import ModelHTTPError

from openside_agent.providers import AuthUnavailable, CredentialPins
from openside_agent.providers.anthropic import CLAUDE_CODE_IDENTITY, ClaudeCodeShaping
from openside_agent.providers.broker import BrokerClient
from openside_agent.providers.models import (
    BrokerOnlyAuth,
    CodexBrokerSource,
    broker_model,
)
from openside_agent.providers.selector import AccountSelector

from .conftest import ACCESS_CANARY, BROKER_CANARY, FakeBroker, credential, model_id


async def test_429_blocks_and_retries_next_account(fake_broker: FakeBroker):
    fake_broker.entries = [credential(cid=1), credential(cid=2)]
    fake_broker.entries[0]["credential"]["expires"] += 10000
    seen = []

    async def wire(request: httpx2.Request):
        seen.append(request.headers["chatgpt-account-id"])
        return httpx2.Response(
            429 if len(seen) == 1 else 200,
            json={"error": "rate_limit" if len(seen) == 1 else "ok"},
        )

    broker = BrokerClient(fake_broker.url, BROKER_CANARY, snapshot_ttl_seconds=0)
    pins = CredentialPins()
    selector = AccountSelector(broker, "openai-codex", "s", pins)
    try:
        async with httpx2.AsyncClient(
            auth=BrokerOnlyAuth(selector), transport=httpx2.MockTransport(wire)
        ) as http:
            response = await http.post(
                "https://chatgpt.com/backend-api/codex/responses", json={}
            )
            assert response.status_code == 200
        assert seen == ["synthetic-account-1", "synthetic-account-2"]
        assert fake_broker.blocks[0][0] == 1
        assert pins.get("s", "openai-codex").credential_id == 2
        assert all("refresh" not in path for _, path in fake_broker.requests)
    finally:
        await broker.aclose()


async def test_429_all_accounts_fail_closed(fake_broker: FakeBroker):
    fake_broker.entries = [credential(cid=1), credential(cid=2)]
    broker = BrokerClient(fake_broker.url, BROKER_CANARY, snapshot_ttl_seconds=0)
    selector = AccountSelector(broker, "openai-codex", "s", CredentialPins())
    try:
        async with httpx2.AsyncClient(
            auth=BrokerOnlyAuth(selector),
            transport=httpx2.MockTransport(
                lambda _: httpx2.Response(429, json={"error": ACCESS_CANARY})
            ),
        ) as http:
            with pytest.raises(AuthUnavailable) as error:
                await http.post(
                    "https://chatgpt.com/backend-api/codex/responses", json={}
                )
            assert ACCESS_CANARY not in str(error.value)
        assert len(fake_broker.blocks) == 2
    finally:
        await broker.aclose()


@pytest.mark.parametrize("rotate", [True, False])
async def test_401_rereads_but_never_refreshes(fake_broker: FakeBroker, rotate: bool):
    seen = []

    def wire(request: httpx2.Request):
        assert request.url.host == "chatgpt.com"
        seen.append(
            (request.headers["authorization"], request.headers["chatgpt-account-id"])
        )
        if len(seen) == 1:
            if rotate:
                fake_broker.entries[0]["credential"]["access"] = (
                    "synthetic-rotated-token"
                )
                fake_broker.entries[0]["credential"]["accountId"] = (
                    "synthetic-rotated-account"
                )
            return httpx2.Response(401, json={"error": "expired"})
        return httpx2.Response(200, json={"ok": True})

    broker = BrokerClient(fake_broker.url, BROKER_CANARY)
    selector = AccountSelector(broker, "openai-codex", "s", CredentialPins())
    try:
        async with httpx2.AsyncClient(
            auth=BrokerOnlyAuth(selector), transport=httpx2.MockTransport(wire)
        ) as http:
            if rotate:
                assert (
                    await http.post(
                        "https://chatgpt.com/backend-api/codex/responses", json={}
                    )
                ).status_code == 200
                assert seen[0] != seen[1]
            else:
                with pytest.raises(AuthUnavailable):
                    await http.post(
                        "https://chatgpt.com/backend-api/codex/responses", json={}
                    )
        assert len(seen) == (2 if rotate else 1)
        assert all(method == "GET" for method, _ in fake_broker.requests)
    finally:
        await broker.aclose()


async def test_concurrent_401_adopts_once(fake_broker: FakeBroker):
    initial = 0
    ready = asyncio.Event()

    async def wire(request: httpx2.Request):
        nonlocal initial
        if request.headers["authorization"].endswith("-1"):
            initial += 1
            if initial == 2:
                fake_broker.entries[0]["credential"]["access"] = (
                    "synthetic-rotated-token"
                )
                ready.set()
            await ready.wait()
            return httpx2.Response(401, json={"error": "expired"})
        return httpx2.Response(200, json={"ok": True})

    broker = BrokerClient(fake_broker.url, BROKER_CANARY)
    selector = AccountSelector(broker, "openai-codex", "s", CredentialPins())
    try:
        async with httpx2.AsyncClient(
            auth=BrokerOnlyAuth(selector), transport=httpx2.MockTransport(wire)
        ) as http:
            responses = await asyncio.gather(
                *[
                    http.post(
                        "https://chatgpt.com/backend-api/codex/responses", json={}
                    )
                    for _ in range(2)
                ]
            )
            assert [response.status_code for response in responses] == [200, 200]
        assert fake_broker.requests.count(("GET", "/v1/snapshot")) == 2
    finally:
        await broker.aclose()


@pytest.mark.parametrize("rotate", [True, False])
async def test_stale_token_reloads_and_fails_if_unchanged(
    fake_broker: FakeBroker, rotate: bool
):
    broker = BrokerClient(fake_broker.url, BROKER_CANARY)
    fake_broker.entries[0]["credential"]["expires"] = (time.time() - 10) * 1000
    await broker.snapshot()
    if rotate:
        fake_broker.entries[0] = credential(cid=1)
    seen = []
    selector = AccountSelector(broker, "openai-codex", "s", CredentialPins())
    try:
        async with httpx2.AsyncClient(
            auth=BrokerOnlyAuth(selector),
            transport=httpx2.MockTransport(
                lambda request: seen.append(request) or httpx2.Response(200)
            ),
        ) as http:
            if rotate:
                assert (
                    await http.post(
                        "https://chatgpt.com/backend-api/codex/responses", json={}
                    )
                ).status_code == 200
            else:
                with pytest.raises(AuthUnavailable, match="stale"):
                    await http.post(
                        "https://chatgpt.com/backend-api/codex/responses", json={}
                    )
        assert len(seen) == int(rotate)
    finally:
        await broker.aclose()


async def test_unexpected_origin_never_receives_credentials(fake_broker: FakeBroker):
    broker = BrokerClient(fake_broker.url, BROKER_CANARY)
    auth = BrokerOnlyAuth(
        AccountSelector(broker, "openai-codex", "s", CredentialPins())
    )
    try:
        async with httpx2.AsyncClient(
            auth=auth,
            transport=httpx2.MockTransport(
                lambda _: pytest.fail("No foreign request allowed")
            ),
        ) as http:
            for url in [
                "http://chatgpt.com/probe",
                "https://foreign.invalid/probe",
                "https://chatgpt.com:444/probe",
            ]:
                with pytest.raises(AuthUnavailable):
                    await http.get(url)
        assert fake_broker.requests == []
        assert ACCESS_CANARY not in repr(auth)
    finally:
        await broker.aclose()


async def test_codex_source_save_is_read_only(fake_broker: FakeBroker):
    broker = BrokerClient(fake_broker.url, BROKER_CANARY)
    source = CodexBrokerSource(
        AccountSelector(broker, "openai-codex", "s", CredentialPins())
    )
    try:
        loaded = await source.load()
        with pytest.raises(AuthUnavailable):
            await source.save(loaded)
        assert all(method == "GET" for method, _ in fake_broker.requests)
    finally:
        await broker.aclose()


@pytest.mark.parametrize("rejection_status", [400, 429])
@pytest.mark.parametrize("reject_again", [True, False])
async def test_anthropic_shaping_and_single_version_retry(
    fake_broker: FakeBroker, reject_again: bool, rejection_status: int
) -> None:
    fake_broker.entries = [credential("anthropic")]
    seen = []
    success = {
        "id": "msg_synthetic",
        "type": "message",
        "role": "assistant",
        "model": model_id("anthropic").split(":", 1)[1],
        "content": [{"type": "text", "text": "pong"}],
        "stop_reason": "end_turn",
        "stop_sequence": None,
        "usage": {"input_tokens": 1, "output_tokens": 1},
    }

    def wire(request: httpx2.Request):
        seen.append(request)
        assert request.headers["authorization"].startswith("Bearer synthetic")
        assert "x-api-key" not in request.headers
        payload = json.loads(request.content)
        assert payload["system"][0]["text"] == CLAUDE_CODE_IDENTITY
        assert request.headers["x-app"] == "cli"
        assert (
            request.headers["anthropic-beta"] == "claude-code-20250219,oauth-2025-04-20"
        )
        if len(seen) == 1 or reject_again:
            return httpx2.Response(
                rejection_status,
                json={
                    "error": {
                        "type": "claude_code_version_too_old",
                        "message": "Claude Code version 9.0.1 or newer is required "
                        + ACCESS_CANARY,
                    }
                },
            )
        return httpx2.Response(200, json=success)

    broker = BrokerClient(fake_broker.url, BROKER_CANARY)
    selector = AccountSelector(broker, "anthropic", "s", CredentialPins())
    http = httpx2.AsyncClient(transport=httpx2.MockTransport(wire))
    model = broker_model(
        model_id("anthropic"),
        selector,
        {"anthropic": {"claude_code_version": "1.0.0"}},
        http_client=http,
    )
    async with Agent(model, system_prompt="Keep this instruction.") as agent:
        if reject_again:
            with pytest.raises(ModelHTTPError) as error:
                await agent.run("Reply pong.")
            assert ACCESS_CANARY not in str(error.value)
        else:
            assert (await agent.run("Reply pong.")).output == "pong"
    assert len(seen) == 2
    assert seen[0].headers["user-agent"] == "claude-cli/1.0.0 (external, cli)"
    assert seen[1].headers["user-agent"] == "claude-cli/9.0.1 (external, cli)"
    assert fake_broker.blocks == []


def test_anthropic_version_environment(monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv("OPENSIDE_CLAUDE_CODE_VERSION", "7.8.9")
    shaping = ClaudeCodeShaping({"anthropic": {"claude_code_version": "1.2.3"}})
    assert "7.8.9" in shaping.headers()["user-agent"]


async def test_selected_broker_down_mid_run_stays_fail_closed(fake_broker: FakeBroker):
    broker = BrokerClient(fake_broker.url, BROKER_CANARY, snapshot_ttl_seconds=0)
    selector = AccountSelector(broker, "openai-codex", "s", CredentialPins())
    sent = 0

    def wire(request: httpx2.Request):
        nonlocal sent
        sent += 1
        return httpx2.Response(200, json={"ok": True})

    try:
        async with httpx2.AsyncClient(
            auth=BrokerOnlyAuth(selector), transport=httpx2.MockTransport(wire)
        ) as http:
            await http.post("https://chatgpt.com/backend-api/codex/responses", json={})
            fake_broker.status = 503
            with pytest.raises(AuthUnavailable):
                await http.post(
                    "https://chatgpt.com/backend-api/codex/responses", json={}
                )
        assert sent == 1
    finally:
        await broker.aclose()
