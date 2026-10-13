import { act, render, renderHook, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ClientContext } from "../context";
import { usePushPresence } from "../push";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { SettingsPage } from "./SettingsPage";

const SUBSCRIPTION = {
  endpoint: "https://push.example/send/abc",
  keys: { p256dh: "BPub", auth: "secret-auth" },
};

/** A browser with service workers and Push API whose subscribe() yields SUBSCRIPTION. */
function stubPushBrowser(permission: NotificationPermission = "granted") {
  type Subscription = { toJSON: () => typeof SUBSCRIPTION; unsubscribe: () => Promise<boolean> };
  let current: Subscription | null = null;
  const subscribe = vi.fn<(options: PushSubscriptionOptionsInit) => Promise<Subscription>>(async () => {
    current = { toJSON: () => SUBSCRIPTION, unsubscribe: async () => ((current = null), true) };
    return current;
  });
  const pushManager = { getSubscription: async () => current, subscribe };
  const registration = { pushManager };
  Object.defineProperty(navigator, "serviceWorker", {
    configurable: true,
    value: { getRegistration: async () => registration, register: async () => registration, ready: Promise.resolve(registration) },
  });
  vi.stubGlobal("PushManager", class {});
  const requestPermission = vi.fn(async () => permission);
  vi.stubGlobal("Notification", { permission: "default", requestPermission });
  return { subscribe, requestPermission, active: () => current !== null };
}

function daemon({ healthy = true }: { healthy?: boolean } = {}) {
  const subscriptions: { id: string; endpoint: string }[] = [];
  return fakeClient((call: Call) => {
    const url = new URL(call.path, "http://daemon.test");
    if (url.pathname === "/v1/health") return healthy ? json(200, { ok: true, api_version: "1" }) : json(503, { code: "unavailable", message: "down" });
    if (url.pathname === "/v1/push/vapid") return json(200, { public_key: "BEl62iUYgUivxIkv69yViEuiBIa-Ib9-SkvMeAtA3LFgDzkrxZJjSgSnfckjBJuBkr3qBUYIHBQFLXYp5Nksh8U" });
    if (url.pathname === "/v1/push/subscriptions" && call.method === "GET") return json(200, { subscriptions });
    if (url.pathname === "/v1/push/subscriptions" && call.method === "POST") {
      const sub = { id: "p1", endpoint: (call.body as typeof SUBSCRIPTION).endpoint, created_at: "2026-10-12T00:00:00Z" };
      subscriptions.push(sub);
      return json(201, sub);
    }
    if (url.pathname.startsWith("/v1/push/subscriptions/")) {
      subscriptions.splice(0);
      return json(204, undefined);
    }
    if (url.pathname === "/v1/push/presence") return json(204, undefined);
    return json(404, { code: "not_found", message: "nope" });
  });
}

function setup(options?: { healthy?: boolean }) {
  const fake = daemon(options);
  const handlers = { onModels: vi.fn(), onChannels: vi.fn(), onRemote: vi.fn(), onWorkspace: vi.fn(), onGuide: vi.fn(), onSignOut: vi.fn() };
  render(
    <ClientContext.Provider value={fake.client}>
      <SettingsPage {...handlers} />
    </ClientContext.Provider>,
  );
  return { ...fake, ...handlers };
}

const notifications = () => screen.getByRole("region", { name: "Notifications" });

afterEach(() => {
  vi.unstubAllGlobals();
  Reflect.deleteProperty(navigator, "serviceWorker");
  localStorage.clear();
});

