import copy
import json
from pathlib import Path

import pytest
from jsonschema import Draft202012Validator
from pydantic_ai.messages import RetryPromptPart, ToolReturnPart
from runtime_helpers import fake_daemon, install_script, text_response, tool_spec
from test_subagents import call, delegate_spec, install

from openside_agent.runtime import PRESENT_INSTRUCTIONS
from openside_agent.runtime.state import SnapshotData, load_messages
from openside_agent.runtime.subagents import (
    CHIEF_INSTRUCTIONS,
    FRONT_INSTRUCTIONS,
    SUBAGENT_INSTRUCTIONS,
    TEAMMATE_INSTRUCTIONS,
)
from openside_agent.scripted import ScriptedModel

SNAPSHOTS = Path(__file__).resolve().parents[2] / "docs/specs/profiles"


def catalog_tool(name, profile="browser-agent"):
    # These snapshots are generated from the daemon's actual sidecar open payload.
    payload = json.loads((SNAPSHOTS / f"{profile}.open.json").read_text())
    return next(tool for tool in payload["tools"] if tool["name"] == name)


def draft():
    return {
        "card": {
            "id": "",
            "kind": "table",
            "site": {"origin": None, "name": "Comparison", "favicon_ref": None},
            "title": "Options",
            "status": "done",
            "blocks": [
                {
                    "type": "table",
                    "id": "options",
                    "columns": [{"id": "name", "label": "Name"}],
                    "rows": [[{"type": "text", "text": "Option A"}]],
                    "layout": "grid",
                }
            ],
            "actions": [],
            "interaction": "none",
            "collapsed_summary": "One option",
        },
        "images": [],
    }


def snapshot(daemon):
    return SnapshotData.model_validate_json(daemon.snapshots()[-1]["data"])


def parts(messages, kind):
    return [
        part for message in messages for part in message.parts if isinstance(part, kind)
    ]


@pytest.fixture
def requests(monkeypatch):
    observed = []
    original = ScriptedModel.stream

    async def capture(self, messages, info):
        observed.append((copy.deepcopy(messages), info))
        async for item in original(self, messages, info):
            yield item

    monkeypatch.setattr(ScriptedModel, "stream", capture)
    return observed


def test_present_examples_follow_the_daemon_catalog_schema():
    spec = catalog_tool("present")
    assert spec == catalog_tool("present", "ultrabrowse")
    assert spec["approval"] == "read" and spec["deferrable"] is False
    example, _ = json.JSONDecoder().raw_decode(
        PRESENT_INSTRUCTIONS.split("present(", 1)[1]
    )
    validator = Draft202012Validator(spec["parameters"])
    validator.validate(example)
    validator.validate(draft())


