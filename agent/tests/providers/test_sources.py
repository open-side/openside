import json
import os
import sys
from pathlib import Path

import pytest
from pydantic_ai.models.fallback import FallbackModel
from pydantic_ai.providers.openai_codex import OpenAICodexProvider

from openside_agent.providers import (
    AuthUnavailable,
    CredentialPins,
    build_model,
    doctor_report,
)
from openside_agent.providers.broker import broker_settings
from openside_agent.providers.models import BrokerModel, SecretSafeModel
from openside_agent.providers.sources import CommandSource

from .conftest import ACCESS_CANARY, BROKER_CANARY, FakeBroker, credential, model_id


async def test_no_token_selects_env(monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv("ANTHROPIC_API_KEY", ACCESS_CANARY)
    model = await build_model(model_id("anthropic"), [], {}, session_id="s")
    assert isinstance(model, SecretSafeModel) and not isinstance(model, BrokerModel)
    async with model:
        assert ACCESS_CANARY not in repr(model)


async def test_explicit_env_priority_avoids_broker(
    fake_broker: FakeBroker, monkeypatch: pytest.MonkeyPatch
):
    monkeypatch.setenv("CUSTOM_OPENAI_KEY", ACCESS_CANARY)
    config = {
        **fake_broker.config(),
        "order": {"openai": ["env", "omp-broker"]},
        "env": {"openai": "CUSTOM_OPENAI_KEY"},
    }
    model = await build_model(model_id("openai"), [], config, session_id="s")
    async with model:
        assert fake_broker.requests == []
        assert model.system == "openai"


async def test_provider_absent_allows_next_source(
    fake_broker: FakeBroker, monkeypatch: pytest.MonkeyPatch
):
    monkeypatch.setenv("ANTHROPIC_API_KEY", ACCESS_CANARY)
    model = await build_model(
        model_id("anthropic"), [], fake_broker.config(), session_id="s"
    )
    async with model:
        assert not isinstance(model, BrokerModel)


async def test_command_key_and_doctor_never_stores_value(tmp_path: Path):
    script = tmp_path / "key.py"
    script.write_text(f"print({ACCESS_CANARY!r})\n")
    config = {
        "order": {"anthropic": ["command"]},
        "command": {"anthropic": [sys.executable, str(script)]},
    }
    model = await build_model(model_id("anthropic"), [], config, session_id="s")
    async with model:
        assert ACCESS_CANARY not in repr(model)
        assert ACCESS_CANARY not in repr(CommandSource(config))
        report = doctor_report(config, {"default": model_id("anthropic")})
        assert report["models"]["providers_ok"] is True
        assert ACCESS_CANARY not in json.dumps(report)
        assert "unverified" in report["credentials"][0]["reason"]


@pytest.mark.parametrize("kind", ["failure", "timeout", "empty", "missing"])
async def test_command_failures_mask_output(tmp_path: Path, kind: str):
    script = tmp_path / "key.py"
    script.write_text(
        {
            "failure": (
                f"import sys\nprint({ACCESS_CANARY!r}, file=sys.stderr)\n"
                "raise SystemExit(1)"
            ),
            "timeout": "import time\ntime.sleep(20)",
            "empty": "print('')",
            "missing": "",
        }[kind]
    )
    argv = (
        ["not-a-real-openside-command"]
        if kind == "missing"
        else [sys.executable, str(script)]
    )
    config = {"command": {"anthropic": argv, "timeout_seconds": 0.01}}
    with pytest.raises(AuthUnavailable) as error:
        await CommandSource(config).build(
            model_id("anthropic"), session_id="s", pins=CredentialPins()
        )
    assert ACCESS_CANARY not in str(error.value)


async def test_command_does_not_invoke_shell(tmp_path: Path):
    target = tmp_path / "should-not-exist"
    config = {"command": {"anthropic": ["echo", f"key; touch {target}"]}}
    model = await build_model(model_id("anthropic"), [], config, session_id="s")
    async with model:
        assert not target.exists()


async def test_codex_cli_delegation(tmp_path: Path):
    path = Path(os.environ["CODEX_HOME"]) / "auth.json"
    data = {
        "tokens": {
            "access_token": ACCESS_CANARY,
            "refresh_token": "synthetic-refresh",
            "account_id": "synthetic-account",
        }
    }
    path.write_text(json.dumps(data))
    model = await build_model(model_id("openai-codex"), [], {}, session_id="s")
    async with model:
        assert isinstance(model.provider, OpenAICodexProvider)
        assert model.provider.credentials.access_token == ACCESS_CANARY
        assert ACCESS_CANARY not in repr(model.provider.credentials)
    assert json.loads(path.read_text()) == data


async def test_builtin_copilot_delegates(monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv("GITHUB_COPILOT_API_KEY", ACCESS_CANARY)
    model = await build_model(
        model_id("github-copilot"),
        [],
        {"order": {"github-copilot": ["builtin"]}},
        session_id="s",
    )
    async with model:
        assert model.system == "github-copilot"


async def test_configured_model_fallback_chain(monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv("OPENAI_API_KEY", ACCESS_CANARY)
    monkeypatch.setenv("ANTHROPIC_API_KEY", ACCESS_CANARY)
    config = {"order": {"anthropic": ["env"], "openai": ["env"]}}
    model = await build_model(
        model_id("anthropic"), [model_id("openai")], config, session_id="s"
    )
    assert isinstance(model, FallbackModel)
    async with model:
        assert [item.system for item in model.models] == ["anthropic", "openai"]
        assert not await model._should_fallback(AuthUnavailable("anthropic"))


async def test_unavailable_primary_uses_available_configured_fallback(
    monkeypatch: pytest.MonkeyPatch,
):
    monkeypatch.setenv("OPENAI_API_KEY", ACCESS_CANARY)
    model = await build_model(
        model_id("anthropic"), [model_id("openai")], {}, session_id="s"
    )
    async with model:
        assert model.system == "openai"


async def test_no_source_failure_lists_reasons():
    with pytest.raises(AuthUnavailable) as error:
        await build_model(model_id("anthropic"), [], {}, session_id="s")
    assert "anthropic" in str(error.value)
    assert "env" in str(error.value) and "command" in str(error.value)
    assert "Configure" in str(error.value)


async def test_broker_token_file_resolution_and_env_precedence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    token_file = tmp_path / "token"
    token_file.write_text(BROKER_CANARY)
    config = {
        "omp_broker": {"url": "http://127.0.0.1:11", "token_file": str(token_file)}
    }
    url, token, _ = broker_settings(config)
    assert url.endswith(":11") and token is not None
    assert token.get_secret_value() == BROKER_CANARY
    monkeypatch.setenv("OPENSIDE_BROKER_URL", "http://127.0.0.1:12")
    monkeypatch.setenv("OPENSIDE_BROKER_TOKEN", ACCESS_CANARY)
    url, token, _ = broker_settings(config)
    assert url.endswith(":12") and token is not None
    assert token.get_secret_value() == ACCESS_CANARY
    assert ACCESS_CANARY not in repr(token)


async def test_broker_copilot_present(fake_broker: FakeBroker):
    fake_broker.entries = [credential("github-copilot")]
    config = {**fake_broker.config(), "order": {"github-copilot": ["omp-broker"]}}
    model = await build_model(model_id("github-copilot"), [], config, session_id="s")
    async with model:
        assert isinstance(model, BrokerModel)


def test_doctor_canary_and_no_models(fake_broker: FakeBroker):
    report = doctor_report(
        fake_broker.config(), {"default": model_id("openai-codex"), "fallback": []}
    )
    serialized = json.dumps(report)
    assert ACCESS_CANARY not in serialized and BROKER_CANARY not in serialized
    assert report["models"]["providers_ok"] is True
    assert set(report) == {"credentials", "models"}
    assert set(report["credentials"][0]) == {
        "provider",
        "source",
        "available",
        "reason",
        "next_step",
    }
    assert doctor_report({}, {})["models"]["providers_ok"] is False
