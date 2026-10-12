"""OP-2: the committed wire schema matches the pydantic models and the version pin."""

import json
from pathlib import Path

from openside_agent import __version__, wire_schema

COMMITTED = Path(__file__).resolve().parents[2] / "docs/specs/sidecar-wire.schema.json"


def test_committed_schema_matches_the_wire_models():
    assert COMMITTED.read_text(encoding="utf-8") == wire_schema.render(), (
        "docs/specs/sidecar-wire.schema.json is stale: run `uv run --project agent "
        "python -m openside_agent.wire_schema > docs/specs/sidecar-wire.schema.json`"
    )


def test_schema_covers_every_message_type_in_both_directions():
    defs = json.loads(COMMITTED.read_text(encoding="utf-8"))["$defs"]
    inbound = set(defs["DaemonToSidecar"]["discriminator"]["mapping"])
    outbound = set(defs["SidecarToDaemon"]["discriminator"]["mapping"])
    assert {
        "open",
        "user_message",
        "tool_result",
        "ticket_resolved",
        "shutdown",
    } <= inbound
    assert {"hello", "ready", "event", "tool_call", "state_snapshot", "bye"} <= outbound
    for name in ("Hello", "Open", "Event"):
        assert {"v", "type"} <= set(defs[name]["required"]), name


def test_this_package_is_inside_the_published_supported_range():
    def parse(version: str) -> tuple[int, ...]:
        return tuple(int(part) for part in version.split(".")[:3])

    lower, upper = wire_schema.SUPPORTED_AGENT_VERSIONS.split(",")
    assert lower.startswith(">=") and upper.startswith("<")
    assert parse(lower[2:]) <= parse(__version__) < parse(upper[1:])
    committed = json.loads(COMMITTED.read_text(encoding="utf-8"))
    assert (
        committed["x-openside-agent-versions"] == wire_schema.SUPPORTED_AGENT_VERSIONS
    )
    assert committed["x-protocol"] == 1
