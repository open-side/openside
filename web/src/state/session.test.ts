import { describe, expect, it } from "vitest";
import type { Event, Ticket } from "../api/types";
import { emptySessionView, pendingTickets, reduceSession, replaySession } from "./session";

type Payload = { kind: string } & Record<string, unknown>;

function log(payloads: Payload[]): Event[] {
  return payloads.map((payload, index) => ({
    session_id: "s1",
    seq: index + 1,
    ts: "2026-01-01T00:00:00Z",
    run_id: null,
    ...payload,
  })) as Event[];
}

const ticket: Ticket = {
  id: "k1",
  session_id: "s1",
  tool_call_id: "ask",
  deferral: "external",
  kind: "info",
  summary: "Pick a color",
  instructions: null,
  options: [
    { value: "blue", label: "Blue" },
    { value: "green", label: "Green" },
  ],
  origin: { subagent_id: null },
  tool: null,
  target: null,
  resume_when: null,
  created_at: "2026-01-01T00:00:00Z",
  expires_at: "2026-01-02T00:00:00Z",
  status: "pending",
  resolution: null,
};

const events = log([
  { kind: "session.created", title: "Task", model: "provider:model", mode: "guard" },
  { kind: "message.queued", message_id: "m1", text: "hello", mode: "steer" },
  { kind: "message.queued", message_id: "m2", text: "later", mode: "queue" },
  { kind: "message.queued", message_id: "m3", text: "drop me", mode: "queue" },
  { kind: "message.queued", message_id: "m4", text: "urgent", mode: "queue" },
  { kind: "message.edited", message_id: "m2", text: "later, edited" },
  { kind: "message.dropped", message_id: "m3", reason: "removed" },
  { kind: "message.injected", message_id: "m1" },
  { kind: "turn.started" },
  { kind: "thinking.delta", text: "hmm" },
  { kind: "assistant.delta", text: "Hel" },
  { kind: "assistant.delta", text: "lo" },
  { kind: "tool.started", tool_call_id: "c1", name: "read", args: { path: "a.txt" } },
  { kind: "tool.updated", tool_call_id: "c1", update: { bytes: 3 } },
  { kind: "tool.ended", tool_call_id: "c1", result: { content: "abc" }, is_error: false },
  { kind: "ticket.opened", ticket },
  { kind: "message.completed", message_id: "m1", outcome: "deferred" },
  { kind: "session.settled" },
  { kind: "message.promoted", message_id: "m4" },
  { kind: "session.updated", pinned: true, title: "Renamed" },
  { kind: "ticket.resolved", ticket_id: "k1", resolution: { answer: "blue" } },
  { kind: "assistant.final", text: "blue it is" },
  { kind: "some.future.kind", data: 1 },
]);

