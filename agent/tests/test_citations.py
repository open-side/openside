"""A-9: citation heuristic, visited sources, ultrabrowse planning and citations."""

import json

import pytest
from pydantic_ai.messages import (
    ModelMessage,
    ModelRequest,
    RetryPromptPart,
    ToolReturnPart,
    UserPromptPart,
)
from pydantic_ai.models.function import AgentInfo, DeltaToolCall, FunctionModel
from pydantic_ai.profiles import ModelProfile
from runtime_helpers import fake_daemon, tool_spec

from openside_agent import citations, research

A = "http://127.0.0.1:41001/rivers"
B = "http://127.0.0.1:41001/lakes"

GOOD = f"""## Findings
- The Nile is about 6,650 km long [Rivers]({A}).
The Amazon carries the largest discharge of any river. [source]({A})
Lake Baikal is the deepest lake on Earth [Lakes]({B}).
Want more detail?"""

UNCITED = f"""The Nile is about 6,650 km long [Rivers]({A}).
Lake Baikal holds about a fifth of the world's fresh surface water.
Lake Superior is the largest lake by area [blog](http://127.0.0.1:41001/never-opened)."""


def test_every_claim_cited_from_visited_pages_scores_zero():
    report = citations.check(GOOD, [A, B + "/"])
    assert len(report.claims) == 3
    assert report.ratio == 0.0
    assert citations.annotate(GOOD, report) == GOOD


def test_uncited_and_unvisited_links_count_and_are_annotated():
    report = citations.check(UNCITED, [A, B])
    assert [claim.cited for claim in report.claims] == [True, False, False]
    assert report.ratio == pytest.approx(2 / 3)
    annotated = citations.annotate(UNCITED, report)
    lines = annotated.splitlines()
    assert lines[0] == UNCITED.splitlines()[0]
    assert lines[1].endswith(f"water. {citations.MARKER}")
    assert lines[2].endswith(f"never-opened). {citations.MARKER}")
    # Annotating twice changes nothing: marked sentences are recognized.
    again = citations.check(annotated, [A, B])
    assert citations.annotate(annotated, again) == annotated


def test_questions_short_fragments_headings_and_code_are_not_claims():
    text = (
        "# Rivers of the world\nShort note.\nIs the Nile longest?\n"
        "```\nprint('x is a long code line here')\n```"
    )
    assert citations.check(text, []).claims == ()
    assert citations.check(text, []).ratio == 0.0


def test_citation_only_line_attaches_to_the_previous_sentence():
    text = f"The Amazon carries the largest discharge of any river.\n[source]({A})"
    report = citations.check(text, [A])
    assert len(report.claims) == 1 and report.claims[0].cited


def test_link_after_the_period_belongs_to_its_own_sentence():
    text = f"Fact one has four words. [one]({A}) Fact two has four words [two]({B})."
    report = citations.check(text, [A, B])
    assert [claim.cited for claim in report.claims] == [True, True]
    assert report.claims[0].text.endswith(f"[one]({A})")


def test_abbreviations_and_initials_do_not_split_a_cited_sentence():
    text = (
        "Rivers in the U.S. carry silt, e.g. the Mississippi delta grows "
        f"each year, per J. R. Smith [Rivers]({A})."
    )
    report = citations.check(text, [A])
    assert len(report.claims) == 1 and report.claims[0].cited
    # A number before the period still ends the sentence.
    two = citations.check(
        f"The river is 6,650 km long in 5. Its delta is wide [r]({A}).", [A]
    )
    assert len(two.claims) == 2


def test_urls_with_parentheses_match_visited_pages():
    page = "https://en.wikipedia.org/wiki/Nile_(river)"
    report = citations.check(f"The Nile flows north to the sea [Nile]({page}).", [page])
    assert report.claims[0].cited and report.claims[0].urls == (page,)


def test_result_urls_reads_only_structured_url_fields():
    found = citations.result_urls(
        [
            {"tab_id": "t", "url": A + "?q=1#x"},
            B,
            {"snapshot": "see http://127.0.0.1:41001/elsewhere", "console": [B + "/x"]},
            "not a url",
        ]
    )
    assert found == {A, B}


def returned(tool, content):
    return ModelRequest(
        parts=[ToolReturnPart(tool_name=tool, content=content, tool_call_id=tool)]
    )


def test_visited_sources_trust_only_page_loading_tool_urls():
    other = "http://127.0.0.1:41001/mentioned"
    messages = [
        returned("browser_open_tab", {"tab_id": "t", "url": A}),
        returned("browser_navigate", {"status": "navigated", "url": B}),
        returned("browser_repl", {"return_value": other, "console": [other], "url": A}),
        returned("browser_snapshot", f"[snapshot_id 1]\nlink {other}"),
        returned("browser_tabs", [{"tab_id": "u", "url": other}]),
        returned("bash", {"url": other}),
    ]
    assert research.visited_sources(messages) == {A, B}


