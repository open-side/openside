import { afterEach, expect, it, vi } from "vitest";
const api = vi.hoisted(() => ({ createSession: vi.fn(async () => ({ id: "new-session" })), submitMessage: vi.fn<(_id: string, request: { text: string }) => Promise<object>>().mockResolvedValue({}) }));
vi.mock("../src/api", () => ({ client: () => api, ledger: vi.fn() }));
vi.mock("../src/agent-tabs", () => ({ groupAgentTabs: vi.fn(), clearAgentTabs: vi.fn(async () => undefined) }));
afterEach(() => vi.unstubAllGlobals());
it("rejects malformed/foreign/iframe requests before opening a panel or creating sessions", async () => {
  let onMessage!: (value: unknown, sender: chrome.runtime.MessageSender, reply: (value: unknown) => void) => boolean;
  let onMenu!: (info: chrome.contextMenus.OnClickData, tab: chrome.tabs.Tab) => void;
  const listener = { addListener: vi.fn() };
  const accessLevel = vi.fn(async () => undefined);
  const open = vi.fn(async () => undefined);
  vi.stubGlobal("__TEST_MATCHES__", false);
  vi.stubGlobal("chrome", {
    runtime: { id: "extension-id", getURL: () => "chrome-extension://extension-id/", onInstalled: listener, onMessage: { addListener: (callback: typeof onMessage) => { onMessage = callback; } }, openOptionsPage: vi.fn() },
    storage: { session: { get: vi.fn(async () => ({ pairing: { url: "http://localhost:43210", token: "scoped-token" } })), set: vi.fn(async () => undefined), remove: vi.fn(async () => undefined), setAccessLevel: accessLevel }, onChanged: listener },
    sidePanel: { open, setPanelBehavior: vi.fn(async () => undefined) },
    contextMenus: { create: vi.fn(), onClicked: { addListener: (callback: typeof onMenu) => { onMenu = callback; } } },
    omnibox: { onInputEntered: listener }, alarms: { create: vi.fn(), onAlarm: listener }, tabs: { onUpdated: listener, onRemoved: listener }, permissions: { contains: vi.fn(async () => false) }, notifications: { onClicked: listener },
  });
  await import("../src/background");
  expect(accessLevel).toHaveBeenCalledWith({ accessLevel: "TRUSTED_CONTEXTS" });
  const url = "https://mail.google.com/mail/u/0/#message";
  const sender = { id: "extension-id", url, frameId: 0, tab: { id: 4, url } } as chrome.runtime.MessageSender;
  const message = { type: "start", text: "Handle", context: { url, title: "Message", selection: "Selected text" } };
  for (const source of [{ ...sender, id: "foreign" }, { ...sender, frameId: 1 }, { ...sender, tab: { id: 4.5 } }]) expect(onMessage(message, source as chrome.runtime.MessageSender, vi.fn())).toBe(false);
  for (const value of [null, {}, { ...message, context: { url } }, { ...message, text: 4 }]) expect(onMessage(value, sender, vi.fn())).toBe(false);
  expect(open).not.toHaveBeenCalled();
  expect(api.createSession).not.toHaveBeenCalled();
  const invalidMenu = { menuItemId: "handle-openside", pageUrl: "https://attacker.test/", selectionText: "Page script text", frameId: 0 } as chrome.contextMenus.OnClickData;
  onMenu(invalidMenu, sender.tab!);
  expect(api.createSession).not.toHaveBeenCalled();
  const reply = vi.fn();
  expect(onMessage(message, sender, reply)).toBe(true);
  await vi.waitFor(() => expect(reply).toHaveBeenCalledWith({ id: "new-session" }));
  expect(api.createSession).toHaveBeenCalledTimes(1);
  expect(open).toHaveBeenCalledWith({ tabId: 4 });
  expect(api.submitMessage.mock.calls[0][1].text).toContain("Selected text");
});
it("opens the side panel inside the omnibox gesture before any async work", async () => {
  // background.ts registers listeners on the stubbed chrome global at load, so each case re-imports it.
  vi.resetModules();
  let onOmnibox!: (text: string) => void;
  const listener = { addListener: vi.fn() };
  const open = vi.fn(async () => undefined);
  const query = vi.fn(async () => [{ id: 7 }]);
  vi.stubGlobal("__TEST_MATCHES__", false);
  vi.stubGlobal("chrome", {
    runtime: { id: "extension-id", getURL: () => "chrome-extension://extension-id/", onInstalled: listener, onMessage: listener, openOptionsPage: vi.fn() },
    storage: { session: { get: vi.fn(async () => ({ pairing: { url: "http://localhost:43210", token: "scoped-token" } })), set: vi.fn(async () => undefined), remove: vi.fn(async () => undefined), setAccessLevel: vi.fn(async () => undefined) }, onChanged: listener },
    sidePanel: { open, setPanelBehavior: vi.fn(async () => undefined) },
    contextMenus: { create: vi.fn(), onClicked: listener },
    omnibox: { onInputEntered: { addListener: (callback: typeof onOmnibox) => { onOmnibox = callback; } } },
    windows: { WINDOW_ID_CURRENT: -2 },
    alarms: { create: vi.fn(), onAlarm: listener }, tabs: { query, onUpdated: listener, onRemoved: listener }, permissions: { contains: vi.fn(async () => false) }, notifications: { onClicked: listener },
  });
  await import("../src/background");
  api.createSession.mockClear();
  onOmnibox("what is on this page");
  // Chrome only honours sidePanel.open() while the listener is still on the stack.
  expect(open).toHaveBeenCalledWith({ windowId: -2 });
  await vi.waitFor(() => expect(api.createSession).toHaveBeenCalledTimes(1));
  expect(open).toHaveBeenCalledTimes(1);
});
