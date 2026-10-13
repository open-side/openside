import { afterEach, beforeEach, expect, it, vi, type Mock } from "vitest";
import { createHash, createHmac, webcrypto } from "node:crypto";
import { cookieDetails, handleImportRequest } from "../src/imports";
import { IMPORT_APPLY, IMPORT_STATUS, IMPORT_PERMISSION_OPEN, IMPORT_PERMISSION_STATUS, type ImportDeliveryBatch } from "../../web/src/import/protocol";

const origin = "http://127.0.0.1:43123";
const device = "a".repeat(32);
const secret = "b".repeat(64);
const revision = "c".repeat(32);
let sequence: number;
let saved: Record<string, unknown>;
let local: Record<string, unknown>;
let signed: Map<string, object>;
let nodes: chrome.bookmarks.BookmarkTreeNode[];
let visits: Set<string>;
let fetcher: Mock;
let api: {
  bookmarks: { create: Mock; getChildren: Mock };
  history: { addUrl: Mock; getVisits: Mock };
  cookies: { set: Mock };
  permissions: { contains: Mock };
};

function batch(): ImportDeliveryBatch {
  return { id: revision, source: { id: "chrome:Default", brand: "chrome", name: "Personal" },
    bookmarks: [
      { url: "https://example.test/one", title: "First", folder: ["Work", "Reference"], added_at_ms: 1 },
      { url: "https://example.test/one", title: "Duplicate", folder: ["Work", "Reference"], added_at_ms: null },
      { url: "https://example.test/one", title: "Other folder", folder: ["Home"], added_at_ms: null },
    ],
    history: [{ url: "https://example.test/old", title: "Old", visited_at_ms: 1 }, { url: "https://example.test/old", title: null, visited_at_ms: 2 }],
    cookies: [{ url: "https://example.test/", name: "fixture", value: "fixture-value", domain: null, path: "/", secure: true, httpOnly: true, sameSite: "lax", expirationDate: null, partitionKey: null }],
  };
}

function authorize(incoming: unknown = batch(), type = IMPORT_APPLY) {
  const ticket = (++sequence).toString(16).padStart(64, "0");
  const payload = JSON.stringify(incoming);
  const hash = createHash("sha256").update(payload).digest("hex");
  const signature = createHmac("sha256", Buffer.from(secret, "hex")).update(ticket + revision + hash).digest("hex");
  signed.set(ticket, { ticket, batch_id: revision, payload, signature });
  return { type, ticket };
}

beforeEach(() => {
  sequence = 0;
  saved = { pairing: { url: origin, token: "device-token", device } };
  local = { importPairing: { device, secret, origin } };
  signed = new Map();
  nodes = [{ id: "1", parentId: "0", title: "Bookmarks bar", folderType: "bookmarks-bar", syncing: false }, { id: "2", parentId: "0", title: "Other bookmarks", folderType: "other", syncing: false }];
  visits = new Set();
  api = {
    bookmarks: {
      getChildren: vi.fn(async (id: string) => nodes.filter(node => node.parentId === id)),
      create: vi.fn(async (details: chrome.bookmarks.CreateDetails) => { const node = { ...details, id: String(nodes.length + 10), title: details.title ?? "", syncing: false }; nodes.push(node); return node; }),
    },
    history: { getVisits: vi.fn(async ({ url }: { url: string }) => visits.has(url) ? [{ id: "1" }] : []), addUrl: vi.fn(async ({ url }: { url: string }) => { visits.add(url); }) },
    cookies: { set: vi.fn(async (details: chrome.cookies.SetDetails) => ({ ...details })) },
    permissions: { contains: vi.fn(async () => true) },
  };
  vi.stubGlobal("chrome", { ...api, runtime: { getURL: (path: string) => `chrome-extension://fixture/${path}` }, tabs: { create: vi.fn(async () => ({ id: 10 })) }, storage: {
    session: { get: vi.fn(async (key: string) => ({ [key]: saved[key] })), set: vi.fn(async (value: Record<string, unknown>) => { Object.assign(saved, value); }) },
    local: { get: vi.fn(async (key: string) => ({ [key]: local[key] })) },
  } });
  fetcher = vi.fn(async (url: string, options: RequestInit) => {
    expect(url.startsWith(`${origin}/v1/extension/import/`)).toBe(true);
    expect(new Headers(options.headers).get("Authorization")).toBe("Bearer device-token");
    expect(options.redirect).toBe("error");
    const ticket = url.split("/").at(-1)!;
    const response = signed.get(ticket);
    signed.delete(ticket);
    return response ? Response.json(response) : new Response(null, { status: 403 });
  });
  vi.stubGlobal("fetch", fetcher);
  vi.stubGlobal("crypto", webcrypto);
  vi.stubGlobal("navigator", { userAgent: "Chromium/154.0" });
});
afterEach(() => vi.unstubAllGlobals());

