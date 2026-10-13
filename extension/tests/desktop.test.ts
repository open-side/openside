import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Session, Ticket } from "../src/api";

const api = vi.hoisted(() => ({
  exchangePairing: vi.fn(async (_code: string): Promise<{ token: string; device_id: string; import_secret?: string }> => ({ token: "minted-token", device_id: "dev-1" })),
  listTickets: vi.fn(async (_status: string) => ({ tickets: [] as Ticket[] })),
  listSessions: vi.fn(async () => ({ sessions: [] as Session[] })),
}));
const continuity = vi.hoisted(() => ({ valid: false, received: vi.fn() }));
vi.mock("../src/api", () => ({
  client: () => api,
  exchangeImportPairing: async (origin: string, code: string, previous: unknown) => {
    continuity.received(origin, previous);
    return { paired: await api.exchangePairing(code), continuous: continuity.valid };
  },
}));

import { checkNotifications, handleDesktopRequest, openConversation } from "../src/desktop";
import { importCredential } from "../src/storage";

const listeners = new Set<(changes: Record<string, chrome.storage.StorageChange>, area: string) => void>();
function answerConsent(allowed: boolean): void {
  const key = Object.keys(session).find(key => key.startsWith("importConnect:"))!;
  const value = { ...(session[key] as object), allowed };
  session[key] = value;
  for (const listener of listeners) listener({ [key]: { newValue: value } }, "session");
}
const area = (store: Record<string, unknown>) => ({
  get: vi.fn(async (key: string) => (key in store ? { [key]: store[key] } : {})),
  set: vi.fn(async (items: Record<string, unknown>) => { Object.assign(store, items); }),
  remove: vi.fn(async (key: string) => { delete store[key]; }),
});
const session: Record<string, unknown> = {};
const local: Record<string, unknown> = {};
let focusedWindow: { focused: boolean; tabs: { active: boolean; url?: string }[] };
const chromeStub = {
  storage: { session: area(session), local: area(local), onChanged: { addListener: (fn: typeof listeners extends Set<infer T> ? T : never) => listeners.add(fn), removeListener: (fn: typeof listeners extends Set<infer T> ? T : never) => listeners.delete(fn) } },
  action: { setBadgeBackgroundColor: vi.fn(async () => {}), setBadgeText: vi.fn(async () => {}) },
  notifications: { create: vi.fn(), clear: vi.fn(async () => true) },
  permissions: { contains: vi.fn(async () => true), request: vi.fn(async () => true) },
  windows: { getLastFocused: vi.fn(async () => focusedWindow), update: vi.fn(async () => ({})) },
  tabs: { query: vi.fn(async () => [] as { id: number; windowId: number }[]), update: vi.fn(async () => ({})), create: vi.fn(async (_details: chrome.tabs.CreateProperties) => ({})) },
  runtime: { getURL: (path: string) => `chrome-extension://fixture/${path}` },
};
vi.stubGlobal("chrome", chromeStub);

const UI = "http://127.0.0.1:47820";
const SECRET = "ab".repeat(32);
const OLD_DEVICE = "11".repeat(16);
const task = (id: string, status: Session["status"], title = `Task ${id}`) => ({ id, status, title, last_seq: 3 }) as Session;
const confirm = { id: "k1", session_id: "s1", kind: "confirm", summary: "Run rm -rf build?", instructions: null, options: [], origin: {}, target: null, otp: null, status: "pending" } as unknown as Ticket;

beforeEach(() => {
  vi.clearAllMocks();
  listeners.clear();
  continuity.valid = false;
  chromeStub.tabs.create.mockImplementation(async () => ({}));
  for (const store of [session, local]) for (const key of Object.keys(store)) delete store[key];
  focusedWindow = { focused: true, tabs: [{ active: true, url: `${UI}/` }] };
  chromeStub.permissions.contains.mockResolvedValue(true);
});

