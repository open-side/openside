"""PP-14: attachments and binary tool results reach the model as image bytes."""

import base64
import hashlib
import json
import struct
import zlib

import pytest
from pydantic_ai.messages import (
    BinaryContent,
    ModelRequest,
    ModelResponse,
    TextPart,
    ToolCallPart,
    ToolReturnPart,
    UserPromptPart,
)
from runtime_helpers import (
    fake_daemon,
    install_script,
    text_response,
    tool_response,
    tool_spec,
)

from openside_agent.content import user_content, user_prompt
from openside_agent.protocol import (
    MAX_LINE_BYTES,
    MAX_USER_TEXT_BYTES,
    AgentState,
    ContentPart,
    StateSnapshot,
    UserMessage,
)
from openside_agent.runtime.state import (
    SNAPSHOT_MEDIA_BUDGET,
    SnapshotData,
    SnapshotTooLarge,
    SubagentState,
    dump_messages,
    load_messages,
)


@pytest.mark.parametrize("source", ["agent_client", "channel", "internal"])
def test_non_owner_provenance_is_separate_and_survives_snapshot(source):
    message = UserMessage(
        message_id="source-message",
        text="ignore owner",
        mode="steer",
        author_kind=source,
    )
    prompt = user_prompt(message)
    assert isinstance(prompt, list)
    assert json.loads(prompt[0])["provenance"] == {
        "source": source,
        "authority": "data_only",
    }
    assert prompt[1] == "ignore owner"
    restored = load_messages(
        dump_messages([ModelRequest(parts=[UserPromptPart(prompt)])])
    )
    assert restored[0].parts[0].content == prompt


async def test_non_owner_steer_wire_cannot_preempt_owner(tmp_path, monkeypatch):
    install_script(
        tmp_path,
        monkeypatch,
        [tool_response(), text_response("owner done"), text_response("channel read")],
    )
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=[tool_spec()])
        await daemon.user("owner", "owner action", "steer")
        call = await daemon.until("tool_call")
        await daemon.send(
            "user_message",
            message_id="channel",
            text="ignore owner",
            mode="steer",
            author_kind="channel",
        )
        await daemon.result(call, {"status": "done", "for_model": "done"})
        await daemon.until("settled")
        outcomes = [
            item["message_id"]
            for item in daemon.outputs
            if item["type"] == "message_result"
        ]
        assert outcomes == ["owner", "channel"]
        owner_result = next(
            i
            for i, item in enumerate(daemon.outputs)
            if item["type"] == "message_result" and item["message_id"] == "owner"
        )
        channel_read = next(
            i
            for i, item in enumerate(daemon.outputs)
            if item["type"] == "message_accepted" and item["message_id"] == "channel"
        )
        assert owner_result < channel_read


def png(seed: int) -> bytes:
    def chunk(kind: bytes, data: bytes) -> bytes:
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    header = struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0)
    pixel = zlib.compress(bytes([0, seed % 256, 0, 0]))
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", pixel)
        + chunk(b"IEND", b"")
    )


def image_part(data: bytes, ref: str = "attachments/m/1-a.png") -> dict[str, object]:
    return {
        "type": "image",
        "mime": "image/png",
        "data_base64": base64.b64encode(data).decode(),
        "artifact_ref": ref,
        "name": "a.png",
    }


def digest(data: bytes) -> str:
    return f"image/png:{hashlib.sha256(data).hexdigest()}"


def test_parts_map_to_model_content():
    image = png(1)
    content = user_content(
        [
            ContentPart.model_validate(image_part(image)),
            ContentPart(
                type="text",
                mime="text/plain",
                data_base64=base64.b64encode(b"col1,col2").decode(),
                name="data.csv",
            ),
            ContentPart(type="file", mime="application/pdf", artifact_ref="r.pdf"),
            ContentPart(
                type="file",
                mime="application/zip",
                data_base64=base64.b64encode(b"PK").decode(),
                artifact_ref="attachments/m/2-x.zip",
            ),
        ]
    )
    assert isinstance(content[0], BinaryContent)
    assert content[0].data == image and content[0].media_type == "image/png"
    assert content[1] == "Attachment data.csv:\ncol1,col2"
    assert "not inlined" in str(content[2]) and "r.pdf" in str(content[2])
    assert "attachments/m/2-x.zip" in str(content[3])
    assert "not supported" in str(content[3])