def test_plan_parsing_takes_the_last_plan_block():
    text = "Plan:\n1. Open rivers\n2) Open lakes\nThen I browse.\n**Plan:** check peaks"
    assert research.parse_plan(text) == ["check peaks"]
    assert research.parse_plan("## Plan\n- rivers\n- lakes") == ["rivers", "lakes"]
    assert research.parse_plan("No plan here.") == []


def test_only_ultrabrowse_raises_thinking():
    assert research.model_settings("ultrabrowse") == {"thinking": "high"}
    assert research.model_settings("browser-agent") is None
    assert research.model_settings(None) is None


def browser_spec(name):
    spec = tool_spec(name)
    spec["parameters"] = {
        "type": "object",
        "properties": {"url": {"type": "string"}},
        "additionalProperties": False,
    }
    return spec


PLAN = "Plan:\n1. Open the rivers page\n2. Open the lakes page\n"


class Researcher:
    """Streams scripted responses and records what each model request carried."""

    def __init__(self, steps):
        self.steps = list(steps)
        self.requests: list[tuple[list[ModelMessage], AgentInfo]] = []
        # A thinking-capable profile, so the unified setting resolves the way it does
        # for real providers instead of being dropped.
        self.model = FunctionModel(
            stream_function=self.stream, profile=ModelProfile(supports_thinking=True)
        )

    async def stream(self, messages: list[ModelMessage], info: AgentInfo):
        self.requests.append((list(messages), info))
        text, calls = self.steps.pop(0)
        if text:
            yield text
        if calls:
            yield {
                index: DeltaToolCall(
                    name="browser_open_tab",
                    json_args=json.dumps({"url": url}),
                    tool_call_id=f"open-{index}-{len(self.requests)}",
                )
                for index, url in enumerate(calls)
            }


def request_text(messages: list[ModelMessage]) -> str:
    parts: list[str] = []
    for message in messages:
        if isinstance(message, ModelRequest):
            for part in message.parts:
                if isinstance(part, (UserPromptPart, RetryPromptPart)):
                    parts.append(str(part.content))
    return "\n".join(parts)


async def run_research(profile, steps, browse=0):
    researcher = Researcher(steps)
    async with fake_daemon(model=researcher.model) as (daemon, _):
        await daemon.send(
            "open",
            session="research",
            model={"id": "test:function", "fallback": []},
            system_prompt_parts=["Cite visited pages."],
            tools=[browser_spec("browser_open_tab")],
            config={"credentials": {}, "agent": {"profile": profile}},
        )
        await daemon.until("ready")
        await daemon.user(text="research rivers and lakes")
        for _ in range(browse):
            call = await daemon.until("tool_call")
            await daemon.result(
                call,
                {
                    "status": "done",
                    "for_model": {"tab_id": "t", "url": call["args"]["url"]},
                },
            )
        final = await daemon.until("event", kind="assistant.final")
        await daemon.until("settled")
        deltas = "".join(
            item["event"]["text"]
            for item in daemon.outputs
            if item["type"] == "event" and item["event"]["kind"] == "assistant.delta"
        )
        return final["event"]["text"], deltas, researcher.requests


async def test_ultrabrowse_plans_thinks_and_retries_until_every_claim_is_cited():
    text, deltas, requests = await run_research(
        "ultrabrowse",
        [
            (None, [A]),  # browsing before planning is refused
            (PLAN, [A]),
            (None, [B]),
            (UNCITED, None),  # sent back: two uncited claims
            (GOOD, None),
        ],
        browse=2,
    )
    assert text == GOOD
    assert all(info.model_request_parameters.thinking == "high" for _, info in requests)
    assert research.PLAN_REQUIRED in request_text(requests[1][0])
    # The stated plan rides along with every later request, outside the history.
    assert "plan-reminder" in request_text(requests[2][0])
    assert "Open the lakes page" in request_text(requests[3][0])
    retry = request_text(requests[4][0])
    assert "fresh surface water" in retry and "largest lake by area" in retry
    assert A in retry and B in retry
    # Narration beside tool calls streams; rejected and final answers never do.
    assert "Open the rivers page" in deltas
    assert "fresh surface water" not in deltas and "Baikal" not in deltas


async def test_ultrabrowse_annotates_once_citation_retries_are_spent():
    text, _, requests = await run_research(
        "ultrabrowse", [(UNCITED, None)] * (research.CITATION_RETRIES + 1)
    )
    assert len(requests) == research.CITATION_RETRIES + 1
    assert text.count(citations.MARKER) == 3
    assert text.replace(f" {citations.MARKER}", "") == UNCITED


async def test_other_profiles_neither_gate_browsing_nor_rewrite_answers():
    text, deltas, requests = await run_research(
        "full", [(None, [A]), (UNCITED, None)], browse=1
    )
    assert text == UNCITED and deltas == UNCITED
    assert len(requests) == 2
    assert all(info.model_request_parameters.thinking is None for _, info in requests)
