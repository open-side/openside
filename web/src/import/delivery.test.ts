import { afterEach, expect, it, vi } from "vitest";
import { ApiError, createClient } from "../api/client";
import { EXTENSION_ID } from "../desktopNotifyProtocol";
import { deliverImport, requestImportPermissions } from "./delivery";
import { IMPORT_APPLY, IMPORT_STATUS, IMPORT_PERMISSION_OPEN } from "./protocol";

const pending = { id: "revision", cookie_origins: [] as string[] };
const ticket = "single-use-ticket";
afterEach(() => vi.unstubAllGlobals());

function nativeReply(request: { type: string }, callback: (reply: unknown) => void) {
  callback(request.type === IMPORT_STATUS ? { ok: true, device: "extension-device" } : { ok: true, id: pending.id, bookmarks: 0, history: 0, cookies: 0 });
}

it("passes only a single-use ticket to the extension, with local authority used only to mint and ack", async () => {
  const calls: { url: string; headers: Headers; body: string | null }[] = [];
  const client = createClient({ token: () => "admin-token-fixture", localSecret: () => "boot-secret-fixture", fetch: async (input, init) => {
    const url = String(input);
    calls.push({ url, headers: new Headers(init?.headers), body: typeof init?.body === "string" ? init.body : null });
    return url.includes("/ack") ? new Response(null, { status: 204 }) : Response.json(url.includes("/ticket") ? { ticket } : { batches: [pending] });
  } });
  const messages: unknown[] = [];
  vi.stubGlobal("chrome", { runtime: { sendMessage: (id: string, request: { type: string }, callback: (reply: unknown) => void) => {
    expect(id).toBe(EXTENSION_ID);
    messages.push(request);
    nativeReply(request, callback);
  } } });
  const progress = vi.fn();
  expect(await deliverImport(client, progress)).toEqual({ delivered: 1, pending: 0, extensionMissing: false });
  expect(messages).toEqual([{ type: IMPORT_STATUS }, { type: IMPORT_APPLY, ticket }]);
  expect(JSON.stringify(messages)).not.toMatch(/admin-token-fixture|boot-secret-fixture|batch|cookie/);
  expect(calls.map(call => call.url)).toEqual(["/v1/import/delivery?target=user", "/v1/import/delivery/revision/ticket", "/v1/import/delivery/revision/ack"]);
  expect(calls[1]?.body).toBe('{"device_id":"extension-device"}');
  expect(calls[2]?.body).toBe('{"target":"user"}');
  for (const call of calls) {
    expect(call.headers.get("Authorization")).toBe("Bearer admin-token-fixture");
    expect(call.headers.get("X-Openside-Local-Secret")).toBe("boot-secret-fixture");
  }
  expect(progress.mock.calls).toEqual([[0, 1], [1, 1]]);
});

it("offers Connect rather than delivering anything when the extension is unpaired", async () => {
  const fetch = vi.fn(async () => Response.json({ batches: [pending] }));
  const client = createClient({ token: () => "fixture", fetch });
  vi.stubGlobal("chrome", { runtime: { sendMessage: (_id: string, _request: unknown, callback: (reply: unknown) => void) => callback({ ok: false, reason: "needs_pairing" }) } });
  await expect(deliverImport(client)).rejects.toMatchObject({ code: "import_delivery_pairing_required" });
  expect(fetch).toHaveBeenCalledTimes(1);
});

it("leaves work pending when the extension is missing or answers for another revision", async () => {
  const fetch = vi.fn(async (input: RequestInfo | URL) => Response.json(String(input).includes("/ticket") ? { ticket } : { batches: [pending] }));
  const client = createClient({ token: () => "fixture", fetch });
  vi.stubGlobal("chrome", undefined);
  expect(await deliverImport(client)).toEqual({ delivered: 0, pending: 1, extensionMissing: true });
  vi.stubGlobal("chrome", { runtime: { sendMessage: (_id: string, request: { type: string }, callback: (reply: unknown) => void) => callback(request.type === IMPORT_STATUS ? { ok: true, device: "extension-device" } : { ok: true, id: "another", bookmarks: 0, history: 0, cookies: 0 }) } });
  expect(await deliverImport(client)).toEqual({ delivered: 0, pending: 1, extensionMissing: true });
  expect(fetch).toHaveBeenCalledTimes(3);
});