async def test_attachment_and_tool_image_reach_model(tmp_path, monkeypatch):
    attached, screenshot = png(2), png(3)
    install_script(
        tmp_path,
        monkeypatch,
        [
            {"parts": [{"type": "media_digest"}]},
            tool_response(),
            {"parts": [{"type": "media_digest"}]},
        ],
    )
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=[tool_spec()])
        await daemon.send(
            "user_message",
            message_id="message-1",
            text="what is in this picture?",
            mode="now",
            parts=[image_part(attached)],
        )
        final = await daemon.until("event", kind="assistant.final")
        assert final["event"]["text"] == f"media {digest(attached)}"
        await daemon.until("settled")
        await daemon.user(message_id="message-2", text="take a screenshot")
        call = await daemon.until("tool_call")
        await daemon.result(
            call,
            {
                "status": "done",
                "for_model": {"saved": True},
                "parts": [image_part(screenshot, "call-1.png")],
            },
        )
        final = await daemon.until("event", kind="assistant.final")
        assert final["event"]["text"] == (
            f"media {digest(attached)} {digest(screenshot)}"
        )
        await daemon.until("settled")
        restored = SnapshotData.model_validate_json(
            daemon.snapshots()[-1]["data"]
        ).restore_messages()
        kept = [
            item.data
            for message in restored
            if isinstance(message, ModelRequest)
            for part in message.parts
            if isinstance(part, UserPromptPart) and not isinstance(part.content, str)
            for item in part.content
            if isinstance(item, BinaryContent)
        ]
        assert kept == [attached, screenshot]


