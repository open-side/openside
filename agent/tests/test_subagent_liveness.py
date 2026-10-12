"""PP-24 ②: random interleavings of child waits, approvals, cancels, steers and
restarts never leave a parent `delegate` call waiting forever."""

import asyncio
import json
import os
import tempfile
from collections import Counter
from pathlib import Path

from hypothesis import HealthCheck, given, settings
from hypothesis import strategies as st
from pydantic_ai.messages import ToolReturnPart
from runtime_helpers import fake_daemon, text_response, tool_spec
from test_subagents import call, delegate_spec

from openside_agent.protocol import OUTBOUND_ADAPTER
from openside_agent.runtime.state import TERMINAL, SnapshotData

ACTION = st.one_of(
    st.tuples(st.just("resolve"), st.integers(0, 7), st.booleans(), st.booleans()),
    st.tuples(st.just("cancel"), st.integers(0, 3)),
    st.tuples(st.just("steer"), st.integers(0, 3)),
    st.tuples(st.just("restart")),
)


class Daemon:
    """Test stand-in for the daemon's ticket, subagent and recovery rules."""

    def __init__(self):
        self.tickets: dict[str, dict] = {}
        self.subagents: dict[str, str] = {}
        self.executions: Counter[str] = Counter()
        self.snapshot = None
        self.finals = 0
        self.errors: list[dict] = []
        self.context = None
        self.peer = None
        self.session = None

    async def start(self, resume_state=None):
        self.context = fake_daemon()
        self.peer, self.session = await self.context.__aenter__()
        ready = await self.peer.open(
            tools=[tool_spec(), delegate_spec()], resume_state=resume_state
        )
        return ready

    async def stop(self):
        # Everything the sidecar emits after the kill point is lost, like SIGKILL.
        await self.peer.send("shutdown")
        await self.peer.until("bye")
        await self.context.__aexit__(None, None, None)

    async def restart(self):
        await self.stop()
        ready = await self.start(self.snapshot)
        for subagent_id, status in self.subagents.items():
            if status == "cancelled":
                await self.peer.send("subagent_cancel", subagent_id=subagent_id)
        for pending in ready["pending_deferred"]:
            ticket = self.tickets[pending["ticket_id"]]
            if ticket["state"] == "pending" or ticket["state"] == "cancelled":
                continue
            if ticket["delivered"]:
                resolution = {"status": "denied", "reason": "Interrupted"}
            else:
                resolution = self.resolution(ticket)
                ticket["delivered"] = True
            await self.deliver(pending["ticket_id"], ticket, resolution)

    @staticmethod
    def resolution(ticket):
        if ticket["state"] == "approved":
            return {"status": "approved"}
        return {"status": "denied", "reason": "Rejected"}

    async def deliver(self, ticket_id, ticket, resolution):
        await self.peer.send(
            "ticket_resolved",
            ticket_id=ticket_id,
            tool_call_id=ticket["call"],
            resolution=resolution,
        )

    def idle(self):
        session = self.session
        return session.run_task is None and not session.children_active()

    async def pump(self, quiet_ticks=2):
        ticks = 0
        for _ in range(2000):
            try:
                line = await asyncio.wait_for(self.peer.reader.readline(), 0.02)
            except TimeoutError:
                ticks = ticks + 1 if self.idle() and not self.session.rpc else 0
                if ticks >= quiet_ticks:
                    return
                continue
            ticks = 0
            assert line, "Sidecar closed unexpectedly"
            OUTBOUND_ADAPTER.validate_json(line)
            await self.handle(json.loads(line))
        raise AssertionError("Sidecar never went quiet")

    async def handle(self, message):
        kind = message["type"]
        if kind == "error":
            self.errors.append(message)
        elif kind == "state_snapshot":
            self.snapshot = message["state"]
        elif kind == "event":
            event = message["event"]
            if event["kind"] == "assistant.final" and event["text"] == "parent done":
                self.finals += 1
            elif event["kind"] == "subagent.lifecycle":
                current = self.subagents.get(event["subagent_id"])
                if current not in TERMINAL:
                    self.subagents[event["subagent_id"]] = event["status"]
        elif kind == "tool_call":
            await self.peer.result(message, self.tool_outcome(message))

    def tool_outcome(self, message):
        if message["name"] == "delegate":
            subagent_id = "sub-" + message["tool_call_id"]
            self.subagents[subagent_id] = "running"
            return {
                "status": "done",
                "for_model": {
                    "subagent_id": subagent_id,
                    "model": "test:child",
                    "tools": ["probe"],
                    "context": "fresh",
                },
            }
        subagent_id = message.get("subagent")
        if self.subagents.get(subagent_id) == "cancelled":
            return {"status": "denied", "reason": "Subagent cancelled", "hint": None}
        claim = message.get("approval_ticket")
        if claim is not None:
            ticket = self.tickets[claim]
            if ticket["state"] != "approved" or ticket["claimed"]:
                return {"status": "denied", "reason": "Not approved", "hint": None}
            ticket["claimed"] = True
            self.executions[claim] += 1
            return {"status": "done", "for_model": "probed"}
        ticket_id = f"t{len(self.tickets)}"
        self.tickets[ticket_id] = {
            "call": message["tool_call_id"],
            "subagent": subagent_id,
            "state": "pending",
            "delivered": False,
            "claimed": False,
        }
        return {"status": "deferred", "kind": "approval", "ticket_id": ticket_id}

    def pending_tickets(self):
        return sorted(
            ticket_id
            for ticket_id, ticket in self.tickets.items()
            if ticket["state"] == "pending"
        )

    def live_subagents(self):
        return sorted(
            subagent_id
            for subagent_id, status in self.subagents.items()
            if status not in TERMINAL
        )

    async def resolve(self, ticket_id, approve):
        ticket = self.tickets[ticket_id]
        ticket["state"] = "approved" if approve else "denied"
        ticket["delivered"] = True
        await self.deliver(ticket_id, ticket, self.resolution(ticket))

    async def act(self, action):
        if action[0] == "resolve":
            _, index, approve, crash = action
            pending = self.pending_tickets()
            if pending:
                await self.resolve(pending[index % len(pending)], approve)
                if crash:
                    await self.restart()
        elif action[0] == "restart":
            await self.restart()
        else:
            live = self.live_subagents()
            if not live:
                return
            subagent_id = live[action[1] % len(live)]
            if action[0] == "cancel":
                self.subagents[subagent_id] = "cancelled"
                for ticket in self.tickets.values():
                    if (
                        ticket["subagent"] == subagent_id
                        and ticket["state"] == "pending"
                    ):
                        ticket["state"] = "cancelled"
                        ticket["delivered"] = True
                await self.peer.send("subagent_cancel", subagent_id=subagent_id)
            else:
                await self.peer.send(
                    "subagent_steer", subagent_id=subagent_id, text="steer"
                )


