import { createContext, useCallback, useContext, useEffect, useRef, useState } from "react";
import { ApiError, type Client } from "../api/client";
import type { PersonBookmark, PersonTab, PersonTabCommand, PersonTabGroup } from "../api/types";
import { markTabUsed } from "../shell";

export const PERSON_TABS_POLL_MS = 3000;
const MAX_BACKOFF_MS = 60_000;

/**
 * The extension groups agent tabs under this title in the person's window (extension/src/agent-tabs.ts);
 * the sidebar lists those tabs from the ownership ledger instead, so they are not shown twice.
 */
export const AGENT_TAB_GROUP_TITLE = "Agent Tabs";

export interface PersonBrowser {
  /** `false` until the extension's bridge answers, and whenever it is not attached. */
  connected: boolean;
  /** The browser window with focus; its active tab is the sidebar's selected row. */
  focusedWindow: number | null;
  tabs: readonly PersonTab[];
  groups: readonly PersonTabGroup[];
  /** The Bookmarks bar folder's children; empty while not connected. */
  bookmarks: readonly PersonBookmark[];
  /** The Bookmarks bar folder id, for drops; `null` until the tree is read. */
  bookmarkBar: string | null;
  /** Sends a tab command and re-reads the tabs at once, so the sidebar follows without waiting a poll. */
  command: (command: PersonTabCommand) => Promise<void>;
  refreshBookmarks: () => void;
}

const EMPTY: PersonBrowser = {
  connected: false,
  focusedWindow: null,
  tabs: [],
  groups: [],
  bookmarks: [],
  bookmarkBar: null,
  command: () => Promise.resolve(),
  refreshBookmarks: () => undefined,
};

export const PersonBrowserContext = createContext<PersonBrowser>(EMPTY);
export const usePersonBrowser = () => useContext(PersonBrowserContext);

/** A refused token, or a daemon without the person-browser routes: asking again cannot help. */
function final(cause: unknown): boolean {
  return cause instanceof ApiError && (cause.status === 401 || cause.status === 403 || cause.status === 404);
}

/**
 * Opens `url` as a new tab in the person's browser: http(s), or one of the browser pages the daemon
 * allows (chrome://bookmarks, downloads, extensions, history, settings, inspect), which a web page
 * cannot open itself. `false` when the browser is not connected; a web address then opens here.
 */
export async function openInBrowser(client: Client, url: string): Promise<boolean> {
  try {
    await client.personTabCommand({ command: "create", url });
    return true;
  } catch {
    if (/^https?:/i.test(url)) window.open(url, "_blank", "noopener");
    return false;
  }
}

/**
 * The address a bookmark may be saved with: a parsed http(s) URL without credentials, normalised;
 * `null` for anything else (javascript:, data:, file:, blob:, browser pages, user:pass@ hosts).
 */
