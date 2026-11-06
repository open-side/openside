import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_LASSO } from "../../web/src/lassoProtocol";

const api = vi.hoisted(() => ({ createSession: vi.fn(async () => ({ id: "new-session" })), submitMessage: vi.fn<(_id: string, request: { text: string }) => Promise<object>>().mockResolvedValue({}) }));
const mocks = vi.hoisted(() => ({ openConversation: vi.fn(async () => undefined) }));
vi.mock("../src/api", () => ({ client: () => api, ledger: vi.fn() }));
vi.mock("../src/agent-tabs", () => ({ groupAgentTabs: vi.fn(), clearAgentTabs: vi.fn(async () => undefined) }));
vi.mock("../src/launcher", () => ({ ADOPT_LAUNCHER_PAIRING: "openside.launcher.adopt", adoptLauncherPairing: vi.fn(async () => true) }));
vi.mock("../src/person-bridge", () => ({ connectPersonBridge: vi.fn(async () => undefined), resetPersonBridge: vi.fn() }));
vi.mock("../src/shell-commands", () => ({ registerShellCommands: vi.fn() }));
vi.mock("../src/desktop", async importOriginal => ({ ...await importOriginal<Record<string, unknown>>(), openConversation: mocks.openConversation }));

type Callback = (...args: never[]) => unknown;
let listeners: Map<string, Callback[]>;
let session: Record<string, unknown>;
let local: Record<string, unknown>;
const EXTENSION = "chrome-extension://extension-id/";

/** Every chrome event background.ts listens to, recorded by name so a case can raise it. */
function event(name: string) {
  return { addListener: (callback: Callback) => { listeners.set(name, [...listeners.get(name) ?? [], callback]); } };
}
function emit(name: string, ...args: unknown[]): unknown[] {
  return (listeners.get(name) ?? []).map(callback => (callback as (...values: unknown[]) => unknown)(...args));
}

/** Loads the worker against a fresh chrome stub; it registers its listeners when the module loads. */
async function loadWorker() {
  vi.resetModules();
  listeners = new Map();
  session = { pairing: { url: "http://localhost:43210", token: "scoped-token" } };
  local = {};
  const chrome = {
    runtime: {
      id: "extension-id", getURL: (path = "") => `${EXTENSION}${path}`, lastError: undefined, openOptionsPage: vi.fn(),
      getManifest: () => ({ externally_connectable: { matches: ["http://127.0.0.1:47820/*"] } }),
      onInstalled: event("runtime.onInstalled"), onStartup: event("runtime.onStartup"), onMessage: event("runtime.onMessage"), onMessageExternal: event("runtime.onMessageExternal"),
    },
    storage: {
      session: {
        get: vi.fn(async (key: string | null) => key === null ? { ...session } : key in session ? { [key]: session[key] } : {}),
        set: vi.fn(async (items: Record<string, unknown>) => { Object.assign(session, items); }),
        remove: vi.fn(async (keys: string | string[]) => { for (const key of [keys].flat()) delete session[key]; }),
        setAccessLevel: vi.fn(async () => undefined),
      },
      local: { get: vi.fn(async (key: string) => key in local ? { [key]: local[key] } : {}), set: vi.fn(async (items: Record<string, unknown>) => { Object.assign(local, items); }) },
      onChanged: event("storage.onChanged"),
    },
    sidePanel: { open: vi.fn(async () => undefined), setPanelBehavior: vi.fn(async () => undefined) },
    contextMenus: { create: vi.fn(), update: vi.fn(async () => undefined), onClicked: event("contextMenus.onClicked") },
    commands: { getAll: vi.fn(async () => [{ name: "start-lasso", shortcut: "Alt+Shift+L" }]), onCommand: event("commands.onCommand") },
    omnibox: { onInputEntered: event("omnibox.onInputEntered") },
    windows: { WINDOW_ID_CURRENT: -2 },
    alarms: { create: vi.fn(), onAlarm: event("alarms.onAlarm") },
    tabs: {
      query: vi.fn(async () => [{ id: 7 }]), get: vi.fn(async (id: number) => ({ id })), create: vi.fn(async () => ({})), sendMessage: vi.fn(async () => undefined), captureVisibleTab: vi.fn(),
      onCreated: event("tabs.onCreated"), onUpdated: event("tabs.onUpdated"), onRemoved: event("tabs.onRemoved"),
    },
    scripting: {
      executeScript: vi.fn(async () => []), getRegisteredContentScripts: vi.fn(async () => []), registerContentScripts: vi.fn(async () => undefined), unregisterContentScripts: vi.fn(async () => undefined),
    },
    permissions: { contains: vi.fn(async () => false), onAdded: event("permissions.onAdded"), onRemoved: event("permissions.onRemoved") },
    notifications: { onClicked: event("notifications.onClicked"), clear: vi.fn() },
  };
  vi.stubGlobal("__TEST_MATCHES__", false);
  vi.stubGlobal("chrome", chrome);
  await import("../src/background");
  return chrome;
}

