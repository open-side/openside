"""Skill mentions: `/name` loads the skill via the daemon tool before the model."""

import pytest
from hypothesis import given
from hypothesis import strategies as st
from pydantic_ai.messages import (
    ModelResponse,
    TextPart,
    ToolCallPart,
    ToolReturnPart,
    UserPromptPart,
)
from pydantic_ai.models.function import AgentInfo, FunctionModel
from runtime_helpers import (
    fake_daemon,
    install_script,
    text_response,
    tool_response,
    tool_spec,
)

from openside_agent.runtime.state import SnapshotData
from openside_agent.skills import INDEX_CLOSE, INDEX_OPEN, index_names, mentioned

INDEX = (
    "Skills are reusable instructions.\n"
    f"{INDEX_OPEN}\n"
    "- pdf-tools: Extract text: from PDFs\n"
    "- notes: Keep notes\n"
    "- repo-helper: [project] From the repository\n"
    "- bad name: ignored\n"
    f"{INDEX_CLOSE}"
)


def test_index_names_reads_only_the_delimited_block():
    assert index_names(["other part", INDEX]) == ["pdf-tools", "notes", "repo-helper"]
    assert index_names(["- pdf-tools: not inside a block"]) == []
    assert index_names([f"{INDEX_OPEN}\n- a: b\n"]) == []


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("use /pdf-tools please", ["pdf-tools"]),
        ("/pdf-tools", ["pdf-tools"]),
        ("(/notes) and /pdf-tools.", ["notes", "pdf-tools"]),
        ("/notes /notes /notes", ["notes"]),
        ("see /usr/notes/file", []),
        ("path /notes/readme", []),
        ("a/notes", []),
        ("/notes.md", []),
        ("/unknown", []),
        ("/Notes", []),
        ("", []),
    ],
)
def test_mentions_are_whole_words_for_known_skills_only(text, expected):
    assert mentioned(text, ["pdf-tools", "notes"]) == expected


@given(st.text(max_size=300))
def test_mention_scan_never_raises_and_only_returns_known_names(text):
    assert set(mentioned(text, ["pdf-tools", "notes"])) <= {"pdf-tools", "notes"}


@given(st.lists(st.text(max_size=80), max_size=10))
def test_index_names_never_raises(parts):
    index_names(parts)


def skill_spec():
    spec = tool_spec("skill")
    spec["parameters"] = {
        "type": "object",
        "properties": {"name": {"type": "string"}, "resource": {"type": "string"}},
        "required": ["name"],
        "additionalProperties": False,
    }
    return spec


async def open_with_skills(daemon, *, tools=None):
    await daemon.send(
        "open",
        session="test-session",
        model={"id": "test:test", "fallback": []},
        system_prompt_parts=["Use daemon tools only.", INDEX],
        tools=tools if tools is not None else [skill_spec()],
        resume_state=None,
        config={"credentials": {}, "agent": {}},
    )
    return await daemon.until("ready")


def recording_model(requests, instructions=None):
    async def respond(messages, info: AgentInfo):
        requests.append(list(messages))
        return ModelResponse(parts=[TextPart("done")])

    async def stream(messages, info: AgentInfo):
        requests.append(list(messages))
        if instructions is not None:
            instructions.append(info.instructions)
        yield "done"

    # The sidecar streams every model request.
    return FunctionModel(respond, stream_function=stream)


def kinds(messages):
    return [
        (type(message).__name__, [type(part).__name__ for part in message.parts])
        for message in messages
    ]


async def test_mentioned_skill_is_loaded_through_the_daemon_before_the_model_runs():
    requests: list[list] = []
    instructions: list[str | None] = []
    async with fake_daemon(model=recording_model(requests, instructions)) as (
        daemon,
        _,
    ):
        await open_with_skills(daemon)
        await daemon.user(text="Please use /pdf-tools on the attached file")
        call = await daemon.until("tool_call")
        assert call["name"] == "skill" and call["args"] == {"name": "pdf-tools"}
        assert call["tool_call_id"].startswith("skill-mention-")
        assert call.get("subagent") is None and call.get("approval_ticket") is None
        assert requests == [], "the model must not run before the skill is loaded"
        body = {"name": "pdf-tools", "instructions": "# Steps\nRun pdftotext."}
        await daemon.result(call, {"status": "done", "for_model": body})
        await daemon.until("settled")

    (seen,) = requests
    # The index still reaches the model although the history now ends in a tool result.
    assert instructions and INDEX_OPEN in (instructions[0] or "")
    assert kinds(seen) == [
        ("ModelRequest", ["UserPromptPart"]),
        ("ModelResponse", ["ToolCallPart"]),
        ("ModelRequest", ["ToolReturnPart"]),
    ]
    assert isinstance(seen[1].parts[0], ToolCallPart)
    returned = seen[2].parts[0]
    assert isinstance(returned, ToolReturnPart)
    assert returned.content == body
    assert returned.tool_call_id == seen[1].parts[0].tool_call_id
    assert isinstance(seen[0].parts[0], UserPromptPart)