def write_script(directory: Path, children: list[int]) -> Path:
    delegates = [f"del-{index}" for index in range(len(children))]
    script = {
        "responses": [
            {"parts": [call("delegate", id, {"task": id}) for id in delegates]},
            text_response("parent done"),
        ],
        "subagents": {
            id: [
                *({"parts": [call("probe", f"{id}-call-{step}")]} for step in range(n)),
                text_response(f"{id} done"),
            ]
            for id, n in zip(delegates, children, strict=True)
        },
    }
    path = directory / "script.json"
    path.write_text(json.dumps(script))
    return path


async def scenario(children: list[int], actions) -> None:
    daemon = Daemon()
    await daemon.start()
    try:
        await daemon.peer.user()
        await daemon.pump()
        for action in actions:
            await daemon.act(action)
            await daemon.pump()
        for _ in range(20):
            pending = daemon.pending_tickets()
            if not pending and daemon.idle() and daemon.finals:
                break
            for ticket_id in pending:
                await daemon.resolve(ticket_id, True)
            await daemon.pump(quiet_ticks=3)
        assert not daemon.errors, daemon.errors
        assert daemon.finals == 1, f"parent final seen {daemon.finals} times"
        state = SnapshotData.model_validate_json(daemon.snapshot["data"])
        assert not state.pending_delegates and not state.pending_deferred
        assert all(child.status in TERMINAL for child in state.subagents)
        returns = Counter(
            part.tool_call_id
            for message in state.restore_messages()
            for part in message.parts
            if isinstance(part, ToolReturnPart) and part.tool_call_id.startswith("del-")
        )
        assert returns == Counter(f"del-{index}" for index in range(len(children)))
        assert all(count == 1 for count in daemon.executions.values())
    finally:
        await daemon.stop()


@settings(
    max_examples=40,
    deadline=None,
    suppress_health_check=[HealthCheck.too_slow],
)
@given(
    st.lists(st.integers(1, 2), min_size=1, max_size=2),
    st.lists(ACTION, max_size=8),
)
def test_random_interleavings_never_deadlock(children, actions):
    with tempfile.TemporaryDirectory() as directory:
        previous = os.environ.get("OPENSIDE_AGENT_SCRIPT")
        os.environ["OPENSIDE_AGENT_SCRIPT"] = str(
            write_script(Path(directory), children)
        )
        try:
            asyncio.run(scenario(children, actions))
        finally:
            if previous is None:
                del os.environ["OPENSIDE_AGENT_SCRIPT"]
            else:
                os.environ["OPENSIDE_AGENT_SCRIPT"] = previous
