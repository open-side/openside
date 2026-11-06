import { daemonPattern } from "./daemonOrigin";
import { LOCALE_KEY, t } from "./i18n";
import type { Pairing } from "./storage";

/**
 * Browser-wide entry points of the openside shell that need no page of their own: the mini popup
 * (a small chat window the person summons from anywhere), the search palette shortcut, and the tab
 * context menu's "Add Tab to Bookmarks". All three open or drive the daemon's web UI.
 */
export const MINI_POPUP_COMMAND = "mini-popup";
export const PALETTE_COMMAND = "open-palette";
export const BOOKMARK_TAB_MENU = "bookmark-tab";

/** The web UI route the mini popup window shows; the window id is kept so the shortcut toggles it. */
export const MINI_POPUP_PATH = "/mini";
const MINI_POPUP_WINDOW = "miniPopupWindow";
const MINI_POPUP_SIZE = { width: 480, height: 600 };
/** The bookmarks bar, where the person bridge also files new bookmarks and the sidebar lists them. */
const BOOKMARKS_BAR = "1";

/**
 * The daemon whose web UI these open. The pairing is the trust boundary: its origin is the daemon
 * the extension already sends its token to, and the daemon serves only its web UI there, so any tab
 * at that origin is the app. Before the extension pairs, the loopback daemon the manifest pins
 * (tools/openside-browser writes its exact origin), validated as daemonOrigin.ts does.
 */
export async function daemonUiOrigin(): Promise<string | null> {
  const stored = (await chrome.storage.session.get("pairing")).pairing as Pairing | undefined;
  if (stored?.url) {
    try {
      const url = new URL(stored.url);
      return url.protocol === "https:" || url.protocol === "http:" ? url.origin : null;
    } catch {
      return null;
    }
  }
  for (const pattern of chrome.runtime.getManifest().externally_connectable?.matches ?? []) {
    const origin = pattern.replace(/\/\*$/, "");
    try {
      daemonPattern(origin);
      return origin;
    } catch {
      // A test build's any-port pattern, or anything else that names no single daemon.
    }
  }
  return null;
}

/** Centred over the person's last browser window, or wherever the window manager puts it without one. */
async function centredBounds(): Promise<{ left?: number; top?: number; width: number; height: number }> {
  const anchor = await chrome.windows.getLastFocused({ windowTypes: ["normal"] }).catch(() => null);
  const { width, height } = MINI_POPUP_SIZE;
  if (anchor?.left === undefined || anchor.top === undefined || anchor.width === undefined || anchor.height === undefined) return { width, height };
  return { left: Math.round(anchor.left + (anchor.width - width) / 2), top: Math.round(anchor.top + (anchor.height - height) / 3), width, height };
}

/** The shortcut opens the mini popup, brings it forward when it is behind, and closes it when it is in front. */
export async function toggleMiniPopup(): Promise<void> {
  const kept = (await chrome.storage.session.get(MINI_POPUP_WINDOW))[MINI_POPUP_WINDOW];
  if (typeof kept === "number") {
    const open = await chrome.windows.get(kept).catch(() => null);
    if (open) {
      if (open.focused) await chrome.windows.remove(kept).catch(() => undefined);
      else await chrome.windows.update(kept, { focused: true }).catch(() => undefined);
      return;
    }
  }
  const origin = await daemonUiOrigin();
  if (!origin) {
    await chrome.runtime.openOptionsPage();
    return;
  }
  const created = await chrome.windows.create({ type: "popup", url: `${origin}${MINI_POPUP_PATH}`, focused: true, ...await centredBounds() });
  if (created?.id !== undefined) await chrome.storage.session.set({ [MINI_POPUP_WINDOW]: created.id });
}

function withSearchHash(url: string): string {
  const next = new URL(url);
  next.hash = "search";
  return next.href;
}

/**
 * The palette lives in the web UI: the shortcut brings the window's openside tab forward (the active
 * one, else the one used last) and opens the palette there through its `#search` fragment, or opens
 * a new openside tab when the window has none. A web UI tab that has focus handles the key itself.
 */
export async function openPalette(): Promise<void> {
  const origin = await daemonUiOrigin();
  if (!origin) {
    await chrome.runtime.openOptionsPage();
    return;
  }
  const window = await chrome.windows.getLastFocused({ windowTypes: ["normal"] }).catch(() => null);
  const tabs = window?.id === undefined ? [] : await chrome.tabs.query({ windowId: window.id, url: `${origin}/*` });
  const target = tabs.find(tab => tab.active) ?? [...tabs].sort((a, b) => (b.lastAccessed ?? 0) - (a.lastAccessed ?? 0))[0];
  if (target?.id !== undefined && target.url) {
    await chrome.tabs.update(target.id, { active: true, url: withSearchHash(target.url) });
    return;
  }
  await chrome.tabs.create({ ...(window?.id === undefined ? {} : { windowId: window.id }), url: `${origin}/#search` });
}

/** "Add Tab to Bookmarks" files the right-clicked tab in the bookmarks bar, as the sidebar's drop zone does. */
export async function bookmarkTab(tab: chrome.tabs.Tab | undefined): Promise<void> {
  if (!tab?.url) return;
  await chrome.bookmarks.create({ parentId: BOOKMARKS_BAR, title: tab.title || tab.url, url: tab.url });
}

/**
 * The tab-strip context menu is Chromium's; extensions add to it with the `tab` context (on by
 * default in the pinned Chromium 155, newer than these typings). A browser without that context
 * rejects the item, and then it is simply absent.
 */
// The typings predate the `tab` context.
const TAB_CONTEXT = ["tab"] as unknown as chrome.contextMenus.CreateProperties["contexts"];
export function createShellMenu(): void {
  try {
    chrome.contextMenus.create({ id: BOOKMARK_TAB_MENU, title: t("ext.menu.bookmarkTab"), contexts: TAB_CONTEXT }, () => void chrome.runtime.lastError);
  } catch {
    // The browser does not know the tab context.
  }
}

/** The item outlives the service worker, so its title follows the language whenever the worker starts or the choice changes. */
export function retitleShellMenu(): Promise<void> {
  return chrome.contextMenus.update(BOOKMARK_TAB_MENU, { title: t("ext.menu.bookmarkTab") }).catch(() => undefined);
}

/**
 * Wires the commands and the menu item. Call after `initLocale`, so that on a language change its
 * storage listener has already switched the language when this module's listener retitles the item.
 */
export function registerShellCommands(localeReady: Promise<unknown>): void {
  chrome.runtime.onInstalled.addListener(() => { void localeReady.catch(() => undefined).then(createShellMenu); });
  void localeReady.then(retitleShellMenu).catch(() => undefined);
  chrome.storage.onChanged.addListener((changes, area) => { if (area === "local" && LOCALE_KEY in changes) void retitleShellMenu(); });
  chrome.commands.onCommand.addListener(command => {
    if (command === MINI_POPUP_COMMAND) void toggleMiniPopup();
    else if (command === PALETTE_COMMAND) void openPalette();
  });
  chrome.contextMenus.onClicked.addListener((info, tab) => {
    if (info.menuItemId === BOOKMARK_TAB_MENU) void bookmarkTab(tab).catch(() => undefined);
  });
  chrome.windows.onRemoved.addListener(windowId => {
    void chrome.storage.session.get(MINI_POPUP_WINDOW).then(kept => {
      if (kept[MINI_POPUP_WINDOW] === windowId) return chrome.storage.session.remove(MINI_POPUP_WINDOW);
    });
  });
}
