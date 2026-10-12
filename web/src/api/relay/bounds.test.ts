import { describe, expect, it, vi } from "vitest";
import { readHead, StreamReader } from "./http1";
import { INITIAL_WINDOW, MAX_DATA, MAX_QUEUED_FRAMES, Session, type MuxStream } from "./mux";
import { CipherState, type Transport } from "./noise";
import { RelayWebSocket } from "./ws";

function sessionWith(decrypt = async (data: Uint8Array) => data, encrypt = async (data: Uint8Array) => new Uint8Array(data.length + 16)) {
  const close = vi.fn();
  const session = new Session({ receive: { decrypt }, send: { encrypt } } as unknown as Transport, { send: vi.fn(), close });
  return { session, close };
}

function readerFor(text: string): StreamReader {
  const bytes = new TextEncoder().encode(text);
  let at = 0;
  return new StreamReader({
    async read() {
      if (at >= bytes.length) return null;
      const chunk = bytes.slice(at, at + MAX_DATA);
      at += chunk.length;
      return chunk;
    },
  } as unknown as MuxStream);
}

describe("mux allocation and credit invariants", () => {
  it("rejects empty DATA and bounds both queued entries and bytes", () => {
    const { session } = sessionWith();
    const stream = session.open();
    expect(stream.onData(new Uint8Array())).toBe(false);
    for (let i = 0; i < MAX_QUEUED_FRAMES; i++) expect(stream.onData(new Uint8Array([1]))).toBe(true);
    expect(stream.onData(new Uint8Array([1]))).toBe(false);
    const full = session.open();
    for (let i = 0; i < INITIAL_WINDOW / MAX_DATA; i++) expect(full.onData(new Uint8Array(MAX_DATA))).toBe(true);
    expect(full.onData(new Uint8Array([1]))).toBe(false);
    session.close();
  });

  it("accepts only credit for bytes actually sent, once", async () => {
    const { session } = sessionWith();
    const stream = session.open();
    const sent = Promise.withResolvers<void>();
    const originalSent = stream.sent.bind(stream);
    vi.spyOn(stream, "sent").mockImplementation((bytes) => {
      originalSent(bytes);
      sent.resolve();
    });
    expect(stream.onWindow(1)).toBe(false);
    expect(stream.onWindow(0)).toBe(false);
    await stream.write(new Uint8Array(12));
    await sent.promise;
    expect(stream.onWindow(13)).toBe(false);
    expect(stream.onWindow(12)).toBe(true);
    expect(stream.onWindow(1)).toBe(false);
    session.close();
  });

  it("bounds ciphertext count before asynchronous cryptographic validation", () => {
    const { session, close } = sessionWith(() => Promise.withResolvers<Uint8Array>().promise);
    for (let i = 0; i < 128; i++) session.receive(new Uint8Array(16));
    expect(session.closed).toBe(false);
    session.receive(new Uint8Array(16));
    expect(session.closed).toBe(true);
    expect(close).toHaveBeenCalledTimes(1);
  });

  it("bounds ciphertext bytes independently of entry count", () => {
    const { session } = sessionWith(() => Promise.withResolvers<Uint8Array>().promise);
    for (let i = 0; i < 64; i++) session.receive(new Uint8Array(65535));
    expect(session.closed).toBe(false);
    session.receive(new Uint8Array(65535));
    expect(session.closed).toBe(true);
  });

  it("terminates the session on an outbound cipher failure and releases readers", async () => {
    const { session, close } = sessionWith(undefined, async () => { throw new Error("synthetic cipher failure"); });
    const pending = session.open().read();
    await expect(pending).rejects.toThrow("relay stream reset");
    expect(session.closed).toBe(true);
    expect(close).toHaveBeenCalledTimes(1);
  });
});

describe("aggregate HTTP response bounds", () => {
  it("rejects too many individually short headers", async () => {
    await expect(readHead(readerFor("HTTP/1.1 200 OK\r\n" + "x-test: a\r\n".repeat(129) + "\r\n"))).rejects.toThrow("too many response headers");
  });

  it("rejects an aggregate head larger than 64 KiB", async () => {
    const value = "a".repeat(33_000);
    await expect(readHead(readerFor(`HTTP/1.1 200 OK\r\nx-a: ${value}\r\nx-b: ${value}\r\n\r\n`))).rejects.toThrow("response header too large");
  });

  it("checks even a fully terminated oversized line", async () => {
    await expect(readerFor("a".repeat(65_536) + "\r\n").line()).rejects.toThrow("response header too large");
  });
});

describe("WebSocket unfinished message budgets", () => {
  it("rejects more than 1024 retained empty fragments before FIN", async () => {
    const socket = new RelayWebSocket("wss://relay.test/", [], () => Promise.withResolvers<MuxStream>().promise);
    let frames = 0;
    const reader = { async exact(n: number) { return n === 2 ? new Uint8Array([frames++ === 0 ? 2 : 0, 0]) : new Uint8Array(); } };
    // Exercise the private parser without an HTTP upgrade or network traffic.
    const parser = socket as unknown as { receive(reader: StreamReader): Promise<void> };
    await expect(parser.receive(reader as StreamReader)).rejects.toThrow("fragmented message too large");
    expect(frames).toBe(1025);
    socket.close();
  });

  it("checks aggregate fragment bytes before reading the next payload", async () => {
    const socket = new RelayWebSocket("wss://relay.test/", [], () => Promise.withResolvers<MuxStream>().promise);
    const size = 16 * 1024 * 1024;
    const extended = new Uint8Array(8);
    new DataView(extended.buffer).setBigUint64(0, BigInt(size));
    const parts = [new Uint8Array([2, 127]), extended, new Uint8Array(size), new Uint8Array([0, 1])];
    const exact = vi.fn(async () => parts.shift()!);
    // Exercise the private parser without an HTTP upgrade or network traffic.
    const parser = socket as unknown as { receive(reader: StreamReader): Promise<void> };
    await expect(parser.receive({ exact } as unknown as StreamReader)).rejects.toThrow("fragmented message too large");
    expect(exact).toHaveBeenCalledTimes(4);
    socket.close();
  });
});

describe("Noise terminal nonce", () => {
  it("allows the last usable nonce and permanently rejects the terminal nonce in both directions", async () => {
    const key = new Uint8Array(32);
    const sender = await CipherState.create(key);
    const receiver = await CipherState.create(key);
    Object.assign(sender, { counter: (1n << 64n) - 2n });
    Object.assign(receiver, { counter: (1n << 64n) - 2n });
    const encrypted = await sender.encrypt(new Uint8Array([42]));
    expect(await receiver.decrypt(encrypted)).toEqual(new Uint8Array([42]));
    for (let i = 0; i < 2; i++) {
      await expect(sender.encrypt(new Uint8Array())).rejects.toThrow("Noise nonce exhausted");
      await expect(receiver.decrypt(encrypted)).rejects.toThrow("Noise nonce exhausted");
    }
  });
});
