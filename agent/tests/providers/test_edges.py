import json

import pytest
from pydantic_ai import Agent
from pydantic_ai.exceptions import ModelAPIError
from pydantic_ai.messages import ModelResponse, TextPart
from pydantic_ai.models.fallback import FallbackModel
from pydantic_ai.models.function import FunctionModel

from openside_agent.providers import (
    AuthUnavailable,
    CredentialPins,
    build_model,
    doctor_report,
)
from openside_agent.providers.broker import BrokerClient
from openside_agent.providers.models import BrokerModel
from openside_agent.providers.selector import AccountSelector
from openside_agent.providers.sources import SOURCE_TYPES, OmpBrokerSource

from .conftest import ACCESS_CANARY, BROKER_CANARY, FakeBroker, credential, model_id


@pytest.mark.parametrize(
    "provider,variable",
    [
        ("openai", "OPENAI_API_KEY"),
        ("openrouter", "OPENROUTER_API_KEY"),
        ("google", "GOOGLE_API_KEY"),
        ("google", "GEMINI_API_KEY"),
    ],
)
async def test_api_key_provider_availability(
    monkeypatch: pytest.MonkeyPatch, provider: str, variable: str
):
    monkeypatch.setenv(variable, ACCESS_CANARY)
    config = {"order": {provider: ["env"]}}
    model = await build_model(model_id(provider), [], config, session_id="s")
    async with model:
        assert ACCESS_CANARY not in repr(model)
        report = doctor_report(config, {"default": model_id(provider)})
        assert report["models"]["providers_ok"] is True
        assert ACCESS_CANARY not in json.dumps(report)


@pytest.mark.parametrize("auth_failure", [True, False])
async def test_fallback_never_catches_auth_unavailable(auth_failure: bool):
    called = []

    def primary(messages, info):
        called.append("primary")
        if auth_failure:
            raise AuthUnavailable("anthropic", ["omp-broker: unavailable"])
        raise ModelAPIError("synthetic", "temporary failure")

    def secondary(messages, info):
        called.append("secondary")
        return ModelResponse(parts=[TextPart("pong")])

    agent = Agent(FallbackModel(FunctionModel(primary), FunctionModel(secondary)))
    if auth_failure:
        with pytest.raises(AuthUnavailable):
            await agent.run("Reply pong.")
        assert called == ["primary"]
    else:
        assert (await agent.run("Reply pong.")).output == "pong"
        assert called == ["primary", "secondary"]


async def test_later_build_failure_closes_already_created_models(
    fake_broker: FakeBroker, monkeypatch: pytest.MonkeyPatch
):
    created: list[BrokerModel] = []

    class RecordingSource(OmpBrokerSource):
        async def build(self, model_id: str, *, session_id: str, pins: CredentialPins):
            model = await super().build(model_id, session_id=session_id, pins=pins)
            assert isinstance(model, BrokerModel)
            created.append(model)
            return model

    monkeypatch.setitem(SOURCE_TYPES, "omp-broker", RecordingSource)
    with pytest.raises(AuthUnavailable):
        await build_model(
            model_id("openai-codex"),
            ["invalid-model-identifier"],
            fake_broker.config(),
            session_id="s",
        )
    assert len(created) == 1 and created[0].http.is_closed


def test_doctor_lower_priority_down_broker_does_not_override_env(
    fake_broker: FakeBroker, monkeypatch: pytest.MonkeyPatch
):
    fake_broker.status = 503
    monkeypatch.setenv("ANTHROPIC_API_KEY", ACCESS_CANARY)
    config = {**fake_broker.config(), "order": {"anthropic": ["env", "omp-broker"]}}
    assert (
        doctor_report(config, {"default": model_id("anthropic")})["models"][
            "providers_ok"
        ]
        is True
    )


def test_doctor_absent_broker_provider_allows_env(
    fake_broker: FakeBroker, monkeypatch: pytest.MonkeyPatch
):
    monkeypatch.setenv("ANTHROPIC_API_KEY", ACCESS_CANARY)
    assert (
        doctor_report(fake_broker.config(), {"default": model_id("anthropic")})[
            "models"
        ]["providers_ok"]
        is True
    )


async def test_force_failure_invalidates_cached_snapshot(fake_broker: FakeBroker):
    client = BrokerClient(fake_broker.url, BROKER_CANARY)
    try:
        await client.snapshot()
        fake_broker.status = 503
        with pytest.raises(AuthUnavailable):
            await client.snapshot(force=True)
        with pytest.raises(AuthUnavailable):
            await client.snapshot()
    finally:
        await client.aclose()


async def test_usage_matches_account_and_org_without_crossing_orgs(
    fake_broker: FakeBroker,
):
    fake_broker.entries = [credential("anthropic", 1), credential("anthropic", 2)]
    for entry in fake_broker.entries:
        entry["credential"].update(
            {"accountId": "shared-account", "orgId": f"org-{entry['id']}"}
        )
    fake_broker.reports = [
        {
            "provider": "anthropic",
            "limits": [
                {
                    "scope": {"accountId": "shared-account", "orgId": f"org-{cid}"},
                    "amount": {"usedFraction": used},
                }
            ],
        }
        for cid, used in [(1, 0.9), (2, 0.1)]
    ]
    client = BrokerClient(fake_broker.url, BROKER_CANARY)
    try:
        selected = await AccountSelector(
            client, "anthropic", "s", CredentialPins()
        ).select()
        assert selected.id == 2
    finally:
        await client.aclose()
