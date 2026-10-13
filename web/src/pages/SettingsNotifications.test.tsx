import { render, renderHook, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ClientContext } from "../context";
import { askExtension, pairExtension, useDesktopResync } from "../desktopNotify";
import { EXTENSION_ID, type DesktopReply, type DesktopRequest } from "../desktopNotifyProtocol";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { SettingsPage } from "./SettingsPage";

const VAPID = "BEl62iUYgUivxIkv69yViEuiBIa-Ib9-SkvMeAtA3LFgDzkrxZJjSgSnfckjBJuBkr3qBUYIHBQFLXYp5Nksh8U";

/** A browser whose service workers exist but whose push service rejects, like the patched openside browser. */
function stubBrowser(options: { permission?: NotificationPermission; subscribe?: () => Promise<never>; push?: boolean } = {}) {
  const subscribe = vi.fn(options.subscribe ?? (async () => Promise.reject(new DOMException("Registration failed - push service error", "AbortError"))));
  const registration = { pushManager: { getSubscription: async () => null, subscribe } };
  Object.defineProperty(navigator, "serviceWorker", {
    configurable: true,
    value: { getRegistration: async () => registration, register: async () => registration, ready: Promise.resolve(registration) },
  });
  if (options.push !== false) vi.stubGlobal("PushManager", class {});
  const requestPermission = vi.fn(async () => options.permission ?? "granted");
  vi.stubGlobal("Notification", { permission: "default", requestPermission });
  return { subscribe, requestPermission };
}

/** The openside extension's side of the bridge, answering like extension/src/desktop.ts. */
function stubExtension(initial: { paired?: boolean; conflict?: boolean; failExchange?: boolean; importing?: boolean } = {}) {
  const state = { paired: initial.paired ?? false, enabled: false, importing: initial.importing ?? false, device: initial.paired ? "d-ext" : null as string | null };
  const requests: DesktopRequest[] = [];
  const sendMessage = vi.fn((id: string, request: DesktopRequest, callback: (reply: DesktopReply) => void) => {
    expect(id).toBe(EXTENSION_ID);
    requests.push(request);
    let reply: DesktopReply;
    if (request.type === "openside.desktop.status") reply = { ok: true, paired: state.paired, enabled: state.enabled, device: state.device, importing: state.importing };
    else if (request.type === "openside.desktop.disable") {
      reply = { ok: true, paired: state.paired, enabled: false, device: state.importing ? null : state.device, importing: state.importing };
      state.enabled = false;
      state.paired &&= state.importing;
    } else if (request.type === "openside.desktop.pair") {
      if (state.importing) reply = { ok: true, paired: true, enabled: state.enabled, device: state.device, importing: true, replaced: null };
      else if (!request.code) reply = { ok: false, reason: "needs_pairing", message: "not paired" };
      else {
        const replaced = state.device;
        state.paired = state.importing = true;
        state.device = "d-import";
        reply = { ok: true, paired: true, enabled: state.enabled, device: "d-import", importing: true, replaced };
      }
    } else if (initial.conflict) reply = { ok: false, reason: "pairing_conflict", message: "The openside extension is paired with http://127.0.0.1:9. Forget that pairing in the extension options, then try again." };
    else if (!state.paired && !request.code) reply = { ok: false, reason: "needs_pairing", message: "not paired" };
    else if (initial.failExchange) reply = { ok: false, reason: "exchange_failed", message: "The daemon did not accept the pairing code. Try again." };
    else {
      state.paired = true;
      state.enabled = true;
      state.device ??= "d-ext";
      reply = { ok: true, paired: true, enabled: true, device: state.device, importing: state.importing };
    }
    queueMicrotask(() => callback(reply));
  });
  vi.stubGlobal("chrome", { runtime: { sendMessage } });
  return { state, requests };
}