describe("handleDesktopRequest", () => {
  it("answers only the daemon UI on this computer and ignores malformed requests", async () => {
    for (const origin of [undefined, "https://127.0.0.1", "http://evil.test", "http://127.0.0.1.evil.test:1", "not a url"]) {
      expect(await handleDesktopRequest({ type: "openside.desktop.status" }, origin)).toMatchObject({ ok: false, reason: "refused" });
    }
    for (const message of [null, "enable", {}, { type: "openside.desktop.enable", code: 7 }, { type: "openside.desktop.enable", code: "" }, { type: "nope" }]) {
      expect(await handleDesktopRequest(message, UI)).toMatchObject({ ok: false, reason: "refused" });
    }
    expect(api.exchangePairing).not.toHaveBeenCalled();
    expect(local).toEqual({});
  });

  it("asks for a pairing code when it holds none, then pairs from that code and turns notifications on", async () => {
    expect(await handleDesktopRequest({ type: "openside.desktop.status" }, UI)).toEqual({ ok: true, paired: false, enabled: false, device: null, importing: false });
    expect(await handleDesktopRequest({ type: "openside.desktop.enable" }, UI)).toMatchObject({ ok: false, reason: "needs_pairing" });
    expect(await handleDesktopRequest({ type: "openside.desktop.enable", code: "one-time" }, UI)).toEqual({ ok: true, paired: true, enabled: true, device: "dev-1", importing: false });
    expect(api.exchangePairing).toHaveBeenCalledExactlyOnceWith("one-time");
    expect(session.pairing).toEqual({ url: UI, token: "minted-token", device: "dev-1" });
    expect(local.desktopNotifications).toBe(true);
    expect(await handleDesktopRequest({ type: "openside.desktop.status" }, UI)).toEqual({ ok: true, paired: true, enabled: true, device: "dev-1", importing: false });
  });

  it("enables without a code when it is already paired to this origin, and keeps a pasted token on disable", async () => {
    session.pairing = { url: UI, token: "pasted" };
    expect(await handleDesktopRequest({ type: "openside.desktop.enable" }, UI)).toEqual({ ok: true, paired: true, enabled: true, device: null, importing: false });
    expect(api.exchangePairing).not.toHaveBeenCalled();
    expect(await handleDesktopRequest({ type: "openside.desktop.disable" }, UI)).toMatchObject({ ok: true, enabled: false, device: null });
    expect(local.desktopNotifications).toBe(false);
    expect(session.pairing).toEqual({ url: UI, token: "pasted" });
  });

  it("forgets a token it minted itself on disable and hands back the device to revoke", async () => {
    await handleDesktopRequest({ type: "openside.desktop.enable", code: "c" }, UI);
    expect(await handleDesktopRequest({ type: "openside.desktop.disable" }, UI)).toMatchObject({ ok: true, device: "dev-1" });
    expect(session.pairing).toBeUndefined();
    expect(local.desktopNotifications).toBe(false);
  });

  it("never replaces a pairing to another daemon", async () => {
    session.pairing = { url: "http://localhost:9999", token: "other" };
    const reply = await handleDesktopRequest({ type: "openside.desktop.enable", code: "c" }, UI);
    expect(reply).toMatchObject({ ok: false, reason: "pairing_conflict", message: expect.stringContaining("http://localhost:9999") });
    expect(api.exchangePairing).not.toHaveBeenCalled();
    expect(session.pairing).toEqual({ url: "http://localhost:9999", token: "other" });
  });

  it("reports a refused host permission and a rejected code without storing anything", async () => {
    chromeStub.permissions.contains.mockResolvedValue(false);
    chromeStub.permissions.request.mockResolvedValueOnce(false);
    expect(await handleDesktopRequest({ type: "openside.desktop.enable", code: "c" }, UI)).toMatchObject({ ok: false, reason: "permission" });
    expect(chromeStub.permissions.request).toHaveBeenCalledWith({ origins: ["http://127.0.0.1/*"] });
    chromeStub.permissions.contains.mockResolvedValue(true);
    api.exchangePairing.mockRejectedValueOnce(new Error("expired"));
    expect(await handleDesktopRequest({ type: "openside.desktop.enable", code: "c" }, UI)).toMatchObject({ ok: false, reason: "exchange_failed" });
    expect(session.pairing).toBeUndefined();
    expect(local.desktopNotifications).toBeUndefined();
  });
});