describe("session view reducer", () => {
  it("replays private-session metadata used to hide browser preview", () => {
    const events = log([{ kind: "session.created", title: "Private", model: "provider:model", incognito: true }]);
    expect(replaySession(events).incognito).toBe(true);
    expect(replaySession(log([{ kind: "session.created", title: "Public", model: "provider:model" }])).incognito).toBe(false);
  });

  it("rebuilds the complete screen state purely from the replayed event log", () => {
    const view = replaySession(events);
    expect(view).toMatchObject({
      lastSeq: events.length,
      title: "Renamed",
      model: "provider:model",
      mode: "guard",
      pinned: true,
      archived: false,
      status: "idle",
      thinking: false,
    });
    expect(view.queue).toEqual([
      { id: "m4", text: "urgent", mode: "steer" },
      { id: "m2", text: "later, edited", mode: "queue" },
    ]);
    expect(view.items).toEqual([
      { type: "user", id: "m1", text: "hello", outcome: "deferred", reason: undefined },
      { type: "assistant", id: "a11", text: "Hello", final: true },
      {
        type: "tool",
        id: "c1",
        name: "read",
        args: { path: "a.txt" },
        updates: [{ bytes: 3 }],
        result: { content: "abc" },
        isError: false,
        done: true,
      },
      { type: "ticket", id: "k1" },
      { type: "assistant", id: "a22", text: "blue it is", final: true, finalSeq: 22 },
    ]);
    expect(view.tickets.k1).toMatchObject({ status: "resolved", resolution: { answer: "blue" } });
    expect(pendingTickets(view)).toEqual([]);
  });

  it("produces the same state when the stream reconnects mid-log and replays overlap", () => {
    const live = events.reduce(reduceSession, emptySessionView);
    for (let cut = 0; cut <= events.length; cut++) {
      // Reconnect at `cut`: the daemon may resend from any earlier cursor.
      const first = replaySession(events.slice(0, cut));
      const resumed = replaySession(events.slice(Math.max(0, cut - 3)), first);
      expect(resumed).toEqual(live);
    }
  });

  it("tracks streaming text, thinking and running status mid-turn", () => {
    const mid = replaySession(events.slice(0, 11));
    expect(mid.status).toBe("running");
    expect(mid.items.at(-1)).toEqual({ type: "assistant", id: "a11", text: "Hel", final: false });
    const thinking = replaySession(events.slice(0, 10));
    expect(thinking.thinking).toBe(true);
    const waiting = replaySession(events.slice(0, 16));
    expect(pendingTickets(waiting).map((t) => t.id)).toEqual(["k1"]);
  });

  it("closes a streaming bubble interrupted by an injected steer or a runtime error", () => {
    for (const interruption of [
      { kind: "message.injected", message_id: "m9" },
      { kind: "runtime.error", code: "provider_error", message: "boom" },
    ]) {
      const view = replaySession(
        log([
          { kind: "turn.started" },
          { kind: "assistant.delta", text: "partial" },
          interruption,
          { kind: "assistant.delta", text: "next" },
          { kind: "session.settled" },
        ]),
      );
      const bubbles = view.items.filter((item) => item.type === "assistant");
      expect(bubbles, interruption.kind).toEqual([
        { type: "assistant", id: "a2", text: "partial", final: true },
        { type: "assistant", id: "a4", text: "next", final: true },
      ]);
      const afterInterruption = replaySession(log([{ kind: "assistant.delta", text: "partial" }, interruption]));
      expect(afterInterruption.items[0], interruption.kind).toMatchObject({ final: true });
    }
  });

  it("reorders a pending message inside its own lane", () => {
    const view = replaySession(
      log([
        { kind: "message.queued", message_id: "s1", text: "steer", mode: "steer" },
        { kind: "message.queued", message_id: "a", text: "a", mode: "queue" },
        { kind: "message.queued", message_id: "b", text: "b", mode: "queue" },
        { kind: "message.queued", message_id: "c", text: "c", mode: "queue" },
        { kind: "message.moved", message_id: "c", position: 0 },
        { kind: "message.moved", message_id: "s1", position: 5 },
      ]),
    );
    expect(view.queue.map((m) => m.id)).toEqual(["s1", "c", "a", "b"]);
  });
});

describe("subagents, artifacts and attachments", () => {
  const attachment = {
    path: "attachments/m1-cat.png",
    kind: "image",
    mime: "image/png",
    name: "cat.png",
    size: 68,
    sha256: "ab",
  };
  const childLog = log([
    { kind: "message.queued", message_id: "m1", text: "look", mode: "steer", attachments: [attachment] },
    { kind: "message.injected", message_id: "m1" },
    { kind: "turn.started" },
    { kind: "tool.started", tool_call_id: "d1", name: "delegate", args: { task: "Research" } },
    { kind: "subagent.started", subagent_id: "sa1", parent_tool_call_id: "d1", task: "Research", model: "m", tools: ["read"] },
    { kind: "subagent.progress", subagent_id: "sa1", progress: { kind: "model_request" } },
    { kind: "subagent.lifecycle", subagent_id: "sa1", status: "waiting_human" },
    { kind: "subagent.lifecycle", subagent_id: "sa1", status: "running" },
    { kind: "subagent.progress", subagent_id: "sa1", progress: { kind: "model_response", text: "done" } },
    { kind: "artifact.saved", path: "out/report.md", mime: "text/markdown", size: 10, tool: "write", tool_call_id: "w1" },
    { kind: "artifact.saved", path: "out/report.md", mime: "text/markdown", size: 20, tool: "edit", tool_call_id: "w2" },
    { kind: "subagent.ended", subagent_id: "sa1", status: "failed", reason: "budget" },
    { kind: "assistant.final", text: "```svg visual\n<svg/>\n```" },
  ]);

  it("projects the child's live stream, saved files and attachments from events alone", () => {
    const view = replaySession(childLog);
    expect(view.subagents).toEqual([
      {
        id: "sa1",
        task: "Research",
        model: "m",
        tools: ["read"],
        parentToolCallId: "d1",
        status: "failed",
        reason: "budget",
        progress: [{ kind: "model_request" }, { kind: "model_response", text: "done" }],
      },
    ]);
    expect(replaySession(childLog.slice(0, 7)).subagents[0]?.status).toBe("waiting_human");
    expect(view.artifacts).toEqual([{ path: "out/report.md", mime: "text/markdown", size: 20, tool: "edit" }]);
    expect(view.items[0]).toEqual({ type: "user", id: "m1", text: "look", attachments: [attachment] });
    expect(view.items.at(-1)).toMatchObject({ type: "assistant", final: true, finalSeq: 13 });
  });
});