beforeEach(() => {
  vi.clearAllMocks();
});
afterEach(() => vi.unstubAllGlobals());

const reply = (name: string, message: unknown, sender: chrome.runtime.MessageSender, respond: (value: unknown) => void) => emit(name, message, sender, respond)[0];

describe("background worker", () => {
  it("rejects malformed/foreign/iframe requests before opening a panel or creating sessions", async () => {
    const chrome = await loadWorker();
    expect(chrome.storage.session.setAccessLevel).toHaveBeenCalledWith({ accessLevel: "TRUSTED_CONTEXTS" });
    const url = "https://mail.google.com/mail/u/0/#message";
    const sender = { id: "extension-id", url, frameId: 0, tab: { id: 4, url } } as chrome.runtime.MessageSender;
    // Only the extension's own pages may ask the worker to pair from the launcher's file.
    const adopt = { type: "openside.launcher.adopt" };
    expect(reply("runtime.onMessage", adopt, sender, vi.fn())).toBe(false);
    const adopted = vi.fn();
    expect(reply("runtime.onMessage", adopt, { id: "extension-id", url: `${EXTENSION}panel.html` } as chrome.runtime.MessageSender, adopted)).toBe(true);
    await vi.waitFor(() => expect(adopted).toHaveBeenCalledWith(true));
    const message = { type: "start", text: "Handle", context: { url, title: "Message", selection: "Selected text" } };
    for (const source of [{ ...sender, id: "foreign" }, { ...sender, frameId: 1 }, { ...sender, tab: { id: 4.5 } }]) expect(reply("runtime.onMessage", message, source as chrome.runtime.MessageSender, vi.fn())).toBe(false);
    for (const value of [null, {}, { ...message, context: { url } }, { ...message, text: 4 }]) expect(reply("runtime.onMessage", value, sender, vi.fn())).toBe(false);
    expect(chrome.sidePanel.open).not.toHaveBeenCalled();
    expect(api.createSession).not.toHaveBeenCalled();
    // An unknown menu item and a selection-less ask do nothing.
    emit("contextMenus.onClicked", { menuItemId: "not-ours", selectionText: "Page script text", pageUrl: url }, sender.tab);
    emit("contextMenus.onClicked", { menuItemId: "ask-selection", selectionText: "  ", pageUrl: url }, sender.tab);
    expect(chrome.sidePanel.open).not.toHaveBeenCalled();
    const respond = vi.fn();
    expect(reply("runtime.onMessage", message, sender, respond)).toBe(true);
    await vi.waitFor(() => expect(respond).toHaveBeenCalledWith({ id: "new-session" }));
    expect(api.createSession).toHaveBeenCalledTimes(1);
    expect(api.createSession).toHaveBeenCalledWith({ agent_id: null, title: null, model: null, mode: null, workspace: null });
    expect(chrome.sidePanel.open).toHaveBeenCalledWith({ tabId: 4 });
    expect(api.submitMessage.mock.calls[0]![1].text).toContain("Selected text");
  });

  it("opens the side panel inside the omnibox gesture before any async work", async () => {
    const chrome = await loadWorker();
    emit("omnibox.onInputEntered", "what is on this page");
    // Chrome only honours sidePanel.open() while the listener is still on the stack.
    expect(chrome.sidePanel.open).toHaveBeenCalledWith({ windowId: -2 });
    await vi.waitFor(() => expect(api.createSession).toHaveBeenCalledTimes(1));
    expect(chrome.sidePanel.open).toHaveBeenCalledTimes(1);
  });

  it("creates the Ask about selection and Capture menu items on install", async () => {
    const chrome = await loadWorker();
    emit("runtime.onInstalled");
    await vi.waitFor(() => expect(chrome.contextMenus.create).toHaveBeenCalledTimes(2));
    expect(chrome.contextMenus.create).toHaveBeenCalledWith({ id: "ask-selection", title: "Ask openside about “%s”", contexts: ["selection"] }, expect.any(Function));
    expect(chrome.contextMenus.create).toHaveBeenCalledWith({ id: "capture", title: "Capture", contexts: ["page", "image", "link", "frame", "video"], documentUrlPatterns: ["https://*/*", "http://*/*"] }, expect.any(Function));
    // Titles follow the language when the worker starts.
    await vi.waitFor(() => expect(chrome.contextMenus.update).toHaveBeenCalledWith("capture", { title: "Capture" }));
    expect(chrome.contextMenus.update).toHaveBeenCalledWith("ask-selection", { title: "Ask openside about “%s”" });
  });

  it("hands a selection to the side panel, opened inside the click", async () => {
    const chrome = await loadWorker();
    const tab = { id: 4, url: "https://example.com/post", title: "Post" };
    emit("contextMenus.onClicked", { menuItemId: "ask-selection", selectionText: "Chosen words", pageUrl: tab.url, frameUrl: "https://example.com/frame" }, tab);
    expect(chrome.sidePanel.open).toHaveBeenCalledWith({ tabId: 4 });
    await vi.waitFor(() => expect(session["panelAsk:4"]).toEqual({ text: "", context: { url: "https://example.com/frame", title: "Post", selection: "Chosen words" } }));
    expect(api.createSession).not.toHaveBeenCalled();
    // A long selection is cut to the same length a dot selection is.
    emit("contextMenus.onClicked", { menuItemId: "ask-selection", selectionText: "x".repeat(25_000), pageUrl: tab.url }, { ...tab, id: 6 });
    await vi.waitFor(() => expect(session["panelAsk:6"]).toMatchObject({ context: { selection: "x".repeat(20_000) } }));
  });

  it("asks in a new web chat when the side panel cannot open", async () => {
    const chrome = await loadWorker();
    chrome.sidePanel.open.mockRejectedValueOnce(new Error("no gesture"));
    const tab = { id: 4, url: "https://example.com/post", title: "Post" };
    emit("contextMenus.onClicked", { menuItemId: "ask-selection", selectionText: "Chosen words", pageUrl: tab.url }, tab);
    await vi.waitFor(() => expect(mocks.openConversation).toHaveBeenCalledWith("new-session"));
    expect(api.submitMessage.mock.calls[0]![1].text).toContain("Explain this.");
    expect(api.submitMessage.mock.calls[0]![1].text).toContain("Chosen words");
    expect(session["panelAsk:4"]).toBeUndefined();
  });

  it("starts a lasso from the Capture menu item and from the command while lasso selection is on", async () => {
    const chrome = await loadWorker();
    emit("contextMenus.onClicked", { menuItemId: "capture", pageUrl: "https://example.com/" }, { id: 4 });
    await vi.waitFor(() => expect(chrome.tabs.sendMessage).toHaveBeenCalledWith(4, { type: "lasso.start" }, { frameId: 0 }));
    expect(chrome.scripting.executeScript).toHaveBeenCalledWith({ target: { tabId: 4 }, files: ["lasso.js"] });

    emit("commands.onCommand", "start-lasso", { id: 9 });
    await vi.waitFor(() => expect(chrome.tabs.sendMessage).toHaveBeenCalledWith(9, { type: "lasso.start" }, { frameId: 0 }));
    emit("commands.onCommand", "start-lasso", undefined);
    await vi.waitFor(() => expect(chrome.tabs.sendMessage).toHaveBeenCalledWith(7, { type: "lasso.start" }, { frameId: 0 }));

    chrome.tabs.sendMessage.mockClear();
    chrome.storage.local.get.mockClear();
    local.lassoSettings = { ...DEFAULT_LASSO, region: { ...DEFAULT_LASSO.region, enabled: false } };
    emit("commands.onCommand", "start-lasso", { id: 9 });
    emit("commands.onCommand", "open-palette", { id: 9 });
    // The worker decides once this settings read settles; its callback runs before ours.
    expect(chrome.storage.local.get).toHaveBeenCalledTimes(1);
    await chrome.storage.local.get.mock.results[0]!.value;
    expect(chrome.tabs.sendMessage).not.toHaveBeenCalled();
  });

  it("answers lasso messages asynchronously, with an error for a failed one", async () => {
    await loadWorker();
    const respond = vi.fn();
    const popover = { id: "extension-id", url: `${EXTENSION}ask.html`, frameId: 2, tab: { id: 4 } } as chrome.runtime.MessageSender;
    expect(reply("runtime.onMessage", { type: "lasso.details", token: "11111111-1111-1111-1111-111111111111" }, popover, respond)).toBe(true);
    await vi.waitFor(() => expect(respond).toHaveBeenCalledWith({ error: "Ask expired" }));
  });

  it("serves Settings › Lasso to the daemon UI only", async () => {
    await loadWorker();
    const respond = vi.fn();
    expect(reply("runtime.onMessageExternal", { type: "openside.lasso.get" }, { origin: "http://127.0.0.1:47820" } as chrome.runtime.MessageSender, respond)).toBe(true);
    await vi.waitFor(() => expect(respond).toHaveBeenCalledWith({ ok: true, settings: DEFAULT_LASSO, access: false, command: "Alt+Shift+L" }));
    const refused = vi.fn();
    reply("runtime.onMessageExternal", { type: "openside.lasso.get" }, { origin: "https://example.com" } as chrome.runtime.MessageSender, refused);
    await vi.waitFor(() => expect(refused).toHaveBeenCalledWith(expect.objectContaining({ ok: false })));
  });

  it("follows site access with the page script and forgets a closed tab's handoff and capture", async () => {
    const chrome = await loadWorker();
    await vi.waitFor(() => expect(chrome.scripting.getRegisteredContentScripts).toHaveBeenCalledTimes(1));
    chrome.permissions.contains.mockResolvedValue(true);
    emit("permissions.onAdded", { origins: ["<all_urls>"] });
    await vi.waitFor(() => expect(chrome.scripting.registerContentScripts).toHaveBeenCalledWith([expect.objectContaining({ id: "lasso-page", js: ["lasso.js"] })]));
    expect(local.lassoAccess).toBe(true);
    session["panelAsk:4"] = { text: "", context: { url: "", title: "", selection: "x" } };
    session["ask:11111111-1111-1111-1111-111111111111"] = { tabId: 4, kind: "region", context: {}, image: { name: "capture.png", data_base64: "AQID" }, created: Date.now() };
    session["ask:22222222-2222-2222-2222-222222222222"] = { tabId: 5, kind: "text", context: {}, created: Date.now() };
    emit("tabs.onRemoved", 4);
    await vi.waitFor(() => expect(Object.keys(session).filter(key => key.endsWith(":4") || key.startsWith("ask:"))).toEqual(["ask:22222222-2222-2222-2222-222222222222"]));
  });
});
