/*
 * A WebSocket (RFC 6455 client side) carried on one relay stream, exposing the subset of the
 * browser `WebSocket` interface the takeover and live-preview views use. The upgrade is an
 * ordinary HTTP/1.1 request on the stream; the daemon's Origin==Host check passes because the
 * page origin is the relay origin and `Host` is the relay host.
 */
import type { MuxStream } from "./mux";
import { readHead, StreamReader, writeRequest } from "./http1";

const encoder = new TextEncoder();
const decoder = new TextDecoder();
const GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const MAX_MESSAGE = 16 * 1024 * 1024;
const MAX_FRAGMENTS = 1024;

function base64(bytes: Uint8Array): string {
  return btoa(String.fromCharCode(...bytes));
}

export function encodeFrame(opcode: number, payload: Uint8Array): Uint8Array {
  const mask = crypto.getRandomValues(new Uint8Array(4));
  const length = payload.length;
  const extended = length < 126 ? 0 : length < 65536 ? 2 : 8;
  const out = new Uint8Array(2 + extended + 4 + length);
  const view = new DataView(out.buffer);
  out[0] = 0x80 | opcode;
  if (extended === 0) out[1] = 0x80 | length;
  else if (extended === 2) {
    out[1] = 0x80 | 126;
    view.setUint16(2, length, false);
  } else {
    out[1] = 0x80 | 127;
    view.setBigUint64(2, BigInt(length), false);
  }
  out.set(mask, 2 + extended);
  const offset = 2 + extended + 4;
  for (let i = 0; i < length; i++) out[offset + i] = (payload[i] ?? 0) ^ (mask[i % 4] ?? 0);
  return out;
}

type Handler<E extends Event> = ((event: E) => void) | null;

export class RelayWebSocket extends EventTarget {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSING = 2;
  static readonly CLOSED = 3;
  readonly CONNECTING = 0;
  readonly OPEN = 1;
  readonly CLOSING = 2;
  readonly CLOSED = 3;

  readyState = 0;
  protocol = "";
  binaryType: BinaryType = "blob";
  readonly extensions = "";
  readonly bufferedAmount = 0;
  onopen: Handler<Event> = null;
  onmessage: Handler<MessageEvent> = null;
  onclose: Handler<CloseEvent> = null;
  onerror: Handler<Event> = null;
  private stream: MuxStream | null = null;
  private writes: Promise<void> = Promise.resolve();

  constructor(
    readonly url: string,
    protocols: string[],
    open: () => Promise<MuxStream>,
  ) {
    super();
    void this.connect(protocols, open);
  }

  private fire<E extends Event>(event: E, handler: Handler<E>): void {
    handler?.call(this, event);
    this.dispatchEvent(event);
  }

  private async connect(protocols: string[], open: () => Promise<MuxStream>): Promise<void> {
    try {
      const stream = await open();
      this.stream = stream;
      if (this.readyState !== RelayWebSocket.CONNECTING) {
        stream.abort();
        return;
      }
      const target = new URL(this.url);
      const key = base64(crypto.getRandomValues(new Uint8Array(16)));
      const headers = new Headers({
        Upgrade: "websocket",
        "Sec-WebSocket-Key": key,
        "Sec-WebSocket-Version": "13",
        Origin: `${target.protocol === "wss:" ? "https:" : "http:"}//${target.host}`,
      });
      if (protocols.length) headers.set("Sec-WebSocket-Protocol", protocols.join(", "));
      await writeRequest(stream, "GET", target.pathname + target.search, target.host, headers, null);
      const reader = new StreamReader(stream);
      const head = await readHead(reader);
      const expected = base64(
        new Uint8Array(await crypto.subtle.digest("SHA-1", encoder.encode(key + GUID) as BufferSource)),
      );
      if (head.status !== 101 || head.headers.get("sec-websocket-accept") !== expected) {
        throw new Error(`upgrade refused (HTTP ${head.status})`);
      }
      this.protocol = head.headers.get("sec-websocket-protocol") ?? "";
      if (this.readyState !== RelayWebSocket.CONNECTING) {
        stream.abort();
        return;
      }
      this.readyState = RelayWebSocket.OPEN;
      this.fire(new Event("open"), this.onopen);
      await this.receive(reader);
    } catch {
      this.fail();
    }
  }

