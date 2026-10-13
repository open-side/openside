import json

import pytest
from pydantic_ai.messages import RetryPromptPart, ToolReturnPart, UserPromptPart
from pydantic_ai.models.function import FunctionModel
from runtime_helpers import (
    fake_daemon,
    install_script,
    text_response,
    tool_response,
    tool_spec,
)

from openside_agent import providers
from openside_agent.runtime.state import SnapshotData


def state_messages(daemon):
    return SnapshotData.model_validate_json(
        daemon.snapshots()[-1]["data"]
    ).restore_messages()


def user_texts(daemon):
    return [
        part.content
        for message in state_messages(daemon)
        for part in message.parts
        if isinstance(part, UserPromptPart)
    ]


@pytest.mark.parametrize(
    "outcome",
    [
        {"status": "done", "for_model": {"value": "daemon result"}},
        {
            "status": "denied",
            "reason": "blocked",
            "hint": "Use the allowed alternative",
        },
        {"status": "error", "message": "Tool unavailable"},
    ],
)
async def test_reverse_rpc_outcomes(tmp_path, monkeypatch, outcome):
    install_script(tmp_path, monkeypatch, [tool_response(), text_response()])
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=[tool_spec()])
        await daemon.user()
        call = await daemon.until("tool_call")
        assert call["tool_call_id"] == "call-1"
        await daemon.result(call, outcome)
        await daemon.until("settled")
        messages = state_messages(daemon)
        if outcome["status"] == "done":
            assert any(
                isinstance(part, ToolReturnPart)
                and part.content == outcome["for_model"]
                for message in messages
                for part in message.parts
            )
        else:
            retries = [
                part
                for message in messages
                for part in message.parts
                if isinstance(part, RetryPromptPart)
            ]
            assert len(retries) == 1
            assert outcome.get("reason", outcome.get("message")) in retries[0].content
            if outcome.get("hint"):
                assert outcome["hint"] in retries[0].content
        assert [
            item["outcome"]
            for item in daemon.outputs
            if item["type"] == "message_result"
        ] == ["completed"]
        kinds = [
            item["event"]["kind"] for item in daemon.outputs if item["type"] == "event"
        ]
        assert kinds.count("turn.started") == kinds.count("turn.ended") == 2
        assert "assistant.delta" in kinds and "usage" in kinds
        assert not any(kind.startswith("tool.") for kind in kinds)


async def test_external_deferred_restart_exactly_once(tmp_path, monkeypatch):
    install_script(tmp_path, monkeypatch, [tool_response(), text_response("resumed")])
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=[tool_spec()])
        await daemon.user()
        call = await daemon.until("tool_call")
        await daemon.result(
            call, {"status": "deferred", "kind": "external", "ticket_id": "ticket-1"}
        )
        await daemon.until("settled")
        snapshot = daemon.snapshots()[-1]
        assert [
            item["outcome"]
            for item in daemon.outputs
            if item["type"] == "message_result"
        ] == ["deferred"]
    async with fake_daemon() as (daemon, _):
        ready = await daemon.open(tools=[tool_spec()], resume_state=snapshot)
        assert ready["pending_deferred"] == [
            {"tool_call_id": "call-1", "ticket_id": "ticket-1", "kind": "external"}
        ]
        fields = {
            "tool_call_id": "call-1",
            "ticket_id": "ticket-1",
            "resolution": {"status": "result", "for_model": {"status": "filled"}},
        }
        await daemon.send("ticket_resolved", **fields)
        final = await daemon.until("event", kind="assistant.final")
        assert final["event"]["text"] == "resumed"
        await daemon.until("settled")
        returns = [
            part
            for message in state_messages(daemon)
            for part in message.parts
            if isinstance(part, ToolReturnPart)
        ]
        assert (
            len(returns) == 1
            and returns[0].tool_call_id == "call-1"
            and returns[0].content == {"status": "filled"}
        )
        assert not any(item["type"] == "tool_call" for item in daemon.outputs)
        assert [
            (item["message_id"], item["outcome"])
            for item in daemon.outputs
            if item["type"] == "message_result"
        ] == [("message-1", "completed")]
        await daemon.send("ticket_resolved", **fields)
        assert (await daemon.until("error"))["code"] == "stale_ticket"


