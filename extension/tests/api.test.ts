import { describe, it, expect, vi } from "vitest";
import { client, openSessionStream } from "../src/api";
import type { Event } from "../../web/src/api/types";
describe("extension public daemon client", () => {
  it("creates a session then submits the first message through authenticated public routes", async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => { calls.push({ url: String(input), init }); return new Response(JSON.stringify({ id: "session-1" }), { status: calls.length === 1 ? 201 : 202 }); });
    const api = client({ url: "http://localhost:4321", token: "scoped-device-token" }, fetcher);
    const session = await api.createSession({ title: "Mail", model: null, mode: null, workspace: null });
    await api.submitMessage(session.id, { message_id: "first", mode: "now", text: "Context from mail" });
    expect(calls.map(call => call.url)).toEqual(["http://localhost:4321/v1/sessions", "http://localhost:4321/v1/sessions/session-1/messages"]);
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