describe("import pairing", () => {
  const pair = (code?: string) => handleDesktopRequest({ type: "openside.desktop.pair", ...(code ? { code } : {}) }, UI);
  beforeEach(() => {
    chromeStub.tabs.create.mockImplementation(async () => { answerConsent(true); return {}; });
  });

  it("keeps the daemon's secret in local storage and the token in session storage, and leaves notifications alone", async () => {
    expect(await pair()).toMatchObject({ ok: false, reason: "needs_pairing" });
    api.exchangePairing.mockResolvedValueOnce({ token: "tok", device_id: "dev-9", import_secret: SECRET });
    expect(await pair("c")).toEqual({ ok: true, paired: true, enabled: false, device: "dev-9", importing: true, replaced: null });
    expect(local.importPairing).toEqual({ device: "dev-9", secret: SECRET, origin: UI });
    expect(session.pairing).toEqual({ url: UI, token: "tok", device: "dev-9" });
    expect(local.desktopNotifications).toBeUndefined();
    expect(await importCredential()).toEqual({ url: UI, token: "tok", device: "dev-9", secret: SECRET, origin: UI });
    expect(await pair()).toMatchObject({ ok: true, importing: true });
    expect(api.exchangePairing).toHaveBeenCalledOnce();
  });

  it("fails closed when the daemon issues no secret or a malformed one, storing nothing", async () => {
    for (const secret of [undefined, "short", "AB".repeat(32), 7 as unknown as string]) {
      api.exchangePairing.mockResolvedValueOnce({ token: "tok", device_id: "dev-9", import_secret: secret });
      expect(await pair("c")).toMatchObject({ ok: false, reason: "exchange_failed", message: expect.stringContaining("import secret") });
    }
    expect(session.pairing).toBeUndefined();
    expect(local.importPairing).toBeUndefined();
  });

  it("is stale, so unusable, once the token is replaced, lost with the browser session or the secret is malformed", async () => {
    api.exchangePairing.mockResolvedValueOnce({ token: "tok", device_id: "dev-9", import_secret: SECRET });
    await pair("c");
    session.pairing = { url: UI, token: "other", device: "dev-2" };
    expect(await importCredential()).toBeNull();
    delete session.pairing;
    expect(await importCredential()).toBeNull();
    session.pairing = { url: UI, token: "tok", device: "dev-9" };
    expect(await importCredential()).not.toBeNull();
    local.importPairing = { device: "dev-9", secret: "nope" };
    expect(await importCredential()).toBeNull();
    session.pairing = { url: UI, token: "pasted" };
    local.importPairing = { device: "dev-9", secret: SECRET };
    expect(await importCredential()).toBeNull();
  });

  it("re-pairs a notification-only pairing with a new code and reports the token it replaced", async () => {
    await handleDesktopRequest({ type: "openside.desktop.enable", code: "c1" }, UI);
    expect(local.importPairing).toBeUndefined();
    api.exchangePairing.mockResolvedValueOnce({ token: "tok2", device_id: "dev-2", import_secret: SECRET });
    expect(await pair("c2")).toMatchObject({ ok: true, device: "dev-2", importing: true, enabled: true, replaced: "dev-1" });
    expect(session.pairing).toEqual({ url: UI, token: "tok2", device: "dev-2" });
  });

  it("a later notification pairing drops the stale secret", async () => {
    local.importPairing = { device: "gone", secret: SECRET };
    await handleDesktopRequest({ type: "openside.desktop.enable", code: "c" }, UI);
    expect(local.importPairing).toBeUndefined();
  });

  it("Turn off notifications keeps the token and hands the page nothing to revoke", async () => {
    api.exchangePairing.mockResolvedValueOnce({ token: "tok", device_id: "dev-9", import_secret: SECRET });
    await pair("c");
    await handleDesktopRequest({ type: "openside.desktop.enable" }, UI);
    expect(await handleDesktopRequest({ type: "openside.desktop.disable" }, UI)).toMatchObject({ ok: true, enabled: false, device: null, importing: true });
    expect(session.pairing).toEqual({ url: UI, token: "tok", device: "dev-9" });
    expect(await importCredential()).not.toBeNull();
  });
});