it("fails closed unpaired and never accepts a page-supplied batch", async () => {
  delete saved.pairing;
  expect(await handleImportRequest(authorize(), origin)).toEqual({ ok: false, reason: "needs_pairing" });
  expect(await handleImportRequest({ type: IMPORT_STATUS }, origin)).toEqual({ ok: false, reason: "needs_pairing" });
  expect(fetcher).not.toHaveBeenCalled();
  expect(api.bookmarks.create).not.toHaveBeenCalled();
  saved.pairing = { url: origin, token: "device-token", device };
  expect(await handleImportRequest({ type: IMPORT_APPLY, batch: batch() }, origin)).toEqual({ ok: false, reason: "invalid_batch" });
});

it("reuses the source root and complete folder path across separately authorized retries", async () => {
  expect(await handleImportRequest(authorize(), origin)).toEqual({ ok: true, id: revision, bookmarks: 2, history: 1, cookies: 1 });
  expect(await handleImportRequest(authorize(), origin)).toEqual({ ok: true, id: revision, bookmarks: 0, history: 0, cookies: 1 });
  expect(nodes.filter(node => node.title === "Imported from Chrome (Personal)")).toHaveLength(1);
  expect(nodes.filter(node => node.title === "Reference")).toHaveLength(1);
  expect(nodes.filter(node => node.url === "https://example.test/one")).toHaveLength(2);
  expect(api.history.addUrl).toHaveBeenCalledTimes(1);
});

it("resumes partial native failures using a new ticket without duplicate folders", async () => {
  api.bookmarks.create.mockImplementationOnce(async () => { throw new Error("temporary"); });
  expect(await handleImportRequest(authorize(), origin)).toEqual({ ok: false, reason: "apply_failed" });
  expect(await handleImportRequest(authorize(), origin)).toMatchObject({ ok: true, bookmarks: 2 });
  api.cookies.set.mockResolvedValueOnce(null);
  expect(await handleImportRequest(authorize(), origin)).toEqual({ ok: false, reason: "apply_failed" });
  expect(await handleImportRequest(authorize(), origin)).toMatchObject({ ok: true, bookmarks: 0, history: 0 });
});

it("serializes distinct authorized tickets so native dedupe cannot race", async () => {
  const replies = await Promise.all([handleImportRequest(authorize(), origin), handleImportRequest(authorize(), origin)]);
  expect(replies).toMatchObject([{ ok: true, bookmarks: 2, history: 1 }, { ok: true, bookmarks: 0, history: 0 }]);
});

it("rejects a replayed ticket before another fetch or native write", async () => {
  const request = authorize();
  expect(await handleImportRequest(request, origin)).toMatchObject({ ok: true });
  const writes = api.cookies.set.mock.calls.length;
  expect(await handleImportRequest(request, origin)).toEqual({ ok: false, reason: "apply_failed" });
  expect(fetcher).toHaveBeenCalledTimes(1);
  expect(api.cookies.set).toHaveBeenCalledTimes(writes);
});

it("rejects wrong origins and pairing/device mismatches without reaching the daemon", async () => {
  for (const source of [undefined, "https://remote.test", "http://example.test", "http://127.0.0.1.evil.test", "http://127.0.0.1:43124"]) expect(await handleImportRequest(authorize(), source)).toEqual({ ok: false, reason: "refused" });
  local.importPairing = { device: "another-device", secret };
  expect(await handleImportRequest(authorize(), origin)).toEqual({ ok: false, reason: "needs_pairing" });
  expect(fetcher).not.toHaveBeenCalled();
});

it("rejects a stale token or impostor server rather than trusting the paired port", async () => {
  fetcher.mockResolvedValueOnce(new Response(null, { status: 401 }));
  expect(await handleImportRequest(authorize(), origin)).toEqual({ ok: false, reason: "apply_failed" });
  const request = authorize();
  signed.set(request.ticket, { ...signed.get(request.ticket), signature: "0".repeat(64) });
  expect(await handleImportRequest(request, origin)).toEqual({ ok: false, reason: "apply_failed" });
  expect(api.bookmarks.create).not.toHaveBeenCalled();
  expect(api.cookies.set).not.toHaveBeenCalled();
});

