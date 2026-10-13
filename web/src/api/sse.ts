import { ApiError, toApiError, type Client } from "./client";
import { whilePageShown } from "./pageLifecycle";
import type { Event, EventPoll } from "./types";

/** One dispatched SSE message (WHATWG event-stream framing). */
export interface SseMessage {
  id: string | undefined;
  event: string;
  data: string;
}

/**
 * Incremental `text/event-stream` parser. Chunks may split anywhere, including
 * inside a CRLF pair; comments (keep-alives) and empty-data blocks are dropped.
 */
export class SseParser {
  private buffer = "";
  private data: string[] = [];
  private id: string | undefined;
  private event = "";
  private pendingCr = false;

  push(chunk: string): SseMessage[] {
    // An empty decode (split multi-byte char) must not forget a trailing CR.
    if (!chunk) return [];
    let text = chunk;
    if (this.pendingCr && text.startsWith("\n")) text = text.slice(1);
    this.pendingCr = text.endsWith("\r");
    this.buffer += text;
    const out: SseMessage[] = [];
    const lines = this.buffer.split(/\r\n|\r|\n/);
    this.buffer = lines.pop() ?? "";
    for (const line of lines) this.line(line, out);
    return out;
  }

  private line(line: string, out: SseMessage[]): void {
    if (line === "") {
      if (this.data.length) out.push({ id: this.id, event: this.event || "message", data: this.data.join("\n") });
      this.data = [];
      this.event = "";
      return;
    }
    if (line.startsWith(":")) return;
    const colon = line.indexOf(":");
    const field = colon === -1 ? line : line.slice(0, colon);
    let value = colon === -1 ? "" : line.slice(colon + 1);
    if (value.startsWith(" ")) value = value.slice(1);
    if (field === "data") this.data.push(value);
    else if (field === "id" && !value.includes("\0")) this.id = value;
    else if (field === "event") this.event = value;
  }
}

export type StreamStatus = "connecting" | "open" | "reconnecting" | "closed";

/** `sse` is the normal live stream; `poll` is the long-poll fallback for connections that buffer or stall SSE. */
export type StreamTransport = "sse" | "poll";

export interface SessionStreamOptions {
  client: Client;
  sessionId: string;
  /** Last seen sequence; only later events are delivered. */
  after?: number;
  onEvent: (event: Event) => void;
  onStatus?: (status: StreamStatus) => void;
  /** Fatal errors (auth, missing session) end the stream; transient ones retry. */
  onError?: (error: ApiError) => void;
  retryDelayMs?: (attempt: number) => number;
  /** Silence (no bytes at all, keep-alives included) after which an open SSE stream counts as stalled. */
  stallMs?: number;
  /** How long a new SSE connection may take to deliver its first byte (the daemon sends one at once). */
  firstByteMs?: number;
  /** How long a long-poll request may be held by the daemon. */
  pollWaitSecs?: number;
  /** How long to stay on long-polling before trying SSE once more. */
  probeAfterMs?: number;
}

export interface SessionStream {
  close(): void;
  /** Highest delivered sequence; used as the resume cursor on reconnect. */
  readonly lastSeq: number;
  /** Transport the stream is following now. */
  readonly transport: StreamTransport;
}

/** `GET /v1/sessions/{id}/events/poll`: events after a cursor, held until one exists or `wait` runs out. */
export async function pollSessionEvents(
  client: Client,
  sessionId: string,
  after: number,
  options: { wait: number; signal?: AbortSignal },
): Promise<EventPoll> {
  const query = new URLSearchParams({ after: String(after), wait: String(options.wait) });
  const url = `${client.baseUrl}/v1/sessions/${encodeURIComponent(sessionId)}/events/poll?${query}`;
  const response = await client.fetch(url, { headers: client.headers(), signal: options.signal, cache: "no-store" });
  if (!response.ok) throw await toApiError(response);
  return (await response.json()) as EventPoll;
}

