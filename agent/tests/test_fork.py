"""Property tests for the forked-subagent history copy (PP-24 ③, A-5 ⑤)."""

from hypothesis import given, settings
from hypothesis import strategies as st
from pydantic_ai.messages import (
    ModelMessage,
    ModelRequest,
    ModelResponse,
    RetryPromptPart,
    TextPart,
    ToolCallPart,
    ToolReturnPart,
    UserPromptPart,
)

from openside_agent.runtime.fork import answer_id, fork_history

# Each step: a user prompt, a tool batch with chosen answered calls, or a stray
# return for a call id that never appears (an orphan).
STEP = st.one_of(
    st.tuples(st.just("prompt"), st.text(max_size=5)),
    st.tuples(
        st.just("batch"),
        st.lists(
            st.sampled_from(["return", "retry", "missing", "late"]),
            min_size=1,
            max_size=3,
        ),
    ),
    st.tuples(st.just("orphan"), st.integers(0, 3)),
    st.tuples(st.just("text"), st.text(max_size=5)),
)


def build(steps) -> list[ModelMessage]:
    messages: list[ModelMessage] = []
    late: list[ToolReturnPart] = []
    counter = 0
    for kind, value in steps:
        if kind == "prompt":
            messages.append(ModelRequest(parts=[UserPromptPart(value), *late]))
            late = []
        elif kind == "text":
            messages.append(ModelResponse(parts=[TextPart(value)]))
        elif kind == "orphan":
            messages.append(
                ModelRequest(
                    parts=[ToolReturnPart("probe", "stray", tool_call_id=f"x{value}")]
                )
            )
        else:
            calls = []
            answers = []
            for fate in value:
                counter += 1
                call_id = f"c{counter}"
                calls.append(ToolCallPart("probe", {}, tool_call_id=call_id))
                if fate == "return":
                    answers.append(ToolReturnPart("probe", "ok", tool_call_id=call_id))
                elif fate == "retry":
                    answers.append(
                        RetryPromptPart("bad", tool_name="probe", tool_call_id=call_id)
                    )
                elif fate == "late":
                    late.append(ToolReturnPart("probe", "late", tool_call_id=call_id))
            messages.append(ModelResponse(parts=calls))
            if answers:
                messages.append(ModelRequest(parts=answers))
    return messages


def well_formed(messages: list[ModelMessage]) -> bool:
    issued: set[str] = set()
    answered: set[str] = set()
    for message in messages:
        for part in message.parts:
            if isinstance(part, ToolCallPart):
                issued.add(part.tool_call_id)
            elif (answer := answer_id(part)) is not None:
                if answer not in issued:
                    return False
                answered.add(answer)
    return issued == answered


@settings(max_examples=300, deadline=None)
@given(st.lists(STEP, max_size=12))
def test_fork_is_well_formed_prefix(steps):
    history = build(steps)
    forked = fork_history(history)
    assert well_formed(forked)
    if well_formed(history):
        assert forked == history
    kept = iter(history)
    for message in forked:
        # Kept messages appear in input order; responses verbatim, requests
        # only lose orphaned returns.
        if isinstance(message, ModelResponse):
            assert message in kept
        else:
            assert any(
                isinstance(source, ModelRequest)
                and all(part in source.parts for part in message.parts)
                for source in kept
            )


@given(st.lists(STEP, max_size=8))
def test_fork_copy_is_independent(steps):
    history = build(steps)
    before = [message.parts.copy() for message in history]
    forked = fork_history(history)
    for message in forked:
        message.parts.clear()
    assert [message.parts for message in history] == before


def test_delegating_batch_and_its_siblings_are_excluded():
    history = build(
        [
            ("prompt", "start"),
            ("batch", ["return"]),
            ("batch", ["missing", "return"]),
        ]
    )
    forked = fork_history(history)
    ids = {
        getattr(part, "tool_call_id", None)
        for message in forked
        for part in message.parts
    }
    assert "c1" in ids and "c2" not in ids and "c3" not in ids