@pytest.mark.parametrize("kind", ["approval", "external"])
async def test_restore_waits_for_daemon_and_accepts_interrupted_resolution(
    tmp_path, monkeypatch, kind
):
    install_script(tmp_path, monkeypatch, [tool_response(), text_response("checked")])
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=[tool_spec()])
        await daemon.user()
        call = await daemon.until("tool_call")
        await daemon.result(
            call, {"status": "deferred", "kind": kind, "ticket_id": "ticket-1"}
        )
        await daemon.until("settled")
        snapshot = daemon.snapshots()[-1]
    saved = json.loads(snapshot["data"])
    saved["pending_deferred"][0]["resolution"] = (
        {"status": "approved"}
        if kind == "approval"
        else {"status": "result", "for_model": {"status": "filled"}}
    )
    snapshot = {**snapshot, "data": json.dumps(saved)}
    interrupted = {
        "status": "interrupted",
        "message": (
            "The previous resumption was interrupted; verify state before retrying."
        ),
    }
    resolution = (
        {"status": "denied", "reason": "interrupted_after_resume"}
        if kind == "approval"
        else {"status": "result", "for_model": interrupted}
    )
    async with fake_daemon() as (daemon, _):
        ready = await daemon.open(tools=[tool_spec()], resume_state=snapshot)
        assert ready["pending_deferred"] == [
            {"tool_call_id": "call-1", "ticket_id": "ticket-1", "kind": kind}
        ]
        await daemon.send(
            "ticket_resolved",
            ticket_id="ticket-1",
            tool_call_id="call-1",
            resolution=resolution,
        )
        await daemon.until("settled")
        returns = [
            part
            for message in state_messages(daemon)
            for part in message.parts
            if isinstance(part, ToolReturnPart)
        ]
        assert len(returns) == 1 and returns[0].tool_call_id == "call-1"
        if kind == "approval":
            assert returns[0].outcome == "denied"
            assert "interrupted_after_resume" in str(returns[0].content)
        else:
            assert returns[0].content == interrupted
        assert not any(
            item["type"] in ("tool_call", "error") for item in daemon.outputs
        )
        assert [
            (item["message_id"], item["outcome"])
            for item in daemon.outputs
            if item["type"] == "message_result"
        ] == [("message-1", "completed")]
        await daemon.send(
            "ticket_resolved",
            ticket_id="ticket-1",
            tool_call_id="call-1",
            resolution=resolution,
        )
        assert (await daemon.until("error"))["code"] == "stale_ticket"


async def test_all_pending_calls_resolve_before_resume(tmp_path, monkeypatch):
    first = {
        "parts": [
            tool_response(id="call-1")["parts"][0],
            tool_response(id="call-2")["parts"][0],
        ]
    }
    install_script(tmp_path, monkeypatch, [first, text_response()])
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=[tool_spec()])
        await daemon.user()
        calls = [await daemon.until("tool_call"), await daemon.until("tool_call")]
        for call in calls:
            await daemon.result(
                call,
                {
                    "status": "deferred",
                    "kind": "external",
                    "ticket_id": "ticket-" + call["tool_call_id"],
                },
            )
        await daemon.until("settled")
        for index, call in enumerate(calls):
            await daemon.send(
                "ticket_resolved",
                ticket_id="ticket-" + call["tool_call_id"],
                tool_call_id=call["tool_call_id"],
                resolution={"status": "result", "for_model": index},
            )
            if index == 0:
                await daemon.send(
                    "ticket_resolved",
                    ticket_id="ticket-" + call["tool_call_id"],
                    tool_call_id=call["tool_call_id"],
                    resolution={"status": "result", "for_model": index},
                )
                assert (await daemon.until("error"))["code"] == "stale_ticket"
        await daemon.until("settled")
        assert (
            len(
                [
                    item
                    for item in daemon.outputs
                    if item["type"] == "event"
                    and item["event"]["kind"] == "assistant.final"
                ]
            )
            == 1
        )