function daemon(override?: (call: Call) => Response | undefined) {
  return fakeClient((call) => {
    const mine = override?.(call);
    if (mine) return mine;
    const path = new URL(call.path, "http://daemon.test").pathname;
    if (path === "/v1/push/vapid") return json(200, { public_key: VAPID });
    if (path === "/v1/push/subscriptions") return json(200, { subscriptions: [] });
    if (path === "/v1/pairing") return json(201, { code: "one-time", expires_at: "2026-10-13T00:03:00Z", kind: "person_device", path: "/pair", scopes: ["read"], url: null });
    if (path.startsWith("/v1/devices/")) return json(204, undefined);
    if (path === "/v1/health") return json(200, { ok: true, api_version: "1" });
    return json(404, { code: "not_found", message: "nope" });
  });
}

function setup(override?: (call: Call) => Response | undefined) {
  const fake = daemon(override);
  render(
    <ClientContext.Provider value={fake.client}>
      <SettingsPage onModels={vi.fn()} onChannels={vi.fn()} onRemote={vi.fn()} onWorkspace={vi.fn()} onGuide={vi.fn()} onSignOut={vi.fn()} />
    </ClientContext.Provider>,
  );
  return fake;
}

const section = () => screen.getByRole("region", { name: "Notifications" });
const enable = async () => userEvent.click(await within(section()).findByRole("button", { name: "Enable notifications" }));

afterEach(() => {
  vi.unstubAllGlobals();
  Reflect.deleteProperty(navigator, "serviceWorker");
  localStorage.clear();
});

