"""Sidecar-owned subagents: parent-session tickets, restart, cancel, steer, fork."""

import json

from pydantic_ai.messages import ToolReturnPart, UserPromptPart
from runtime_helpers import fake_daemon, text_response, tool_spec

from openside_agent.runtime.state import SnapshotData, load_messages


def delegate_spec():
    return {
        "name": "delegate",
        "description": "Delegate a task to a subagent",
        "parameters": {
            "type": "object",
            "properties": {
                "task": {"type": "string"},
                "model": {"type": "string"},
                "tools": {"type": "array", "items": {"type": "string"}},
                "context": {"type": "string", "enum": ["fresh", "fork"]},
            },
            "required": ["task"],
            "additionalProperties": False,
        },
        "approval": "read",
        "deferrable": True,
    }


def call(name, id, args=None):
    return {"type": "tool_call", "name": name, "args": args or {}, "id": id}


def install(tmp_path, monkeypatch, responses, subagents, delay_ms=0):
    script = tmp_path / "script.json"
    script.write_text(
        json.dumps(
            {"responses": responses, "subagents": subagents, "delay_ms": delay_ms}
        )
    )
    monkeypatch.setenv("OPENSIDE_AGENT_SCRIPT", str(script))


def admission(call_message, subagent_id="sub-1", context="fresh"):
    assert call_message["name"] == "delegate" and call_message.get("subagent") is None
    return {
        "status": "done",
        "for_model": {
            "subagent_id": subagent_id,
            "model": "test:child-model",
            "tools": ["probe"],
            "context": context,
        },
    }


def snapshot_data(daemon):
    return SnapshotData.model_validate_json(daemon.snapshots()[-1]["data"])


def delegate_returns(daemon, call_id="del-1"):
    return [
        part
        for message in snapshot_data(daemon).restore_messages()
        for part in message.parts
        if isinstance(part, ToolReturnPart) and part.tool_call_id == call_id
    ]


def lifecycle(daemon, subagent_id="sub-1"):
    return [
        item["event"]["status"]
        for item in daemon.outputs
        if item["type"] == "event"
        and item["event"]["kind"] == "subagent.lifecycle"
        and item["event"]["subagent_id"] == subagent_id
    ]


PARENT = [
    {"parts": [call("delegate", "del-1", {"task": "inspect the fixture"})]},
    text_response("parent done"),
]
CHILD = {"del-1": [{"parts": [call("probe", "child-1")]}, text_response("child done")]}


async def defer_child(daemon):
    """Delegate, then defer the child's probe on an approval ticket."""
    await daemon.open(tools=[tool_spec(), delegate_spec()])
    await daemon.user()
    delegate = await daemon.until("tool_call")
    await daemon.result(delegate, admission(delegate))
    probe = await daemon.until("tool_call")
    assert probe["subagent"] == "sub-1" and probe["tool_call_id"] == "child-1"
    await daemon.result(
        probe, {"status": "deferred", "kind": "approval", "ticket_id": "ticket-1"}
    )
    deferred = await daemon.until("deferred")
    assert deferred["calls"] == [
        {"tool_call_id": "child-1", "ticket_id": "ticket-1", "kind": "approval"}
    ]
    await daemon.until("settled")
    assert [
        item["outcome"] for item in daemon.outputs if item["type"] == "message_result"
    ] == ["deferred"]
    return daemon.snapshots()[-1]


async def approve_and_finish(daemon):
    await daemon.send(
        "ticket_resolved",
        ticket_id="ticket-1",
        tool_call_id="child-1",
        resolution={"status": "approved"},
    )
    repeated = await daemon.until("tool_call")
    assert repeated["subagent"] == "sub-1"
    assert repeated["approval_ticket"] == "ticket-1"
    await daemon.result(repeated, {"status": "done", "for_model": "probed"})
    final = await daemon.until("event", kind="assistant.final")
    assert final["event"]["text"] == "parent done"
    await daemon.until("settled")


async def test_child_approval_is_parent_ticket_and_resumes_child_once(
    tmp_path, monkeypatch
):
    install(tmp_path, monkeypatch, PARENT, CHILD)
    async with fake_daemon() as (daemon, _):
        snapshot = await defer_child(daemon)
        saved = SnapshotData.model_validate_json(snapshot["data"])
        assert [item.tool_call_id for item in saved.pending_delegates] == ["del-1"]
        assert saved.subagents[0].status == "waiting_human"
        assert saved.subagents[0].model == "test:child-model"
        assert lifecycle(daemon) == ["running", "waiting_human"]
        await approve_and_finish(daemon)
        assert lifecycle(daemon) == [
            "running",
            "waiting_human",
            "running",
            "completed",
        ]
        returns = delegate_returns(daemon)
        assert len(returns) == 1
        assert returns[0].content == {
            "status": "completed",
            "subagent_id": "sub-1",
            "output": "child done",
        }
        child = snapshot_data(daemon).subagents[0]
        assert child.status == "completed" and not child.pending_deferred
        await daemon.send(
            "ticket_resolved",
            ticket_id="ticket-1",
            tool_call_id="child-1",
            resolution={"status": "approved"},
        )
        assert (await daemon.until("error"))["code"] == "stale_ticket"


async def test_child_wait_survives_restart_and_completes_once(tmp_path, monkeypatch):
    install(tmp_path, monkeypatch, PARENT, CHILD)
    async with fake_daemon() as (daemon, _):
        snapshot = await defer_child(daemon)
    async with fake_daemon() as (daemon, _):
        ready = await daemon.open(
            tools=[tool_spec(), delegate_spec()], resume_state=snapshot
        )
        assert ready["pending_deferred"] == [
            {"tool_call_id": "child-1", "ticket_id": "ticket-1", "kind": "approval"}
        ]
        await approve_and_finish(daemon)
        assert len(delegate_returns(daemon)) == 1
        assert [
            (item["message_id"], item["outcome"])
            for item in daemon.outputs
            if item["type"] == "message_result"
        ] == [("message-1", "completed")]