it("verifies payload hash, ticket and batch identity before any native mutation", async () => {
  for (const alteration of ["payload", "ticket", "batch_id"] as const) {
    const request = authorize();
    const response = signed.get(request.ticket)!;
    signed.set(request.ticket, { ...response, [alteration]: alteration === "payload" ? JSON.stringify({ ...batch(), cookies: [] }) : "d".repeat(alteration === "ticket" ? 64 : 32) });
    expect(await handleImportRequest(request, origin)).toEqual({ ok: false, reason: "apply_failed" });
  }
  expect(api.bookmarks.create).not.toHaveBeenCalled();
});

it("rejects malformed authenticated payloads before touching native data", async () => {
  for (const value of [null, {}, { ...batch(), bookmarks: null }, { ...batch(), cookies: [null] }]) expect(await handleImportRequest(authorize(value), origin)).toEqual({ ok: false, reason: "apply_failed" });
  expect(api.cookies.set).not.toHaveBeenCalled();
});

it("maps secure, HttpOnly, expiry, SameSite and partition boundaries exactly", () => {
  const cookie = batch().cookies[0]!;
  expect(cookieDetails(cookie)).toEqual({ url: cookie.url, name: cookie.name, value: cookie.value, path: "/", secure: true, httpOnly: true, sameSite: "lax" });
  expect(cookieDetails({ ...cookie, domain: ".example.test", expirationDate: 2_000_000_000, sameSite: "no_restriction", partitionKey: { topLevelSite: "https://top.test", hasCrossSiteAncestor: true } })).toMatchObject({ domain: ".example.test", expirationDate: 2_000_000_000, sameSite: "no_restriction", partitionKey: { topLevelSite: "https://top.test", hasCrossSiteAncestor: true } });
  vi.stubGlobal("navigator", { userAgent: "Chrome/126.0" });
  expect(() => cookieDetails({ ...cookie, partitionKey: { topLevelSite: "https://top.test", hasCrossSiteAncestor: false } })).toThrow("Chrome 130");
});

it("keeps cookies pending without host permission and never widens permissions", async () => {
  api.permissions.contains.mockResolvedValue(false);
  expect(await handleImportRequest(authorize(), origin)).toEqual({ ok: false, reason: "permission" });
  expect(api.cookies.set).not.toHaveBeenCalled();
  expect(api.bookmarks.create).not.toHaveBeenCalled();
});

it("derives exact-site consent only from a signed batch and binds status to the paired origin", async () => {
  expect(await handleImportRequest({ type: IMPORT_PERMISSION_OPEN, origins: ["https://*/*"] }, origin)).toEqual({ ok: false, reason: "invalid_batch" });
  api.permissions.contains.mockResolvedValue(false);
  const opened = await handleImportRequest(authorize(batch(), IMPORT_PERMISSION_OPEN), origin);
  expect(opened).toMatchObject({ ok: true, granted: false, denied: false });
  if (!opened.ok || !("id" in opened)) throw new Error("permission fixture was refused");
  expect(saved[`importPermissions:${opened.id}`]).toEqual({ origin, origins: ["https://example.test/*"], denied: false });
  expect(await handleImportRequest({ type: IMPORT_PERMISSION_STATUS, id: opened.id }, "http://127.0.0.1:43124")).toEqual({ ok: false, reason: "refused" });
  api.permissions.contains.mockResolvedValue(true);
  expect(await handleImportRequest({ type: IMPORT_PERMISSION_STATUS, id: opened.id }, origin)).toEqual({ ok: true, id: opened.id, granted: true, denied: false });
  expect(api.cookies.set).not.toHaveBeenCalled();
});

it("keeps supported web/file/FTP bookmarks while skipping individual malformed entries", async () => {
  const incoming = { ...batch(), bookmarks: [...batch().bookmarks,
    { url: "file:///tmp/import-fixture.txt", title: "Local file", folder: [] },
    { url: "ftp://example.test/pub/fixture.txt", title: "FTP file", folder: [] },
    { url: "chrome://settings/", title: "Internal", folder: [] }, null,
  ] };
  expect(await handleImportRequest(authorize(incoming), origin)).toEqual({ ok: true, id: revision, bookmarks: 4, history: 1, cookies: 1, skipped: 2 });
  expect(await handleImportRequest(authorize(incoming), origin)).toMatchObject({ ok: true, bookmarks: 0, history: 0, skipped: 2 });
});
