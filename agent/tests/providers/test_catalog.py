import json

from openside_agent.providers.catalog import CURATED, catalog_report


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
            assert set(model) == {"id", "label"}
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
    assert groups["openrouter"] == groups["github-copilot"] == []
