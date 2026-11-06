import { createContext, useCallback, useContext, useEffect, useState } from "react";

/** What screens inside the app shell can ask of it. */
export interface Shell {
  /** Opens the sidebar drawer (phones; the sidebar is a column on desktop). */
  openDrawer: () => void;
  /** Opens the search palette over the shell (Mod+Shift+A, the sidebar's Search button). */
  openSearch: () => void;
  /** Reloads the sidebar chat list after a create, rename, pin or archive. */
  refreshSessions: () => void;
  /** Chats (not archived) whose run is waiting for the person: an approval, an answer, a takeover. */
  needsYou: number;
  /** Desktop: the sidebar column is folded away (`localStorage["openside.sidebar"]`). */
  sidebarCollapsed: boolean;
  toggleSidebar: () => void;
}

// No-op defaults keep screens renderable on their own in unit tests.
export const ShellContext = createContext<Shell>({
  openDrawer: () => undefined,
  openSearch: () => undefined,
  refreshSessions: () => undefined,
  needsYou: 0,
  sidebarCollapsed: false,
  toggleSidebar: () => undefined,
});

export const useShell = () => useContext(ShellContext);

/**
 * Sidebar choices from Settings › Appearance ("Recent chats in sidebar", "Customize sidebar", "New
 * tab position", "Tab switcher order"). They belong to this browser, like the theme, so they live
 * in localStorage rather than the daemon.
 */
export interface SidebarPrefs {
  recentChats: 3 | 5 | 10;
  bookmarks: boolean;
  chats: boolean;
  /** `top` lists the newest tab first in the sidebar; `bottom` keeps the browser's order. */
  newTabPosition: "top" | "bottom";
  /** `recent` orders the search palette's tabs by last use; `tabs` by their place in the sidebar. */
  tabSwitcherOrder: "recent" | "tabs";
}

export const SIDEBAR_PREFS_KEY = "openside.sidebar.prefs";
export const RECENT_CHAT_CHOICES = [3, 5, 10] as const;
const DEFAULT_PREFS: SidebarPrefs = { recentChats: 3, bookmarks: true, chats: true, newTabPosition: "top", tabSwitcherOrder: "recent" };
const PREFS_EVENT = "openside:sidebar-prefs";

export function loadSidebarPrefs(): SidebarPrefs {
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(SIDEBAR_PREFS_KEY) ?? "null");
    if (!stored || typeof stored !== "object") return DEFAULT_PREFS;
    const value = stored as Partial<Record<keyof SidebarPrefs, unknown>>;
    return {
      recentChats: RECENT_CHAT_CHOICES.find((count) => count === value.recentChats) ?? DEFAULT_PREFS.recentChats,
      bookmarks: typeof value.bookmarks === "boolean" ? value.bookmarks : DEFAULT_PREFS.bookmarks,
      chats: typeof value.chats === "boolean" ? value.chats : DEFAULT_PREFS.chats,
      newTabPosition: value.newTabPosition === "bottom" ? "bottom" : DEFAULT_PREFS.newTabPosition,
      tabSwitcherOrder: value.tabSwitcherOrder === "tabs" ? "tabs" : DEFAULT_PREFS.tabSwitcherOrder,
    };
  } catch {
    return DEFAULT_PREFS;
  }
}

/** Every mounted sidebar (desktop column, phone drawer, other tabs) follows a change at once. */
export function saveSidebarPrefs(patch: Partial<SidebarPrefs>): SidebarPrefs {
  const next = { ...loadSidebarPrefs(), ...patch };
  localStorage.setItem(SIDEBAR_PREFS_KEY, JSON.stringify(next));
  window.dispatchEvent(new Event(PREFS_EVENT));
  return next;
}

export function useSidebarPrefs(): [SidebarPrefs, (patch: Partial<SidebarPrefs>) => void] {
  const [prefs, setPrefs] = useState(loadSidebarPrefs);
  useEffect(() => {
    const sync = () => setPrefs(loadSidebarPrefs());
    window.addEventListener(PREFS_EVENT, sync);
    window.addEventListener("storage", sync);
    return () => {
      window.removeEventListener(PREFS_EVENT, sync);
      window.removeEventListener("storage", sync);
    };
  }, []);
  const update = useCallback((patch: Partial<SidebarPrefs>) => setPrefs(saveSidebarPrefs(patch)), []);
  return [prefs, update];
}

const TAB_USE_KEY = "openside.tabs.used";
// Enough for every tab a person keeps open; older entries belong to tabs long closed.
const TAB_USE_LIMIT = 200;

function loadTabUse(): Record<string, number> {
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(TAB_USE_KEY) ?? "null");
    if (!stored || typeof stored !== "object") return {};
    return Object.fromEntries(Object.entries(stored).filter((entry): entry is [string, number] => typeof entry[1] === "number"));
  } catch {
    return {};
  }
}

/** Remembers when a tab was last opened from the sidebar or the search palette, for "Recently used" order. */
export function markTabUsed(tabId: string | number, now = Date.now()): void {
  const entries = Object.entries({ ...loadTabUse(), [String(tabId)]: now })
    .sort((a, b) => b[1] - a[1])
    .slice(0, TAB_USE_LIMIT);
  localStorage.setItem(TAB_USE_KEY, JSON.stringify(Object.fromEntries(entries)));
}

/** Most recently used first; tabs never opened here keep their sidebar order after them. */
export function tabsByUse<T extends { tab_id: string | number }>(tabs: readonly T[]): T[] {
  const used = loadTabUse();
  return tabs
    .map((tab, index) => ({ tab, index, at: used[String(tab.tab_id)] ?? -1 }))
    .sort((a, b) => b.at - a.at || a.index - b.index)
    .map(({ tab }) => tab);
}
