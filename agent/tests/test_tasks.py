"""Front agent and background Tasks: the front replies while Tasks run."""

import json

import pytest
from pydantic_ai.models.test import TestModel
from runtime_helpers import fake_daemon, text_response, tool_spec

from openside_agent import providers
from openside_agent.runtime.state import SnapshotData, SubagentState
from openside_agent.runtime.subagents import Child


def task_spec(name, properties, required):
    return {
        "name": name,
        "description": name,
        "parameters": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": False,
        },
        "approval": "read",
        "deferrable": False,
    }


TOOLS = [
    tool_spec(),
    task_spec("create_task", {"goal": {"type": "string"}}, ["goal"]),
    task_spec(
        "steer_task",
        {"task_id": {"type": "string"}, "message": {"type": "string"}},
        ["task_id", "message"],
    ),
    task_spec("cancel_task", {"task_id": {"type": "string"}}, ["task_id"]),
    task_spec("send_message", {"text": {"type": "string"}}, ["text"]),
]
FRONT = {
    "orchestration": "front",
    "front_tools": ["create_task", "steer_task", "cancel_task", "send_message"],
}


def call(name, id, args=None):
    return {"type": "tool_call", "name": name, "args": args or {}, "id": id}


def install(tmp_path, monkeypatch, responses, subagents):
    script = tmp_path / "script.json"
    script.write_text(json.dumps({"responses": responses, "subagents": subagents}))
    monkeypatch.setenv("OPENSIDE_AGENT_SCRIPT", str(script))


def admit(call_message, subagent_id):
    assert call_message["name"] == "create_task"
    return {
        "status": "done",
        "for_model": {
            "subagent_id": subagent_id,
            "model": "test:task-model",
            "tools": ["probe"],
            "context": "fresh",
        },
    }


def events(daemon, kind):
    return [
        item["event"]
        for item in daemon.outputs
        if item["type"] == "event" and item["event"]["kind"] == kind
    ]


async def test_front_replies_while_the_task_runs_and_settles_once(
    tmp_path, monkeypatch
):
    install(
        tmp_path,
        monkeypatch,
        [
            {"parts": [call("create_task", "start-1", {"goal": "probe it"})]},
            {"parts": [call("send_message", "started-notice", {"text": "started"})]},
            {"parts": [call("send_message", "quick-answer", {"text": "4"})]},
        ],
        {"start-1": [{"parts": [call("probe", "probe-1")]}, text_response("probed")]},
    )
    async with fake_daemon() as (daemon, _):
        ready = await daemon.open(tools=TOOLS, agent=FRONT)
        assert "probe" in ready["tools"]
        await daemon.user()
        start = await daemon.until("tool_call")
        await daemon.result(start, admit(start, "task-1"))
        probe = None
        while True:
            notice = await daemon.until("tool_call")
            if notice["name"] == "send_message":
                break
            assert notice["name"] == "probe" and notice["subagent"] == "task-1"
            probe = notice
        assert notice["args"] == {"text": "started"}
        assert notice.get("subagent") is None
        await daemon.result(notice, {"status": "done", "for_model": {"sent": True}})
        # The front run settles while the Task is still in its tool call.
        await daemon.until("settled")
        if probe is None:
            probe = next(
                (
                    item
                    for item in daemon.outputs
                    if item["type"] == "tool_call" and item["name"] == "probe"
                ),
                None,
            )
        if probe is None:
            probe = await daemon.until("tool_call")
        assert probe["subagent"] == "task-1"
        await daemon.user("question", "what is 2+2?")
        answer = await daemon.until("tool_call")
        assert answer["name"] == "send_message" and answer["args"] == {"text": "4"}
        assert answer.get("subagent") is None
        await daemon.result(answer, {"status": "done", "for_model": {"sent": True}})
        assert events(daemon, "assistant.final") == []
        assert events(daemon, "assistant.delta") == []
        assert events(daemon, "thinking.delta") == []
        await daemon.until("settled")
        await daemon.result(probe, {"status": "done", "for_model": "ok"})
        ended = [
            event
            for event in events(daemon, "subagent.lifecycle")
            if event["status"] != "running"
        ]
        while not ended:
            await daemon.receive()
            ended = [
                event
                for event in events(daemon, "subagent.lifecycle")
                if event["status"] != "running"
            ]
        assert ended == [
            {
                "kind": "subagent.lifecycle",
                "subagent_id": "task-1",
                "status": "completed",
                "output": "probed",
                "known_subagents": [],
            }
        ]
        # The daemon needs the idle signal to dispatch its durable report queue;
        # it withholds public SessionSettled until those reports are complete.
        await daemon.until("settled")
        start_result = [
            part
            for message in SnapshotData.model_validate_json(
                daemon.snapshots()[-1]["data"]
            ).restore_messages()
            for part in message.parts
            if getattr(part, "tool_call_id", None) == "start-1"
            and part.part_kind == "tool-return"
        ]
        assert start_result[0].content == {"task_id": "task-1", "status": "running"}


