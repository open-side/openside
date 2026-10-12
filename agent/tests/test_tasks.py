"""Front agent and background Tasks: the front replies while Tasks run."""

import asyncio
import json

from runtime_helpers import fake_daemon, text_response, tool_spec

from openside_agent.runtime.state import SnapshotData


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
    task_spec("task_start", {"goal": {"type": "string"}}, ["goal"]),
    task_spec(
        "task_steer",
        {"task_id": {"type": "string"}, "message": {"type": "string"}},
        ["task_id", "message"],
    ),
    task_spec("task_cancel", {"task_id": {"type": "string"}}, ["task_id"]),
]
FRONT = {
    "orchestration": "front",
    "front_tools": ["task_start", "task_steer", "task_cancel"],
}


def call(name, id, args=None):
    return {"type": "tool_call", "name": name, "args": args or {}, "id": id}


def install(tmp_path, monkeypatch, responses, subagents):
    script = tmp_path / "script.json"
    script.write_text(json.dumps({"responses": responses, "subagents": subagents}))
    monkeypatch.setenv("OPENSIDE_AGENT_SCRIPT", str(script))


def admit(call_message, subagent_id):
    assert call_message["name"] == "task_start"
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
            {"parts": [call("task_start", "start-1", {"goal": "probe it"})]},
            text_response("started"),
            text_response("4"),
        ],
        {"start-1": [{"parts": [call("probe", "probe-1")]}, text_response("probed")]},
    )
    async with fake_daemon() as (daemon, _):
        ready = await daemon.open(tools=TOOLS, agent=FRONT)
        assert "probe" in ready["tools"]
        await daemon.user()
        start = await daemon.until("tool_call")
        await daemon.result(start, admit(start, "task-1"))
        assert (await daemon.until("event", kind="assistant.final"))["event"][
            "text"
        ] == "started"
        # The front run settles while the Task is still in its tool call.
        probe = await daemon.until("tool_call")
        assert probe["subagent"] == "task-1"
        assert any(item["type"] == "settled" for item in daemon.outputs)
        await daemon.user("question", "what is 2+2?")
        assert (await daemon.until("event", kind="assistant.final"))["event"][
            "text"
        ] == "4"
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
            }
        ]
        # The daemon queues the report; the sidecar must not settle in between.
        await asyncio.sleep(0.2)
        while not daemon.reader.at_eof():
            try:
                message = await asyncio.wait_for(daemon.receive(), 0.2)
            except TimeoutError:
                break
            assert message["type"] != "settled", message
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
                    call("task_start", "start-1", {"goal": "first"}),
                    call("task_start", "start-2", {"goal": "second"}),
                ]
            },
            text_response("started both"),
            {
                "parts": [
                    call(
                        "task_steer",
                        "steer-1",
                        {
                            "task_id": "{{result:start-2.task_id}}",
                            "message": "answer BETA",
                        },
                    )
                ]
            },
            text_response("steered"),
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
        assert steer["name"] == "task_steer"
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
    install(tmp_path, monkeypatch, [text_response("hi")], {})
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
            {"parts": [call("task_start", "start", {"goal": "finish"})]},
            text_response("started"),
            {
                "parts": [
                    call("task_steer", "steer", {"task_id": "task", "message": "late"})
                ]
            },
            text_response("checked"),
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
        assert steer["name"] == "task_steer"
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