async def test_loaded_skill_stays_in_the_persisted_conversation():
    requests: list[list] = []
    async with fake_daemon(model=recording_model(requests)) as (daemon, _):
        await open_with_skills(daemon)
        await daemon.user(text="/notes")
        call = await daemon.until("tool_call")
        await daemon.result(
            call, {"status": "done", "for_model": {"instructions": "x"}}
        )
        await daemon.until("settled")
        state = SnapshotData.model_validate_json(daemon.snapshots()[-1]["data"])
    restored = state.restore_messages()
    assert any(
        isinstance(part, ToolReturnPart) and part.tool_name == "skill"
        for message in restored
        for part in message.parts
    )


async def test_skill_the_daemon_refuses_is_skipped_and_the_message_still_runs():
    requests: list[list] = []
    async with fake_daemon(model=recording_model(requests)) as (daemon, _):
        await open_with_skills(daemon)
        await daemon.user(text="/pdf-tools then /notes")
        first = await daemon.until("tool_call")
        assert first["args"] == {"name": "pdf-tools"}
        await daemon.result(
            first, {"status": "denied", "reason": "skill_not_found", "hint": None}
        )
        second = await daemon.until("tool_call")
        assert second["args"] == {"name": "notes"}
        await daemon.result(second, {"status": "error", "message": "unreadable"})
        await daemon.until("settled")
        results = [
            item["outcome"]
            for item in daemon.outputs
            if item["type"] == "message_result"
        ]
    assert results == ["completed"]
    (seen,) = requests
    assert kinds(seen) == [("ModelRequest", ["UserPromptPart"])]


async def test_unlisted_names_and_messages_without_mentions_cause_no_tool_call():
    requests: list[list] = []
    async with fake_daemon(model=recording_model(requests)) as (daemon, _):
        await open_with_skills(daemon)
        await daemon.user(text="open /usr/bin/env and /ghost and /notes/readme")
        await daemon.until("settled")
        calls = [item for item in daemon.outputs if item["type"] == "tool_call"]
    assert calls == []
    assert len(requests) == 1


async def test_without_the_skill_tool_mentions_do_nothing():
    requests: list[list] = []
    async with fake_daemon(model=recording_model(requests)) as (daemon, _):
        await open_with_skills(daemon, tools=[tool_spec("probe")])
        await daemon.user(text="/pdf-tools")
        await daemon.until("settled")
        calls = [item for item in daemon.outputs if item["type"] == "tool_call"]
    assert calls == []


async def test_a_message_steered_into_a_running_turn_can_mention_a_skill(
    tmp_path, monkeypatch
):
    install_script(tmp_path, monkeypatch, [tool_response(), text_response("done")])
    async with fake_daemon() as (daemon, _):
        await open_with_skills(daemon, tools=[tool_spec(), skill_spec()])
        await daemon.user("m1", "plain request")
        probe = await daemon.until("tool_call")
        assert probe["name"] == "probe"
        # m1 is still running (waiting on its tool); m2 joins it as a steer.
        await daemon.user("m2", "also use /notes", "steer")
        await daemon.result(probe, {"status": "done", "for_model": "probe ok"})
        mention = await daemon.until("tool_call")
        assert mention["name"] == "skill" and mention["args"] == {"name": "notes"}
        assert mention["tool_call_id"].startswith("skill-mention-")
        await daemon.result(
            mention, {"status": "done", "for_model": {"instructions": "NOTES BODY"}}
        )
        await daemon.until("settled")
        messages = SnapshotData.model_validate_json(
            daemon.snapshots()[-1]["data"]
        ).restore_messages()
    parts = [part for message in messages for part in message.parts]
    order = [
        (type(part).__name__, getattr(part, "tool_name", None) or str(part.content))
        for part in parts
        if isinstance(part, UserPromptPart | ToolCallPart | ToolReturnPart)
    ]
    assert order == [
        ("UserPromptPart", "plain request"),
        ("ToolCallPart", "probe"),
        ("ToolReturnPart", "probe"),
        ("UserPromptPart", "also use /notes"),
        ("ToolCallPart", "skill"),
        ("ToolReturnPart", "skill"),
    ]
    assert not any(
        item["type"] == "message_result" and item["outcome"] != "completed"
        for item in daemon.outputs
    )