async def test_task_steer_reaches_only_its_task(tmp_path, monkeypatch):
    wait = {
        "parts": [{"type": "wait_steer", "timeout_ms": 5000}, call("probe", "probe")]
    }
    install(
        tmp_path,
        monkeypatch,
        [
            {
                "parts": [
                    call("create_task", "start-1", {"goal": "first"}),
                    call("create_task", "start-2", {"goal": "second"}),
                ]
            },
            {"parts": [call("stay_silent", "started-both")]},
            {
                "parts": [
                    call(
                        "steer_task",
                        "steer-1",
                        {
                            "task_id": "{{result:start-2.task_id}}",
                            "message": "answer BETA",
                        },
                    )
                ]
            },
            {"parts": [call("stay_silent", "steered")]},
        ],
        {"start-1": [wait], "start-2": [wait, {"parts": [{"type": "echo"}]}]},
    )
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=TOOLS, agent=FRONT)
        await daemon.user()
        first = await daemon.until("tool_call")
        await daemon.result(first, admit(first, "task-1"))
        second = await daemon.until("tool_call")
        await daemon.result(second, admit(second, "task-2"))
        await daemon.until("settled")
        await daemon.user("steer", "make the second one say BETA")
        steer = await daemon.until("tool_call")
        assert steer["name"] == "steer_task"
        assert steer["args"]["task_id"] == "task-2"
        await daemon.result(
            steer,
            {"status": "done", "for_model": {"task_id": "task-2", "status": "pending"}},
        )
        while True:
            done = [
                event
                for event in events(daemon, "subagent.lifecycle")
                if event["status"] == "completed"
            ]
            if done:
                break
            message = await daemon.receive()
            if message["type"] == "tool_call" and message["subagent"] == "task-2":
                await daemon.result(message, {"status": "done", "for_model": "ok"})
        assert done == [
            {
                "kind": "subagent.lifecycle",
                "subagent_id": "task-2",
                "status": "completed",
                "output": "answer BETA",
                "known_subagents": ["task-1"],
            }
        ]
        await daemon.send("subagent_cancel", subagent_id="task-1")
        while not [
            event
            for event in events(daemon, "subagent.lifecycle")
            if event["subagent_id"] == "task-1" and event["status"] == "cancelled"
        ]:
            await daemon.receive()


async def test_front_exposes_only_its_tools(tmp_path, monkeypatch):
    install(tmp_path, monkeypatch, [{"parts": [call("stay_silent", "silent")]}], {})
    async with fake_daemon() as (daemon, _):
        ready = await daemon.open(tools=TOOLS, agent=FRONT)
        assert sorted(ready["tools"]) == sorted(tool["name"] for tool in TOOLS)
        await daemon.user()
        await daemon.until("settled")


async def test_task_steer_denies_if_child_finishes_after_host_admission(
    tmp_path, monkeypatch
):
    install(
        tmp_path,
        monkeypatch,
        [
            {"parts": [call("create_task", "start", {"goal": "finish"})]},
            {"parts": [call("stay_silent", "started")]},
            {
                "parts": [
                    call("steer_task", "steer", {"task_id": "task", "message": "late"})
                ]
            },
            {"parts": [call("stay_silent", "checked")]},
        ],
        {"start": [{"parts": [call("probe", "probe")]}, text_response("done")]},
    )
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=TOOLS, agent=FRONT)
        await daemon.user()
        start = await daemon.until("tool_call")
        await daemon.result(start, admit(start, "task"))
        probe = await daemon.until("tool_call")
        if not any(item["type"] == "settled" for item in daemon.outputs):
            await daemon.until("settled")
        await daemon.user("steer", "late instruction")
        steer = await daemon.until("tool_call")
        assert steer["name"] == "steer_task"
        await daemon.result(probe, {"status": "done", "for_model": "ok"})
        while not any(
            event["status"] == "completed"
            for event in events(daemon, "subagent.lifecycle")
        ):
            await daemon.receive()
        await daemon.result(
            steer,
            {"status": "done", "for_model": {"task_id": "task", "status": "pending"}},
        )
        await daemon.until("settled")
        returns = [
            part
            for message in SnapshotData.model_validate_json(
                daemon.snapshots()[-1]["data"]
            ).restore_messages()
            for part in message.parts
            if getattr(part, "tool_call_id", None) == "steer"
            and part.part_kind == "tool-return"
        ]
        assert returns[0].content == {"status": "denied", "reason": "task_finished"}
        assert not any(
            event["progress"]["kind"] == "steer_queued"
            for event in events(daemon, "subagent.progress")
        )
        await daemon.send(
            "subagent_steer", subagent_id="task", text="late", control_id="finished"
        )
        ack = await daemon.until("subagent_steer_result")
        assert ack == {
            "v": 1,
            "type": "subagent_steer_result",
            "control_id": "finished",
            "delivered": False,
        }


@pytest.mark.parametrize("fast", ["openai:gpt-6-luna", None])
@pytest.mark.parametrize("profile", [None, "ultrabrowse"])
async def test_front_model_never_leaks_into_task_children(monkeypatch, fast, profile):
    selected = []

    async def build(identifier, fallback, credentials, **kwargs):
        selected.append((identifier, list(fallback), kwargs.get("subagent")))
        return TestModel()

    monkeypatch.delenv("OPENSIDE_AGENT_SCRIPT", raising=False)
    monkeypatch.setattr(providers, "build_model", build)
    async with fake_daemon() as (daemon, session):
        await daemon.open(
            tools=TOOLS, agent={**FRONT, "front_model": fast, "profile": profile}
        )
        reasoning = session.spec.model.id
        session.spec.model.fallback = ["test:captured-fallback"]
        child = Child(
            SubagentState(
                id="task-model-role",
                parent_tool_call_id="start-role",
                task="reason about the result",
                model=reasoning,
                tools=[],
                status="running",
                background=True,
            )
        )
        await session.child_agent(child)
        assert session.agent.model_settings == {
            "thinking": "minimal",
            "max_tokens": 1024,
        }
        assert child.agent.model_settings is None
        assert selected == [
            (fast or reasoning, [], None),
            (reasoning, ["test:captured-fallback"], child.id),
        ]


def test_front_routing_guidance_snapshot():
    from pathlib import Path

    from openside_agent.runtime.subagents import FRONT_INSTRUCTIONS

    snapshot = Path(__file__).parent / "snapshots" / "front-instructions.txt"
    assert FRONT_INSTRUCTIONS + "\n" == snapshot.read_text()
