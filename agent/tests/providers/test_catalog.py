import json

import pytest

from openside_agent.providers.catalog import CURATED, catalog_report, metadata


async def test_catalog_is_only_a_credential_free_sdk_index(monkeypatch):
    canary = "synthetic-catalog-secret-canary"
    monkeypatch.setenv("ANTHROPIC_API_KEY", canary)
    monkeypatch.setenv("OPENSIDE_BROKER_TOKEN", canary)
    report = await catalog_report()
    assert set(report) == {"providers"}
    assert [row["provider"] for row in report["providers"]] == sorted(CURATED)
    for provider in report["providers"]:
        assert set(provider) == {"provider", "models"}
        for model in provider["models"]:
            assert set(model) == {
                "id",
                "label",
                "recommended_for",
                "in_catalog",
                "tier",
                "strengths",
            }
            assert model["in_catalog"] is True
            assert model["tier"] in {"fast", "reasoning", "frontier"}
            assert model["strengths"]
            assert model["recommended_for"] == metadata(model["id"])[2]
            assert model["id"] == f"{provider['provider']}:{model['label']}"
    assert canary not in json.dumps(report)


async def test_catalog_omits_unavailable_sdk_without_initializing_clients(monkeypatch):
    from openside_agent.providers import catalog

    def factory(provider):
        if provider == "google":
            raise ImportError("synthetic-private-canary")
        return object

    monkeypatch.setattr(catalog, "infer_provider_class", factory)
    report = await catalog_report()
    assert "google" not in [row["provider"] for row in report["providers"]]
    assert "synthetic-private-canary" not in json.dumps(report)


async def test_model_selectors_and_broker_names_add_supported_sdk_namespaces_only():
    report = await catalog_report(
        {
            "default": "openrouter:test/model",
            "front": "openai-chat:gpt-5",
            "fallback": ["unsupported:model"],
        },
        ["github-copilot", "openai-responses", "unsupported"],
    )
    groups = {row["provider"]: row["models"] for row in report["providers"]}
    assert {
        "github-copilot",
        "openrouter",
        "openai-chat",
        "openai-responses",
    } <= groups.keys()
    assert "unsupported" not in groups
    assert groups["github-copilot"] == []
    assert groups["openrouter"] == [
        {
            "id": "openrouter:test/model",
            "label": "test/model",
            "recommended_for": None,
            "in_catalog": False,
            "tier": "reasoning",
            "strengths": "Explicitly configured model; capabilities not curated",
        }
    ]


@pytest.mark.parametrize(
    ("model", "role"),
    [
        ("openai:GPT-6-LUNA", "fast"),
        ("claude-haiku-5-5", "fast"),
        ("gemini-3.8-flash", "fast"),
        ("gpt-5-mini", "fast"),
        ("gpt-5-nano", "fast"),
        ("gemini-flash-lite", "fast"),
        ("gpt-6.1-sol", "reasoning"),
        ("claude-sonnet-5-5", "reasoning"),
        ("claude-opus-5-5", "reasoning"),
        ("gemini-3.1-pro-preview", "reasoning"),
        ("gpt-6-astra", "reasoning"),
        ("gpt-5", "reasoning"),
        ("custom-model", None),
        ("professional", None),
    ],
)
def test_curated_role_recommendations(model, role):
    assert metadata(model)[2] == role


async def test_superseded_generations_are_hidden_unless_explicitly_configured():
    removed = {"gpt-5.4-mini", "claude-opus-4-8", "claude-sonnet-4-6", "gemini-2.5-pro"}
    report = await catalog_report()
    assert not removed.intersection(
        model["label"]
        for provider in report["providers"]
        for model in provider["models"]
    )
    report = await catalog_report({"default": "openai:gpt-5.4-mini"})
    models = {
        model["id"]: model
        for provider in report["providers"]
        for model in provider["models"]
    }
    assert models["openai:gpt-5.4-mini"]["in_catalog"] is False
    assert models["openai:gpt-6-luna"]["in_catalog"] is True
    assert models["openai:gpt-6-astra"]["tier"] == "frontier"
