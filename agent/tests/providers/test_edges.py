import pytest
from pydantic_ai import Agent
from pydantic_ai.exceptions import ModelAPIError
from pydantic_ai.messages import ModelResponse, TextPart
from pydantic_ai.models.fallback import FallbackModel
from pydantic_ai.models.function import FunctionModel

from openside_agent.providers import AuthUnavailable


@pytest.mark.parametrize("auth_failure", [True, False])
async def test_explicit_model_fallback_never_catches_auth_unavailable(auth_failure):
    called = []

    def primary(messages, info):
        called.append("primary")
        if auth_failure:
            raise AuthUnavailable(
                "anthropic", ["No eligible account in the configured pool"]
            )
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
