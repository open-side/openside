import { describe, expect, it } from "vitest";
import type { Event, Ticket } from "../api/types";
import { reduceSession, replaySession } from "./session";

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

const queued = (id: string, mode = "queue") => ({ kind: "message.queued", message_id: id, text: id, mode });

describe("message delivery state", () => {
  it("walks sent → read → working → first output for a message sent to an idle session", () => {
    const events = log([
      queued("m1"),
      { kind: "message.injected", message_id: "m1" },
      { kind: "turn.started" },
      { kind: "assistant.delta", text: "Hi" },
    ]);
    const states = events.map((_, i) => {
      const view = replaySession(events.slice(0, i + 1));
      return { delivering: view.delivering, userItems: view.items.filter((item) => item.type === "user").length, engaged: view.engaged };
    });
    expect(states).toEqual([
      { delivering: "m1", userItems: 0, engaged: false }, // Sent
      { delivering: null, userItems: 1, engaged: false }, // Read
      { delivering: null, userItems: 1, engaged: true }, // working: typing dots
      { delivering: null, userItems: 1, engaged: true }, // output has arrived; the log shows it
    ]);
  });

  it("keeps a message sent during a run Queued until it is dispatched", () => {
    const view = replaySession(log([queued("m1"), { kind: "message.injected", message_id: "m1" }, { kind: "turn.started" }, queued("m2")]));
    expect(view.delivering).toBeNull();
    expect(view.queue.map((message) => message.id)).toEqual(["m2"]);
  });

  it("does not call a message Sent while a pending ticket holds the queue", () => {
    const ticket = { id: "k1", status: "pending", origin: { subagent_id: null } } as unknown as Ticket;
    const view = replaySession(log([{ kind: "ticket.opened", ticket }, queued("m1")]));
    expect(view.delivering).toBeNull();
    expect(view.queue).toHaveLength(1);
  });

  it("clears the sent state when the message is dropped before dispatch", () => {
    const view = replaySession(log([queued("m1"), { kind: "message.dropped", message_id: "m1", reason: "cancelled" }]));
    expect(view.delivering).toBeNull();
    expect(view.status).toBe("idle");
  });

  it("resets engagement when the run settles, so the next message starts at Sent again", () => {
    const view = replaySession(
      log([
        queued("m1"),
        { kind: "message.injected", message_id: "m1" },
        { kind: "turn.started" },
        { kind: "message.completed", message_id: "m1", outcome: "completed" },
        { kind: "session.settled" },
        queued("m2"),
      ]),
    );
    expect(view.engaged).toBe(false);
    expect(view.delivering).toBe("m2");
  });

  it("rebuilds the same state from a reload (full replay) as from live events", () => {
    const events = log([queued("m1"), { kind: "message.injected", message_id: "m1" }, { kind: "turn.started" }, { kind: "thinking.delta", text: "…" }]);
    const live = events.reduce(reduceSession, replaySession([]));
    expect(replaySession(events)).toEqual(live);
  });
});