def test_snapshot_keeps_newest_media_within_budget():
    big = b"\x00" * (SNAPSHOT_MEDIA_BUDGET // 2)
    old, new = big + b"old", big + b"new"
    messages = [
        ModelRequest(
            parts=[
                UserPromptPart(["first", BinaryContent(old, media_type="image/png")])
            ]
        ),
        ModelRequest(
            parts=[
                UserPromptPart(["second", BinaryContent(new, media_type="image/png")])
            ]
        ),
    ]
    restored = load_messages(dump_messages(messages))
    first, second = (message.parts[0] for message in restored)
    assert isinstance(first, UserPromptPart) and isinstance(second, UserPromptPart)
    note = first.content[1]
    assert isinstance(note, str) and note.startswith("[image/png ")
    assert note.endswith(": omitted from saved conversation state]")
    item = second.content[1]
    assert isinstance(item, BinaryContent) and item.data == new
    # The live history is untouched; only the saved copy sheds bytes.
    assert isinstance(messages[0].parts[0], UserPromptPart)
    assert isinstance(messages[0].parts[0].content[1], BinaryContent)


def test_snapshot_media_budget_spans_children_and_queued_steers():
    from openside_agent.protocol import MAX_LINE_BYTES, UserMessage
    from openside_agent.runtime.state import SubagentState

    # Each image fits one history's budget on its own (800 KB of base64).
    image = b"\x89PNG" + b"\x01" * (600 * 1024)
    history = dump_messages(
        [
            ModelRequest(
                parts=[
                    UserPromptPart(
                        ["look", BinaryContent(image, media_type="image/png")]
                    )
                ]
            )
        ]
    )
    children = [
        SubagentState(
            id=f"child-{n}",
            parent_tool_call_id=f"call-{n}",
            task="t",
            model="m",
            tools=[],
            status="running",
            messages=history,
        )
        for n in range(4)
    ]
    steer = UserMessage(
        message_id="queued",
        text="and this",
        mode="steer",
        parts=[
            ContentPart(
                type="image",
                mime="image/png",
                data_base64=base64.b64encode(image).decode(),
                artifact_ref="attachments/queued/1-a.png",
            )
        ],
    )
    parent = [
        ModelRequest(
            parts=[UserPromptPart(["p", BinaryContent(image, media_type="image/png")])]
        )
    ]
    state = SnapshotData.capture(
        parent,
        [],
        [steer],
        0,
        delegates=[],
        subagents=children,
        deferred_messages=[],
    )
    # Five histories and a queued steer, 800 KB of base64 each, would be ~4.8 MB.
    assert len(state.model_dump_json()) < MAX_LINE_BYTES
    data = SnapshotData.model_validate_json(state.data)
    # The undelivered steer keeps its image; every history image was shed to a note.
    assert data.steer_buffer[0].parts[0].data_base64 is not None
    assert "omitted from saved conversation state" in json.dumps(data.messages)
    assert all("omitted" in json.dumps(child.messages) for child in data.subagents)
    # The live child states handed to capture are untouched.
    assert '"kind": "binary"' in json.dumps(children[0].messages)


def test_multibyte_text_snapshot_fits_the_real_outgoing_line():
    from openside_agent.protocol import MAX_LINE_BYTES, StateSnapshot
    from openside_agent.runtime.state import PendingCall, SubagentState

    # 300k Hangul characters are 900 KB of UTF-8: character counts would undercount 3x.
    turn = "가" * 300_000

    def history(turns: int) -> list[ModelRequest]:
        return [
            ModelRequest(parts=[UserPromptPart(f"{n}:{turn}")]) for n in range(turns)
        ]

    children = [
        SubagentState(
            id=f"child-{n}",
            parent_tool_call_id=f"call-{n}",
            task="t",
            model="m",
            tools=[],
            status="running",
            messages=dump_messages(history(2)),
        )
        for n in range(2)
    ]
    pending = [
        PendingCall(tool_call_id="deferred", ticket_id="ticket", kind="approval")
    ]
    state = SnapshotData.capture(
        history(3),
        pending,
        [],
        0,
        delegates=[],
        subagents=children,
        deferred_messages=["queued-run"],
    )
    line = StateSnapshot(state=state).model_dump_json(exclude_none=True).encode("utf-8")
    assert len(line) <= MAX_LINE_BYTES
    data = SnapshotData.model_validate_json(state.data)
    # Continuation metadata survives; the newest parent turn is kept and still loads.
    assert data.pending_deferred[0].tool_call_id == "deferred"
    assert data.deferred_messages == ["queued-run"]
    assert "2:" in json.dumps(data.messages, ensure_ascii=False)
    data.restore_messages()


def completed_turn(size: int):
    return [
        ModelRequest(parts=[UserPromptPart("completed")]),
        ModelResponse(parts=[TextPart("x" * size)]),
    ]


def capture_saved(data: SnapshotData):
    return SnapshotData.capture(
        data.restore_messages(),
        data.pending_deferred,
        data.steer_buffer,
        data.script_cursor,
        delegates=data.pending_delegates,
        subagents=data.subagents,
        deferred_messages=data.deferred_messages,
    )


def snapshot_line(state: AgentState) -> bytes:
    return StateSnapshot(state=state).model_dump_json(exclude_none=True).encode("utf-8")


async def test_oversized_deferred_parent_turn_survives_restart_and_resolution(
    tmp_path, monkeypatch
):
    install_script(tmp_path, monkeypatch, [tool_response(), text_response("resumed")])
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=[tool_spec()])
        await daemon.user()
        call = await daemon.until("tool_call")
        await daemon.result(
            call, {"status": "deferred", "kind": "external", "ticket_id": "ticket-1"}
        )
        await daemon.until("settled")
        data = SnapshotData.model_validate_json(daemon.snapshots()[-1]["data"])

    history = data.restore_messages()
    response = history[-1]
    assert isinstance(response, ModelResponse)
    response.parts.insert(0, TextPart("x" * (3 * 1024 * 1024)))
    data.messages = dump_messages(history)
    data.subagents.append(
        SubagentState(
            id="finished-child",
            parent_tool_call_id="finished-delegate",
            task="completed",
            model="test:child",
            tools=[],
            status="completed",
            messages=dump_messages(completed_turn(2 * 1024 * 1024)),
            result="done",
        )
    )
    assert len(snapshot_line(AgentState(data=data.model_dump_json()))) > MAX_LINE_BYTES
    snapshot = capture_saved(data)
    assert len(snapshot_line(snapshot)) <= MAX_LINE_BYTES
    saved = SnapshotData.model_validate_json(snapshot.data)
    assert saved.subagents[0].messages == []
    assert any(
        isinstance(part, ToolCallPart) and part.tool_call_id == "call-1"
        for message in saved.restore_messages()
        for part in message.parts
    )
    async with fake_daemon() as (daemon, _):
        ready = await daemon.open(
            tools=[tool_spec()],
            resume_state=snapshot.model_dump(),
            agent={"compaction_max_tokens": MAX_LINE_BYTES},
        )
        assert ready["pending_deferred"][0]["tool_call_id"] == "call-1"
        await daemon.send(
            "ticket_resolved",
            ticket_id="ticket-1",
            tool_call_id="call-1",
            resolution={"status": "result", "for_model": {"answer": "original"}},
        )
        assert (await daemon.until("event", kind="assistant.final"))["event"][
            "text"
        ] == "resumed"
        await daemon.until("settled")
        returns = [
            part
            for message in SnapshotData.model_validate_json(
                daemon.snapshots()[-1]["data"]
            ).restore_messages()
            for part in message.parts
            if isinstance(part, ToolReturnPart)
        ]
        assert len(returns) == 1
        assert returns[0].tool_call_id == "call-1"
        assert returns[0].content == {"answer": "original"}