@pytest.mark.parametrize("approved", [True, False])
async def test_approval_resume_and_denial(tmp_path, monkeypatch, approved):
    install_script(tmp_path, monkeypatch, [tool_response(), text_response()])
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=[tool_spec()])
        await daemon.user()
        call = await daemon.until("tool_call")
        await daemon.result(
            call, {"status": "deferred", "kind": "approval", "ticket_id": "approval-1"}
        )
        await daemon.until("settled")
        resolution = (
            {"status": "approved"}
            if approved
            else {"status": "denied", "reason": "Approval rejected"}
        )
        await daemon.send(
            "ticket_resolved",
            ticket_id="approval-1",
            tool_call_id="call-1",
            resolution=resolution,
        )
        if approved:
            repeated = await daemon.until("tool_call")
            assert (
                repeated["approval_ticket"] == "approval-1"
                and repeated["tool_call_id"] == "call-1"
            )
            await daemon.result(
                repeated,
                {"status": "denied", "reason": "Policy still denies", "hint": None},
            )
        await daemon.until("settled")
        messages = state_messages(daemon)
        if approved:
            assert any(
                isinstance(part, RetryPromptPart)
                and "Policy still denies" in part.content
                for message in messages
                for part in message.parts
            )
        else:
            assert any(
                isinstance(part, ToolReturnPart)
                and "Approval rejected" in str(part.content)
                and part.outcome == "denied"
                for message in messages
                for part in message.parts
            )


@pytest.mark.parametrize("deferred", [False, True])
async def test_steer_during_tool_request_boundary(tmp_path, monkeypatch, deferred):
    install_script(tmp_path, monkeypatch, [tool_response(), text_response()])
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=[tool_spec()])
        await daemon.user()
        call = await daemon.until("tool_call")
        await daemon.user("steer-1", "steered", "steer")
        outcome = (
            {"status": "deferred", "kind": "external", "ticket_id": "ticket-1"}
            if deferred
            else {"status": "done", "for_model": "ok"}
        )
        await daemon.result(call, outcome)
        if deferred:
            await daemon.until("settled")
            assert not any(
                item["type"] == "message_accepted" and item["message_id"] == "steer-1"
                for item in daemon.outputs
            )
            assert "steered" not in user_texts(daemon)
            await daemon.send(
                "ticket_resolved",
                ticket_id="ticket-1",
                tool_call_id="call-1",
                resolution={"status": "result", "for_model": "ok"},
            )
        await daemon.until("settled")
        assert user_texts(daemon).count("steered") == 1
        assert (
            len(
                [
                    item
                    for item in daemon.outputs
                    if item["type"] == "message_accepted"
                    and item["message_id"] == "steer-1"
                ]
            )
            == 1
        )
        assert (
            len(
                [
                    item
                    for item in daemon.outputs
                    if item["type"] == "message_result"
                    and item["message_id"] == "steer-1"
                ]
            )
            == 1
        )


async def test_idle_non_owner_burst_batches_and_settles_every_id(tmp_path, monkeypatch):
    install_script(tmp_path, monkeypatch, [text_response("batch complete")])
    async with fake_daemon() as (daemon, _):
        await daemon.open()
        for index in range(3):
            await daemon.user(f"report-{index}", f"report {index}", "queue")
        await daemon.until("settled")
        assert user_texts(daemon) == ["report 0", "report 1", "report 2"]
        assert [
            (item["message_id"], item["outcome"])
            for item in daemon.outputs
            if item["type"] == "message_result"
        ] == [(f"report-{index}", "completed") for index in range(3)]
        assert (
            sum(
                item["type"] == "event" and item["event"]["kind"] == "turn.started"
                for item in daemon.outputs
            )
            == 1
        )


@pytest.mark.parametrize("count,size,turns", [(33, 1, 2), (3, 600_000, 3)])
async def test_non_owner_batches_bound_count_and_bytes(
    tmp_path, monkeypatch, count, size, turns
):
    install_script(
        tmp_path, monkeypatch, [text_response("batch complete") for _ in range(turns)]
    )
    async with fake_daemon() as (daemon, _):
        await daemon.open(agent={"compaction_max_tokens": 2_000_000})
        for index in range(count):
            await daemon.user(f"bounded-{index}", "x" * size, "queue")
        await daemon.until("settled")
        assert sum(item["type"] == "message_result" for item in daemon.outputs) == count
        assert (
            sum(
                item["type"] == "event" and item["event"]["kind"] == "turn.started"
                for item in daemon.outputs
            )
            == turns
        )


async def test_owner_bypasses_idle_report_batch(tmp_path, monkeypatch):
    install_script(
        tmp_path, monkeypatch, [text_response("owner"), text_response("reports")]
    )
    async with fake_daemon() as (daemon, _):
        await daemon.open()
        await daemon.user("report", "queued report", "queue")
        await daemon.user("owner", "owner update", "steer")
        await daemon.until("settled")
        assert [
            item["message_id"]
            for item in daemon.outputs
            if item["type"] == "message_accepted"
        ] == ["owner", "report"]
        assert user_texts(daemon) == ["owner update", "queued report"]


