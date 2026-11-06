import { beforeEach, describe, expect, it, vi } from "vitest";

const UI = "http://127.0.0.1:47820";
const session: Record<string, unknown> = {};
const windows = new Map<number, { id: number; focused: boolean; left?: number; top?: number; width?: number; height?: number }>();
const listeners = { command: [] as ((name: string) => void)[], menu: [] as ((info: { menuItemId: string }, tab?: chrome.tabs.Tab) => void)[], removed: [] as ((id: number) => void)[] };
let nextWindow = 70;
let lastFocused: { id?: number; left?: number; top?: number; width?: number; height?: number } | null = null;
let tabs: Partial<chrome.tabs.Tab>[] = [];

const chromeStub = {
  storage: {
    session: {
      get: vi.fn(async (key: string) => (key in session ? { [key]: session[key] } : {})),
      set: vi.fn(async (items: Record<string, unknown>) => { Object.assign(session, items); }),
      remove: vi.fn(async (key: string) => { delete session[key]; }),
    },
    onChanged: { addListener: vi.fn() },
  },
  runtime: {
    getManifest: vi.fn(() => ({ externally_connectable: { matches: [`${UI}/*`] } })),
    openOptionsPage: vi.fn(async () => undefined),
    onInstalled: { addListener: vi.fn() },
    lastError: undefined,
  },
  windows: {
    get: vi.fn(async (id: number) => { const found = windows.get(id); if (!found) throw new Error("No window"); return found; }),
    getLastFocused: vi.fn(async () => { if (!lastFocused) throw new Error("none"); return lastFocused; }),
    create: vi.fn(async (properties: { left?: number; top?: number; width?: number; height?: number }) => { const id = nextWindow++; windows.set(id, { id, focused: true, ...properties }); return { id }; }),
    update: vi.fn(async () => ({})),
    remove: vi.fn(async (id: number) => { windows.delete(id); }),
    onRemoved: { addListener: (listener: (id: number) => void) => listeners.removed.push(listener) },
  },
  tabs: {
    query: vi.fn(async () => tabs),
    update: vi.fn(async () => ({})),
    create: vi.fn(async () => ({})),
  },
  bookmarks: { create: vi.fn(async () => ({})) },
  contextMenus: {
    create: vi.fn(),
    update: vi.fn(async () => undefined),
    onClicked: { addListener: (listener: (info: { menuItemId: string }, tab?: chrome.tabs.Tab) => void) => listeners.menu.push(listener) },
  },
  commands: { onCommand: { addListener: (listener: (name: string) => void) => listeners.command.push(listener) } },
};
vi.stubGlobal("chrome", chromeStub);

import { BOOKMARK_TAB_MENU, bookmarkTab, createShellMenu, daemonUiOrigin, openPalette, registerShellCommands, toggleMiniPopup } from "../src/shell-commands";

beforeEach(() => {
  vi.clearAllMocks();
  for (const list of Object.values(listeners)) list.length = 0;
  for (const key of Object.keys(session)) delete session[key];
  windows.clear();
  nextWindow = 70;
  lastFocused = { id: 1, left: 100, top: 50, width: 1280, height: 900 };
  tabs = [];
});

describe("daemonUiOrigin", () => {
  it("prefers the paired daemon and falls back to the loopback daemon the manifest pins", async () => {
    expect(await daemonUiOrigin()).toBe(UI);
    session.pairing = { url: "https://daemon.example/", token: "t" };
    expect(await daemonUiOrigin()).toBe("https://daemon.example");
  });

  it("opens nothing for a malformed or non-web pairing", async () => {
    session.pairing = { url: "not a url", token: "t" };
    expect(await daemonUiOrigin()).toBeNull();
    session.pairing = { url: "javascript:alert(1)", token: "t" };
    expect(await daemonUiOrigin()).toBeNull();
  });

  it("before pairing, trusts only a manifest pattern naming one loopback daemon", async () => {
    chromeStub.runtime.getManifest.mockReturnValueOnce({ externally_connectable: { matches: ["http://127.0.0.1/*", "http://evil.example:80/*", "http://localhost:5555/*"] } });
    expect(await daemonUiOrigin()).toBe("http://localhost:5555");
    chromeStub.runtime.getManifest.mockReturnValueOnce({ externally_connectable: { matches: ["http://127.0.0.1/*", "http://localhost/*"] } });
    expect(await daemonUiOrigin()).toBeNull();
  });
});

