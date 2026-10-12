/*
 * Stream multiplexer inside one Noise session, mirroring crates/openside-relay/src/mux.rs:
 * every transport message carries `[kind u8][stream u32 BE][payload]`. The device opens odd
 * stream ids; each direction has a credit window so one unread stream (an SSE response in a
 * background tab) cannot make the daemon buffer without bound.
 */
import { MAX_MESSAGE, TAG_LEN, type Transport } from "./noise";

export const OPEN = 1;
export const DATA = 2;
export const FIN = 3;
export const RST = 4;
export const WINDOW = 5;
export const INITIAL_WINDOW = 262_144;
export const MAX_DATA = 16_384;
const MAX_STREAMS = 128;
export const MAX_QUEUED_FRAMES = 256;
const MAX_PENDING_MESSAGES = 128;
const MAX_PENDING_BYTES = 4 * 1024 * 1024;
const MAX_OUTBOUND_FRAMES = 1024;
const MAX_OUTBOUND_BYTES = 8 * 1024 * 1024;
const GRANT_STEP = INITIAL_WINDOW / 4;
const HEADER = 5;

/** What the session needs from the relay WebSocket. */
export interface Link {
  send(message: Uint8Array): void;
  close(): void;
}

function frame(kind: number, id: number, payload: Uint8Array = new Uint8Array()): Uint8Array {
  const out = new Uint8Array(HEADER + payload.length);
  out[0] = kind;
  new DataView(out.buffer).setUint32(1, id, false);
  out.set(payload, HEADER);
  return out;
}

export class StreamReset extends Error {
  constructor() {
    super("relay stream reset");
  }
}

export class MuxStream {
  private readonly chunks: Uint8Array[] = [];
  private recvFin = false;
  private reset = false;
  private finSent = false;
  private sendCredit = INITIAL_WINDOW;
  private sendOutstanding = 0;
  private recvAllowed = INITIAL_WINDOW;
  private consumed = 0;
  private readWaiter: (() => void) | null = null;
  private creditWaiter: (() => void) | null = null;

  constructor(
    readonly id: number,
    private readonly session: Session,
  ) {}

  /** Next received bytes, or `null` once the daemon finished writing. */
  async read(): Promise<Uint8Array | null> {
    while (this.chunks.length === 0 && !this.recvFin && !this.reset) {
      const { promise, resolve } = Promise.withResolvers<void>();
      this.readWaiter = resolve;
      await promise;
    }
    if (this.reset && !this.recvFin) throw new StreamReset();
    const chunk = this.chunks.shift();
    if (!chunk) return null;
    this.consumed += chunk.length;
    if (this.consumed >= GRANT_STEP && !this.recvFin) {
      const grant = new Uint8Array(4);
      new DataView(grant.buffer).setUint32(0, this.consumed, false);
      this.recvAllowed += this.consumed;
      this.consumed = 0;
      this.session.sendFrame(WINDOW, this.id, grant);
    }
    return chunk;
  }

  async write(data: Uint8Array): Promise<void> {
    let offset = 0;
    while (offset < data.length) {
      if (this.reset || this.finSent) throw new StreamReset();
      if (this.sendCredit === 0) {
        const { promise, resolve } = Promise.withResolvers<void>();
        this.creditWaiter = resolve;
        await promise;
        continue;
      }
      const n = Math.min(data.length - offset, MAX_DATA, this.sendCredit);
      this.sendCredit -= n;
      this.session.sendFrame(DATA, this.id, data.subarray(offset, offset + n));
      offset += n;
    }
  }

  /** Finish writing; reading continues until the daemon finishes too. */
  end(): void {
    if (this.finSent || this.reset) return;
    this.finSent = true;
    this.session.sendFrame(FIN, this.id);
    if (this.recvFin) this.session.forget(this.id);
  }

  /** Abort both directions. */
  abort(): void {
    if (this.reset) return;
    const tellPeer = !(this.finSent && this.recvFin);
    this.onReset();
    this.session.forget(this.id);
    if (tellPeer) this.session.sendFrame(RST, this.id);
  }

  get closed(): boolean {
    return this.reset || (this.finSent && this.recvFin);
  }

  /** @internal */
  onData(payload: Uint8Array): boolean {
    if (payload.length === 0 || payload.length > this.recvAllowed || payload.length > MAX_DATA ||
        this.recvFin || this.chunks.length >= MAX_QUEUED_FRAMES) return false;
    this.recvAllowed -= payload.length;
    this.chunks.push(payload);
    this.wakeReader();
    return true;
  }

  /** @internal */
  onFin(): void {
    this.recvFin = true;
    this.wakeReader();
  }

  /** @internal */
  onWindow(grant: number): boolean {
    if (grant === 0 || grant > this.sendOutstanding) return false;
    this.sendOutstanding -= grant;
    this.sendCredit += grant;
    this.wakeWriter();
    return true;
  }

  /** @internal */
  sent(bytes: number): void {
    this.sendOutstanding += bytes;
  }

  private wakeWriter(): void {
    const waiter = this.creditWaiter;
    this.creditWaiter = null;
    waiter?.();
  }