  private async receive(reader: StreamReader): Promise<void> {
    let fragments: Uint8Array[] = [];
    let fragmentOpcode = 0;
    let fragmentBytes = 0;
    for (;;) {
      const [first, second] = await reader.exact(2);
      const fin = ((first ?? 0) & 0x80) !== 0;
      const opcode = (first ?? 0) & 0x0f;
      let length = (second ?? 0) & 0x7f;
      if (((second ?? 0) & 0x80) !== 0) throw new Error("masked server frame");
      if (length === 126) length = new DataView((await reader.exact(2)).buffer).getUint16(0, false);
      else if (length === 127) length = Number(new DataView((await reader.exact(8)).buffer).getBigUint64(0, false));
      if (length > MAX_MESSAGE) throw new Error("message too large");
      if (opcode >= 8) {
        if (!fin || length > 125 || ![8, 9, 10].includes(opcode)) throw new Error("invalid control frame");
      } else {
        if (opcode === 0 ? fragmentOpcode === 0 : ![1, 2].includes(opcode) || fragmentOpcode !== 0) {
          throw new Error("invalid fragmented message");
        }
        if (fragmentBytes + length > MAX_MESSAGE || fragments.length >= MAX_FRAGMENTS) {
          throw new Error("fragmented message too large");
        }
      }
      const payload = await reader.exact(length);
      if (opcode === 8) {
        const code = payload.length >= 2 ? new DataView(payload.buffer).getUint16(0, false) : 1005;
        const reason = decoder.decode(payload.subarray(2));
        if (this.readyState === RelayWebSocket.OPEN) this.write(8, payload.subarray(0, 2));
        this.finish(code, reason, true);
        return;
      }
      if (opcode === 9) {
        this.write(10, payload);
        continue;
      }
      if (opcode === 10) continue;
      if (opcode !== 0) {
        fragmentBytes = 0;
        fragmentOpcode = opcode;
      }
      fragments.push(payload);
      fragmentBytes += payload.length;
      if (!fin) continue;
      const size = fragmentBytes;
      const message = new Uint8Array(size);
      let offset = 0;
      for (const part of fragments) {
        message.set(part, offset);
        offset += part.length;
      }
      fragments = [];
      fragmentBytes = 0;
      if (this.readyState !== RelayWebSocket.OPEN) continue;
      const data =
        fragmentOpcode === 1
          ? decoder.decode(message)
          : this.binaryType === "arraybuffer"
            ? message.buffer
            : new Blob([message]);
      this.fire(new MessageEvent("message", { data }), this.onmessage);
      fragmentOpcode = 0;
    }
  }

  private write(opcode: number, payload: Uint8Array): void {
    const stream = this.stream;
    if (!stream) return;
    const frame = encodeFrame(opcode, payload);
    this.writes = this.writes.then(() => stream.write(frame)).catch(() => this.fail());
  }

  send(data: string | ArrayBufferLike | ArrayBufferView | Blob): void {
    if (this.readyState === RelayWebSocket.CONNECTING) throw new DOMException("WebSocket is not open", "InvalidStateError");
    if (this.readyState !== RelayWebSocket.OPEN) return;
    if (typeof data === "string") this.write(1, encoder.encode(data));
    else if (data instanceof Blob) void data.arrayBuffer().then((buffer) => this.write(2, new Uint8Array(buffer)));
    else if (ArrayBuffer.isView(data)) this.write(2, new Uint8Array(data.buffer, data.byteOffset, data.byteLength));
    else this.write(2, new Uint8Array(data));
  }

  close(code = 1000, reason = ""): void {
    if (this.readyState === RelayWebSocket.CLOSING || this.readyState === RelayWebSocket.CLOSED) return;
    if (this.readyState === RelayWebSocket.CONNECTING) {
      this.finish(1006, "", false);
      return;
    }
    this.readyState = RelayWebSocket.CLOSING;
    const payload = new Uint8Array(2 + encoder.encode(reason).length);
    new DataView(payload.buffer).setUint16(0, code, false);
    payload.set(encoder.encode(reason), 2);
    this.write(8, payload);
    // The daemon answers with its own close frame, which ends the stream in `receive`.
    setTimeout(() => this.finish(code, reason, false), 3000);
  }

  private fail(): void {
    if (this.readyState === RelayWebSocket.CLOSED) return;
    this.fire(new Event("error"), this.onerror);
    this.finish(1006, "", false);
  }

  private finish(code: number, reason: string, wasClean: boolean): void {
    if (this.readyState === RelayWebSocket.CLOSED) return;
    this.readyState = RelayWebSocket.CLOSED;
    const stream = this.stream;
    stream?.abort();
    this.fire(new CloseEvent("close", { code, reason, wasClean }), this.onclose);
  }
}