const FATAL_STATUS: Record<number, true> = { 401: true, 403: true, 404: true };

/** Exponential backoff from 250 ms, capped at 10 s. */
const defaultDelay = (attempt: number) => Math.min(10_000, 250 * 2 ** Math.min(attempt, 6));

/** The daemon sends a keep-alive every 15 s, so a longer silence means something is holding the stream. */
const DEFAULT_STALL_MS = 20_000;
/** The daemon writes a comment as soon as the stream opens; a tunnel that buffers shows up here, not after 15 s. */
const DEFAULT_FIRST_BYTE_MS = 6_000;
const DEFAULT_POLL_WAIT_SECS = 20;
const DEFAULT_PROBE_AFTER_MS = 10 * 60_000;
const MIN_EMPTY_POLL_MS = 1_000;

class StallError extends Error {}

/**
 * Follow a session's durable event log. Uses fetch rather than EventSource because
 * EventSource cannot send the Authorization header. Reconnects resume with
 * `Last-Event-ID` and drop any event at or below the last delivered `seq`, so
 * replays never duplicate or reorder events. The connection is dropped while the page sits
 * in the back/forward cache and resumes from the cursor when it returns (`whilePageShown`).
 *
 * Some paths (proxies that buffer responses, Cloudflare quick tunnels) never deliver SSE
 * frames. When nothing arrives within `stallMs` the stream switches to long-polling the same
 * log by sequence, with the same cursor and duplicate filter, and tries SSE again after
 * `probeAfterMs`. Every consumer sees one stream whichever transport carries it.
 */
