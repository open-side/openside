import { describe, expect, it } from "vitest";
import type { Card, Event, Ticket } from "../api/types";
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

const pendingCard: Card = {
  id: "ticket:k1",
  kind: "form",
  site: { origin: "https://shop.example", name: "shop.example", favicon_ref: null },
  title: "Pick a color",
  subtitle: null,
  status: "pending",
  interaction: "ask",
  ticket_id: "k1",
  collapsed_summary: "Pick a color",
  blocks: [],
  actions: [],
};

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
  card: pendingCard,
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
  { kind: "ticket.resolved", ticket_id: "k1", resolution: { answer: "blue" }, card: { ...pendingCard, status: "done", interaction: "ask", collapsed_summary: "Answered", actions: [], blocks: [] } },
  { kind: "assistant.final", text: "blue it is" },
  { kind: "some.future.kind", data: 1 },
]);

describe("session view reducer", () => {
  it("retains daemon authors through edits and delivery and defaults old events to internal", () => {
    const view = replaySession(log([
      { kind: "message.queued", message_id: "person", text: "hello", mode: "queue", author_kind: "person" },
      { kind: "message.queued", message_id: "api", text: "api", mode: "queue", author_kind: "agent_client" },
      { kind: "message.queued", message_id: "team", text: "team", mode: "queue", author_kind: "internal", from_agent: "chief" },
      { kind: "message.queued", message_id: "old", text: "old", mode: "queue" },
      { kind: "message.edited", message_id: "team", text: "edited" },
      { kind: "message.promoted", message_id: "team" },
      { kind: "message.injected", message_id: "team" },
    ]));
    expect(view.queue.map((message) => [message.id, message.author_kind, message.from_agent])).toEqual([
      ["person", "person", null], ["api", "agent_client", null], ["old", "internal", null],
    ]);
    expect(view.items[0]).toMatchObject({ type: "user", text: "edited", author_kind: "internal", from_agent: "chief" });
  });

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
      { id: "m4", text: "urgent", mode: "steer", author_kind: "internal", from_agent: null },
      { id: "m2", text: "later, edited", mode: "queue", author_kind: "internal", from_agent: null },
    ]);
    expect(view.items).toEqual([
      { type: "user", id: "m1", text: "hello", outcome: "deferred", reason: undefined, author_kind: "internal", from_agent: null },
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
      { type: "card", id: "ticket:k1" },
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
        background: false,
        reason: "budget",
        progress: [{ kind: "model_request" }, { kind: "model_response", text: "done" }],
      },
    ]);
    expect(replaySession(childLog.slice(0, 7)).subagents[0]?.status).toBe("waiting_human");
    expect(view.artifacts).toEqual([{ path: "out/report.md", mime: "text/markdown", size: 20, tool: "edit" }]);
    expect(view.items[0]).toEqual({ type: "user", id: "m1", text: "look", attachments: [attachment], author_kind: "internal", from_agent: null });
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

describe("conversation metadata", () => {
  it("uses one proposed title and never replaces a user rename", () => {
    const proposals = log([
      { kind: "session.created", title: "New session", model: "script" },
      { kind: "session.title_proposed", title: "Compare Acme plans" },
      { kind: "session.title_proposed", title: "A second proposed title" },
      { kind: "session.updated", title: "My plans" },
      { kind: "session.title_proposed", title: "Ignore the rename" },
    ]);
    expect(replaySession(proposals.slice(0, 3)).title).toBe("Compare Acme plans");
    expect(replaySession(proposals).title).toBe("My plans");
  });

  it("keeps a page favicon during its next action and drops it on navigation", () => {
    const page = {
      kind: "browser.state", tab_id: "tab", url: "https://example.test/plans",
      host: "example.test", title: "Plans", phase: "reading", page_kind: "article", facts: {},
    };
    const events = log([
      { ...page, favicon: "data:image/png;base64,icon" },
      { ...page, phase: "acting" },
      { ...page, url: "https://other.test/", host: "other.test", title: "Other" },
    ]);
    expect(replaySession(events.slice(0, 2)).browserStates["front:tab"]!.favicon).toBe("data:image/png;base64,icon");
    expect(replaySession(events).browserStates["front:tab"]!.favicon).toBeUndefined();
  });

  it("preserves calm error copy separately from technical diagnostics", () => {
    const view = replaySession(log([{
      kind: "runtime.error", code: "interrupted", message: "process EOF",
      user_message: "The agent stopped. Try again.", retryable: true,
    }]));
    expect(view.items[0]).toMatchObject({
      type: "error", message: "process EOF", userMessage: "The agent stopped. Try again.", retryable: true,
    });
  });
});

describe("conversation cards in the thread", () => {
  const info = (id: string, over: Partial<Card> = {}): Card => ({ ...pendingCard, id, ticket_id: null, interaction: "none", status: "done", ...over });

  it("places a card where its event lands and keeps its first position when it updates in place", () => {
    const view = replaySession(
      log([
        { kind: "assistant.final", text: "one" },
        { kind: "card.presented", card: info("present:p1", { title: "A" }) },
        { kind: "assistant.final", text: "two" },
        { kind: "card.presented", card: info("present:p1", { title: "B" }) },
      ]),
    );
    expect(view.items.map((item) => (item.type === "card" ? item.id : item.type))).toEqual(["assistant", "present:p1", "assistant"]);
    expect(view.cards["present:p1"]).toMatchObject({ firstSeq: 2, card: { title: "B" } });
  });

  it("a pending request never adds its own timeline row; its card is one", () => {
    const view = replaySession(log([{ kind: "ticket.opened", ticket }]));
    expect(view.items).toEqual([{ type: "card", id: "ticket:k1" }]);
    expect(view.cards["ticket:k1"]!.card.status).toBe("pending");
    expect(pendingTickets(view).map((pending) => pending.id)).toEqual(["k1"]);
  });

  it("a settled request's card moves to where it settled, and replays to the same state", () => {
    const settled = { ...pendingCard, status: "failed" as const, actions: [] };
    const events = log([
      { kind: "ticket.opened", ticket },
      { kind: "assistant.final", text: "waiting" },
      { kind: "ticket.rejected", ticket_id: "k1", card: settled },
    ]);
    const view = replaySession(events);
    expect(view.items.map((item) => (item.type === "card" ? item.id : item.type))).toEqual(["assistant", "ticket:k1"]);
    expect(view.cards["ticket:k1"]!.card.status).toBe("failed");
    expect(replaySession(events.slice(2), replaySession(events.slice(0, 2)))).toEqual(view);
  });

  it("a tool result patches a card by id instead of adding a second one", () => {
    const view = replaySession(
      log([
        { kind: "ticket.opened", ticket },
        { kind: "tool.ended", tool_call_id: "c9", result: { ok: true }, is_error: false, card: { ...pendingCard, status: "done", collapsed_summary: "Paid" } },
      ]),
    );
    expect(view.items.filter((item) => item.type === "card")).toHaveLength(1);
    expect(view.cards["ticket:k1"]!.card).toMatchObject({ status: "done", collapsed_summary: "Paid" });
  });
});