async def test_cancel_waiting_child_returns_cancelled_to_parent(tmp_path, monkeypatch):
    install(tmp_path, monkeypatch, PARENT, CHILD)
    async with fake_daemon() as (daemon, _):
        await defer_child(daemon)
        await daemon.send("subagent_cancel", subagent_id="sub-1")
        final = await daemon.until("event", kind="assistant.final")
        assert final["event"]["text"] == "parent done"
        await daemon.until("settled")
        assert lifecycle(daemon)[-1] == "cancelled"
        assert delegate_returns(daemon)[0].content == {
            "status": "cancelled",
            "subagent_id": "sub-1",
        }
        assert not any(
            item["type"] == "tool_call" and item.get("approval_ticket")
            for item in daemon.outputs
        )
        # Cancelling a finished child is harmless and reports no error.
        await daemon.send("subagent_cancel", subagent_id="sub-1")
        await daemon.send(
            "subagent_steer", subagent_id="sub-1", text="late", control_id="late"
        )
        assert (await daemon.until("subagent_steer_result"))["delivered"] is False
        await daemon.user("after", "after cancel")
        await daemon.until("settled")
        assert not any(item["type"] == "error" for item in daemon.outputs)


async def test_cancel_running_child_returns_cancelled(tmp_path, monkeypatch):
    install(tmp_path, monkeypatch, PARENT, CHILD)
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=[tool_spec(), delegate_spec()])
        await daemon.user()
        delegate = await daemon.until("tool_call")
        await daemon.result(delegate, admission(delegate))
        child_call = await daemon.until("tool_call")
        await daemon.send("subagent_cancel", subagent_id="sub-1")
        await daemon.until("event", kind="assistant.final")
        await daemon.until("settled")
        assert lifecycle(daemon) == ["running", "cancelled"]
        assert delegate_returns(daemon)[0].content["status"] == "cancelled"
        # The daemon still finishes the abandoned host call; that reply is harmless.
        await daemon.result(child_call, {"status": "done", "for_model": "late"})
        await daemon.user("after", "after cancel")
        await daemon.until("settled")
        assert [item for item in daemon.outputs if item["type"] == "error"] == []
        # A reply to an RPC that never existed is still a protocol error.
        await daemon.send(
            "tool_result",
            rpc_id="never-sent",
            outcome={"status": "done", "for_model": "x"},
        )
        assert (await daemon.until("error"))["code"] == "bad_request"


async def test_steer_injects_at_child_model_request_boundary(tmp_path, monkeypatch):
    child = {
        "del-1": [
            {
                "parts": [
                    {"type": "wait_steer", "timeout_ms": 5000},
                    call("probe", "child-1"),
                ]
            },
            text_response("child steered"),
        ]
    }
    install(tmp_path, monkeypatch, PARENT, child)
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=[tool_spec(), delegate_spec()])
        await daemon.user()
        delegate = await daemon.until("tool_call")
        await daemon.result(delegate, admission(delegate))
        await daemon.until("event", kind="subagent.progress")
        await daemon.send(
            "subagent_steer", subagent_id="sub-1", text="focus on X", control_id="live"
        )
        probe = await daemon.until("tool_call")
        await daemon.result(probe, {"status": "done", "for_model": "probed"})
        await daemon.until("settled")
        state = snapshot_data(daemon).subagents[0]
        prompts = [
            part.content
            for message in load_messages(state.messages)
            for part in message.parts
            if isinstance(part, UserPromptPart)
        ]
        assert prompts == ["inspect the fixture", "focus on X"]
        progress = [
            item["event"]["progress"]["kind"]
            for item in daemon.outputs
            if item["type"] == "event" and item["event"]["kind"] == "subagent.progress"
        ]
        assert "steer_queued" in progress and "steer_injected" in progress
        assert delegate_returns(daemon)[0].content["output"] == "child steered"
        parent_prompts = [
            part.content
            for message in snapshot_data(daemon).restore_messages()
            for part in message.parts
            if isinstance(part, UserPromptPart)
        ]
        assert "focus on X" not in parent_prompts


async def test_fork_context_excludes_delegating_batch(tmp_path, monkeypatch):
    parent = [
        {"parts": [call("probe", "parent-1")]},
        {
            "parts": [
                call("delegate", "del-1", {"task": "forked", "context": "fork"}),
                call("probe", "parent-2"),
            ]
        },
        text_response("parent done"),
    ]
    install(tmp_path, monkeypatch, parent, {"del-1": [text_response("child done")]})
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=[tool_spec(), delegate_spec()])
        await daemon.user()
        first = await daemon.until("tool_call")
        await daemon.result(first, {"status": "done", "for_model": "one"})
        calls = {}
        while len(calls) < 2:
            message = await daemon.until("tool_call")
            calls[message["name"]] = message
        await daemon.result(calls["probe"], {"status": "done", "for_model": "two"})
        await daemon.result(
            calls["delegate"], admission(calls["delegate"], context="fork")
        )
        await daemon.until("settled")
        child = snapshot_data(daemon).subagents[0]
        ids = {
            getattr(part, "tool_call_id", None)
            for message in load_messages(child.messages)
            for part in message.parts
        }
        assert "parent-1" in ids
        assert "del-1" not in ids and "parent-2" not in ids