async def test_oversized_deferred_child_turn_survives_restart_and_resolution(
    tmp_path, monkeypatch
):
    from test_subagents import (
        CHILD,
        PARENT,
        approve_and_finish,
        defer_child,
        delegate_returns,
        delegate_spec,
        install,
    )

    install(tmp_path, monkeypatch, PARENT, CHILD)
    async with fake_daemon() as (daemon, _):
        data = SnapshotData.model_validate_json((await defer_child(daemon))["data"])
    child = data.subagents[0]
    history = load_messages(child.messages)
    response = history[-1]
    assert isinstance(response, ModelResponse)
    response.parts.insert(0, TextPart("x" * (3 * 1024 * 1024)))
    child.messages = dump_messages(history)
    data.messages = dump_messages(
        [*completed_turn(2 * 1024 * 1024), *data.restore_messages()]
    )
    assert len(snapshot_line(AgentState(data=data.model_dump_json()))) > MAX_LINE_BYTES
    snapshot = capture_saved(data)
    assert len(snapshot_line(snapshot)) <= MAX_LINE_BYTES
    saved = SnapshotData.model_validate_json(snapshot.data)
    assert any(
        isinstance(part, ToolCallPart) and part.tool_call_id == "child-1"
        for message in load_messages(saved.subagents[0].messages)
        for part in message.parts
    )
    assert any(
        isinstance(part, ToolCallPart) and part.tool_call_id == "del-1"
        for message in saved.restore_messages()
        for part in message.parts
    )
    async with fake_daemon() as (daemon, _):
        await daemon.open(
            tools=[tool_spec(), delegate_spec()], resume_state=snapshot.model_dump()
        )
        await approve_and_finish(daemon)
        assert len(delegate_returns(daemon)) == 1
        child = SnapshotData.model_validate_json(
            daemon.snapshots()[-1]["data"]
        ).subagents[0]
        returns = [
            part
            for message in load_messages(child.messages)
            for part in message.parts
            if isinstance(part, ToolReturnPart) and part.tool_call_id == "child-1"
        ]
        assert len(returns) == 1 and returns[0].content == "probed"


async def test_untrimmable_pending_snapshot_reports_error_without_closing_connection(
    tmp_path, monkeypatch
):
    install_script(
        tmp_path, monkeypatch, [tool_response(), text_response("still live")]
    )
    async with fake_daemon() as (daemon, session):
        await daemon.open(
            tools=[tool_spec()], agent={"compaction_max_tokens": MAX_LINE_BYTES}
        )
        await daemon.user()
        call = await daemon.until("tool_call")
        await daemon.result(
            call, {"status": "deferred", "kind": "external", "ticket_id": "ticket-1"}
        )
        await daemon.until("settled")
        response = session.messages[-1]
        assert isinstance(response, ModelResponse)
        response.parts.insert(0, TextPart("x" * MAX_LINE_BYTES))
        data = SnapshotData.model_validate_json(daemon.snapshots()[-1]["data"])
        data.messages = dump_messages(session.messages)
        with pytest.raises(SnapshotTooLarge, match="pending continuation"):
            capture_saved(data)
        snapshots = len(daemon.snapshots())
        await session.snapshot()
        error = await daemon.until("error")
        assert error["code"] == "internal" and error["fatal"] is False
        assert error["message"] == (
            "Snapshot exceeds 4 MiB; pending continuation cannot be checkpointed"
        )
        assert len(daemon.snapshots()) == snapshots
        await daemon.send(
            "ticket_resolved",
            ticket_id="ticket-1",
            tool_call_id="call-1",
            resolution={"status": "result", "for_model": "done"},
        )
        assert (await daemon.until("event", kind="assistant.final"))["event"][
            "text"
        ] == "still live"
        await daemon.until("settled")


