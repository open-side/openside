import { afterEach, describe, expect, it, vi } from "vitest";
import { createClient } from "./client";
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

  it("lets go of the connection on pagehide and resumes from the cursor on a back/forward restore", async () => {
    const signals: AbortSignal[] = [];
    const cursors: string[] = [];
    const fetchImpl = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      signals.push(init!.signal!);
      cursors.push((init!.headers as Record<string, string>)["Last-Event-ID"]!);
      // An open stream: one event, then nothing until the request is aborted.
      const body = new ReadableStream<Uint8Array>({
        start(controller) {
          controller.enqueue(new TextEncoder().encode(frame(event(signals.length))));
          init!.signal!.addEventListener("abort", () => controller.error(new DOMException("aborted", "AbortError")));
        },
      });
      return new Response(body, { status: 200, headers: { "Content-Type": "text/event-stream" } });
    });
    const client = createClient({ token: () => "t", fetch: fetchImpl as typeof fetch });
    const seen: number[] = [];
    const statuses: string[] = [];
    const stream = openSessionStream({ client, sessionId: "s1", onEvent: (e) => seen.push(e.seq), onStatus: (s) => statuses.push(s), retryDelayMs: () => 0 });
    await vi.waitFor(() => expect(seen).toEqual([1]));

    window.dispatchEvent(new PageTransitionEvent("pagehide", { persisted: true }));
    expect(signals[0]!.aborted).toBe(true);

    window.dispatchEvent(new PageTransitionEvent("pageshow", { persisted: true }));
    await vi.waitFor(() => expect(seen).toEqual([1, 2]));
    expect(cursors).toEqual(["0", "1"]);
    // A retry while hidden would have reported "reconnecting".
    expect(statuses).toEqual(["connecting", "open", "connecting", "open"]);

    stream.close();
    expect(signals[1]!.aborted).toBe(true);
    await vi.waitFor(() => expect(statuses.at(-1)).toBe("closed"));
    window.dispatchEvent(new PageTransitionEvent("pageshow", { persisted: true }));
    expect(fetchImpl).toHaveBeenCalledTimes(2);
  });
});

/** An SSE response that sends `first` and then nothing until the request is aborted. */
function silentStream(signal: AbortSignal, first: string[] = []): Response {
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      for (const chunk of first) controller.enqueue(new TextEncoder().encode(chunk));
      signal.addEventListener("abort", () => controller.error(new DOMException("aborted", "AbortError")));
    },
  });
  return new Response(body, { status: 200, headers: { "Content-Type": "text/event-stream" } });
}

function pollPage(events: Event[], lastSeq = events.at(-1)?.seq ?? 0): Response {
  return json(200, { events, last_seq: lastSeq });
}

