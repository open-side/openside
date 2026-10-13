import pytest
from pydantic_ai.messages import ModelRequest, TextContent, UserPromptPart
from runtime_helpers import fake_daemon, install_script, text_response

from openside_agent.content import user_prompt
from openside_agent.protocol import UserMessage
from openside_agent.scripted import resolve_refs


def test_dynamic_front_output_uses_report_not_ephemeral_context():
    report = UserMessage(
        message_id="report",
        text="[task report] durable result",
        mode="queue",
        author_kind="internal",
    )
    messages = [
        ModelRequest(parts=[UserPromptPart(user_prompt(report))]),
        ModelRequest(
            parts=[UserPromptPart([TextContent("memory context")])],
        ),
    ]
    assert resolve_refs({"text": "{{user_text}}"}, messages) == {
        "text": "[task report] durable result"
    }


SEND = {
    "name": "send_message",
    "description": "Send one useful message to the person.",
    "parameters": {
        "type": "object",
        "properties": {
            "text": {"type": "string"},
            "attachments": {"type": "array", "items": {"type": "object"}},
        },
        "required": ["text"],
        "additionalProperties": False,
    },
    "approval": "read",
    "deferrable": False,
}
FRONT = {
    "orchestration": "front",
    "front_tools": ["send_message"],
    "title_needed": False,
}


def output(name, args, call_id="finish"):
    return {"parts": [{"type": "tool_call", "name": name, "args": args, "id": call_id}]}


def bubbles(daemon):
    return [
        item
        for item in daemon.outputs
        if item["type"] == "event"
        and item["event"]["kind"]
        in {"assistant.final", "assistant.delta", "thinking.delta"}
    ]


async def test_send_message_is_terminal_and_only_daemon_publishes(
    tmp_path, monkeypatch
):
    args = {
        "text": "Here is the result.",
        "attachments": [{"artifact": "doc/report.md", "display": "panel"}],
    }
    install_script(tmp_path, monkeypatch, [output("send_message", args)])
    async with fake_daemon() as (daemon, session):
        await daemon.open(tools=[SEND], agent=FRONT)
        await daemon.user()
        call = await daemon.until("tool_call")
        assert call["name"] == "send_message"
        assert call["tool_call_id"] == "finish"
        assert call["args"] == args
        await daemon.result(call, {"status": "done", "for_model": {"status": "sent"}})
        await daemon.until("settled")
        assert session.scripted.cursor == 1
        assert not bubbles(daemon)
        assert [
            item["outcome"]
            for item in daemon.outputs
            if item["type"] == "message_result"
        ] == ["completed"]


async def test_silent_report_ends_without_empty_text_retry_or_bubble(
    tmp_path, monkeypatch
):
    install_script(tmp_path, monkeypatch, [output("stay_silent", {})])
    async with fake_daemon() as (daemon, session):
        await daemon.open(tools=[SEND], agent=FRONT)
        await daemon.send(
            "user_message",
            message_id="report",
            text="No useful update",
            mode="queue",
            author_kind="internal",
        )
        await daemon.until("settled")
        assert session.scripted.cursor == 1
        assert not bubbles(daemon)
        assert not any(item["type"] == "tool_call" for item in daemon.outputs)
        assert [
            item["outcome"]
            for item in daemon.outputs
            if item["type"] == "message_result"
        ] == ["completed"]


@pytest.mark.parametrize(
    "failure",
    [
        {
            "status": "denied",
            "reason": "attachment[0]:not_found",
            "hint": "Create the document first",
        },
        {"status": "error", "message": "Message must not be empty"},
    ],
)
async def test_daemon_rejection_retries_output_without_claiming_delivery(
    tmp_path, monkeypatch, failure
):
    install_script(
        tmp_path,
        monkeypatch,
        [
            output("send_message", {"text": "bad"}, "bad"),
            output("send_message", {"text": "good"}, "good"),
        ],
    )
    async with fake_daemon() as (daemon, session):
        await daemon.open(tools=[SEND], agent=FRONT)
        await daemon.user()
        first = await daemon.until("tool_call")
        await daemon.result(first, failure)
        second = await daemon.until("tool_call")
        assert second["tool_call_id"] == "good"
        await daemon.result(second, {"status": "done", "for_model": {"status": "sent"}})
        await daemon.until("settled")
        assert session.scripted.cursor == 2
        assert not bubbles(daemon)


async def test_plain_text_is_not_a_front_output(tmp_path, monkeypatch):
    install_script(
        tmp_path,
        monkeypatch,
        [text_response("Do not publish this"), output("stay_silent", {})],
    )
    async with fake_daemon() as (daemon, session):
        await daemon.open(tools=[SEND], agent=FRONT)
        await daemon.user()
        await daemon.until("settled")
        assert session.scripted.cursor == 2
        assert not bubbles(daemon)


async def test_multiple_terminal_messages_publish_at_most_one(tmp_path, monkeypatch):
    first = output("send_message", {"text": "first"}, "first")
    first["parts"].extend(output("send_message", {"text": "second"}, "second")["parts"])
    install_script(tmp_path, monkeypatch, [first])
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=[SEND], agent=FRONT)
        await daemon.user()
        call = await daemon.until("tool_call")
        await daemon.result(call, {"status": "done", "for_model": {"status": "sent"}})
        await daemon.until("settled")
        assert sum(item["type"] == "tool_call" for item in daemon.outputs) == 1
        assert not bubbles(daemon)


async def test_report_echo_output_survives_model_normalization_and_context_tail(
    tmp_path, monkeypatch
):
    install_script(
        tmp_path, monkeypatch, [output("send_message", {"text": "{{user_text}}"})]
    )
    async with fake_daemon() as (daemon, session):
        await daemon.open(tools=[SEND], agent=FRONT)
        session.prompt_sections["profile"] = "changed profile"
        await daemon.send(
            "user_message",
            message_id="report",
            text="[task report] durable result",
            mode="queue",
            author_kind="internal",
        )
        call = await daemon.until("tool_call")
        assert call["args"] == {"text": "[task report] durable result"}
        await daemon.result(call, {"status": "done", "for_model": {"status": "sent"}})
        await daemon.until("settled")
