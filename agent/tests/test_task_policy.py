from pydantic_ai.models.test import TestModel
from runtime_helpers import fake_daemon, tool_spec

from openside_agent.runtime.task_policy import SUBAGENT_INSTRUCTIONS


async def test_worker_policy_is_installed_without_inventing_tools():
    async with fake_daemon(model=TestModel()) as (daemon, session):
        await daemon.open(tools=[tool_spec()])
        assert session.agent is not None
        assert SUBAGENT_INSTRUCTIONS in [
            item.instruction for item in session.agent._instructions
        ]
        assert session.expected_tools == {"probe"}


async def test_front_does_not_receive_worker_escalation_policy():
    async with fake_daemon(model=TestModel()) as (daemon, session):
        await daemon.open(
            tools=[tool_spec("send_message")],
            agent={"orchestration": "front", "front_tools": ["send_message"]},
        )
        assert session.agent is not None
        assert SUBAGENT_INSTRUCTIONS not in [
            item.instruction for item in session.agent._instructions
        ]


def test_escalation_ladder_distinguishes_absence_from_failure():
    policy = SUBAGENT_INSTRUCTIONS
    assert policy.index("1. Use information") < policy.index("2. Use an available")
    assert policy.index("2. Use an available") < policy.index("3. Use web search")
    assert policy.index("3. Use web search") < policy.index("4. Use the authorized")
    assert "only if a search capability is actually exposed" in policy
    assert "absence permits the next available" in policy
    assert "expired authentication, or access denial means stop" in policy
    assert "Never bypass it with the browser" in policy
    assert "structure before screenshots" in policy
    assert "security challenges require the\nperson" in policy
