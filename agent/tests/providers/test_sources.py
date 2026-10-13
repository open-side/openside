import pytest
from pydantic_ai.models.fallback import FallbackModel

from openside_agent.providers import AuthUnavailable, build_model
from openside_agent.providers.request_model import RequestCredentialModel

from .test_request_credentials import FakeCredentialDaemon


@pytest.mark.parametrize(
    "identifier",
    [
        "anthropic:claude-sonnet-5-5",
        "openai:gpt-6.1-sol",
        "google:gemini-3.8-flash",
        "openai-codex:gpt-6.1-sol",
        "github-copilot:gpt-5",
    ],
)
async def test_model_build_uses_placeholder_sdk_clients_and_no_source_reads(
    identifier, monkeypatch
):
    canary = "synthetic-build-secret-canary"
    monkeypatch.setenv("OPENAI_API_KEY", canary)
    monkeypatch.setenv("ANTHROPIC_API_KEY", canary)
    for name in (
        "OPENAI_ADMIN_KEY",
        "OPENAI_WEBHOOK_SECRET",
        "OPENAI_ORG_ID",
        "OPENAI_PROJECT_ID",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_WEBHOOK_SIGNING_KEY",
    ):
        monkeypatch.setenv(name, canary)
    daemon = FakeCredentialDaemon()
    model = await build_model(
        identifier, [], {"_vault_keys": {"anthropic": canary}}, credential_rpc=daemon
    )
    assert isinstance(model, RequestCredentialModel)
    async with model:
        assert model.model_name == identifier.split(":", 1)[1]
        assert canary not in repr(model)
        assert not daemon.acquisitions
        if identifier.startswith("openai-codex:"):
            assert model.provider._credentials is None
        elif identifier.startswith("anthropic:"):
            assert model.provider.client.api_key == "daemon-managed"
            assert model.provider.client.auth_token is None
            assert model.provider.client.webhook_key == ""
        if identifier.startswith(("openai:", "openai-codex:", "github-copilot:")):
            client = model.provider.client
            assert (
                client.admin_api_key
                == client.organization
                == client.project
                == client.webhook_secret
                == ""
            )
    assert model.http.is_closed


async def test_only_explicit_fallback_models_are_constructed():
    model = await build_model(
        "anthropic:claude-sonnet-5-5",
        ["openai:gpt-6.1-sol"],
        {},
        credential_rpc=FakeCredentialDaemon(),
    )
    assert isinstance(model, FallbackModel)
    async with model:
        assert [inner.provider_name for inner in model.models] == [
            "anthropic",
            "openai",
        ]
        assert not await model._should_fallback(AuthUnavailable("anthropic"))


async def test_invalid_identifier_fails_closed():
    with pytest.raises(AuthUnavailable):
        await build_model("invalid", [], {}, credential_rpc=FakeCredentialDaemon())