describe("Notifications without a push service", () => {
  it("falls back to the extension's desktop notifications when subscribe() is rejected", async () => {
    const browser = stubBrowser();
    const extension = stubExtension();
    const { calls } = setup();
    await enable();
    expect(await within(section()).findByRole("button", { name: "Turn off notifications" })).toBeInTheDocument();
    expect(within(section()).getByText("Enabled (desktop)")).toBeInTheDocument();
    expect(within(section()).getByText(/Using desktop notifications from the openside extension instead/)).toBeInTheDocument();
    expect(browser.subscribe).toHaveBeenCalledOnce();
    expect(within(section()).queryByRole("alert")).toBeNull();
    expect(extension.requests.at(-1)).toEqual({ type: "openside.desktop.enable", code: "one-time" });
    const pairing = calls.find((call) => call.method === "POST" && call.path === "/v1/pairing")!;
    expect(pairing.body).toMatchObject({ name: "openside extension", kind: "person_device", scopes: ["read", "steer", "approve", "human_step"] });
    expect(pairing.body).not.toHaveProperty("import_delivery");
    expect(calls.some((call) => call.path === "/v1/push/subscriptions" && call.method === "POST")).toBe(false);
    expect(JSON.parse(localStorage.getItem("openside.notify.desktop")!)).toEqual({ device: "d-ext" });
  });

  it("does not mint a pairing code when the extension is already paired to this daemon", async () => {
    stubBrowser();
    stubExtension({ paired: true });
    const { calls } = setup();
    await enable();
    expect(await within(section()).findByText("Enabled (desktop)")).toBeInTheDocument();
    expect(calls.some((call) => call.path === "/v1/pairing")).toBe(false);
  });

  it("uses the extension without asking for site permission when notifications are blocked for the site", async () => {
    const browser = stubBrowser({ permission: "denied" });
    stubExtension();
    const { calls } = setup();
    await enable();
    expect(await within(section()).findByText("Enabled (desktop)")).toBeInTheDocument();
    expect(browser.subscribe).not.toHaveBeenCalled();
    expect(calls.some((call) => call.path === "/v1/push/vapid")).toBe(false);
  });

  it("without the extension explains how to allow notifications when they are blocked", async () => {
    stubBrowser({ permission: "denied" });
    setup();
    await enable();
    expect(await within(section()).findByRole("alert")).toHaveTextContent("Click the lock icon in the address bar, set Notifications to Allow, then try again.");
  });

  it("without the extension says the push service is unavailable and what to install", async () => {
    stubBrowser();
    setup();
    await enable();
    const alert = await within(section()).findByRole("alert");
    expect(alert).toHaveTextContent("push service is not available");
    expect(alert).toHaveTextContent("Install the openside extension");
    expect(alert).not.toHaveTextContent("Request failed");
  });

  it("shows the daemon's own message when it cannot provide the VAPID key", async () => {
    const browser = stubBrowser();
    stubExtension();
    setup((call) => (call.path === "/v1/push/vapid" ? json(500, { code: "internal", message: "VAPID key file is unreadable" }) : undefined));
    await enable();
    expect(await within(section()).findByRole("alert")).toHaveTextContent("The daemon could not provide its push key: VAPID key file is unreadable");
    expect(browser.subscribe).not.toHaveBeenCalled();
  });

  it("shows the browser's message for a subscribe() failure that is neither a missing service nor a permission", async () => {
    stubBrowser({ subscribe: async () => Promise.reject(new DOMException("The provided applicationServerKey is not valid.", "InvalidAccessError")) });
    stubExtension();
    setup();
    await enable();
    const alert = await within(section()).findByRole("alert");
    expect(alert).toHaveTextContent("The provided applicationServerKey is not valid.");
    expect(alert).not.toHaveTextContent("Request failed");
  });

  it("offers Enable when the browser has no Push API but the extension is installed", async () => {
    stubBrowser({ push: false });
    const extension = stubExtension();
    const { calls } = setup();
    await enable();
    expect(await within(section()).findByText("Enabled (desktop)")).toBeInTheDocument();
    expect(calls.some((call) => call.path === "/v1/push/vapid")).toBe(false);
    expect(extension.state.enabled).toBe(true);
  });

  it("keeps saying Unavailable when the browser has no Push API and no extension", async () => {
    stubBrowser({ push: false });
    setup();
    expect(await within(section()).findByText(/cannot receive push notifications here/)).toBeInTheDocument();
    expect(within(section()).queryByRole("button")).toBeNull();
  });

  it("turns desktop notifications off, asks the extension to stop and revokes the token it minted", async () => {
    stubBrowser();
    const extension = stubExtension();
    const { calls } = setup();
    await enable();
    await userEvent.click(await within(section()).findByRole("button", { name: "Turn off notifications" }));
    expect(await within(section()).findByRole("button", { name: "Enable notifications" })).toBeInTheDocument();
    expect(extension.requests.at(-1)).toEqual({ type: "openside.desktop.disable" });
    expect(calls.some((call) => call.method === "DELETE" && call.path === "/v1/devices/d-ext")).toBe(true);
    expect(localStorage.getItem("openside.notify.desktop")).toBeNull();
  });

  it("shows desktop notifications as on after a reload while the extension still has them enabled", async () => {
    stubBrowser();
    const extension = stubExtension({ paired: true });
    extension.state.enabled = true;
    localStorage.setItem("openside.notify.desktop", JSON.stringify({ device: "d-ext" }));
    setup();
    expect(await within(section()).findByText("Enabled (desktop)")).toBeInTheDocument();
  });

  it("reports an extension that is paired with another daemon", async () => {
    stubBrowser();
    stubExtension({ conflict: true });
    setup();
    await enable();
    expect(await within(section()).findByRole("alert")).toHaveTextContent("The openside extension is paired with http://127.0.0.1:9.");
  });

  it("says why pairing the extension failed instead of a generic error", async () => {
    stubBrowser();
    stubExtension();
    setup((call) => (call.path === "/v1/pairing" ? json(403, { code: "local_access_required", message: "no" }) : undefined));
    await enable();
    const alert = await within(section()).findByRole("alert");
    expect(alert).toHaveTextContent("Could not pair the openside extension: Reopen openside using the desktop launcher");
    expect(alert).not.toHaveTextContent(/^Request failed$/);
  });
});

