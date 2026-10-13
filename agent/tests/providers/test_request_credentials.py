import asyncio

import httpx2
import pytest

from openside_agent.providers import AuthUnavailable
from openside_agent.providers.request_auth import (
    CURRENT_ATTEMPT,
    CredentialAttempt,
    RequestCredentialAuth,
)
from openside_agent.providers.secret import CredentialSecret

CANARY = "synthetic-per-request-secret-canary"


def attempt(provider="openai", token=CANARY, **fields):
    return CredentialAttempt(
        provider, "vault", "api_key", CredentialSecret(token), **fields
    )


async def test_request_auth_classifies_retry_after_and_scrubs_headers():
    used = attempt()
    seen = []

    def wire(request):
        assert request.headers["authorization"] == f"Bearer {CANARY}"
        seen.append(request)
        return httpx2.Response(
            429, headers={"retry-after": "23"}, json={"error": CANARY}
        )

    auth = RequestCredentialAuth("openai", "https://api.openai.com", {})
    marker = CURRENT_ATTEMPT.set(used)
    try:
        async with httpx2.AsyncClient(
            auth=auth, transport=httpx2.MockTransport(wire)
        ) as http:
            response = await http.post("https://api.openai.com/v1/responses", json={})
            assert response.status_code == 429
        assert used.outcome.kind == "rate_limit"
        assert used.outcome.retry_after == 23
        assert "authorization" not in seen[0].headers
        assert CANARY not in repr(auth) + repr(used) + repr(used.secret) + repr(
            used.outcome
        )
    finally:
        CURRENT_ATTEMPT.reset(marker)
        used.secret.clear()
    assert not used.secret.reveal()


async def test_foreign_origin_and_missing_attempt_never_send_credentials():
    used = attempt()
    auth = RequestCredentialAuth("openai", "https://api.openai.com", {})
    async with httpx2.AsyncClient(
        auth=auth,
        transport=httpx2.MockTransport(
            lambda _: pytest.fail("Foreign requests must not reach the transport")
        ),
    ) as http:
        with pytest.raises(AuthUnavailable):
            await http.post("https://api.openai.com/v1/responses", json={})
        marker = CURRENT_ATTEMPT.set(used)
        try:
            for url in (
                "https://foreign.invalid/path",
                "http://api.openai.com/path",
                "https://api.openai.com:444/path",
            ):
                with pytest.raises(AuthUnavailable) as error:
                    await http.post(url, json={})
                assert CANARY not in str(error.value)
        finally:
            CURRENT_ATTEMPT.reset(marker)
            used.secret.clear()


async def test_concurrent_attempts_do_not_share_credentials():
    seen = []
    ready = asyncio.Event()

    async def wire(request):
        seen.append(request.headers["authorization"])
        if len(seen) == 2:
            ready.set()
        await ready.wait()
        return httpx2.Response(200, json={"ok": True})

    async with httpx2.AsyncClient(
        auth=RequestCredentialAuth("openai", "https://api.openai.com", {}),
        transport=httpx2.MockTransport(wire),
    ) as http:

        async def run(token):
            used = attempt(token=token)
            marker = CURRENT_ATTEMPT.set(used)
            try:
                await http.post("https://api.openai.com/v1/responses", json={})
                assert used.outcome.kind == "success"
            finally:
                CURRENT_ATTEMPT.reset(marker)
                used.secret.clear()

        await asyncio.gather(run("fake-account-1"), run("fake-account-2"))
    assert set(seen) == {"Bearer fake-account-1", "Bearer fake-account-2"}


async def test_codex_uses_daemon_account_metadata_without_opening_cli_home(tmp_path):
    used = CredentialAttempt(
        "openai-codex",
        "codex-cli",
        "codex_cli",
        CredentialSecret(CANARY),
        upstream_account_id="fake-upstream-account",
    )
    seen = []

    def wire(request):
        seen.append(request)
        assert request.headers["chatgpt-account-id"] == "fake-upstream-account"
        assert request.headers["authorization"] == f"Bearer {CANARY}"
        return httpx2.Response(401, json={"error": CANARY})

    marker = CURRENT_ATTEMPT.set(used)
    try:
        async with httpx2.AsyncClient(
            auth=RequestCredentialAuth(
                "openai-codex", "https://chatgpt.com/backend-api/codex", {}
            ),
            transport=httpx2.MockTransport(wire),
        ) as http:
            await http.post("https://chatgpt.com/backend-api/codex/responses", json={})
        assert used.outcome.kind == "auth"
        assert "chatgpt-account-id" not in seen[0].headers
        assert not (tmp_path / "missing").exists()
    finally:
        CURRENT_ATTEMPT.reset(marker)
        used.secret.clear()


