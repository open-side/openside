import json
from pathlib import Path

import pytest
from runtime_helpers import fake_daemon, install_script, text_response

from openside_agent.runtime.task_policy import SUBAGENT_INSTRUCTIONS

SNAPSHOTS = Path(__file__).parent / "snapshots"
PROFILES = Path(__file__).parents[2] / "docs" / "specs" / "profiles"


def test_task_language_continuation_and_bulk_scope_policy_snapshot():
    assert SUBAGENT_INSTRUCTIONS == (SNAPSHOTS / "task-instructions.txt").read_text()


def test_task_counts_scopes_and_candidate_facts_are_explicit():
    policy = " ".join(SUBAGENT_INSTRUCTIONS.split())
    assert "restate the exact selected scope to the person" in policy
    assert "name every included and excluded folder, category, or target" in policy
    assert (
        'A generic "go ahead" does not choose among unresolved alternatives' in policy
    )
    assert (
        "Compute counts from observed rows or explicit tool totals, never estimate"
        in (policy)
    )
    assert (
        "sum the group sizes and name which groups contribute to each total" in policy
    )
    assert "status candidate is only a discovered sign-in" in policy
    assert "authentication is not verified" in policy
    assert "permission for this agent is a separate grant" in policy
    assert "Candidate status alone is not a failed connection" in policy
    assert "does not justify asking the person to reconnect" in policy


@pytest.mark.parametrize("profile", ["browser-agent", "ultrabrowse"])
def test_profile_snapshots_keep_language_and_human_step_continuation(profile):
    prompt = "\n".join(
        json.loads((PROFILES / f"{profile}.open.json").read_text())[
            "system_prompt_parts"
        ]
    )
    assert "person's latest explicit language preference" in prompt
    assert "page/tool language never overrides it" in prompt
    assert "Tool-adjacent narration is internal, not a reply" in prompt
    assert "start the continuation with a fresh browser_snapshot" in prompt
    assert "before replying" in prompt
    assert "Never reuse earlier snapshot ids or refs" in prompt


async def test_takeover_continuation_captures_and_checks_before_reply(
    tmp_path, monkeypatch
):
    install_script(
        tmp_path,
        monkeypatch,
        [
            {
                "parts": [
                    {
                        "type": "tool_call",
                        "name": "request_human_step",
                        "args": {"tab_id": "tab-1"},
                        "id": "takeover",
                    }
                ]
            },
            {
                "parts": [
                    {
                        "type": "tool_call",
                        "name": "browser_snapshot",
                        "args": {"tab_id": "{{result:takeover.tab_id}}"},
                        "id": "fresh",
                    }
                ]
            },
            text_response("Verified the signed-in account on the fresh page."),
        ],
    )
    tools = [
        {
            "name": name,
            "description": name,
            "parameters": {
                "type": "object",
                "properties": {"tab_id": {"type": "string"}},
                "required": ["tab_id"],
                "additionalProperties": False,
            },
            "approval": "read",
            "deferrable": True,
        }
        for name in ["request_human_step", "browser_snapshot"]
    ]
    async with fake_daemon() as (daemon, _):
        await daemon.open(tools=tools)
        await daemon.user(text="Sign in and verify the account.")
        takeover = await daemon.until("tool_call")
        await daemon.result(
            takeover,
            {"status": "deferred", "kind": "external", "ticket_id": "takeover-1"},
        )
        await daemon.until("settled")
        await daemon.send(
            "ticket_resolved",
            tool_call_id="takeover",
            ticket_id="takeover-1",
            resolution={
                "status": "result",
                "for_model": {
                    "status": "done",
                    "tab_id": "tab-1",
                    "page_changed": True,
                    "note": (
                        "Start the continuation with a fresh browser_snapshot; inspect "
                        "the requested outcome before replying. Earlier refs are void."
                    ),
                },
            },
        )
        fresh = await daemon.until("tool_call")
        assert fresh["name"] == "browser_snapshot"
        assert fresh["args"] == {"tab_id": "tab-1"}
        assert not any(
            item["type"] == "event" and item["event"]["kind"] == "assistant.final"
            for item in daemon.outputs
        )
        await daemon.result(
            fresh,
            {
                "status": "done",
                "for_model": "[snapshot_id fresh-1]\nAccount settings\nSigned in",
            },
        )
        final = await daemon.until("event", kind="assistant.final")
        assert final["event"]["text"] == (
            "Verified the signed-in account on the fresh page."
        )
        await daemon.until("settled")
        assert [
            item["name"] for item in daemon.outputs if item["type"] == "tool_call"
        ] == ["request_human_step", "browser_snapshot"]
        assert sum(item["type"] == "message_accepted" for item in daemon.outputs) == 1
