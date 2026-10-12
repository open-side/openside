"""A resumed run reports the final outcome of the messages whose run deferred."""

from runtime_helpers import (
    fake_daemon,
    install_script,
    text_response,
    tool_response,
    tool_spec,
)

from openside_agent.runtime.state import SnapshotData


def results(daemon):
    return [
        (item["message_id"], item["outcome"])
        for item in daemon.outputs
        if item["type"] == "message_result"
    ]


async def defer(daemon, ticket):
    call = await daemon.until("tool_call")
    await daemon.result(
        call, {"status": "deferred", "kind": "external", "ticket_id": ticket}
    )
    await daemon.until("settled")
    return call


async def resolve(daemon, call, ticket):
    await daemon.send(
        "ticket_resolved",
        ticket_id=ticket,
        tool_call_id=call["tool_call_id"],
        resolution={"status": "result", "for_model": "filled"},
    )


async def test_resume_completes_originating_message_across_redeferral_and_restart(
    tmp_path, monkeypatch
):
    install_script(
        tmp_path,
        monkeypatch,
        [tool_response(id="call-1"), tool_response(id="call-2"), text_response()],
    )
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=[tool_spec()])
        await daemon.user()
        first = await defer(daemon, "ticket-1")
        assert results(daemon) == [("message-1", "deferred")]
        await resolve(daemon, first, "ticket-1")
        await defer(daemon, "ticket-2")
        assert results(daemon) == [("message-1", "deferred")]
        snapshot = daemon.snapshots()[-1]
        state = SnapshotData.model_validate_json(snapshot["data"])
        assert state.deferred_messages == ["message-1"]
    async with fake_daemon() as (daemon, _):
        ready = await daemon.open(tools=[tool_spec()], resume_state=snapshot)
        assert ready["pending_deferred"][0]["tool_call_id"] == "call-2"
        await resolve(daemon, {"tool_call_id": "call-2"}, "ticket-2")
        await daemon.until("settled")
        assert results(daemon) == [("message-1", "completed")]
        state = SnapshotData.model_validate_json(daemon.snapshots()[-1]["data"])
        assert state.deferred_messages == []


async def test_discard_drops_restored_continuation_without_rerunning_it(
    tmp_path, monkeypatch
):
    install_script(
        tmp_path,
        monkeypatch,
        [tool_response(id="call-1"), text_response("resumed"), text_response("next")],
    )
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=[tool_spec()])
        await daemon.user()
        await defer(daemon, "ticket-1")
        snapshot = daemon.snapshots()[-1]
    async with fake_daemon() as (daemon, _):
        ready = await daemon.open(tools=[tool_spec()], resume_state=snapshot)
        assert ready["pending_deferred"][0]["ticket_id"] == "ticket-1"
        await daemon.send("discard_deferred")
        state = SnapshotData.model_validate_json(
            (await daemon.until("state_snapshot"))["state"]["data"]
        )
        assert state.pending_deferred == [] and state.deferred_messages == []
        # The unanswered batch left the model's history, so the daemon must retire any
        # tool-result base (browser snapshot id) that lived only in it.
        notices = [
            item for item in daemon.outputs if item["type"] == "context_compacted"
        ]
        assert len(notices) == 1 and "subagent" not in notices[0], notices
        # The discarded batch is gone from history, so a new run is well-formed.
        await daemon.user("message-2", "next")
        await daemon.until("settled")
        assert results(daemon) == [("message-2", "completed")]
        assert not any(
            item["type"] in ("error", "tool_call") for item in daemon.outputs
        )