async def test_owner_steers_before_report_without_replaying_mutation(
    tmp_path, monkeypatch
):
    install_script(
        tmp_path,
        monkeypatch,
        [tool_response(), text_response("owner updated"), text_response("report read")],
    )
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=[tool_spec()])
        await daemon.user("initial", "perform action", "steer")
        call = await daemon.until("tool_call")
        await daemon.user("report", "routine report", "queue")
        await daemon.user("owner", "changed requirement", "steer")
        await daemon.result(call, {"status": "done", "for_model": "effect committed"})
        await daemon.until("settled")
        accepted = [
            item["message_id"]
            for item in daemon.outputs
            if item["type"] == "message_accepted"
        ]
        assert accepted == ["initial", "owner", "report"]
        assert sum(item["type"] == "tool_call" for item in daemon.outputs) == 1
        assert [
            item["message_id"]
            for item in daemon.outputs
            if item["type"] == "message_result"
        ] == ["initial", "owner", "report"]


async def test_queue_processed_in_new_run(tmp_path, monkeypatch):
    install_script(
        tmp_path,
        monkeypatch,
        [tool_response(), text_response("first"), text_response("second")],
    )
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=[tool_spec()])
        await daemon.user()
        call = await daemon.until("tool_call")
        await daemon.user("queued", "queued text", "queue")
        await daemon.result(call, {"status": "done", "for_model": "ok"})
        await daemon.until("settled")
        results = [
            item["message_id"]
            for item in daemon.outputs
            if item["type"] == "message_result"
        ]
        assert results == ["message-1", "queued"]
        snapshots = [
            SnapshotData.model_validate_json(state["data"])
            for state in daemon.snapshots()
        ]
        assert len(snapshots) == 2
        assert "queued text" not in json.dumps(snapshots[0].messages)
        assert user_texts(daemon).count("queued text") == 1


async def test_abort_mid_tool_preserves_undelivered_steer(tmp_path, monkeypatch):
    install_script(
        tmp_path, monkeypatch, [tool_response(), text_response("after abort")]
    )
    async with fake_daemon() as (daemon, session):
        await daemon.open(tools=[tool_spec()])
        await daemon.user()
        await daemon.until("tool_call")
        await daemon.user("pending", "undelivered", "steer")
        await daemon.send("abort")
        await daemon.until("settled")
        assert not session.rpc and session.run is None
        assert [
            item["outcome"]
            for item in daemon.outputs
            if item["type"] == "message_result"
        ] == ["aborted"]
        assert "undelivered" not in user_texts(daemon)
        assert (
            SnapshotData.model_validate_json(daemon.snapshots()[-1]["data"])
            .steer_buffer[0]
            .message_id
            == "pending"
        )
        interrupted_snapshot = daemon.snapshots()[-1]
        await daemon.user("new", "restart")
        await daemon.until("settled")
        assert user_texts(daemon).count("undelivered") == 1
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=[tool_spec()], resume_state=interrupted_snapshot)
        await daemon.user("after-restart", "reconcile interrupted work")
        await daemon.until("settled")
        assert "undelivered" not in user_texts(daemon)
        assert not any(item["type"] == "tool_call" for item in daemon.outputs)


@pytest.mark.parametrize("midrun", [False, True])
async def test_auth_unavailable_is_fatal(monkeypatch, midrun):
    async def unavailable(*args, **kwargs):
        raise providers.AuthUnavailable("test", ["synthetic unavailable"])

    async def unavailable_stream(*args, **kwargs):
        await unavailable()
        yield ""

    model = (
        FunctionModel(unavailable, stream_function=unavailable_stream)
        if midrun
        else None
    )
    if not midrun:
        monkeypatch.setattr(providers, "build_model", unavailable)
    async with fake_daemon(model=model) as (daemon, _):
        if midrun:
            await daemon.open()
            await daemon.user()
            error = await daemon.until("error")
        else:
            from pydantic_ai.models.test import TestModel

            fake = TestModel()
            await daemon.send(
                "open",
                session="auth",
                model={"id": f"{fake.system}:{fake.model_name}", "fallback": []},
                tools=[],
                system_prompt_parts=[],
                config={"credentials": {}, "agent": {}},
            )
            error = await daemon.receive()
        assert error["code"] == "auth_unavailable" and error["fatal"]