describe("useDesktopResync", () => {
  it("re-pairs a restarted browser's extension quietly, replacing the stale device", async () => {
    const extension = stubExtension();
    localStorage.setItem("openside.notify.desktop", JSON.stringify({ device: "d-old" }));
    const fake = daemon();
    renderHook(() => useDesktopResync(fake.client));
    await vi.waitFor(() => expect(extension.state.enabled).toBe(true));
    expect(fake.calls.some((call) => call.method === "DELETE" && call.path === "/v1/devices/d-old")).toBe(true);
    expect(fake.calls.some((call) => call.method === "POST" && call.path === "/v1/pairing")).toBe(true);
  });

  it("does nothing when the person never turned desktop notifications on", async () => {
    const extension = stubExtension();
    const fake = daemon();
    renderHook(() => useDesktopResync(fake.client));
    await Promise.resolve();
    expect(extension.requests).toEqual([]);
    expect(fake.calls).toEqual([]);
  });
});

describe("pairExtension", () => {
  it("pairs for imports with a fresh code without switching notifications on", async () => {
    const extension = stubExtension();
    const fake = daemon();
    const reply = await pairExtension(fake.client);
    expect(reply).toMatchObject({ ok: true, importing: true, device: "d-import", enabled: false });
    expect(extension.requests).toEqual([{ type: "openside.desktop.pair" }, { type: "openside.desktop.pair", code: "one-time" }]);
    expect(extension.state.enabled).toBe(false);
    expect(localStorage.getItem("openside.notify.desktop")).toBeNull();
    expect(fake.calls.find((call) => call.path === "/v1/pairing")!.body).toMatchObject({ import_delivery: true });
  });

  it("asks for no code once the extension holds an import pairing", async () => {
    const extension = stubExtension({ paired: true, importing: true });
    const fake = daemon();
    expect(await pairExtension(fake.client)).toMatchObject({ ok: true, importing: true });
    expect(extension.requests).toEqual([{ type: "openside.desktop.pair" }]);
    expect(fake.calls).toEqual([]);
  });

  it("revokes the notification token an import pairing replaced and remembers the new one", async () => {
    stubExtension({ paired: true });
    localStorage.setItem("openside.notify.desktop", JSON.stringify({ device: "d-ext" }));
    const fake = daemon();
    await pairExtension(fake.client);
    expect(fake.calls.some((call) => call.method === "DELETE" && call.path === "/v1/devices/d-ext")).toBe(true);
    expect(JSON.parse(localStorage.getItem("openside.notify.desktop")!)).toEqual({ device: "d-import" });
  });

  it("returns what the extension said instead of pairing when it refuses, and null when it is absent", async () => {
    vi.stubGlobal("chrome", { runtime: { sendMessage: (_id: string, _request: unknown, callback: (reply: DesktopReply) => void) => callback({ ok: false, reason: "pairing_conflict", message: "elsewhere" }) } });
    const fake = daemon();
    expect(await pairExtension(fake.client)).toEqual({ ok: false, reason: "pairing_conflict", message: "elsewhere" });
    expect(fake.calls).toEqual([]);
    vi.unstubAllGlobals();
    expect(await pairExtension(fake.client)).toBeNull();
  });

  it("Turn off notifications keeps the token an import pairing still needs", async () => {
    stubBrowser();
    stubExtension({ paired: true, importing: true }).state.enabled = true;
    localStorage.setItem("openside.notify.desktop", JSON.stringify({ device: "d-ext" }));
    const { calls } = setup();
    await userEvent.click(await within(section()).findByRole("button", { name: "Turn off notifications" }));
    expect(await within(section()).findByRole("button", { name: "Enable notifications" })).toBeInTheDocument();
    expect(calls.some((call) => call.method === "DELETE")).toBe(false);
  });
});

describe("askExtension timeout", () => {
  it("waits for the consent click on an import redemption but gives up quickly on everything else", async () => {
    vi.useFakeTimers();
    try {
      vi.stubGlobal("chrome", { runtime: { sendMessage: vi.fn() } });
      const quick = askExtension({ type: "openside.desktop.status" });
      const consent = askExtension({ type: "openside.desktop.pair", code: "c" });
      const settled = vi.fn();
      void consent.then(settled);
      await vi.advanceTimersByTimeAsync(4_001);
      expect(await quick).toBeNull();
      expect(settled).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(146_000);
      expect(await consent).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });
});