class FakeCredentialDaemon:
    def __init__(self, count=2):
        self.accounts = [f"fake-account-{index}" for index in range(1, count + 1)]
        self.invalid = set()
        self.cooldowns = {}
        self.affinity = {}
        self.acquisitions = []
        self.reports = []
        self.buffers = []

    async def acquire(self, provider, model, request_id, number, subagent):
        from openside_agent.providers.request_model import CredentialLease

        affinity_key = ("fake-session", provider)
        self.acquisitions.append((provider, "fake-session", request_id, number))
        candidates = [
            account
            for account in self.accounts
            if account not in self.invalid and account not in self.cooldowns
        ]
        if not candidates:
            raise AuthUnavailable(
                provider, ["all accounts exhausted"], reason="all_exhausted"
            )
        account = self.affinity.get(affinity_key)
        if account not in candidates:
            account = candidates[0]
        self.affinity[affinity_key] = account
        secret = CredentialSecret(account)
        self.buffers.append(secret)
        return CredentialLease(
            f"lease-{len(self.acquisitions)}",
            account,
            CredentialAttempt(provider, "vault", "api_key", secret),
        )

    async def report(
        self, lease, outcome, request_id, *, stream_started=False, usage=None, wait=True
    ):
        self.reports.append((lease.account_id, outcome, wait))
        if outcome.kind == "auth":
            self.invalid.add(lease.account_id)
        elif outcome.kind in ("rate_limit", "quota", "policy"):
            self.cooldowns[lease.account_id] = outcome.retry_after
        return not stream_started and outcome.kind in (
            "auth",
            "rate_limit",
            "quota",
            "policy",
        )


def chat_ok():
    return httpx2.Response(
        200,
        json={
            "id": "fake-chat",
            "object": "chat.completion",
            "created": 1,
            "model": "gpt-5",
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": "done"},
                    "finish_reason": "stop",
                }
            ],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
        },
    )


async def pool_model(daemon, wire, *, max_attempts=64):
    from openside_agent.providers.request_auth import request_model
    from openside_agent.providers.request_model import RequestCredentialModel

    wrapped, http = await request_model(
        "openai-chat:gpt-5",
        {},
        http_client=httpx2.AsyncClient(
            trust_env=False,
            transport=httpx2.MockTransport(wire),
        ),
    )
    return RequestCredentialModel(
        wrapped, http, daemon, model_id="openai-chat:gpt-5", max_attempts=max_attempts
    )


@pytest.mark.parametrize("failure", ["rate_limit", "auth", "quota"])
async def test_daemon_rotates_accounts_and_affinity_sticks_across_turns(failure):
    from pydantic_ai import Agent

    daemon = FakeCredentialDaemon()
    seen = []

    def wire(request):
        seen.append(request.headers["authorization"])
        if len(seen) == 1:
            return httpx2.Response(
                401 if failure == "auth" else 429,
                headers={"retry-after": "42"},
                json={
                    "error": {
                        "code": "insufficient_quota" if failure == "quota" else CANARY
                    }
                },
            )
        return chat_ok()

    model = await pool_model(daemon, wire)
    async with Agent(model) as agent:
        first = await agent.run("first")
        second = await agent.run("second", message_history=first.all_messages())
        assert first.output == second.output == "done"
    assert seen == [
        "Bearer fake-account-1",
        "Bearer fake-account-2",
        "Bearer fake-account-2",
    ]
    assert [outcome.kind for _, outcome, _ in daemon.reports] == [
        failure,
        "success",
        "success",
    ]
    if failure == "rate_limit":
        assert daemon.cooldowns["fake-account-1"] == 42
    elif failure == "auth":
        assert "fake-account-1" in daemon.invalid
    assert {row[:2] for row in daemon.acquisitions} == {("openai-chat", "fake-session")}
    assert all(not secret.reveal() for secret in daemon.buffers)


async def test_exhausted_pool_is_typed_and_never_uses_environment(
    monkeypatch, caplog, capsys
):
    from pydantic_ai import Agent

    caplog.set_level("DEBUG")
    monkeypatch.setenv("OPENAI_API_KEY", CANARY)
    daemon = FakeCredentialDaemon()
    seen = []

    def wire(request):
        seen.append(request.headers["authorization"])
        return httpx2.Response(
            429, headers={"retry-after": "17"}, json={"error": CANARY}
        )

    model = await pool_model(daemon, wire)
    async with Agent(model) as agent:
        with pytest.raises(AuthUnavailable) as error:
            await agent.run("first")
    assert error.value.reason == "all_exhausted"
    assert CANARY not in str(error.value) + repr(model) + repr(daemon.reports)
    captured = capsys.readouterr()
    assert CANARY not in caplog.text + captured.out + captured.err
    assert "Bearer fake-account-" not in caplog.text + captured.out + captured.err
    assert seen == ["Bearer fake-account-1", "Bearer fake-account-2"]
    assert all(not secret.reveal() for secret in daemon.buffers)


