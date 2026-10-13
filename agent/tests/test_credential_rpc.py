import asyncio
import json

import httpx2
import pytest
from pydantic_ai.models.test import TestModel
from runtime_helpers import fake_daemon

from openside_agent.providers import AuthUnavailable
from openside_agent.providers.outcomes import ClassifiedOutcome

CANARY = "synthetic-daemon-rpc-secret-canary"


async def credential(daemon, request, account):
    await daemon.send(
        "credential_response",
        id=request["id"],
        request_id=request["request_id"],
        lease_id=f"lease-{account}-{request['attempt']}",
        generation=1,
        account={"source": "vault", "account_id": f"opaque-{account}"},
        auth={"kind": "api_key", "value": f"{CANARY}-{account}"},
    )


async def acknowledge(daemon, report, action, *, reason=None):
    await daemon.send(
        "credential_reported",
        id=report["id"],
        request_id=report["request_id"],
        lease_id=report["lease_id"],
        generation=2,
        action=action,
        reason=reason,
    )


def success_stream():
    chunks = [
        {
            "id": "fake-chat",
            "object": "chat.completion.chunk",
            "created": 1,
            "model": "gpt-5",
            "choices": [
                {
                    "index": 0,
                    "delta": {"role": "assistant", "content": "done"},
                    "finish_reason": None,
                }
            ],
        },
        {
            "id": "fake-chat",
            "object": "chat.completion.chunk",
            "created": 1,
            "model": "gpt-5",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
        },
    ]
    return httpx2.Response(
        200,
        headers={"content-type": "text/event-stream"},
        content="".join(f"data: {json.dumps(chunk)}\n\n" for chunk in chunks)
        + "data: [DONE]\n\n",
    )


async def open_model(daemon):
    await daemon.send(
        "open",
        session="test-session",
        model={"id": "openai-chat:gpt-5", "fallback": []},
        system_prompt_parts=[],
        tools=[],
        config={
            "credentials": {},
            "agent": {"title_needed": False, "orchestration": "single"},
        },
    )
    await daemon.until("ready")


@pytest.mark.parametrize("failure", [401, 429])
async def test_runtime_awaits_report_ack_rotates_and_requests_every_turn(
    monkeypatch, failure
):
    from openside_agent.providers import request_auth

    seen = []
    build = request_auth.request_model

    def wire(request):
        seen.append(request.headers["authorization"])
        if len(seen) == 1:
            return httpx2.Response(
                failure, headers={"retry-after": "7"}, json={"error": CANARY}
            )
        return success_stream()

    monkeypatch.setattr(
        request_auth,
        "request_model",
        lambda model, config, **kwargs: build(
            model,
            config,
            http_client=httpx2.AsyncClient(
                trust_env=False, transport=httpx2.MockTransport(wire)
            ),
        ),
    )
    async with fake_daemon() as (daemon, session):
        await open_model(daemon)
        await daemon.user()
        first = await daemon.until("credential_request")
        assert first["attempt"] == 0 and first["provider"] == "openai-chat"
        assert "session" not in first and "affinity_key" not in first
        await credential(daemon, first, "one")
        report = await daemon.until("credential_report")
        assert report["outcome"] == ("auth" if failure == 401 else "rate_limit")
        assert report["stream_started"] is False
        assert report.get("retry_after_ms") == (7000 if failure == 429 else None)
        assert len(seen) == 1 and len(session.credential_futures) == 1
        await acknowledge(daemon, report, "rotate")
        second = await daemon.until("credential_request")
        assert second["attempt"] == 1 and second["request_id"] == first["request_id"]
        await credential(daemon, second, "two")
        success = await daemon.until("credential_report")
        assert success["outcome"] == "success" and success["stream_started"]
        assert success.get("usage") is None
        await acknowledge(daemon, success, "complete")
        await daemon.until("settled")
        await daemon.user("message-2", "next turn")
        third = await daemon.until("credential_request")
        assert third["attempt"] == 0 and third["request_id"] != first["request_id"]
        await credential(daemon, third, "two")
        await acknowledge(daemon, await daemon.until("credential_report"), "complete")
        await daemon.until("settled")
        assert CANARY not in json.dumps(daemon.outputs)
    assert seen == [
        f"Bearer {CANARY}-one",
        f"Bearer {CANARY}-two",
        f"Bearer {CANARY}-two",
    ]


async def test_denial_has_typed_reason_and_redacts_daemon_error_text():
    async with fake_daemon(model=TestModel()) as (daemon, session):
        await daemon.open()
        pending = asyncio.create_task(
            session.credential_client.acquire(
                "openai", "openai:gpt-5", "model1", 0, None
            )
        )
        request = await daemon.until("credential_request")
        await daemon.send(
            "credential_error",
            id=request["id"],
            request_id="model1",
            code="auth_unavailable",
            reason=CANARY,
            retry_at_ms=12345,
            generation=1,
        )
        with pytest.raises(AuthUnavailable) as error:
            await pending
        assert (
            error.value.reason == "auth_unavailable"
            and error.value.retry_at_ms == 12345
        )
        assert CANARY not in str(error.value)


async def test_cancelled_report_is_send_only_and_late_ack_is_discarded():
    async with fake_daemon(model=TestModel()) as (daemon, session):
        await daemon.open()
        pending = asyncio.create_task(
            session.credential_client.acquire(
                "openai", "openai:gpt-5", "model1", 0, None
            )
        )
        await credential(daemon, await daemon.until("credential_request"), "one")
        lease = await pending
        try:
            assert (
                await session.credential_client.report(
                    lease, ClassifiedOutcome("cancelled"), "model1", wait=False
                )
                is False
            )
            report = await daemon.until("credential_report")
            assert report["outcome"] == "cancelled"
            await acknowledge(daemon, report, "complete")
            await daemon.user()
            await daemon.until("settled")
            assert not session.abandoned_credentials
            assert not any(item["type"] == "error" for item in daemon.outputs)
            assert CANARY not in json.dumps(daemon.outputs)
        finally:
            lease.credential.secret.clear()