it("never acknowledges refused native application and remints after a lost ack", async () => {
  const fetch = vi.fn(async (input: RequestInfo | URL) => String(input).includes("/ack") ? Response.json({ code: "storage_full" }, { status: 507 }) : Response.json(String(input).includes("/ticket") ? { ticket } : { batches: [pending] }));
  const client = createClient({ token: () => "fixture", fetch });
  vi.stubGlobal("chrome", { runtime: { sendMessage: (_id: string, request: { type: string }, callback: (reply: unknown) => void) => callback(request.type === IMPORT_STATUS ? { ok: true, device: "extension-device" } : { ok: false, reason: "permission" }) } });
  await expect(deliverImport(client)).rejects.toMatchObject({ code: "import_delivery_permission_required" });
  expect(fetch).toHaveBeenCalledTimes(2);
  vi.stubGlobal("chrome", { runtime: { sendMessage: (_id: string, request: { type: string }, callback: (reply: unknown) => void) => nativeReply(request, callback) } });
  await expect(deliverImport(client)).rejects.toBeInstanceOf(ApiError);
  await expect(deliverImport(client)).rejects.toBeInstanceOf(ApiError);
  expect(fetch.mock.calls.filter(([input]) => String(input).includes("/ticket"))).toHaveLength(3);
});

it("single-flights shared callers and never imports through a relay", async () => {
  const fetch = vi.fn(async () => Response.json({ batches: [] }));
  const client = createClient({ token: () => "fixture", fetch });
  const first = deliverImport(client);
  expect(deliverImport(client)).toBe(first);
  await first;
  expect(fetch).toHaveBeenCalledTimes(1);
  const relay = createClient({ token: () => "fixture", fetch, relay: true, localSecret: () => "must-not-use" });
  expect(await deliverImport(relay)).toEqual({ delivered: 0, pending: 0, extensionMissing: false });
  expect(fetch).toHaveBeenCalledTimes(1);
});

it("authorizes the extension to derive exact hosts, never sending page-supplied hosts or cookie values", async () => {
  const fetch = vi.fn(async (input: RequestInfo | URL) => String(input).includes("/ack") ? new Response(null, { status: 204 }) : Response.json(String(input).includes("/ticket") ? { ticket } : { batches: [{ ...pending, cookie_origins: ["https://example.test/*"] }] }));
  const client = createClient({ token: () => "owner-token", localSecret: () => "boot-secret", fetch });
  const messages: unknown[] = [];
  vi.stubGlobal("chrome", { runtime: { sendMessage: (_id: string, request: { type: string }, callback: (reply: unknown) => void) => {
    messages.push(request);
    if (request.type === IMPORT_PERMISSION_OPEN) callback({ ok: true, id: "permission-id", granted: true, denied: false });
    else nativeReply(request, callback);
  } } });
  expect(await requestImportPermissions(client)).toEqual({ delivered: 1, pending: 0, extensionMissing: false });
  expect(messages).toContainEqual({ type: IMPORT_PERMISSION_OPEN, ticket });
  expect(JSON.stringify(messages)).not.toMatch(/example.test|origins|owner-token|boot-secret|value/);
  expect(fetch).toHaveBeenCalledTimes(5);
});

it("preserves skipped-item counts and acknowledges the authenticated revision", async () => {
  const fetch = vi.fn(async (input: RequestInfo | URL) => String(input).includes("/ack") ? new Response(null, { status: 204 }) : Response.json(String(input).includes("/ticket") ? { ticket } : { batches: [pending] }));
  const client = createClient({ token: () => "fixture", fetch });
  vi.stubGlobal("chrome", { runtime: { sendMessage: (_id: string, request: { type: string }, callback: (reply: unknown) => void) => callback(request.type === IMPORT_STATUS ? { ok: true, device: "extension-device" } : { ok: true, id: pending.id, bookmarks: 0, history: 0, cookies: 0, skipped: 2 }) } });
  expect(await deliverImport(client)).toEqual({ delivered: 1, pending: 0, extensionMissing: false, skipped: 2 });
});