async def test_retry_limit_bounds_account_rotations():
    from pydantic_ai import Agent

    daemon = FakeCredentialDaemon(count=65)
    model = await pool_model(
        daemon, lambda _: httpx2.Response(401, json={"error": CANARY})
    )
    async with Agent(model) as agent:
        with pytest.raises(AuthUnavailable) as error:
            await agent.run("first")
    assert error.value.reason == "attempt_limit"
    assert len(daemon.reports) == len(daemon.acquisitions) == 64
    assert [row[3] for row in daemon.acquisitions] == list(range(64))
    assert all(not secret.reveal() for secret in daemon.buffers)


async def test_stream_is_not_replayed_after_becoming_visible():
    from contextlib import asynccontextmanager
    from types import SimpleNamespace

    from pydantic_ai.exceptions import ModelAPIError, ModelHTTPError
    from pydantic_ai.models import ModelRequestParameters
    from pydantic_ai.models.test import TestModel
    from pydantic_ai.usage import RequestUsage

    from openside_agent.providers.outcomes import ClassifiedOutcome
    from openside_agent.providers.request_model import RequestCredentialModel

    class InterruptedStream(TestModel):
        @asynccontextmanager
        async def request_stream(self, *args, **kwargs):
            CURRENT_ATTEMPT.get().outcome = ClassifiedOutcome("auth")
            yield SimpleNamespace(usage=RequestUsage())
            raise ModelHTTPError(401, "fake", body=CANARY)

    daemon = FakeCredentialDaemon()
    model = RequestCredentialModel(
        InterruptedStream(), httpx2.AsyncClient(), daemon, model_id="openai-chat:gpt-5"
    )
    async with model:
        with pytest.raises(ModelAPIError) as error:
            async with model.request_stream([], None, ModelRequestParameters()):
                pass
    assert CANARY not in str(error.value)
    assert len(daemon.acquisitions) == len(daemon.reports) == 1
    assert daemon.reports[0][1].kind == "auth"
    assert not daemon.buffers[0].reveal()


async def test_cancel_reports_without_waiting_for_daemon_ack():
    from pydantic_ai.models import ModelRequestParameters
    from pydantic_ai.models.test import TestModel

    from openside_agent.providers.request_model import RequestCredentialModel

    entered = asyncio.Event()

    class WaitingModel(TestModel):
        async def request(self, *args, **kwargs):
            entered.set()
            await asyncio.Event().wait()

    daemon = FakeCredentialDaemon()
    model = RequestCredentialModel(
        WaitingModel(), httpx2.AsyncClient(), daemon, model_id="openai-chat:gpt-5"
    )
    async with model:
        task = asyncio.create_task(model.request([], None, ModelRequestParameters()))
        await entered.wait()
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
    assert len(daemon.reports) == 1
    assert daemon.reports[0][1].kind == "cancelled" and daemon.reports[0][2] is False
    assert not daemon.buffers[0].reveal()


@pytest.mark.parametrize("kind", ["api_key", "claude_code"])
async def test_anthropic_shaping_is_selected_by_daemon_auth_kind(kind):
    import json

    from openside_agent.providers.anthropic import CLAUDE_CODE_IDENTITY

    used = CredentialAttempt(
        "anthropic",
        "vault" if kind == "api_key" else "claude-code",
        kind,
        CredentialSecret(CANARY),
    )
    seen = []

    def wire(request):
        payload = json.loads(request.content)
        if kind == "api_key":
            assert request.headers["x-api-key"] == CANARY
            assert "authorization" not in request.headers
            assert payload["system"] == "original"
        else:
            assert request.headers["authorization"] == f"Bearer {CANARY}"
            assert "x-api-key" not in request.headers
            assert payload["system"][0]["text"] == CLAUDE_CODE_IDENTITY
        seen.append(request)
        return httpx2.Response(200, json={"ok": True})

    marker = CURRENT_ATTEMPT.set(used)
    try:
        async with httpx2.AsyncClient(
            auth=RequestCredentialAuth("anthropic", "https://api.anthropic.com", {}),
            transport=httpx2.MockTransport(wire),
        ) as http:
            await http.post(
                "https://api.anthropic.com/v1/messages",
                json={"system": "original", "messages": []},
            )
        assert (
            "authorization" not in seen[0].headers
            and "x-api-key" not in seen[0].headers
        )
    finally:
        CURRENT_ATTEMPT.reset(marker)
        used.secret.clear()