@pytest.mark.parametrize("role", ["single", "front", "chief", "teammate"])
async def test_catalog_present_returns_only_card_id(
    tmp_path, monkeypatch, requests, role
):
    front = role in {"front", "chief"}
    reply = "Here is the option I found."
    install_script(
        tmp_path,
        monkeypatch,
        [
            {"parts": [call("present", "present-1", draft())]},
            {
                "parts": [
                    call("probe", "check-id", {"value": "{{result:present-1.card_id}}"})
                ]
            },
            {"parts": [call("send_message", "present-reply", {"text": reply})]}
            if front
            else text_response(reply),
        ],
    )
    team = (
        {"role": role, "agent_id": role, "persona": "Careful helper", "roster": "Team"}
        if role in {"chief", "teammate"}
        else None
    )
    spec = catalog_tool("present")
    tools = [spec, tool_spec()]
    if front:
        send = tool_spec("send_message")
        send["parameters"] = {
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"],
            "additionalProperties": False,
        }
        tools.append(send)
    async with fake_daemon() as (daemon, session):
        await daemon.open(
            tools=tools,
            agent={
                "orchestration": "single" if role == "single" else "front",
                "front_tools": ["present", "probe", "send_message"]
                if front
                else ["present", "probe"],
                "title_needed": False,
            },
            team=team,
        )
        await daemon.user()
        presented = await daemon.until("tool_call")
        assert presented["v"] == 1 and presented["name"] == "present"
        assert presented["args"] == draft()
        result = {"card_id": "present:present-1"}
        await daemon.result(presented, {"status": "done", "for_model": result})
        checked = await daemon.until("tool_call")
        assert checked["args"] == {"value": result["card_id"]}
        await daemon.result(checked, {"status": "done", "for_model": "ok"})
        if front:
            final = await daemon.until("tool_call")
            assert final["name"] == "send_message" and final["args"] == {"text": reply}
            await daemon.result(final, {"status": "done", "for_model": {"sent": True}})
        else:
            final = await daemon.until("event", kind="assistant.final")
            assert final["event"]["text"] == reply
        await daemon.until("settled")
        if front:
            assert not any(
                item["type"] == "event"
                and item["event"]["kind"] in {"assistant.final", "assistant.delta"}
                for item in daemon.outputs
            )
        returns = parts(snapshot(daemon).restore_messages(), ToolReturnPart)
        assert [p.content for p in returns if p.tool_name == "present"] == [result]
        assert session.front is (role in {"front", "chief"})
    for messages, info in requests:
        assert PRESENT_INSTRUCTIONS in info.instructions
        tool = next(t for t in info.function_tools if t.name == "present")
        assert tool.parameters_json_schema == spec["parameters"]
        assert all(
            p.content == result
            for p in parts(messages, ToolReturnPart)
            if p.tool_name == "present"
        )
        if team:
            expected = CHIEF_INSTRUCTIONS if role == "chief" else TEAMMATE_INSTRUCTIONS
            assert expected in info.instructions
            assert team["persona"] in info.instructions
            assert team["roster"] in str(messages)
            assert (FRONT_INSTRUCTIONS in info.instructions) is (role == "chief")
        elif role == "front":
            assert FRONT_INSTRUCTIONS in info.instructions


async def test_present_instructions_require_catalog_exposure(
    tmp_path, monkeypatch, requests
):
    install_script(tmp_path, monkeypatch, [text_response()])
    async with fake_daemon() as (daemon, _):
        await daemon.open(agent={"title_needed": False})
        await daemon.user()
        await daemon.until("settled")
    assert PRESENT_INSTRUCTIONS not in requests[0][1].instructions


@pytest.mark.parametrize(
    "mutation,outcome",
    [
        (
            "malformed",
            {"status": "error", "message": "invalid_card: $: invalid shape"},
        ),
        (
            "privileged",
            {
                "status": "denied",
                "reason": "card_action_forbidden",
                "hint": "Use the daemon's secure page tools instead",
            },
        ),
    ],
)
async def test_present_draft_rejection_reaches_model_retry(
    tmp_path, monkeypatch, requests, mutation, outcome
):
    invalid = draft()
    if mutation == "malformed":
        invalid["card"]["blocks"][0]["rows"] = [[]]
    else:
        invalid["card"]["actions"] = [
            {
                "id": "reject",
                "label": "Reject",
                "style": "secondary",
                "target": {"type": "ticket_reject"},
            }
        ]
    install_script(
        tmp_path,
        monkeypatch,
        [
            {"parts": [call("present", "bad-draft", invalid)]},
            {"parts": [call("present", "corrected-draft", draft())]},
            text_response("Here is the corrected comparison."),
        ],
    )
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=[catalog_tool("present")])
        await daemon.user()
        rejected = await daemon.until("tool_call")
        assert rejected["args"] == invalid
        await daemon.result(rejected, outcome)
        retried = await daemon.until("tool_call")
        assert retried["tool_call_id"] == "corrected-draft"
        assert retried["args"] == draft()
        result = {"card_id": "present:corrected-draft"}
        await daemon.result(retried, {"status": "done", "for_model": result})
        await daemon.until("settled")
        messages = snapshot(daemon).restore_messages()
        retries = parts(messages, RetryPromptPart)
        assert len(retries) == 1 and retries[0].tool_call_id == "bad-draft"
        assert outcome.get("reason", outcome.get("message")) in retries[0].content
        if "hint" in outcome:
            assert outcome["hint"] in retries[0].content
        assert [p.content for p in parts(messages, ToolReturnPart)] == [result]
    assert parts(requests[1][0], RetryPromptPart)