describe("extension-owned import connection consent", () => {
  it("opens the extension page for an unknown origin and redeems only after the person allows", async () => {
    api.exchangePairing.mockResolvedValueOnce({ token: "tok", device_id: "dev-new", import_secret: SECRET });
    const pending = handleDesktopRequest({ type: "openside.desktop.pair", code: "code" }, UI);
    await vi.waitFor(() => expect(chromeStub.tabs.create).toHaveBeenCalledOnce());
    expect(chromeStub.tabs.create.mock.calls[0]?.[0]).toMatchObject({ url: expect.stringContaining("chrome-extension://fixture/import-connect.html#") });
    expect(api.exchangePairing).not.toHaveBeenCalled();
    expect(session.pairing).toBeUndefined();
    answerConsent(true);
    expect(await pending).toMatchObject({ ok: true, importing: true });
    expect(local.importPairing).toMatchObject({ origin: UI });
  });

  it("declines without redeeming or storing any pairing", async () => {
    const pending = handleDesktopRequest({ type: "openside.desktop.pair", code: "code" }, UI);
    await vi.waitFor(() => expect(chromeStub.tabs.create).toHaveBeenCalledOnce());
    answerConsent(false);
    expect(await pending).toMatchObject({ ok: false, reason: "refused" });
    expect(api.exchangePairing).not.toHaveBeenCalled();
    expect(session.pairing).toBeUndefined();
    expect(local.importPairing).toBeUndefined();
  });

  it("silently re-pairs only when the retained key proves the exact replacement response", async () => {
    local.importPairing = { device: OLD_DEVICE, secret: SECRET, origin: UI };
    continuity.valid = true;
    api.exchangePairing.mockResolvedValueOnce({ token: "tok", device_id: "dev-new", import_secret: "cd".repeat(32) });
    expect(await handleDesktopRequest({ type: "openside.desktop.pair", code: "code" }, UI)).toMatchObject({ ok: true });
    expect(continuity.received).toHaveBeenCalledWith(UI, { device: OLD_DEVICE, secret: SECRET, origin: UI });
    expect(chromeStub.tabs.create).not.toHaveBeenCalled();
    expect(local.importPairing).toEqual({ device: "dev-new", secret: "cd".repeat(32), origin: UI });
  });

  it("a same-origin attacker without the old key prompts and cannot replace it on decline", async () => {
    const previous = { device: OLD_DEVICE, secret: SECRET, origin: UI };
    local.importPairing = previous;
    const candidate = { token: "attacker-token", device_id: "attacker", import_secret: "ef".repeat(32) };
    api.exchangePairing.mockResolvedValueOnce(candidate);
    const pending = handleDesktopRequest({ type: "openside.desktop.pair", code: "code" }, UI);
    await vi.waitFor(() => expect(chromeStub.tabs.create).toHaveBeenCalledOnce());
    expect(Object.values(session).some(value => typeof value === "object" && value !== null && "changedApp" in value && value.changedApp === true)).toBe(true);
    expect(local.importPairing).toEqual(previous);
    expect(session.pairing).toBeUndefined();
    answerConsent(false);
    expect(await pending).toMatchObject({ ok: false, reason: "refused" });
    expect(local.importPairing).toEqual(previous);
    expect(session.pairing).toBeUndefined();
    expect(candidate.token).toBe("");
    expect(candidate.import_secret).toBeUndefined();
  });

  it("a failed continuity proof can establish a new app only after fresh person consent", async () => {
    local.importPairing = { device: OLD_DEVICE, secret: SECRET, origin: UI };
    api.exchangePairing.mockResolvedValueOnce({ token: "fresh-token", device_id: "fresh-app", import_secret: "ef".repeat(32) });
    const pending = handleDesktopRequest({ type: "openside.desktop.pair", code: "code" }, UI);
    await vi.waitFor(() => expect(chromeStub.tabs.create).toHaveBeenCalledOnce());
    expect(session.pairing).toBeUndefined();
    answerConsent(true);
    expect(await pending).toMatchObject({ ok: true, importing: true });
    expect(local.importPairing).toMatchObject({ device: "fresh-app", secret: "ef".repeat(32) });
  });

  it("origin or host permission without a retained key never bypasses fresh consent", async () => {
    local.importPairing = { device: OLD_DEVICE, origin: UI };
    const pending = handleDesktopRequest({ type: "openside.desktop.pair", code: "code" }, UI);
    await vi.waitFor(() => expect(chromeStub.tabs.create).toHaveBeenCalledOnce());
    expect(api.exchangePairing).not.toHaveBeenCalled();
    answerConsent(false);
    expect(await pending).toMatchObject({ ok: false, reason: "refused" });
  });

  it("discards an unverified replacement on consent timeout without changing the old key", async () => {
    vi.useFakeTimers();
    try {
      const previous = { device: OLD_DEVICE, secret: SECRET, origin: UI };
      local.importPairing = previous;
      const candidate = { token: "timed-out-token", device_id: "candidate", import_secret: "ef".repeat(32) };
      api.exchangePairing.mockResolvedValueOnce(candidate);
      const pending = handleDesktopRequest({ type: "openside.desktop.pair", code: "code" }, UI);
      await vi.advanceTimersByTimeAsync(0);
      expect(chromeStub.tabs.create).toHaveBeenCalledOnce();
      await vi.advanceTimersByTimeAsync(120_000);
      expect(await pending).toMatchObject({ ok: false, reason: "refused" });
      expect(local.importPairing).toEqual(previous);
      expect(session.pairing).toBeUndefined();
      expect(candidate.token).toBe("");
      expect(candidate.import_secret).toBeUndefined();
      expect(Object.keys(session).some(key => key.startsWith("importConnect:"))).toBe(false);
    } finally { vi.useRealTimers(); }
  });

  it("a remembered key for another origin never permits silent pairing", async () => {
    local.importPairing = { device: OLD_DEVICE, secret: SECRET, origin: UI };
    const different = handleDesktopRequest({ type: "openside.desktop.pair", code: "code2" }, "http://localhost:47821");
    await vi.waitFor(() => expect(chromeStub.tabs.create).toHaveBeenCalledOnce());
    expect(api.exchangePairing).not.toHaveBeenCalled();
    answerConsent(false);
    expect(await different).toMatchObject({ ok: false, reason: "refused" });
    expect(local.importPairing).toMatchObject({ origin: UI });
  });
});