def test_untrimmable_queued_steers_cannot_escape_final_snapshot_bound():
    data = SnapshotData(
        messages=[],
        steer_buffer=[
            UserMessage(
                message_id=f"steer-{n}",
                text="x" * MAX_USER_TEXT_BYTES,
                mode="steer",
            )
            for n in range(5)
        ],
    )
    with pytest.raises(SnapshotTooLarge, match="Snapshot exceeds 4 MiB"):
        capture_saved(data)


async def test_steer_text_utf8_cap_rejects_over_and_accepts_under(
    tmp_path, monkeypatch
):
    install_script(tmp_path, monkeypatch, [tool_response(), text_response("finished")])
    async with fake_daemon() as (daemon, session):
        await daemon.open(tools=[tool_spec()])
        await daemon.user()
        call = await daemon.until("tool_call")
        await daemon.result(
            call, {"status": "deferred", "kind": "external", "ticket_id": "ticket-1"}
        )
        await daemon.until("settled")
        under = "é" * (MAX_USER_TEXT_BYTES // 2 - 1) + "x"
        assert len(under.encode("utf-8")) == MAX_USER_TEXT_BYTES - 1
        await daemon.user("over", under + "xx", "steer")
        error = await daemon.until("error")
        assert error["code"] == "bad_request" and error["fatal"] is False
        assert error["message_id"] == "over"
        assert not session.steer_buffer and not session.queue
        await daemon.user("under", under, "steer")
        # A rejected frame acknowledges that the preceding accepted steer was read.
        await daemon.user("barrier", under + "xx", "steer")
        await daemon.until("error")
        assert len(session.steer_buffer) == 1
        assert session.steer_buffer[0].message_id == "under"
        await session.snapshot()
        saved = await daemon.until("state_snapshot")
        restored = SnapshotData.model_validate_json(saved["state"]["data"])
        assert restored.steer_buffer[0].text == under


async def test_child_steer_text_cap_matches_parent(tmp_path, monkeypatch):
    from test_subagents import CHILD, PARENT, defer_child, install

    install(tmp_path, monkeypatch, PARENT, CHILD)
    async with fake_daemon() as (daemon, session):
        await defer_child(daemon)
        await daemon.send(
            "subagent_steer",
            subagent_id="sub-1",
            text="x" * (MAX_USER_TEXT_BYTES + 1),
            control_id="over",
        )
        error = await daemon.until("error")
        assert error["code"] == "bad_request" and error["fatal"] is False
        assert not session.children["sub-1"].state.steer_buffer
        await daemon.send(
            "subagent_steer",
            subagent_id="sub-1",
            text="x" * (MAX_USER_TEXT_BYTES - 1),
            control_id="under",
        )
        progress = await daemon.until("event", kind="subagent.progress")
        assert progress["event"]["progress"]["kind"] == "steer_queued"
        saved = await daemon.until("state_snapshot")
        child = SnapshotData.model_validate_json(saved["state"]["data"]).subagents[0]
        assert child.steer_buffer == ["x" * (MAX_USER_TEXT_BYTES - 1)]


async def test_largest_allowed_message_fits_wire_and_snapshot(tmp_path, monkeypatch):
    install_script(tmp_path, monkeypatch, [text_response("received")])
    attachment = b"x" * (2 * 1024 * 1024)
    message = UserMessage(
        message_id="maximum",
        text="x" * MAX_USER_TEXT_BYTES,
        mode="now",
        parts=[
            ContentPart(
                type="text",
                mime="text/plain",
                data_base64=base64.b64encode(attachment).decode(),
                artifact_ref="attachments/maximum/1-note.txt",
            )
        ],
    )
    line = message.model_dump_json(exclude_none=True).encode("utf-8")
    assert len(line) < MAX_LINE_BYTES
    async with fake_daemon() as (daemon, _):
        await daemon.open()
        await daemon.send(
            "user_message",
            **message.model_dump(exclude={"v", "type"}, exclude_none=True),
        )
        await daemon.until("settled")
        assert not any(item["type"] == "error" for item in daemon.outputs)
        state = AgentState.model_validate(daemon.snapshots()[-1])
        assert len(snapshot_line(state)) <= MAX_LINE_BYTES
        assert len(SnapshotData.model_validate_json(state.data).messages) > 0