  /** @internal */
  onReset(): void {
    this.reset = true;
    // Data before a FIN is a complete response; the daemon drops its stream right after.
    if (!this.recvFin) this.chunks.length = 0;
    this.wakeReader();
    this.wakeWriter();
  }

  private wakeReader(): void {
    const waiter = this.readWaiter;
    this.readWaiter = null;
    waiter?.();
  }
}

export class Session {
  private readonly streams = new Map<number, MuxStream>();
  private nextId = 1;
  private readonly outbound: Uint8Array[] = [];
  private readonly inbound: Uint8Array[] = [];
  private inboundBytes = 0;
  private inboundCount = 0;
  private outboundBytes = 0;
  private outboundCount = 0;
  private receiving = false;
  private sending = false;
  private ended = false;
  /** Called once when the session ends (relay gone, daemon gone, bad message). */
  onclose: (() => void) | null = null;

  constructor(
    private readonly transport: Transport,
    private readonly link: Link,
  ) {}

  get closed(): boolean {
    return this.ended;
  }

  open(): MuxStream {
    if (this.ended) throw new StreamReset();
    if (this.streams.size >= MAX_STREAMS) throw new Error("too many relay streams");
    if (this.nextId > 0xffff_fffd) {
      this.close();
      throw new Error("relay stream ids exhausted");
    }
    const id = this.nextId;
    this.nextId += 2;
    const stream = new MuxStream(id, this);
    this.streams.set(id, stream);
    this.sendFrame(OPEN, id);
    return stream;
  }

  /** Feed one received WebSocket message (ciphertext). */
  receive(message: Uint8Array): void {
    if (this.ended) return;
    if (message.length < TAG_LEN || message.length > MAX_MESSAGE ||
        this.inboundCount >= MAX_PENDING_MESSAGES ||
        this.inboundBytes + message.length > MAX_PENDING_BYTES) {
      this.close();
      return;
    }
    this.inbound.push(message);
    this.inboundBytes += message.length;
    this.inboundCount++;
    if (!this.receiving) void this.receivePending();
  }

  private async receivePending(): Promise<void> {
    this.receiving = true;
    try {
      while (!this.ended) {
        const message = this.inbound.shift();
        if (!message) break;
        const plain = await this.transport.receive.decrypt(message);
        if (this.ended) return;
        this.inboundBytes -= message.length;
        this.inboundCount--;
        this.handle(plain);
      }
    } catch {
      this.close();
    } finally {
      this.receiving = false;
    }
  }

  close(): void {
    if (this.ended) return;
    this.ended = true;
    for (const stream of this.streams.values()) stream.onReset();
    this.streams.clear();
    this.inbound.length = 0;
    this.outbound.length = 0;
    this.inboundBytes = this.outboundBytes = 0;
    this.inboundCount = this.outboundCount = 0;
    this.link.close();
    this.onclose?.();
  }

  /** @internal */
  sendFrame(kind: number, id: number, payload?: Uint8Array): void {
    if (this.ended) return;
    if (kind === RST) this.streams.delete(id);
    const plain = frame(kind, id, payload);
    if (this.outboundCount >= MAX_OUTBOUND_FRAMES ||
        this.outboundBytes + plain.length > MAX_OUTBOUND_BYTES) {
      this.close();
      return;
    }
    this.outbound.push(plain);
    this.outboundBytes += plain.length;
    this.outboundCount++;
    if (!this.sending) void this.sendPending();
  }

  private async sendPending(): Promise<void> {
    this.sending = true;
    try {
      while (!this.ended) {
        const plain = this.outbound.shift();
        if (!plain) break;
        const message = await this.transport.send.encrypt(plain);
        if (this.ended) return;
        if (message.length > MAX_MESSAGE || message.length < TAG_LEN) throw new StreamReset();
        this.link.send(message);
        if (plain[0] === DATA) {
          const id = new DataView(plain.buffer).getUint32(1, false);
          this.streams.get(id)?.sent(plain.length - HEADER);
        }
        this.outboundBytes -= plain.length;
        this.outboundCount--;
      }
    } catch {
      this.close();
    } finally {
      this.sending = false;
    }
  }

  /** @internal */
  forget(id: number): void {
    this.streams.delete(id);
  }

  private handle(plain: Uint8Array): void {
    if (plain.length < HEADER) {
      this.close();
      return;
    }
    const view = new DataView(plain.buffer, plain.byteOffset, plain.byteLength);
    const kind = plain[0];
    const id = view.getUint32(1, false);
    const payload = plain.slice(HEADER);
    const stream = this.streams.get(id);
    switch (kind) {
      case DATA:
        if (stream && !stream.onData(payload)) {
          stream.onReset();
          this.sendFrame(RST, id);
        }
        break;
      case FIN:
        if (stream) {
          stream.onFin();
          if (stream.closed) this.streams.delete(id);
        }
        break;
      case RST:
        if (stream) {
          this.streams.delete(id);
          stream.onReset();
        }
        break;
      case WINDOW:
        if (payload.length !== 4) {
          this.close();
        } else if (stream && !stream.onWindow(new DataView(payload.buffer).getUint32(0, false))) {
          stream.onReset();
          this.sendFrame(RST, id);
        }
        break;
      default:
        // The daemon never opens streams; anything else is a protocol error.
        this.close();
    }
  }
}