@pytest.mark.parametrize(
    "name,args,result",
    [
        (
            "request_login",
            {"tab_id": "tab"},
            {"status": "submitted", "next": "unknown"},
        ),
        ("request_otp", {"tab_id": "tab"}, {"status": "filled"}),
        (
            "request_form",
            {"tab_id": "tab", "purpose": "payment"},
            {"status": "filled", "field_count": 4, "next": "review_page"},
        ),
    ],
)
async def test_secure_outcomes_keep_private_values_out_of_model_and_snapshots(
    tmp_path, monkeypatch, requests, capsys, name, args, result
):
    private_value = "private-page-answer-canary-938247"
    monkeypatch.setenv("OPENSIDE_BROKER_TOKEN", private_value)
    install_script(
        tmp_path,
        monkeypatch,
        [
            {"parts": [call(name, "secure-step", args)]},
            text_response("The secure step is complete."),
        ],
    )
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=[catalog_tool(name), catalog_tool("present")])
        await daemon.user()
        secure = await daemon.until("tool_call")
        await daemon.result(
            secure, {"status": "deferred", "ticket_id": "secure", "kind": "external"}
        )
        await daemon.until("settled")
        await daemon.send(
            "ticket_resolved",
            ticket_id="secure",
            tool_call_id="secure-step",
            resolution={"status": "result", "for_model": result},
        )
        await daemon.until("settled")
        returns = parts(snapshot(daemon).restore_messages(), ToolReturnPart)
        assert [p.content for p in returns] == [result]
        assert private_value not in json.dumps(daemon.outputs)
        assert all(item["v"] == 1 for item in daemon.outputs)
        assert all(
            item["format"] == "openside-agent-state/1" for item in daemon.snapshots()
        )
    assert private_value not in repr(requests)
    captured = capsys.readouterr()
    assert private_value not in captured.out + captured.err


async def test_delegated_present_retains_subagent_instructions(
    tmp_path, monkeypatch, requests
):
    install(
        tmp_path,
        monkeypatch,
        [
            {
                "parts": [
                    call("delegate", "delegate-present", {"task": "Compare options"})
                ]
            },
            text_response("The comparison is ready."),
        ],
        {
            "delegate-present": [
                {"parts": [call("present", "child-present", draft())]},
                text_response("Comparison presented."),
            ]
        },
    )
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=[delegate_spec(), catalog_tool("present")])
        await daemon.user()
        delegated = await daemon.until("tool_call")
        await daemon.result(
            delegated,
            {
                "status": "done",
                "for_model": {
                    "subagent_id": "sub-present",
                    "model": "test:child-model",
                    "tools": ["present"],
                    "context": "fresh",
                },
            },
        )
        presented = await daemon.until("tool_call")
        assert presented["name"] == "present" and presented["subagent"] == "sub-present"
        result = {"card_id": "present:child-present"}
        await daemon.result(presented, {"status": "done", "for_model": result})
        await daemon.until("settled")
        child = snapshot(daemon).subagents[0]
        assert child.status == "completed"
        assert [
            p.content for p in parts(load_messages(child.messages), ToolReturnPart)
        ] == [result]
    child_requests = [
        info
        for _, info in requests
        if {t.name for t in info.function_tools} == {"present"}
    ]
    assert child_requests
    assert all(
        SUBAGENT_INSTRUCTIONS in info.instructions
        and PRESENT_INSTRUCTIONS in info.instructions
        for info in child_requests
    )