describe("checkNotifications", () => {
  beforeEach(() => {
    session.pairing = { url: UI, token: "t" };
    local.desktopNotifications = true;
  });
  const titles = () => chromeStub.notifications.create.mock.calls.map(([id, options]) => [id, (options as { title: string }).title]);

  it("notifies once when a running task finishes, naming the conversation", async () => {
    api.listSessions.mockResolvedValue({ sessions: [task("s1", "idle"), task("s2", "running", "Summarise the report")] });
    expect(await checkNotifications()).toBe(true);
    expect(chromeStub.notifications.create).not.toHaveBeenCalled();

    api.listSessions.mockResolvedValue({ sessions: [task("s1", "idle"), task("s2", "idle", "Summarise the report")] });
    expect(await checkNotifications()).toBe(false);
    expect(titles()).toEqual([["settled:s2", "Task finished"]]);
    expect(chromeStub.notifications.create.mock.calls[0]![1]).toMatchObject({ message: "Summarise the report" });

    await checkNotifications();
    expect(chromeStub.notifications.create).toHaveBeenCalledOnce();
  });

  it("does not announce what was already idle or stopped, nor the first look after enabling", async () => {
    api.listSessions.mockResolvedValue({ sessions: [task("s1", "idle"), task("s2", "running")] });
    await checkNotifications();
    api.listSessions.mockResolvedValue({ sessions: [task("s1", "running"), task("s2", "stopped"), task("s3", "idle")] });
    await checkNotifications();
    expect(chromeStub.notifications.create).not.toHaveBeenCalled();
  });

  it("stays silent for the conversation open in the focused window, but not when the window is unfocused", async () => {
    api.listSessions.mockResolvedValue({ sessions: [task("s1", "running"), task("s2", "running")] });
    await checkNotifications();
    focusedWindow = { focused: true, tabs: [{ active: false, url: `${UI}/sessions/s2` }, { active: true, url: `${UI}/sessions/s1?ticket=k9` }] };
    api.listSessions.mockResolvedValue({ sessions: [task("s1", "idle"), task("s2", "idle")] });
    await checkNotifications();
    expect(titles()).toEqual([["settled:s2", "Task finished"]]);

    chromeStub.notifications.create.mockClear();
    api.listSessions.mockResolvedValue({ sessions: [task("s1", "running")] });
    await checkNotifications();
    focusedWindow = { focused: false, tabs: [{ active: true, url: `${UI}/sessions/s1` }] };
    api.listSessions.mockResolvedValue({ sessions: [task("s1", "idle")] });
    await checkNotifications();
    expect(titles()).toEqual([["settled:s1", "Task finished"]]);
  });

  it("announces every pending request, not only code requests, except the focused conversation's", async () => {
    api.listTickets.mockResolvedValue({ tickets: [confirm, { ...confirm, id: "k2", session_id: "s2", kind: "info", summary: "Which colour?" } as Ticket] });
    focusedWindow = { focused: true, tabs: [{ active: true, url: `${UI}/sessions/s1` }] };
    await checkNotifications();
    expect(titles()).toEqual([["code:k2", "Question from the agent"]]);
    expect(chromeStub.action.setBadgeText).toHaveBeenLastCalledWith({ text: "2" });
    await checkNotifications();
    expect(chromeStub.notifications.create).toHaveBeenCalledOnce();
  });

  it("with desktop notifications off keeps the code-only behaviour and never lists sessions", async () => {
    local.desktopNotifications = false;
    api.listTickets.mockResolvedValue({ tickets: [confirm] });
    expect(await checkNotifications()).toBe(false);
    expect(chromeStub.notifications.create).not.toHaveBeenCalled();
    expect(api.listSessions).not.toHaveBeenCalled();
  });
});

describe("openConversation", () => {
  it("reuses an open openside tab and focuses its window, else opens a new tab", async () => {
    session.pairing = { url: UI, token: "t" };
    chromeStub.tabs.query.mockResolvedValueOnce([{ id: 4, windowId: 2 }]);
    await openConversation("s 1");
    expect(chromeStub.tabs.update).toHaveBeenCalledWith(4, { url: `${UI}/sessions/s%201`, active: true });
    expect(chromeStub.windows.update).toHaveBeenCalledWith(2, { focused: true });
    await openConversation("s2");
    expect(chromeStub.tabs.create).toHaveBeenCalledWith({ url: `${UI}/sessions/s2` });
  });
});