export function openSessionStream(options: SessionStreamOptions): SessionStream {
  const delay = options.retryDelayMs ?? defaultDelay;
  const stallMs = options.stallMs ?? DEFAULT_STALL_MS;
  const firstByteMs = options.firstByteMs ?? Math.min(DEFAULT_FIRST_BYTE_MS, stallMs);
  const pollWaitSecs = options.pollWaitSecs ?? DEFAULT_POLL_WAIT_SECS;
  const probeAfterMs = options.probeAfterMs ?? DEFAULT_PROBE_AFTER_MS;
  let lastSeq = options.after ?? 0;
  let closed = false;
  let transport: StreamTransport = "sse";
  let pollingSince = 0;

  const setStatus = (status: StreamStatus) => options.onStatus?.(status);

  const sleep = (ms: number, signal: AbortSignal) => {
    const { promise, resolve } = Promise.withResolvers<void>();
    const timer = setTimeout(resolve, ms);
    signal.addEventListener("abort", () => {
      clearTimeout(timer);
      resolve();
    });
    return promise;
  };

  function deliver(event: Event): void {
    if (typeof event.seq !== "number" || event.seq <= lastSeq) return;
    lastSeq = event.seq;
    options.onEvent(event);
  }

  /** Runs `body` with its own abort signal, rejecting with `StallError` after a silence (`ms`, default `stallMs`) without `touch()`. */
  async function watched<T>(signal: AbortSignal, body: (signal: AbortSignal, touch: (ms?: number) => void) => Promise<T>): Promise<T> {
    const controller = new AbortController();
    const onAbort = () => controller.abort();
    signal.addEventListener("abort", onAbort);
    const stalled = Promise.withResolvers<never>();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const touch = (ms = stallMs) => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        controller.abort();
        stalled.reject(new StallError());
      }, ms);
    };
    touch(firstByteMs);
    // The race loser is aborted, never left to reject unhandled.
    stalled.promise.catch(() => undefined);
    try {
      return await Promise.race([body(controller.signal, touch), stalled.promise]);
    } finally {
      clearTimeout(timer);
      signal.removeEventListener("abort", onAbort);
    }
  }

  async function onceSse(signal: AbortSignal): Promise<void> {
    const url = `${options.client.baseUrl}/v1/sessions/${encodeURIComponent(options.sessionId)}/events`;
    await watched(signal, async (guarded, touch) => {
      const response = await options.client.fetch(url, {
        headers: options.client.headers({ Accept: "text/event-stream", "Last-Event-ID": String(lastSeq) }),
        signal: guarded,
        cache: "no-store",
      });
      if (!response.ok) throw await toApiError(response);
      if (!response.body) throw new ApiError(0, "network_error");
      touch(firstByteMs);
      setStatus("open");
      const reader = response.body.getReader();
      const decoder = new TextDecoder();
      const parser = new SseParser();
      for (;;) {
        const { value, done } = await reader.read();
        if (done) return;
        touch();
        for (const message of parser.push(decoder.decode(value, { stream: true }))) {
          let event: Event;
          try {
            event = JSON.parse(message.data) as Event;
          } catch {
            continue;
          }
          deliver(event);
          if (closed || guarded.aborted) return;
        }
      }
    });
  }

  /** Returns how many events arrived. */
  async function oncePoll(signal: AbortSignal): Promise<number> {
    // The daemon answers within `wait`; a response that takes far longer is being held on the way.
    const controller = new AbortController();
    const onAbort = () => controller.abort();
    signal.addEventListener("abort", onAbort);
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      controller.abort();
    }, pollWaitSecs * 1000 + stallMs);
    try {
      return await pollRequest(controller.signal);
    } catch (error) {
      if (timedOut && !signal.aborted) throw new ApiError(0, "network_error");
      throw error;
    } finally {
      clearTimeout(timer);
      signal.removeEventListener("abort", onAbort);
    }
  }

  async function pollRequest(signal: AbortSignal): Promise<number> {
    const page = await pollSessionEvents(options.client, options.sessionId, lastSeq, { wait: pollWaitSecs, signal });
    setStatus("open");
    const before = lastSeq;
    for (const event of page.events) {
      deliver(event);
      if (closed || signal.aborted) return page.events.length;
    }
    // The daemon's cursor also moves past events it filtered out; staying behind it would refetch them forever.
    if (page.last_seq > lastSeq) lastSeq = page.last_seq;
    return lastSeq === before ? 0 : Math.max(page.events.length, 1);
  }

  async function follow(signal: AbortSignal): Promise<void> {
    let attempt = 0;
    setStatus("connecting");
    while (!closed && !signal.aborted) {
      try {
        if (transport === "poll" && Date.now() - pollingSince >= probeAfterMs) transport = "sse";
        if (transport === "sse") {
          await onceSse(signal);
        } else {
          const started = Date.now();
          const received = await oncePoll(signal);
          attempt = 0;
          // A held poll that ran out of events is a healthy round trip: ask again at once. An empty
          // answer that came back immediately means the daemon is not holding; do not spin.
          if (received === 0 && Date.now() - started < MIN_EMPTY_POLL_MS && !closed && !signal.aborted) {
            await sleep(MIN_EMPTY_POLL_MS, signal);
          }
          continue;
        }
        attempt = 0;
      } catch (error) {
        if (closed || signal.aborted) break;
        if (error instanceof StallError) {
          transport = "poll";
          pollingSince = Date.now();
          attempt = 0;
          continue;
        }
        const apiError = error instanceof ApiError ? error : new ApiError(0, "network_error");
        if (FATAL_STATUS[apiError.status]) {
          options.onError?.(apiError);
          stream.close();
          break;
        }
        attempt += 1;
      }
      if (closed || signal.aborted) break;
      setStatus("reconnecting");
      await sleep(delay(attempt), signal);
    }
    if (closed) setStatus("closed");
  }

  const stop = whilePageShown(() => {
    const controller = new AbortController();
    void follow(controller.signal);
    return () => controller.abort();
  });

  const stream: SessionStream = {
    close() {
      if (closed) return;
      closed = true;
      stop();
    },
    get lastSeq() {
      return lastSeq;
    },
    get transport() {
      return transport;
    },
  };
  return stream;
}
