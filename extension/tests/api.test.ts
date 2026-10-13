import { afterEach, describe, it, expect, vi } from "vitest";
import { client, exchangeImportPairing, openSessionStream } from "../src/api";
import type { Event } from "../../web/src/api/types";
import { createHash, createHmac, webcrypto } from "node:crypto";
afterEach(() => vi.unstubAllGlobals());
describe("extension public daemon client", () => {
  it("creates a session then submits the first message through authenticated public routes", async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => { calls.push({ url: String(input), init }); return new Response(JSON.stringify({ id: "session-1" }), { status: calls.length === 1 ? 201 : 202 }); });
    const api = client({ url: "http://localhost:4321", token: "scoped-device-token" }, fetcher);
    const session = await api.createSession({ agent_id: null, title: "Mail", model: null, mode: null, workspace: null });
    await api.submitMessage(session.id, { message_id: "first", mode: "now", text: "Context from mail" });
    expect(calls.map(call => call.url)).toEqual(["http://localhost:4321/v1/sessions", "http://localhost:4321/v1/sessions/session-1/messages"]);
    expect(JSON.parse(calls[0].init!.body as string)).toMatchObject({ agent_id: null });
    expect(calls[1].init?.headers).toMatchObject({ Authorization: "Bearer scoped-device-token" }); expect(calls.some(call => call.url.includes("token"))).toBe(false);
    expect(JSON.parse(calls[1].init!.body as string)).toMatchObject({ text: "Context from mail", mode: "now" });
  });
  it("hydrates pending approval cards through the authenticated public ticket list", async () => {
    const fetcher = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) => new Response(JSON.stringify({ tickets: [] }), { status: 200 }));
    const api = client({ url: "http://localhost:4321", token: "scoped-device-token" }, fetcher);
    expect(await api.listTickets("pending")).toEqual({ tickets: [] });
    expect(fetcher).toHaveBeenCalledWith("http://localhost:4321/v1/tickets?status=pending", expect.objectContaining({ headers: expect.objectContaining({ Authorization: "Bearer scoped-device-token" }) }));
  });
  it("streams a reply over SSE with the token in the header, not the URL", async () => {
    const event = { kind: "assistant.delta", session_id: "session-1", seq: 1, text: "streamed reply" };
    const fetcher = vi.fn(async () => new Response(new ReadableStream({ start(controller) { const bytes = new TextEncoder().encode(`id: 1\r\ndata: ${JSON.stringify(event)}\r\n\r\n`); controller.enqueue(bytes.slice(0, 12)); controller.enqueue(bytes.slice(12)); } }), { headers: { "Content-Type": "text/event-stream" } }));
    const events: Event[] = [];
    const stream = openSessionStream({ client: client({ url: "http://localhost:4321", token: "private" }, fetcher), sessionId: "session-1", onEvent: value => events.push(value) });
    await vi.waitFor(() => expect(events).toHaveLength(1)); stream.close();
    expect(events[0]).toMatchObject(event); expect(fetcher.mock.calls[0]).toHaveLength(2);
    const [url, init] = fetcher.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).not.toContain("private"); expect(init.headers).toMatchObject({ Authorization: "Bearer private", Accept: "text/event-stream" });
  });
});

describe("retained import pairing continuity", () => {
  const previous = { origin: "http://127.0.0.1:4321", device: "11".repeat(16), secret: "ab".repeat(32) };
  const replacement = { token: "replacement-token", device_id: "22".repeat(16), scopes: ["read"], kind: "person_device", import_secret: "cd".repeat(32) };
  function signature(headers: Headers, payload: string, secret = previous.secret, nonce = headers.get("x-openside-import-nonce")!) {
    return createHmac("sha256", Buffer.from(secret, "hex")).update("openside-import-pairing-v1\n")
      .update(previous.device).update(nonce).update(createHash("sha256").update(payload).digest()).digest("hex");
  }

  it("sends a fresh nonce, never the retained secret, and verifies the entire replacement response", async () => {
    vi.stubGlobal("crypto", webcrypto);
    vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit) => {
      expect(url).toBe(`${previous.origin}/v1/pairing/exchange`);
      const headers = new Headers(init.headers);
      expect(headers.get("x-openside-import-device")).toBe(previous.device);
      expect(headers.get("x-openside-import-nonce")).toMatch(/^[0-9a-f]{64}$/);
      expect(headers.has("Authorization")).toBe(false);
      expect(init.body).toBe(JSON.stringify({ code: "one-use" }));
      expect(JSON.stringify(init)).not.toContain(previous.secret);
      const payload = JSON.stringify(replacement);
      return new Response(payload, { headers: { "x-openside-import-proof": signature(headers, payload) } });
    }));
    expect(await exchangeImportPairing(previous.origin, "one-use", previous)).toEqual({ paired: replacement, continuous: true });
  });

  it("cannot authenticate a same-origin listener without the retained key", async () => {
    vi.stubGlobal("crypto", webcrypto);
    vi.stubGlobal("fetch", vi.fn(async (_url: string, init: RequestInit) => {
      const payload = JSON.stringify(replacement);
      return new Response(payload, { headers: { "x-openside-import-proof": signature(new Headers(init.headers), payload, "ef".repeat(32)) } });
    }));
    expect((await exchangeImportPairing(previous.origin, "attacker", previous)).continuous).toBe(false);
  });

  it("rejects a valid proof transplanted onto a different token or import key", async () => {
    vi.stubGlobal("crypto", webcrypto);
    for (const changed of [{ ...replacement, token: "attacker-token" }, { ...replacement, import_secret: "ef".repeat(32) }]) {
      vi.stubGlobal("fetch", vi.fn(async (_url: string, init: RequestInit) => new Response(JSON.stringify(changed), {
        headers: { "x-openside-import-proof": signature(new Headers(init.headers), JSON.stringify(replacement)) },
      })));
      expect((await exchangeImportPairing(previous.origin, "one-use", previous)).continuous).toBe(false);
    }
  });

  it("cannot replay an old nonce proof onto a later pairing exchange", async () => {
    vi.stubGlobal("crypto", webcrypto);
    let firstNonce = "";
    const seen: string[] = [];
    vi.stubGlobal("fetch", vi.fn(async (_url: string, init: RequestInit) => {
      const headers = new Headers(init.headers);
      const nonce = headers.get("x-openside-import-nonce")!;
      seen.push(nonce);
      firstNonce ||= nonce;
      const payload = JSON.stringify(replacement);
      return new Response(payload, { headers: { "x-openside-import-proof": signature(headers, payload, previous.secret, firstNonce) } });
    }));
    expect((await exchangeImportPairing(previous.origin, "first", previous)).continuous).toBe(true);
    expect((await exchangeImportPairing(previous.origin, "second", previous)).continuous).toBe(false);
    expect(seen[0]).not.toBe(seen[1]);
  });

  it("never claims continuity without a retained key or a valid proof header", async () => {
    vi.stubGlobal("crypto", webcrypto);
    for (const proof of ["", "not-a-proof"]) {
      vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify(replacement), { headers: { "x-openside-import-proof": proof } })));
      expect((await exchangeImportPairing(previous.origin, "code", previous)).continuous).toBe(false);
      expect((await exchangeImportPairing(previous.origin, "code", null)).continuous).toBe(false);
    }
  });
});
