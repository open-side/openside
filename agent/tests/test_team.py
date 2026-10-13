import pytest
from pydantic import ValidationError
from runtime_helpers import fake_daemon, install_script, text_response, tool_spec

from openside_agent.protocol import TeamContext
from openside_agent.runtime.subagents import (
    CHIEF_TOOLS,
    FRONT_INSTRUCTIONS,
    TEAMMATE_TOOLS,
)
from openside_agent.scripted import ScriptedModel


@pytest.mark.parametrize(
    "role,names", [("chief", CHIEF_TOOLS), ("teammate", TEAMMATE_TOOLS)]
)
async def test_team_role_prompt_and_tool_visibility(tmp_path, monkeypatch, role, names):
    if role == "chief":
        names = names | {"send_message"}
    terminal = (
        {
            "parts": [
                {"type": "tool_call", "name": "stay_silent", "args": {}, "id": "silent"}
            ]
        }
        if role == "chief"
        else text_response()
    )
    install_script(tmp_path, monkeypatch, [terminal])
    persona = "Persona: " + "🦉" * (1500 - len("Persona: "))
    roster = "Roster: " + "界" * (2000 - len("Roster: "))
    observed = []
    original = ScriptedModel.stream

    async def capture(self, messages, info):
        observed.append((messages, info))
        async for item in original(self, messages, info):
            yield item

    monkeypatch.setattr(ScriptedModel, "stream", capture)
    tools = [tool_spec(name) for name in sorted(names)]
    for spec in tools:
        if spec["name"] == "send_message":
            spec["parameters"] = {
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"],
                "additionalProperties": False,
            }
    async with fake_daemon() as (daemon, session):
        ready = await daemon.open(
            tools=tools,
            team={"role": role, "agent_id": role, "persona": persona, "roster": roster},
            agent={
                "orchestration": "front",
                "front_tools": sorted(names),
                "title_needed": False,
            },
        )
        assert set(ready["tools"]) == names
        assert session.expected_tools == (
            names - {"send_message"} if role == "chief" else names
        )
        await daemon.user()
        await daemon.until("settled")
    assert len(observed) == 1
    messages, info = observed[0]
    assert {tool.name for tool in info.function_tools} == (
        names - {"send_message"} if role == "chief" else names
    )
    if role == "chief":
        assert {tool.name for tool in info.output_tools} == {
            "send_message",
            "stay_silent",
        }
    assert persona in info.instructions
    assert roster in str(messages)
    assert len(persona) == 1500 and len(roster) == 2000
    assert f"[{role.upper()}]" in info.instructions
    if role == "chief":
        assert FRONT_INSTRUCTIONS in info.instructions
    else:
        assert "You are the front agent" not in info.instructions


@pytest.mark.parametrize(
    "role,wrong",
    [("chief", "team_report"), ("teammate", "team_hire"), (None, "team_status")],
)
async def test_team_wrong_role_tools_fail_closed(role, wrong):
    async with fake_daemon() as (daemon, _):
        await daemon.send(
            "open",
            session="test",
            model={"id": "test:model", "fallback": []},
            tools=[tool_spec(wrong)],
            system_prompt_parts=[],
            config={"credentials": {}, "agent": {}},
            team=None
            if role is None
            else {"role": role, "agent_id": role, "persona": "", "roster": ""},
        )
        error = await daemon.until("error")
        assert error["code"] == "tool_gate" and error["fatal"]


@pytest.mark.parametrize("field,limit", [("persona", 1500), ("roster", 2000)])
def test_team_wire_character_caps(field, limit):
    data = {"role": "chief", "agent_id": "chief", "persona": "", "roster": ""}
    data[field] = "🦉" * limit
    assert len(getattr(TeamContext.model_validate(data), field)) == limit
    data[field] += "x"
    with pytest.raises(ValidationError):
        TeamContext.model_validate(data)


def test_team_wire_rejects_unknown_role():
    with pytest.raises(ValidationError):
        TeamContext(role="admin", agent_id="chief", persona="", roster="")
