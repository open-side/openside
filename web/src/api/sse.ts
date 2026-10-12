import { ApiError, toApiError, type Client } from "./client";
import { whilePageShown } from "./pageLifecycle";
import type { Event } from "./types";

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
}

export interface SessionStream {
  close(): void;
  /** Highest delivered sequence; used as `Last-Event-ID` on reconnect. */
  readonly lastSeq: number;
}

const FATAL_STATUS: Record<number, true> = { 401: true, 403: true, 404: true };

/** Exponential backoff from 250 ms, capped at 10 s. */
const defaultDelay = (attempt: number) => Math.min(10_000, 250 * 2 ** Math.min(attempt, 6));

/**
 * Follow a session's durable event log. Uses fetch rather than EventSource because
 * EventSource cannot send the Authorization header. Reconnects resume with
 * `Last-Event-ID` and drop any event at or below the last delivered `seq`, so
 * replays never duplicate or reorder events. The connection is dropped while the page sits
 * in the back/forward cache and resumes from the cursor when it returns (`whilePageShown`).
 */
export function openSessionStream(options: SessionStreamOptions): SessionStream {
  const delay = options.retryDelayMs ?? defaultDelay;
  let lastSeq = options.after ?? 0;
  let closed = false;

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

  async function once(signal: AbortSignal): Promise<void> {
    const url = `${options.client.baseUrl}/v1/sessions/${encodeURIComponent(options.sessionId)}/events`;
    const response = await options.client.fetch(url, {
      headers: options.client.headers({ Accept: "text/event-stream", "Last-Event-ID": String(lastSeq) }),
      signal,
      cache: "no-store",
    });
    if (!response.ok) throw await toApiError(response);
    if (!response.body) throw new ApiError(0, "network_error");
    setStatus("open");
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    const parser = new SseParser();
    for (;;) {
      const { value, done } = await reader.read();
      if (done) return;
      for (const message of parser.push(decoder.decode(value, { stream: true }))) {
        let event: Event;
        try {
          event = JSON.parse(message.data) as Event;
        } catch {
          continue;
        }
        if (typeof event.seq !== "number" || event.seq <= lastSeq) continue;
        lastSeq = event.seq;
        options.onEvent(event);
        if (closed || signal.aborted) return;
      }
    }
  }

  async function follow(signal: AbortSignal): Promise<void> {
    let attempt = 0;
    setStatus("connecting");
    while (!closed && !signal.aborted) {
      try {
        await once(signal);
        attempt = 0;
      } catch (error) {
        if (closed || signal.aborted) break;
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
  };
  return stream;
}