export function bookmarkableUrl(raw: string): string | null {
  let parsed: URL;
  try {
    parsed = new URL(raw.trim());
  } catch {
    return null;
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return null;
  if (parsed.username || parsed.password) return null;
  return parsed.href;
}

/**
 * The person's own tabs and bookmarks for the shell, read once for both sidebars (desktop column and
 * phone drawer). Tabs are polled one request at a time while the page is visible, backing off on
 * failure; bookmarks are read on start, on focus and after a change, since they rarely move.
 */
export function usePersonBrowserState(client: Client): PersonBrowser {
  const [tabsState, setTabsState] = useState<{ connected: boolean; focusedWindow: number | null; tabs: PersonTab[]; groups: PersonTabGroup[] }>({ connected: false, focusedWindow: null, tabs: [], groups: [] });
  const [bookmarkState, setBookmarkState] = useState<{ bookmarks: PersonBookmark[]; bar: string | null }>({ bookmarks: [], bar: null });
  const pollNow = useRef<() => void>(() => undefined);
  const readBookmarks = useRef<() => void>(() => undefined);

  useEffect(() => {
    let stopped = false;
    let inFlight = false;
    let again = false;
    let timer: number | undefined;
    let delay = PERSON_TABS_POLL_MS;
    let lastActive: number | null = null;
    const schedule = () => {
      window.clearTimeout(timer);
      timer = undefined;
      if (!stopped && document.visibilityState === "visible") timer = window.setTimeout(() => void poll(), delay);
    };
    const poll = async () => {
      window.clearTimeout(timer);
      timer = undefined;
      if (stopped || document.visibilityState !== "visible") return;
      if (inFlight) {
        again = true;
        return;
      }
      inFlight = true;
      try {
        const list = await client.personTabs();
        if (stopped) return;
        setTabsState({ connected: list.connected, focusedWindow: list.focused_window_id ?? null, tabs: list.tabs, groups: list.groups });
        // Switches made in the browser itself count for the palette's "Recently used" order too.
        const active = list.tabs.find((tab) => tab.active && tab.window_id === list.focused_window_id);
        if (active && active.tab_id !== lastActive) {
          lastActive = active.tab_id;
          markTabUsed(active.tab_id);
        }
        // No browser attached: keep asking, but back off as for a failure until it attaches.
        delay = list.connected ? PERSON_TABS_POLL_MS : Math.min(delay * 2, MAX_BACKOFF_MS);
      } catch (cause) {
        if (stopped) return;
        setTabsState({ connected: false, focusedWindow: null, tabs: [], groups: [] });
        if (final(cause)) {
          stopped = true;
          return;
        }
        delay = Math.min(delay * 2, MAX_BACKOFF_MS);
      } finally {
        inFlight = false;
      }
      if (again) {
        again = false;
        void poll();
      } else schedule();
    };
    pollNow.current = () => void poll();
    const onVisibility = () => {
      if (document.visibilityState !== "visible") {
        window.clearTimeout(timer);
        timer = undefined;
      } else if (!inFlight && timer === undefined) void poll();
    };
    void poll();
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      stopped = true;
      window.clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [client]);

  useEffect(() => {
    let live = true;
    let refused = false;
    let inFlight = false;
    let queued = false;
    // One read at a time; a refresh asked for meanwhile runs once after it, so focus storms and
    // back-to-back changes never stack requests or let an older answer land last.
    const read = () => {
      if (refused || !live) return;
      if (inFlight) {
        queued = true;
        return;
      }
      inFlight = true;
      client
        .personBookmarks()
        .then(
          (tree) => {
            if (!live) return;
            // Chrome's first root is the Bookmarks bar; the sidebar shows what the person keeps there.
            const bar = tree.connected ? tree.roots[0] : undefined;
            setBookmarkState({ bookmarks: bar?.children ?? [], bar: bar?.id ?? null });
          },
          (cause: unknown) => {
            if (!live) return;
            if (final(cause)) refused = true;
            setBookmarkState({ bookmarks: [], bar: null });
          },
        )
        .finally(() => {
          inFlight = false;
          if (queued && live) {
            queued = false;
            read();
          }
        });
    };
    readBookmarks.current = read;
    read();
    window.addEventListener("focus", read);
    return () => {
      live = false;
      queued = false;
      readBookmarks.current = () => undefined;
      window.removeEventListener("focus", read);
    };
  }, [client]);

  const command = useCallback(
    async (next: PersonTabCommand) => {
      try {
        await client.personTabCommand(next);
      } finally {
        pollNow.current();
      }
    },
    [client],
  );
  const refreshBookmarks = useCallback(() => readBookmarks.current(), []);

  // Once the bridge attaches (the browser started after the page), the bookmarks become readable.
  const { connected } = tabsState;
  useEffect(() => {
    if (connected) readBookmarks.current();
  }, [connected]);

  return { ...tabsState, bookmarks: bookmarkState.bookmarks, bookmarkBar: bookmarkState.bar, command, refreshBookmarks };
}