describe("openSessionStream fallback to long-polling", () => {
  afterEach(() => vi.useRealTimers());

  /** A daemon whose SSE never delivers a byte after `sseEvents`, with `polls` answering the poll route in order. */
  function stalledDaemon(sseEvents: Event[], polls: Array<() => Response | Promise<Response>>) {
    const calls: Call[] = [];
    const fetchImpl = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(String(input), "http://daemon.test");
      calls.push({ method: init?.method ?? "GET", path: url.pathname + url.search, headers: (init?.headers ?? {}) as Record<string, string>, body: undefined });
      if (url.pathname.endsWith("/events/poll")) {
        return (polls.shift() ?? (() => new Promise<Response>(() => undefined)))();
      }
      return silentStream(init!.signal!, sseEvents.map(frame));
    });
    return { client: createClient({ token: () => "t", fetch: fetchImpl as typeof fetch }), calls };
  }

  it("switches to long-polling when SSE goes silent, resuming after the last delivered seq without duplicates or gaps", async () => {
    vi.useFakeTimers();
    const { client, calls } = stalledDaemon([event(1), event(2)], [
      () => pollPage([event(2), event(3), event(4)]),
      () => pollPage([event(5)]),
    ]);
    const seen: number[] = [];
    const stream = openSessionStream({ client, sessionId: "s1", onEvent: (e) => seen.push(e.seq), stallMs: 20_000, retryDelayMs: () => 0 });

    await vi.advanceTimersByTimeAsync(10_000);
    expect(seen).toEqual([1, 2]);
    expect(stream.transport).toBe("sse");

    await vi.advanceTimersByTimeAsync(20_000);
    expect(stream.transport).toBe("poll");
    expect(seen).toEqual([1, 2, 3, 4, 5]);
    const polls = calls.filter((call) => call.path.includes("/events/poll"));
    expect(polls.map((call) => new URL(call.path, "http://x").searchParams.get("after"))).toEqual(["2", "4", "5"]);
    expect(new URL(polls[0]!.path, "http://x").searchParams.get("wait")).toBe("20");
    expect(polls[0]!.headers.Authorization).toBe("Bearer t");
    stream.close();
  });

  it("does not count keep-alive comments as a stall", async () => {
    vi.useFakeTimers();
    let push: (text: string) => void = () => undefined;
    const fetchImpl = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      const body = new ReadableStream<Uint8Array>({
        start(controller) {
          push = (text) => controller.enqueue(new TextEncoder().encode(text));
          init!.signal!.addEventListener("abort", () => controller.error(new DOMException("aborted", "AbortError")));
        },
      });
      return new Response(body, { status: 200, headers: { "Content-Type": "text/event-stream" } });
    });
    const client = createClient({ token: () => "t", fetch: fetchImpl as typeof fetch });
    const seen: number[] = [];
    const stream = openSessionStream({ client, sessionId: "s1", onEvent: (e) => seen.push(e.seq), stallMs: 20_000, firstByteMs: 20_000 });
    for (let beat = 0; beat < 6; beat++) {
      await vi.advanceTimersByTimeAsync(15_000);
      push(": keep-alive\n\n");
    }
    push(frame(event(1)));
    await vi.advanceTimersByTimeAsync(1);
    expect(stream.transport).toBe("sse");
    expect(fetchImpl).toHaveBeenCalledTimes(1);
    expect(seen).toEqual([1]);
    stream.close();
  });

  it("advances its cursor past events the daemon filtered out", async () => {
    vi.useFakeTimers();
    const { client, calls } = stalledDaemon([], [() => pollPage([event(3)], 7), () => pollPage([], 7)]);
    const seen: number[] = [];
    const stream = openSessionStream({ client, sessionId: "s1", onEvent: (e) => seen.push(e.seq), stallMs: 1_000 });
    await vi.advanceTimersByTimeAsync(5_000);
    expect(seen).toEqual([3]);
    expect(stream.lastSeq).toBe(7);
    const afters = calls.filter((call) => call.path.includes("/events/poll")).map((call) => new URL(call.path, "http://x").searchParams.get("after"));
    expect(afters.slice(0, 2)).toEqual(["0", "7"]);
    stream.close();
  });

  it("does not spin when the daemon answers an empty poll immediately", async () => {
    vi.useFakeTimers();
    const { client, calls } = stalledDaemon([], Array.from({ length: 50 }, () => () => pollPage([], 0)));
    const stream = openSessionStream({ client, sessionId: "s1", onEvent: () => undefined, stallMs: 1_000 });
    await vi.advanceTimersByTimeAsync(1_000 + 5_000);
    const polls = calls.filter((call) => call.path.includes("/events/poll")).length;
    expect(polls).toBeGreaterThan(1);
    expect(polls).toBeLessThanOrEqual(6);
    stream.close();
  });

  it("tries SSE again after the probe interval and stays on it when it delivers", async () => {
    vi.useFakeTimers();
    let sseOpens = 0;
    const fetchImpl = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(String(input), "http://daemon.test");
      if (url.pathname.endsWith("/events/poll")) return new Promise<Response>((resolve) => setTimeout(() => resolve(pollPage([], 0)), 20_000));
      sseOpens += 1;
      return sseOpens === 1 ? silentStream(init!.signal!) : silentStream(init!.signal!, [frame(event(1))]);
    });
    const client = createClient({ token: () => "t", fetch: fetchImpl as typeof fetch });
    const seen: number[] = [];
    const stream = openSessionStream({ client, sessionId: "s1", onEvent: (e) => seen.push(e.seq), stallMs: 5_000, firstByteMs: 1_000, probeAfterMs: 60_000, retryDelayMs: () => 0 });
    await vi.advanceTimersByTimeAsync(2_000);
    expect(stream.transport).toBe("poll");
    // The probe happens at the first poll boundary after the interval; check before the quiet probe stream itself goes stale.
    await vi.advanceTimersByTimeAsync(63_000);
    expect(sseOpens).toBe(2);
    expect(stream.transport).toBe("sse");
    expect(seen).toEqual([1]);
    stream.close();
  });

  it("reports a missing session found while polling as fatal", async () => {
    vi.useFakeTimers();
    const { client } = stalledDaemon([], [() => json(404, { code: "not_found", message: "gone" })]);
    const errors: string[] = [];
    const statuses: string[] = [];
    openSessionStream({ client, sessionId: "s1", onEvent: () => undefined, onError: (e) => errors.push(e.code), onStatus: (s) => statuses.push(s), stallMs: 1_000 });
    await vi.advanceTimersByTimeAsync(2_000);
    expect(errors).toEqual(["not_found"]);
    expect(statuses.at(-1)).toBe("closed");
  });

  it("retries a failed poll with backoff and keeps the cursor", async () => {
    vi.useFakeTimers();
    const { client, calls } = stalledDaemon([event(1)], [() => json(503, { code: "busy", message: "busy" }), () => pollPage([event(2)])]);
    const seen: number[] = [];
    const stream = openSessionStream({ client, sessionId: "s1", onEvent: (e) => seen.push(e.seq), stallMs: 1_000, retryDelayMs: () => 100 });
    await vi.advanceTimersByTimeAsync(5_000);
    expect(seen).toEqual([1, 2]);
    const afters = calls.filter((call) => call.path.includes("/events/poll")).map((call) => new URL(call.path, "http://x").searchParams.get("after"));
    expect(afters.slice(0, 2)).toEqual(["1", "1"]);
    stream.close();
  });
});