async def test_canary_not_in_snapshot_events_or_logs(tmp_path, monkeypatch, capsys):
    canary = "vault-like-canary-do-not-disclose-893242"
    monkeypatch.setenv("OPENSIDE_BROKER_TOKEN", canary)
    install_script(tmp_path, monkeypatch, [text_response("public answer")])
    async with fake_daemon() as (daemon, _):
        await daemon.open()
        await daemon.user()
        await daemon.until("settled")
        assert canary not in json.dumps(daemon.outputs)
    captured = capsys.readouterr()
    assert canary not in captured.err and canary not in captured.out


async def test_steer_after_last_model_request_starts_new_run(tmp_path, monkeypatch):
    first = {"parts": [{"type": "wait_steer"}, {"type": "text", "text": "first"}]}
    install_script(
        tmp_path, monkeypatch, [first, text_response("second")], delay_ms=5000
    )
    async with fake_daemon() as (daemon, _):
        await daemon.open()
        await daemon.user()
        await daemon.until("event", kind="turn.started")
        await daemon.user("steer", "late steer", "steer")
        await daemon.until("settled")
        assert [
            item["message_id"]
            for item in daemon.outputs
            if item["type"] == "message_result"
        ] == ["message-1", "steer"]
        assert len(daemon.snapshots()) == 2


async def test_snapshot_ignores_retired_pins_and_restores_script_cursor(
    tmp_path, monkeypatch
):
    install_script(
        tmp_path, monkeypatch, [text_response("first"), text_response("second")]
    )

    async def must_not_resolve_credentials(*args, **kwargs):
        raise AssertionError("Script mode must not open credential sources")

    monkeypatch.setattr(providers, "build_model", must_not_resolve_credentials)
    async with fake_daemon() as (daemon, session):
        await daemon.open()
        await daemon.user()
        await daemon.until("settled")
        snapshot = daemon.snapshots()[-1]
        old_data = json.loads(snapshot["data"])
        assert "pins" not in old_data
        old_data["pins"] = (
            '{"test-session":{"synthetic":{"credential_id":17,"last_request_at":25}}}'
        )
        snapshot["data"] = json.dumps(old_data)
    async with fake_daemon() as (daemon, session):
        await daemon.open(resume_state=snapshot)
        assert not hasattr(session, "pins")
        await daemon.user("continued")
        final = await daemon.until("event", kind="assistant.final")
        assert final["event"]["text"] == "second"
        await daemon.until("settled")
        state = SnapshotData.model_validate_json(daemon.snapshots()[-1]["data"])
        assert state.script_cursor == 2
        assert "pins" not in state.model_dump()


async def test_shutdown_mid_tool_aborts_snapshots_then_bye(tmp_path, monkeypatch):
    install_script(tmp_path, monkeypatch, [tool_response()])
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=[tool_spec()])
        await daemon.user()
        await daemon.until("tool_call")
        await daemon.send("shutdown")
        await daemon.until("bye")
        results = [
            item["outcome"]
            for item in daemon.outputs
            if item["type"] == "message_result"
        ]
        assert results == ["aborted"]
        assert daemon.outputs[-2]["type"] == "state_snapshot"
        assert daemon.outputs[-1]["type"] == "bye"


async def test_thinking_stream_is_forwarded_without_tool_lifecycle():
    from pydantic_ai.models.function import DeltaThinkingPart

    async def stream(messages, info):
        yield {0: DeltaThinkingPart(content="public reasoning")}
        yield {0: DeltaThinkingPart(content=" continuation")}
        yield "public "
        yield "answer"

    async with fake_daemon(model=FunctionModel(stream_function=stream)) as (daemon, _):
        await daemon.open()
        await daemon.user()
        await daemon.until("settled")
        events = [item["event"] for item in daemon.outputs if item["type"] == "event"]
        assert (
            "".join(
                event["text"] for event in events if event["kind"] == "thinking.delta"
            )
            == "public reasoning continuation"
        )
        assert (
            "".join(
                event["text"] for event in events if event["kind"] == "assistant.delta"
            )
            == "public answer"
        )
        assert not any(event["kind"].startswith("tool.") for event in events)
