/*
 * HTTP/1.1 over one relay stream: one request per stream (`Connection: close`), the response
 * body streamed as it arrives so SSE works. The daemon serves these streams with the same
 * router as its loopback listener, so the bearer token in the request is still required.
 */
import type { MuxStream } from "./mux";

const encoder = new TextEncoder();
const decoder = new TextDecoder();
const MAX_HEAD = 64 * 1024;
const MAX_HEADERS = 128;

/** Buffered reader over a relay stream. */
export class StreamReader {
  private buffer = new Uint8Array();

  constructor(readonly stream: MuxStream) {}

  private async fill(): Promise<boolean> {
    const chunk = await this.stream.read();
    if (!chunk) return false;
    const next = new Uint8Array(this.buffer.length + chunk.length);
    next.set(this.buffer);
    next.set(chunk, this.buffer.length);
    this.buffer = next;
    return true;
  }

  /** One CRLF-terminated line without the CRLF; `null` at end of stream. */
  async line(maxBytes = MAX_HEAD): Promise<string | null> {
    for (;;) {
      const end = this.buffer.findIndex((byte, i) => byte === 13 && this.buffer[i + 1] === 10);
      if (end >= 0) {
        if (end + 2 > maxBytes) throw new Error("response header too large");
        const text = decoder.decode(this.buffer.subarray(0, end));
        this.buffer = this.buffer.slice(end + 2);
        return text;
      }
      if (this.buffer.length > maxBytes) throw new Error("response header too large");
      if (!(await this.fill())) return null;
    }
  }

  async exact(n: number): Promise<Uint8Array> {
    while (this.buffer.length < n) {
      if (!(await this.fill())) throw new Error("relay stream ended early");
    }
    const out = this.buffer.slice(0, n);
    this.buffer = this.buffer.slice(n);
    return out;
  }

  /** Whatever is buffered, else the next chunk; `null` at end of stream. */
  async some(): Promise<Uint8Array | null> {
    if (this.buffer.length === 0 && !(await this.fill())) return null;
    const out = this.buffer;
    this.buffer = new Uint8Array();
    return out;
  }

  /** Put bytes back in front of the buffer. */
  unshift(bytes: Uint8Array): void {
    const next = new Uint8Array(bytes.length + this.buffer.length);
    next.set(bytes);
    next.set(this.buffer, bytes.length);
    this.buffer = next;
  }
}

export interface ResponseHead {
  status: number;
  statusText: string;
  headers: Headers;
}

export async function writeRequest(
  stream: MuxStream,
  method: string,
  target: string,
  host: string,
  headers: Headers,
  body: Uint8Array | null,
): Promise<void> {
  const lines = [`${method} ${target} HTTP/1.1`, `Host: ${host}`];
  headers.forEach((value, name) => {
    if (!["host", "connection", "content-length", "transfer-encoding"].includes(name)) lines.push(`${name}: ${value}`);
  });
  lines.push(headers.has("upgrade") ? "Connection: Upgrade" : "Connection: close");
  if (body && body.length > 0) lines.push(`Content-Length: ${body.length}`);
  else if (!["GET", "HEAD"].includes(method)) lines.push("Content-Length: 0");
  await stream.write(encoder.encode(lines.join("\r\n") + "\r\n\r\n"));
  if (body && body.length > 0) await stream.write(body);
}

export async function readHead(reader: StreamReader): Promise<ResponseHead> {
  const statusLine = await reader.line();
  const match = statusLine?.match(/^HTTP\/1\.[01] (\d{3})(?: (.*))?$/);
  if (!match) throw new Error("malformed HTTP response");
  const headers = await readHeaders(reader, MAX_HEAD - encoder.encode(statusLine!).length - 2);
  return { status: Number(match[1]), statusText: match[2] ?? "", headers };
}

async function readHeaders(reader: StreamReader, remaining = MAX_HEAD): Promise<Headers> {
  const headers = new Headers();
  let count = 0;
  for (;;) {
    const line = await reader.line(remaining);
    if (line === null) throw new Error("relay stream ended in the response header");
    remaining -= encoder.encode(line).length + 2;
    if (remaining < 0) throw new Error("response header too large");
    if (line === "") break;
    if (++count > MAX_HEADERS) throw new Error("too many response headers");
    const colon = line.indexOf(":");
    if (colon > 0) headers.append(line.slice(0, colon).trim(), line.slice(colon + 1).trim());
  }
  return headers;
}

/** The response body as it arrives: chunked, sized, or until the daemon finishes. */
function bodyStream(reader: StreamReader, head: ResponseHead): ReadableStream<Uint8Array> {
  const chunked = head.headers.get("transfer-encoding")?.toLowerCase().includes("chunked") ?? false;
  const length = head.headers.get("content-length");
  let remaining = length === null ? Infinity : Number(length);
  let chunkLeft = 0;
  let done = false;
  return new ReadableStream<Uint8Array>({
    async pull(controller) {
      try {
        if (done || remaining === 0) {
          controller.close();
          reader.stream.end();
          return;
        }
        if (chunked) {
          if (chunkLeft === 0) {
            const size = await reader.line();
            if (size === null) throw new Error("relay stream ended in a chunked body");
            chunkLeft = parseInt(size.split(";")[0] ?? "", 16);
            if (!Number.isFinite(chunkLeft)) throw new Error("malformed chunk size");
            if (chunkLeft === 0) {
              await readHeaders(reader);
              done = true;
              controller.close();
              reader.stream.end();
              return;
            }
          }
          const data = await reader.some();
          if (!data) throw new Error("relay stream ended in a chunk");
          const take = Math.min(chunkLeft, data.length);
          chunkLeft -= take;
          // Return the excess (next chunk header) to the reader.
          if (take < data.length) reader.unshift(data.subarray(take));
          if (chunkLeft === 0) await reader.exact(2);
          controller.enqueue(data.slice(0, take));
          return;
        }
        const data = await reader.some();
        if (!data) {
          if (Number.isFinite(remaining)) throw new Error("relay stream ended early");
          done = true;
          controller.close();
          reader.stream.end();
          return;
        }
        const take = Math.min(remaining, data.length);
        remaining -= take;
        controller.enqueue(data.slice(0, take));
      } catch (error) {
        reader.stream.abort();
        controller.error(error);
      }
    },
    cancel() {
      reader.stream.abort();
    },
  });
}

const NULL_BODY = [101, 103, 204, 205, 304];

export async function exchange(
  stream: MuxStream,
  request: { method: string; target: string; host: string; headers: Headers; body: Uint8Array | null },
  signal?: AbortSignal | null,
): Promise<Response> {
  const onAbort = () => stream.abort();
  signal?.addEventListener("abort", onAbort, { once: true });
  try {
    await writeRequest(stream, request.method, request.target, request.host, request.headers, request.body);
    const reader = new StreamReader(stream);
    const head = await readHead(reader);
    const empty = NULL_BODY.includes(head.status) || request.method === "HEAD";
    if (empty) stream.end();
    return new Response(empty ? null : bodyStream(reader, head), {
      status: head.status,
      statusText: head.statusText,
      headers: head.headers,
    });
  } catch (error) {
    stream.abort();
    if (signal?.aborted) throw signal.reason ?? new DOMException("Aborted", "AbortError");
    throw new TypeError(`relay request failed: ${error instanceof Error ? error.message : String(error)}`);
  }
}
