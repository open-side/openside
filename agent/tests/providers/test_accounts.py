import json
import sys
from pathlib import Path

import httpx
import httpx2
import pytest
from pydantic_ai.models import infer_model
from pydantic_ai.providers.openai_codex import OpenAICodexProvider

from openside_agent.providers import build_model, sources
from openside_agent.providers.accounts import (
    MAX_ACCOUNTS,
    accounts_report,
    usage_windows,
)
from openside_agent.providers.broker import (
    BrokerClient,
    BrokerCredential,
    broker_settings,
)
from openside_agent.providers.errors import AuthUnavailable

from .conftest import ACCESS_CANARY, BROKER_CANARY, FakeBroker, credential


async def test_accounts_metadata_usage_blocks_and_no_private_identifiers(
    fake_broker: FakeBroker,
):
    fake_broker.entries = [credential(cid=1), credential(cid=2), credential(cid=3)]
    fake_broker.entries[0]["credential"]["email"] = "private@example.test"
    fake_broker.entries[0]["blocks"] = [
        {"providerKey": "openai-codex", "blockScope": "chat", "blockedUntilMs": 1e15}
    ]
    fake_broker.reports = [
        {
            "provider": "openai-codex",
            "metadata": {"credentialId": 1},
            "limits": [
                {
                    "scope": {},
                    "window": {"id": "5h", "label": ACCESS_CANARY, "resetsAt": 12345},
                    "amount": {"usedFraction": 0.25},
                },
                {
                    "scope": {},
                    "window": {"id": "7d"},
                    "amount": {"used": 40, "limit": 100},
                },
            ],
        }
    ]
    result = await accounts_report(
        fake_broker.config(), {"default": "openai-codex:test/model-a"}
    )
    source = result["sources"][0]
    assert source["name"] == "omp-broker" and source["available"]
    assert len(source["accounts"]) == 3
    account = source["accounts"][0]
    assert account["id"] == "broker:1"
    assert account["blocked_until_ms"] == 1e15
    assert account["models"] == ["openai-codex:test/model-a"]
    assert account["usage_windows"] == [
        {"name": "5h", "used_percent": 25, "reset_at_ms": 12345},
        {"name": "7d", "used_percent": 40, "reset_at_ms": None},
    ]
    serialized = json.dumps(result)
    for private in [
        ACCESS_CANARY,
        BROKER_CANARY,
        "private@example.test",
        "synthetic-account",
        "refresh",
    ]:
        assert private not in serialized


