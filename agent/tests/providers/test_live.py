"""Opt-in smoke only; never part of deterministic provider verification."""

import json
import os

import pytest
from pydantic_ai import Agent

from openside_agent.providers import AuthUnavailable, build_model
from openside_agent.providers.broker import BrokerClient


@pytest.mark.live
async def test_live_provider(monkeypatch: pytest.MonkeyPatch):
    model_id = os.getenv("OPENSIDE_LIVE_MODEL")
    if not model_id:
        pytest.skip(
            "Set OPENSIDE_LIVE_MODEL to explicitly enable a real provider request"
        )

    async def refuse_live_block(*args, **kwargs):
        raise AuthUnavailable(
            "live", ["rate limited; broker writes prohibited in smoke"]
        )

    monkeypatch.setattr(BrokerClient, "block", refuse_live_block)
    credentials = json.loads(os.getenv("OPENSIDE_LIVE_CREDENTIALS", "{}"))
    model = await build_model(model_id, [], credentials, session_id="live-smoke")
    async with Agent(model) as agent:
        result = await agent.run("Reply with exactly: pong")
        assert result.output.strip() == "pong"
