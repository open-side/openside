import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initiate, type Transport } from "./noise";
import type * as NoiseModule from "./noise";
import { relayTransport } from "./transport";

vi.mock("./noise", async (original) => {
  const noise = await original<typeof NoiseModule>();
  return { ...noise, daemonIdFor: vi.fn(async () => "test-daemon"), initiate: vi.fn() };
});

class FakeSocket {
  static created = Promise.withResolvers<FakeSocket>();
  binaryType = "";
  onopen: (() => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  sent: Uint8Array[] = [];
  closed = false;
  constructor() { FakeSocket.created.resolve(this); }
  send(data: Uint8Array): void { this.sent.push(data); }
  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.onclose?.();
  }
}

beforeEach(() => {
  FakeSocket.created = Promise.withResolvers<FakeSocket>();
  vi.stubGlobal("WebSocket", FakeSocket);
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

const target = { kind: "relay" as const, relay: "https://relay.test", daemon_id: "test-daemon", daemon_key: "A".repeat(43) };

describe("relay handshake lifecycle", () => {
  it("refuses a second handshake message while authentication is pending", async () => {
    const authentication = Promise.withResolvers<Transport>();
    const called = Promise.withResolvers<void>();
    const finish = vi.fn(() => { called.resolve(); return authentication.promise; });
    vi.mocked(initiate).mockResolvedValue({ message: new Uint8Array(48), finish });
    const transport = relayTransport(target);
    const request = transport.fetch("https://relay.test/v1/ping");
    const rejected = expect(request).rejects.toThrow("unexpected relay handshake message");
    const socket = await FakeSocket.created.promise;
    socket.onmessage?.(new MessageEvent("message", { data: new ArrayBuffer(48) }));
    await called.promise;
    socket.onmessage?.(new MessageEvent("message", { data: new ArrayBuffer(48) }));
    await rejected;
    expect(finish).toHaveBeenCalledTimes(1);
    expect(socket.closed).toBe(true);
    authentication.resolve({} as Transport);
    transport.close();
  });

  it("does not revive a session after close during authentication", async () => {
    const authentication = Promise.withResolvers<Transport>();
    const called = Promise.withResolvers<void>();
    vi.mocked(initiate).mockResolvedValue({ message: new Uint8Array(48), finish: () => { called.resolve(); return authentication.promise; } });
    const transport = relayTransport(target);
    const request = transport.fetch("https://relay.test/v1/ping");
    const rejected = expect(request).rejects.toThrow("relay transport closed");
    const socket = await FakeSocket.created.promise;
    socket.onmessage?.(new MessageEvent("message", { data: new ArrayBuffer(48) }));
    await called.promise;
    transport.close();
    await rejected;
    authentication.resolve({} as Transport);
    expect(socket.closed).toBe(true);
    await expect(transport.fetch("https://relay.test/v1/ping")).rejects.toThrow("relay transport closed");
    expect(socket.sent).toHaveLength(0);
  });

  it("cleans up a failed socket before any request stream exists", async () => {
    vi.mocked(initiate).mockResolvedValue({ message: new Uint8Array(48), finish: vi.fn() });
    const transport = relayTransport(target);
    const request = transport.fetch("https://relay.test/v1/ping");
    const rejected = expect(request).rejects.toThrow("relay connection failed");
    const socket = await FakeSocket.created.promise;
    socket.onerror?.();
    await rejected;
    expect(socket.closed).toBe(true);
    transport.close();
  });
});