describe("SettingsPage", () => {
  it("shows the daemon connection and opens Workspace and Sign out", async () => {
    const user = userEvent.setup();
    const { onWorkspace, onSignOut } = setup();
    const connection = screen.getByRole("region", { name: "Connection" });
    expect(await within(connection).findByText("Connected")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Workspace/ }));
    expect(onWorkspace).toHaveBeenCalledOnce();
    await user.click(screen.getByRole("button", { name: "Sign out" }));
    expect(onSignOut).toHaveBeenCalledOnce();
  });

  it("opens Models & accounts", async () => {
    const user = userEvent.setup();
    const { onModels } = setup();
    await user.click(screen.getByRole("button", { name: /Models & accounts/ }));
    expect(onModels).toHaveBeenCalledOnce();
  });

  it("opens Channels", async () => {
    const user = userEvent.setup();
    const { onChannels } = setup();
    await user.click(screen.getByRole("button", { name: /Channels/ }));
    expect(onChannels).toHaveBeenCalledOnce();
  });

  it("opens Remote access", async () => {
    const user = userEvent.setup();
    const { onRemote } = setup();
    await user.click(screen.getByRole("button", { name: /Remote access/ }));
    expect(onRemote).toHaveBeenCalledOnce();
  });

  it("reopens the setup guide", async () => {
    const user = userEvent.setup();
    const { onGuide } = setup();
    await user.click(screen.getByRole("button", { name: /Setup guide/ }));
    expect(onGuide).toHaveBeenCalledOnce();
  });

  it("reports an unreachable daemon", async () => {
    setup({ healthy: false });
    expect(await within(screen.getByRole("region", { name: "Connection" })).findByText("Unavailable")).toBeInTheDocument();
  });

  // Permission is requested only from this click (DESIGN.md: no prompt on page load).
  it("enables notifications by registering the browser subscription with the daemon, then turns them off", async () => {
    const user = userEvent.setup();
    const browser = stubPushBrowser();
    const { calls } = setup();
    const enable = await within(notifications()).findByRole("button", { name: "Enable notifications" });
    expect(browser.requestPermission).not.toHaveBeenCalled();
    await user.click(enable);
    expect(await within(notifications()).findByRole("button", { name: "Turn off notifications" })).toBeInTheDocument();
    expect(within(notifications()).getByText("Enabled")).toBeInTheDocument();
    expect(browser.requestPermission).toHaveBeenCalledOnce();
    expect(browser.subscribe).toHaveBeenCalledWith(expect.objectContaining({ userVisibleOnly: true }));
    expect(browser.subscribe.mock.calls[0]![0].applicationServerKey).toHaveLength(65);
    expect(calls.find((c) => c.method === "POST" && c.path === "/v1/push/subscriptions")?.body).toEqual(SUBSCRIPTION);
    expect(localStorage.getItem("openside.push.subscription")).toBe("p1");

    await user.click(screen.getByRole("button", { name: "Turn off notifications" }));
    expect(await screen.findByRole("button", { name: "Enable notifications" })).toBeInTheDocument();
    expect(calls.some((c) => c.method === "DELETE" && c.path === "/v1/push/subscriptions/p1")).toBe(true);
    expect(localStorage.getItem("openside.push.subscription")).toBeNull();
    expect(browser.active()).toBe(false);
  });

  it("explains a denied permission", async () => {
    const user = userEvent.setup();
    stubPushBrowser("denied");
    const { calls } = setup();
    await user.click(await within(notifications()).findByRole("button", { name: "Enable notifications" }));
    expect(await within(notifications()).findByRole("alert")).toHaveTextContent("Notifications are blocked for this site.");
    expect(calls.some((c) => c.path === "/v1/push/subscriptions" && c.method === "POST")).toBe(false);
  });

  it("without the Push API says why and offers no button", async () => {
    setup();
    expect(within(notifications()).getByText(/cannot receive push notifications here/)).toBeInTheDocument();
    expect(within(notifications()).queryByRole("button")).toBeNull();
  });

  it("in an iPhone browser tab shows Home Screen installation instead of a prompt", async () => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) Safari/604.1");
    stubPushBrowser();
    setup();
    expect(within(notifications()).getByText(/Add to Home Screen/)).toBeInTheDocument();
    expect(within(notifications()).queryByRole("button")).toBeNull();
    vi.restoreAllMocks();
  });
});

describe("usePushPresence", () => {
  it("reports focus changes for a stored subscription and forgets one the daemon no longer knows", async () => {
    localStorage.setItem("openside.push.subscription", "p9");
    let known = true;
    const fake = fakeClient(() => (known ? json(204, undefined) : json(404, { code: "unknown_subscription", message: "gone" })));
    const hasFocus = vi.spyOn(document, "hasFocus").mockReturnValue(true);
    renderHook(() => usePushPresence(fake.client));
    await vi.waitFor(() => expect(fake.calls).toHaveLength(1));
    expect(fake.calls[0]).toMatchObject({ method: "POST", path: "/v1/push/presence", body: { subscription_id: "p9", focused: true } });

    hasFocus.mockReturnValue(false);
    known = false;
    act(() => {
      window.dispatchEvent(new Event("blur"));
    });
    await vi.waitFor(() => expect(localStorage.getItem("openside.push.subscription")).toBeNull());
    expect(fake.calls[1]?.body).toEqual({ subscription_id: "p9", focused: false });
  });
});
