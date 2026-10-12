import { describe, expect, it, vi } from "vitest";
import { openSessionStream, SseParser } from "./sse";
import type { Event } from "./types";
import { fakeClient, json, type Call } from "../test/fakeDaemon";

function event(seq: number, text = `t${seq}`): Event {
  return { session_id: "s1", seq, ts: "2026-01-01T00:00:00Z", kind: "assistant.delta", text } as Event;
}

function frame(e: Event): string {
  return `id: ${e.seq}\ndata: ${JSON.stringify(e)}\n\n`;
}

function streamResponse(chunks: string[]): Response {
  const encoder = new TextEncoder();
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      for (const chunk of chunks) controller.enqueue(encoder.encode(chunk));
      controller.close();
    },
  });
  return new Response(body, { status: 200, headers: { "Content-Type": "text/event-stream" } });
}

describe("SseParser", () => {
  it("parses frames split across arbitrary chunk boundaries", () => {
    const parser = new SseParser();
    const input = ": keep-alive\n\nid: 7\nevent: update\ndata: {\"a\":\ndata: 1}\n\nid: 8\r\ndata: x\r\n\r\n";
    const out = [];
    for (const char of input) out.push(...parser.push(char));
    expect(out).toEqual([
      { id: "7", event: "update", data: '{"a":\n1}' },
      { id: "8", event: "message", data: "x" },
    ]);
  });

  it("ignores comments and blocks without data", () => {
    const parser = new SseParser();
    expect(parser.push(":ping\n\nid: 3\n\n")).toEqual([]);
    expect(parser.push("data: y\n\n")).toEqual([{ id: "3", event: "message", data: "y" }]);
  });

  it("keeps a CRLF split by an empty decoded chunk as one line break", () => {
    const parser = new SseParser();
    expect(parser.push("data: a\r")).toEqual([]);
    expect(parser.push("")).toEqual([]);
    expect(parser.push("\ndata: b\r\n\r\n")).toEqual([{ id: undefined, event: "message", data: "a\nb" }]);
  });
});

describe("openSessionStream", () => {
  it("resumes with Last-Event-ID after a drop and never delivers a duplicate seq", async () => {
    const responses = [
      () => streamResponse([frame(event(1)), frame(event(2)).slice(0, 10), frame(event(2)).slice(10)]),
      () => streamResponse([frame(event(2)), frame(event(1)), frame(event(3))]),
      () => Promise.withResolvers<Response>().promise,
    ];
    const { client, calls } = fakeClient(() => (responses.shift() ?? responses[0]!)());
    const seen: number[] = [];
    const stream = openSessionStream({
      client,
      sessionId: "s1",
      onEvent: (e) => seen.push(e.seq),
      retryDelayMs: () => 0,
    });
    await vi.waitFor(() => expect(calls.length).toBeGreaterThanOrEqual(3));
    stream.close();
    expect(seen).toEqual([1, 2, 3]);
    expect(stream.lastSeq).toBe(3);
    const lastIds = calls.map((call: Call) => call.headers["Last-Event-ID"]);
    expect(lastIds).toEqual(["0", "2", "3"]);
    expect(calls[0]!.headers.Authorization).toBe("Bearer test-token");
    expect(calls[0]!.path).toBe("/v1/sessions/s1/events");
  });

  it("starts after a given cursor and stops on fatal auth errors", async () => {
    const { client, calls } = fakeClient(() => json(401, { code: "unauthorized", message: "unauthorized" }));
    const errors: string[] = [];
    const statuses: string[] = [];
    openSessionStream({
      client,
      sessionId: "s1",
      after: 41,
      onEvent: () => undefined,
      onError: (error) => errors.push(error.code),
      onStatus: (status) => statuses.push(status),
      retryDelayMs: () => 0,
    });
    await vi.waitFor(() => expect(statuses).toContain("closed"));
    expect(calls).toHaveLength(1);
    expect(calls[0]!.headers["Last-Event-ID"]).toBe("41");
    expect(errors).toEqual(["unauthorized"]);
  });

  it("retries transient server errors", async () => {
    let attempt = 0;
    const { client, calls } = fakeClient(() =>
      ++attempt === 1 ? json(503, { code: "busy", message: "busy" }) : streamResponse([frame(event(1))]),
    );
    const seen: number[] = [];
    const stream = openSessionStream({ client, sessionId: "s1", onEvent: (e) => seen.push(e.seq), retryDelayMs: () => 0 });
    await vi.waitFor(() => expect(seen).toHaveLength(1));
    stream.close();
    expect(calls.length).toBeGreaterThanOrEqual(2);
  });
});
