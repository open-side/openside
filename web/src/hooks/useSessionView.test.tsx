import { renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { Event } from "../api/types";
import { replaySession } from "../state/session";
import { fakeClient, type Call } from "../test/fakeDaemon";
import { useSessionView } from "./useSessionView";

const ticket = {
  id: "k1",
  session_id: "s1",
  tool_call_id: "write-1",
  deferral: "approval",
  kind: "confirm",
  summary: "Allow write outside the workspace?",
  instructions: null,
  options: [],
  origin: { subagent_id: "sa1" },
  tool: { name: "write", args: { path: "/tmp/x" } },
  target: null,
  resume_when: null,
  created_at: "2026-01-01T00:00:00Z",
  expires_at: "2026-01-01T00:10:00Z",
  status: "pending",
  resolution: null,
};

// A session log with every surface the screen shows: queue, streaming text, tools, a
// subagent with progress and an approval, saved files, attachments and a visual.
const payloads: Record<string, unknown>[] = [
  { kind: "session.created", title: "Report", model: "provider:model", mode: "guard", workspace: null, incognito: false },
  {
    kind: "message.queued",
    message_id: "m1",
    text: "Summarize /research",
    mode: "steer",
    author_kind: "person",
    from_agent: null,
    attachments: [{ path: "attachments/a.png", kind: "image", mime: "image/png", name: "a.png", size: 9, sha256: "00" }],
  },
  { kind: "message.injected", message_id: "m1" },
  { kind: "turn.started" },
  { kind: "thinking.delta", text: "…" },
  { kind: "tool.started", tool_call_id: "d1", name: "delegate", args: { task: "Write the report" } },
  { kind: "subagent.started", subagent_id: "sa1", parent_tool_call_id: "d1", task: "Write the report", model: "provider:model", tools: ["write"] },
  { kind: "subagent.progress", subagent_id: "sa1", progress: { kind: "model_request" } },
  { kind: "ticket.opened", ticket },
  { kind: "subagent.lifecycle", subagent_id: "sa1", status: "waiting_human" },
  { kind: "message.queued", message_id: "m2", text: "also add a chart", mode: "queue", author_kind: "person", from_agent: null },
  { kind: "ticket.resolved", ticket_id: "k1", resolution: { approved: true } },
  { kind: "subagent.lifecycle", subagent_id: "sa1", status: "running" },
  { kind: "artifact.saved", path: "report.md", mime: "text/markdown", size: 42, tool: "write", tool_call_id: "write-1" },
  { kind: "subagent.ended", subagent_id: "sa1", status: "completed", reason: null },
  { kind: "tool.ended", tool_call_id: "d1", result: { output: "ok" }, is_error: false },
  { kind: "assistant.delta", text: "Here is the " },
  { kind: "assistant.delta", text: "chart" },
  { kind: "assistant.final", text: "Here is the chart\n```svg visual\n<svg/>\n```" },
  { kind: "message.completed", message_id: "m1", outcome: "completed", reason: null },
  { kind: "session.settled" },
];
const log = payloads.map((payload, index) => ({
  session_id: "s1",
  seq: index + 1,
  ts: "2026-01-01T00:00:00Z",
  run_id: null,
  ...payload,
})) as Event[];

function stream(events: Event[], end: boolean): Response {
  const encoder = new TextEncoder();
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      for (const event of events) controller.enqueue(encoder.encode(`id: ${event.seq}\ndata: ${JSON.stringify(event)}\n\n`));
      if (end) controller.close();
    },
  });
  return new Response(body, { status: 200, headers: { "Content-Type": "text/event-stream" } });
}

describe("useSessionView (PP-6)", () => {
  it("builds the same screen state live, across a dropped connection, as a pure replay of the log", async () => {
    const cursors: string[] = [];
    const { client } = fakeClient((call: Call) => {
      cursors.push(call.headers["Last-Event-ID"] ?? "");
      // First connection drops after event 9; the daemon then resends from an earlier
      // cursor (overlap) and the last connection stays open.
      if (cursors.length === 1) return stream(log.slice(0, 9), true);
      return stream(log.slice(5), false);
    });
    const { result, unmount } = renderHook(() => useSessionView(client, "s1"));
    await waitFor(() => expect(result.current.view.lastSeq).toBe(log.length), { timeout: 3000 });

    expect(cursors.slice(0, 2)).toEqual(["0", "9"]);
    expect(result.current.view).toEqual(replaySession(log));
    // The rebuilt state carries every surface the session screen renders.
    const view = result.current.view;
    expect(view.subagents.map((child) => [child.id, child.status, child.progress.length])).toEqual([["sa1", "completed", 1]]);
    expect(view.tickets.k1?.status).toBe("resolved");
    expect(view.artifacts.map((artifact) => artifact.path)).toEqual(["report.md"]);
    expect(view.queue.map((message) => message.id)).toEqual(["m2"]);
    expect(view.items[0]).toMatchObject({ type: "user", attachments: [{ path: "attachments/a.png" }] });
    expect(view.items.at(-1)).toMatchObject({ type: "assistant", final: true, finalSeq: 19 });
    expect(view.status).toBe("idle");
    unmount();
  });
});