async def test_accounts_local_plugins_do_not_execute_key_commands(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    monkeypatch.setenv("TEST_API_KEY", ACCESS_CANARY)
    marker = tmp_path / "executed"
    cli_home = tmp_path / "codex"
    cli_home.mkdir()
    (cli_home / "auth.json").write_text("not opened by diagnostic")
    monkeypatch.setenv("CODEX_HOME", str(cli_home))
    config = {
        "order": {"openai": ["env", "command"], "openai-codex": ["codex-cli"]},
        "env": {"openai": "TEST_API_KEY"},
        "command": {
            "openai": [sys.executable, "-c", f"open({str(marker)!r}, 'w').write('ran')"]
        },
    }
    result = await accounts_report(
        config,
        {"default": "openai:test/model-a", "fallback": ["openai-codex:test/model-b"]},
    )
    assert [source["kind"] for source in result["sources"]] == [
        "env",
        "command",
        "codex-cli",
    ]
    assert all(source["available"] for source in result["sources"])
    assert not marker.exists()
    assert ACCESS_CANARY not in json.dumps(result)
    assert (cli_home / "auth.json").read_text() == "not opened by diagnostic"


async def test_accounts_are_bounded_and_unavailable_broker_is_redacted(
    fake_broker: FakeBroker,
):
    fake_broker.entries = [credential(cid=index) for index in range(MAX_ACCOUNTS + 1)]
    result = await accounts_report(fake_broker.config(), {})
    assert len(result["sources"][0]["accounts"]) == MAX_ACCOUNTS
    assert result["truncated"]
    fake_broker.status = 503
    result = await accounts_report(fake_broker.config(), {})
    assert not result["sources"][0]["available"]
    assert result["sources"][0]["accounts"] == []
    assert ACCESS_CANARY not in json.dumps(result)


async def test_broker_response_size_limit_masks_oversized_body():
    client = httpx.AsyncClient(
        transport=httpx.MockTransport(
            lambda _: httpx.Response(200, content=b"x" * (4 * 1024 * 1024 + 1))
        )
    )
    broker = BrokerClient("http://127.0.0.1:1", BROKER_CANARY, http_client=client)
    async with client:
        with pytest.raises(AuthUnavailable) as error:
            await broker.snapshot()
        assert BROKER_CANARY not in str(error.value)


@pytest.mark.parametrize(
    "amount,expected",
    [
        ({"unit": "percent", "used": 63}, 63),
        ({"remainingFraction": 0.2}, 80),
        ({"limit": 10, "remaining": 3}, 70),
        ({"usedFraction": 2}, 100),
        ({"usedFraction": float("nan")}, None),
    ],
)
def test_usage_amount_shapes_and_private_window_names(amount, expected):
    entry = BrokerCredential.model_validate(credential())
    report = {
        "provider": "openai-codex",
        "metadata": {"credentialId": 1},
        "limits": [{"scope": {}, "window": {"id": ACCESS_CANARY}, "amount": amount}],
    }
    assert usage_windows(entry, [report])[0] == {
        "name": "Window 1",
        "used_percent": expected,
        "reset_at_ms": None,
    }


def test_custom_broker_token_env_and_file_precedence(tmp_path, monkeypatch):
    token_file = tmp_path / "source.token"
    token_file.write_text(BROKER_CANARY)
    config = {
        "omp_broker": {
            "url": "http://127.0.0.1:1",
            "token_file": str(token_file),
            "token_env": "TEST_BROKER_TOKEN",
        }
    }
    monkeypatch.delenv("OPENSIDE_BROKER_TOKEN", raising=False)
    monkeypatch.delenv("TEST_BROKER_TOKEN", raising=False)
    assert broker_settings(config)[1].get_secret_value() == BROKER_CANARY
    monkeypatch.setenv("TEST_BROKER_TOKEN", ACCESS_CANARY)
    assert broker_settings(config)[1].get_secret_value() == ACCESS_CANARY
    monkeypatch.setenv("OPENSIDE_BROKER_TOKEN", "override-value")
    assert broker_settings(config)[1].get_secret_value() == "override-value"
    assert token_file.read_text() == BROKER_CANARY
    config["omp_broker"]["token_env"] = BROKER_CANARY
    _, token, reason = broker_settings(config)
    assert token is None and BROKER_CANARY not in reason


async def test_codex_cli_refresh_is_memory_only_and_does_not_write_auth_file(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    path = tmp_path / "auth.json"
    data = {
        "tokens": {
            "access_token": ACCESS_CANARY,
            "refresh_token": "test-refresh-canary",
            "account_id": "test-account",
        }
    }
    path.write_text(json.dumps(data))
    before = {
        candidate: candidate.read_bytes()
        for candidate in tmp_path.rglob("*")
        if candidate.is_file()
    }
    monkeypatch.setenv("CODEX_HOME", str(tmp_path))
    seen = []

    async def wire(request: httpx2.Request):
        seen.append(request.url.path)
        if request.url.path.endswith("/oauth/token"):
            return httpx2.Response(
                200,
                json={
                    "access_token": "renewed-access-canary",
                    "refresh_token": "renewed-refresh-canary",
                    "account_id": "test-account",
                },
            )
        if request.headers["authorization"] == f"Bearer {ACCESS_CANARY}":
            return httpx2.Response(401, json={"error": {"message": ACCESS_CANARY}})
        assert request.headers["authorization"] == "Bearer renewed-access-canary"
        return httpx2.Response(
            200,
            json={
                "id": "test-response",
                "object": "response",
                "created_at": 1,
                "model": "test/model-a",
                "status": "completed",
                "output": [],
            },
        )

    async with httpx2.AsyncClient(transport=httpx2.MockTransport(wire)) as http:
        provider = OpenAICodexProvider(http_client=http)
        monkeypatch.setattr(
            sources,
            "infer_model",
            lambda identifier: infer_model(
                identifier, provider_factory=lambda _: provider
            ),
        )
        model = await build_model(
            "openai-codex:test/model-a",
            [],
            {"order": {"openai-codex": ["codex-cli"]}},
            session_id="test-session",
        )
        async with model:
            result = await model.provider.client.responses.create(
                model="test/model-a", input="public prompt"
            )
            assert result.id == "test-response"
            assert provider.credentials.access_token == "renewed-access-canary"
            assert ACCESS_CANARY not in repr(model)
    assert any(path.endswith("/oauth/token") for path in seen)
    assert len(seen) == 3
    assert json.loads(path.read_text()) == data
    assert {
        candidate: candidate.read_bytes()
        for candidate in tmp_path.rglob("*")
        if candidate.is_file()
    } == before


async def test_empty_codex_home_uses_native_auth_default_in_accounts(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    monkeypatch.setenv("CODEX_HOME", "")
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    auth = tmp_path / ".codex" / "auth.json"
    auth.parent.mkdir()
    auth.write_text('{"tokens":{"access_token":"test-codex-canary"}}')
    report = await accounts_report(
        {"order": {"openai-codex": ["codex-cli"]}},
        {"default": "openai-codex:test/model-a"},
    )
    assert report["sources"][0]["available"] is True
    assert "test-codex-canary" not in json.dumps(report)