describe("final answer ordering", () => {
  it("keeps one bubble when turn.ended closes the streamed text before assistant.final", () => {
    const view = replaySession(
      log([
        { kind: "turn.started" },
        { kind: "assistant.delta", text: "parent " },
        { kind: "assistant.delta", text: "done" },
        { kind: "turn.ended" },
        { kind: "assistant.final", text: "parent done" },
        { kind: "session.settled" },
      ]),
    );
    expect(view.items).toEqual([{ type: "assistant", id: "a2", text: "parent done", final: true, finalSeq: 5 }]);
  });
});

describe("user-task lifecycle across model requests", () => {
  it("keeps a replay ending inside a tool call running after turn.ended", () => {
    const task = log([
      { kind: "message.queued", message_id: "m1", text: "inspect", mode: "queue" },
      { kind: "message.injected", message_id: "m1" },
      { kind: "turn.started" },
      { kind: "thinking.delta", text: "plan" },
      { kind: "turn.ended" },
      { kind: "tool.started", tool_call_id: "c1", name: "read", args: {} },
      { kind: "tool.ended", tool_call_id: "c1", result: "read", is_error: false },
      { kind: "turn.started" },
      { kind: "turn.ended" },
      { kind: "tool.started", tool_call_id: "c2", name: "read", args: {} },
    ]);
    expect(replaySession(task.slice(0, 2)).status).toBe("running");
    const view = replaySession(task);
    expect(view.status).toBe("running");
    expect(view.thinking).toBe(false);
    expect(view.items.at(-1)).toMatchObject({ type: "tool", id: "c2", done: false });
    expect(replaySession(task.slice(4), replaySession(task.slice(0, 7)))).toEqual(view);
  });

  it("does not finish until all injected messages complete", () => {
    const task = log([
      { kind: "message.queued", message_id: "m1", text: "first", mode: "queue" },
      { kind: "message.injected", message_id: "m1" },
      { kind: "message.queued", message_id: "m2", text: "steer", mode: "steer" },
      { kind: "message.injected", message_id: "m2" },
      { kind: "turn.ended" },
      { kind: "message.completed", message_id: "m1", outcome: "completed" },
      { kind: "message.completed", message_id: "m2", outcome: "failed" },
    ]);
    expect(replaySession(task.slice(0, 6)).status).toBe("running");
    expect(replaySession(task).status).toBe("idle");
  });

  it("releases running state on a human deferral and resumes the same message across turn.ended", () => {
    const task = log([
      { kind: "message.queued", message_id: "m1", text: "ask", mode: "queue" },
      { kind: "message.injected", message_id: "m1" },
      { kind: "ticket.opened", ticket },
      { kind: "message.completed", message_id: "m1", outcome: "deferred" },
      { kind: "session.settled" },
      { kind: "ticket.resolved", ticket_id: ticket.id, resolution: { answer: "blue" } },
      { kind: "turn.started" },
      { kind: "turn.ended" },
      { kind: "message.completed", message_id: "m1", outcome: "completed" },
    ]);
    const waiting = replaySession(task.slice(0, 5));
    expect(waiting.status).toBe("idle");
    expect(pendingTickets(waiting).map((pending) => pending.id)).toEqual([ticket.id]);
    expect(replaySession(task.slice(0, 8)).status).toBe("running");
    expect(replaySession(task).status).toBe("idle");
  });
});