describe("toggleMiniPopup", () => {
  it("opens the web UI's /mini route in a popup window centred over the last browser window", async () => {
    await toggleMiniPopup();
    expect(chromeStub.windows.create).toHaveBeenCalledWith({ type: "popup", url: `${UI}/mini`, focused: true, left: 500, top: 150, width: 480, height: 600 });
    expect(session.miniPopupWindow).toBe(70);
  });

  it("closes the popup when it is in front and brings it forward when it is behind", async () => {
    await toggleMiniPopup();
    await toggleMiniPopup();
    expect(chromeStub.windows.remove).toHaveBeenCalledWith(70);
    await toggleMiniPopup();
    windows.get(71)!.focused = false;
    await toggleMiniPopup();
    expect(chromeStub.windows.update).toHaveBeenCalledWith(71, { focused: true });
    expect(chromeStub.windows.create).toHaveBeenCalledTimes(2);
  });

  it("opens a fresh popup when the remembered one was closed, and forgets a closed window", async () => {
    await toggleMiniPopup();
    registerShellCommands(Promise.resolve());
    for (const listener of listeners.removed) listener(70);
    await vi.waitFor(() => expect(session.miniPopupWindow).toBeUndefined());
    session.miniPopupWindow = 99;
    await toggleMiniPopup();
    expect(chromeStub.windows.create).toHaveBeenCalledTimes(2);
  });

  it("lets the window manager place it without a browser window", async () => {
    lastFocused = null;
    await toggleMiniPopup();
    expect(chromeStub.windows.create).toHaveBeenCalledWith({ type: "popup", url: `${UI}/mini`, focused: true, width: 480, height: 600 });
  });
});

describe("openPalette", () => {
  it("opens the palette in the window's active openside tab through its #search fragment", async () => {
    tabs = [{ id: 4, active: false, url: `${UI}/chats`, lastAccessed: 5 }, { id: 5, active: true, url: `${UI}/sessions/abc#old` }];
    await openPalette();
    expect(chromeStub.tabs.query).toHaveBeenCalledWith({ windowId: 1, url: `${UI}/*` });
    expect(chromeStub.tabs.update).toHaveBeenCalledWith(5, { active: true, url: `${UI}/sessions/abc#search` });
  });

  it("brings the openside tab used last forward when another page is in front", async () => {
    tabs = [{ id: 4, active: false, url: `${UI}/chats`, lastAccessed: 5 }, { id: 6, active: false, url: `${UI}/`, lastAccessed: 9 }];
    await openPalette();
    expect(chromeStub.tabs.update).toHaveBeenCalledWith(6, { active: true, url: `${UI}/#search` });
  });

  it("opens a new openside tab when the window has none", async () => {
    await openPalette();
    expect(chromeStub.tabs.create).toHaveBeenCalledWith({ windowId: 1, url: `${UI}/#search` });
  });

  it("dispatches both commands by name and ignores others", async () => {
    registerShellCommands(Promise.resolve());
    for (const listener of listeners.command) listener("start-lasso");
    expect(chromeStub.windows.create).not.toHaveBeenCalled();
    for (const listener of listeners.command) listener("open-palette");
    await vi.waitFor(() => expect(chromeStub.tabs.create).toHaveBeenCalled());
    for (const listener of listeners.command) listener("mini-popup");
    await vi.waitFor(() => expect(chromeStub.windows.create).toHaveBeenCalled());
  });
});

describe("Add Tab to Bookmarks", () => {
  it("adds a tab-strip context menu item", () => {
    createShellMenu();
    expect(chromeStub.contextMenus.create).toHaveBeenCalledWith({ id: BOOKMARK_TAB_MENU, title: "Add Tab to Bookmarks", contexts: ["tab"] }, expect.any(Function));
  });

  it("is absent, without an error, in a browser that rejects the tab context", () => {
    chromeStub.contextMenus.create.mockImplementationOnce(() => { throw new TypeError("Invalid value for argument 1"); });
    expect(() => createShellMenu()).not.toThrow();
  });

  it("files the right-clicked tab in the bookmarks bar", async () => {
    await bookmarkTab({ url: "https://example.com/a", title: "Example" } as chrome.tabs.Tab);
    expect(chromeStub.bookmarks.create).toHaveBeenCalledWith({ parentId: "1", title: "Example", url: "https://example.com/a" });
    await bookmarkTab({ url: "https://example.com/b", title: "" } as chrome.tabs.Tab);
    expect(chromeStub.bookmarks.create).toHaveBeenLastCalledWith({ parentId: "1", title: "https://example.com/b", url: "https://example.com/b" });
  });

  it("acts only on its own menu item", async () => {
    registerShellCommands(Promise.resolve());
    for (const listener of listeners.menu) listener({ menuItemId: "ask-selection" }, { url: "https://example.com/" } as chrome.tabs.Tab);
    expect(chromeStub.bookmarks.create).not.toHaveBeenCalled();
    for (const listener of listeners.menu) listener({ menuItemId: BOOKMARK_TAB_MENU }, { url: "https://example.com/", title: "E" } as chrome.tabs.Tab);
    await vi.waitFor(() => expect(chromeStub.bookmarks.create).toHaveBeenCalledTimes(1));
  });
});
