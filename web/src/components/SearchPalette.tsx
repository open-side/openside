import { Dialog as BaseDialog } from "@base-ui/react/dialog";
import { AppWindow, ArrowUpRight, MessagesSquare, Puzzle, Search } from "lucide-react";
import { useCallback, useContext, useEffect, useId, useMemo, useState, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from "react";
import { useNavigate } from "react-router";
import type { PersonBookmark, PersonExtension, PersonHistoryItem, PersonRecentlyClosed, PersonTab, Session } from "../api/types";
import { useClient } from "../context";
import { AgentTabsSettledContext, type OwnedTab } from "../hooks/useAgentTabs";
import { originOf } from "../hooks/useFavicon";
import { AGENT_TAB_GROUP_TITLE, bookmarkableUrl, openInBrowser, usePersonBrowser } from "../hooks/usePersonBrowser";
import { formatDate, t, type MessageKey } from "../i18n";
import { markTabUsed, tabsByUse, useSidebarPrefs, type SidebarPrefs } from "../shell";
import { keyLabel, shortcutKeys } from "../shortcuts";
import { BrandMark, cx, ICON_STROKE, useToast } from "../ui";
import { browserFailure } from "./SidebarChats";
import { Favicon, ourPage, PageLabel, tabTarget } from "./SidebarTabs";
import { displaySessionTitle } from "./toolStatus";
import "../shortcuts.css";
import "./searchPalette.css";

const SECTION_LIMIT = 6;
const SEARCH_DEBOUNCE_MS = 150;

const DATE: Intl.DateTimeFormatOptions = { month: "short", day: "numeric" };
const DATE_YEAR: Intl.DateTimeFormatOptions = { year: "numeric", month: "short", day: "numeric" };
// Local calendar date as a UTC day number: whole days with no DST hour to round away.
const dayNumber = (at: Date) => Date.UTC(at.getFullYear(), at.getMonth(), at.getDate()) / 86_400_000;

/**
 * "Just now", "7 minutes ago", "3 hours ago" within a day, then calendar days: "Yesterday",
 * "4 days ago", "Oct 3". A time ahead of this device's clock (skew) reads "Just now"; a missing or
 * unreadable time is `null`.
 */
export function relativeTime(at: string | null | undefined, now = new Date()): string | null {
  if (!at) return null;
  const date = new Date(at);
  const ms = now.getTime() - date.getTime();
  if (Number.isNaN(ms)) return null;
  const minutes = Math.floor(Math.max(0, ms) / 60_000);
  if (minutes < 1) return t("shell.time.justNow");
  if (minutes < 60) return t("shell.time.minutesAgo", { count: minutes });
  if (minutes < 24 * 60) return t("shell.time.hoursAgo", { count: Math.floor(minutes / 60) });
  const days = dayNumber(now) - dayNumber(date);
  if (days <= 1) return t("shell.time.yesterday");
  if (days < 7) return t("shell.time.daysAgo", { count: days });
  return formatDate(date, date.getFullYear() === now.getFullYear() ? DATE : DATE_YEAR);
}

type OpenTab = { source: "person"; tab: PersonTab } | { source: "agent"; tab: OwnedTab };

type Item =
  | { kind: "tab"; key: string; title: string; page: boolean; url: string; favicon: string | null; open: OpenTab }
  | { kind: "chat"; key: string; title: string; session: Session; time: string | null }
  | { kind: "closed"; key: string; title: string; entry: PersonRecentlyClosed }
  | { kind: "history"; key: string; title: string; url: string; detail: string; time: string | null }
  | { kind: "bookmark"; key: string; title: string; url: string; detail: string }
  | { kind: "extension"; key: string; title: string; detail: string };

const SECTIONS: readonly { kind: Item["kind"]; heading: MessageKey }[] = [
  { kind: "tab", heading: "shell.search.openTabs" },
  { kind: "chat", heading: "shell.search.recentChat" },
  { kind: "closed", heading: "shell.search.recentlyClosed" },
  { kind: "history", heading: "shell.search.history" },
  { kind: "bookmark", heading: "shell.search.bookmarks" },
  { kind: "extension", heading: "shell.search.extensions" },
];

function matches(query: string, ...fields: string[]): boolean {
  const haystack = fields.join(" ").toLowerCase();
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((word) => haystack.includes(word));
}

const hostOf = (url: string) => {
  const origin = originOf(url);
  return origin ? new URL(origin).host : "";
};

/**
 * Every open tab once: the agents' tabs from the ownership ledger, then the person's own in strip
 * order (the focused window first). "Recently used" puts the current tab first, then the most
 * recently switched to (`markTabUsed`), then the rest in that same order.
 */
function openTabs(agentTabs: readonly OwnedTab[], person: readonly PersonTab[], current: PersonTab | undefined, order: SidebarPrefs["tabSwitcherOrder"]): OpenTab[] {
  const focused = current?.window_id;
  const strip = [...person].sort((a, b) => Number(b.window_id === focused) - Number(a.window_id === focused) || a.window_id - b.window_id || a.index - b.index);
  const all: OpenTab[] = [...agentTabs.map((tab) => ({ source: "agent" as const, tab })), ...strip.map((tab) => ({ source: "person" as const, tab }))];
  if (order === "tabs") return all;
  const used = tabsByUse(all.map((entry) => ({ tab_id: entry.tab.tab_id, entry }))).map(({ entry }) => entry);
  const first = used.findIndex((entry) => entry.source === "person" && entry.tab.tab_id === current?.tab_id);
  return first > 0 ? [used[first]!, ...used.slice(0, first), ...used.slice(first + 1)] : used;
}

function flattenBookmarks(nodes: readonly PersonBookmark[]): PersonBookmark[] {
  return nodes.flatMap((node) => (node.url ? [node] : flattenBookmarks(node.children)));
}

/**
 * The answer for `key`, fetched after a short debounce while typing (at once for an empty key);
 * `null` until it arrives, `fallback` when `key` is `null` (nothing to ask) or the request fails.
 * A request for a key typed past, or still out when the palette closes, is aborted.
 */
function useAnswer<T>(key: string | null, ask: (key: string, signal: AbortSignal) => Promise<T>, fallback: T): T | null {
  const [result, setResult] = useState<{ key: string; value: T } | null>(null);
  useEffect(() => {
    if (key === null) return;
    const controller = new AbortController();
    const timer = window.setTimeout(
      () => {
        ask(key, controller.signal).then(
          (value) => {
            if (!controller.signal.aborted) setResult({ key, value });
          },
          () => {
            // An unreachable daemon or browser is announced elsewhere; the section just stays empty.
            if (!controller.signal.aborted) setResult({ key, value: fallback });
          },
        );
      },
      key ? SEARCH_DEBOUNCE_MS : 0,
    );
    return () => {
      window.clearTimeout(timer);
      controller.abort();
    };
  }, [key, ask, fallback]);
  if (key === null) return fallback;
  return result && result.key === key ? result.value : null;
}

const NO_SESSIONS: Session[] = [];
const NO_CLOSED: PersonRecentlyClosed[] = [];
const NO_HISTORY: PersonHistoryItem[] = [];
const NO_BOOKMARKS: PersonBookmark[] = [];
const NO_EXTENSIONS: PersonExtension[] = [];

function Results({ agentTabs, pageLabel, onDone }: { agentTabs: readonly OwnedTab[]; pageLabel: (pathname: string) => string | null; onDone: () => void }) {
  const client = useClient();
  const navigate = useNavigate();
  const toast = useToast();
  const browser = usePersonBrowser();
  const agentTabsSettled = useContext(AgentTabsSettledContext);
  const [{ tabSwitcherOrder }] = useSidebarPrefs();
  const [query, setQuery] = useState("");
  const trimmed = query.trim();
  const now = useMemo(() => new Date(), []);

  const sessions = useAnswer(trimmed, useCallback((q: string, signal: AbortSignal) => client.listSessions({ q, limit: SECTION_LIMIT }, signal).then((list) => list.sessions), [client]), NO_SESSIONS);
  const closed = useAnswer("", useCallback((_: string, signal: AbortSignal) => client.personRecentlyClosed(SECTION_LIMIT, signal).then((list) => list.entries), [client]), NO_CLOSED);
  const history = useAnswer(trimmed || null, useCallback((q: string, signal: AbortSignal) => client.personHistory({ q, limit: SECTION_LIMIT }, signal).then((list) => list.items), [client]), NO_HISTORY);
  const bookmarks = useAnswer(trimmed ? "" : null, useCallback((_: string, signal: AbortSignal) => client.personBookmarks(signal).then((tree) => flattenBookmarks(tree.roots)), [client]), NO_BOOKMARKS);
  const extensions = useAnswer(trimmed ? "" : null, useCallback((_: string, signal: AbortSignal) => client.personExtensions(signal).then((list) => list.extensions), [client]), NO_EXTENSIONS);

  const items = useMemo(() => {
    const groups = new Map(browser.groups.map((group) => [group.group_id, group]));
    // As in the sidebar: agent tabs come from the ledger once it has answered, not twice.
    const person = agentTabsSettled ? browser.tabs.filter((tab) => tab.group_id == null || groups.get(tab.group_id)?.title !== AGENT_TAB_GROUP_TITLE) : browser.tabs;
    const current = browser.tabs.find((tab) => tab.active && tab.window_id === browser.focusedWindow);
    const tabs: Item[] = openTabs(agentTabs, person, current, tabSwitcherOrder)
      .map((open): Item => {
        const page = open.source === "person" ? ourPage(open.tab.url) : null;
        const title = page !== null ? (pageLabel(page) ?? t("shell.tabs.newTabTitle")) : open.tab.title.trim() || hostOf(open.tab.url) || open.tab.url;
        return { kind: "tab", key: `tab:${open.source}:${open.tab.tab_id}`, title, page: page !== null, url: open.tab.url, favicon: open.source === "person" ? (open.tab.favicon_url ?? null) : null, open };
      })
      .filter((item) => item.kind === "tab" && matches(trimmed, item.title, item.url))
      .slice(0, SECTION_LIMIT);
    const chats: Item[] = (sessions ?? []).slice(0, SECTION_LIMIT).map((session) => ({ kind: "chat", key: `chat:${session.id}`, title: displaySessionTitle(session.title), session, time: relativeTime(session.last_activity_at, now) }));
    const reopen: Item[] = (closed ?? [])
      .filter((entry) => matches(trimmed, entry.title, entry.url ?? ""))
      .slice(0, SECTION_LIMIT)
      .map((entry) => ({ kind: "closed", key: `closed:${entry.session_id}`, title: entry.title.trim() || entry.url || t("shell.bookmarks.untitled"), entry }));
    // Only web pages become rows: a synced javascript:, data: or file: entry must not be one Enter away.
    const visited: Item[] = (history ?? [])
      .filter((item) => bookmarkableUrl(item.url) !== null)
      .slice(0, SECTION_LIMIT)
      .map((item) => ({ kind: "history", key: `history:${item.url}`, title: item.title.trim() || item.url, url: item.url, detail: hostOf(item.url), time: relativeTime(item.last_visit_at, now) }));
    const saved: Item[] = trimmed
      ? (bookmarks ?? [])
          .filter((bookmark) => bookmark.url !== null && bookmark.url !== undefined && bookmarkableUrl(bookmark.url) !== null && matches(trimmed, bookmark.title, bookmark.url))
          .slice(0, SECTION_LIMIT)
          .map((bookmark) => ({ kind: "bookmark", key: `bookmark:${bookmark.id}`, title: bookmark.title.trim() || bookmark.url || t("shell.bookmarks.untitled"), url: bookmark.url ?? "", detail: hostOf(bookmark.url ?? "") }))
      : [];
    const installed: Item[] = trimmed
      ? (extensions ?? [])
          .filter((extension) => matches(trimmed, extension.name, extension.description))
          .slice(0, SECTION_LIMIT)
          .map((extension) => ({ kind: "extension", key: `extension:${extension.id}`, title: extension.name, detail: extension.enabled ? "" : t("shell.search.extensionOff") }))
      : [];
    return [...tabs, ...chats, ...reopen, ...visited, ...saved, ...installed];
  }, [browser.groups, browser.tabs, browser.focusedWindow, agentTabsSettled, agentTabs, tabSwitcherOrder, pageLabel, trimmed, sessions, closed, history, bookmarks, extensions, now]);

  const [active, setActive] = useState(0);
  const listId = useId();
  const current = Math.min(active, items.length - 1);
  const optionId = (index: number) => `${listId}-option-${index}`;

  useEffect(() => {
    if (current >= 0) document.getElementById(`${listId}-option-${current}`)?.scrollIntoView?.({ block: "nearest" });
  }, [listId, current]);

  // A page or bookmark already open in a tab switches to it, as the sidebar's bookmarks do.
  const openUrl = async (url: string) => {
    const tab = browser.tabs.find((candidate) => candidate.url === url);
    if (tab) await browser.command({ command: "activate", tab_id: tab.tab_id });
    // A web address opens in this window's browser when the person's is not connected; a browser page cannot.
    else if (!(await openInBrowser(client, url)) && !/^https?:/i.test(url)) toast.notify(t("shell.browser.notConnected"), "error");
  };

  const activate = (item: Item | undefined) => {
    if (!item) return;
    onDone();
    const run = (work: Promise<unknown>) => void work.catch((cause: unknown) => toast.notify(browserFailure(cause), "error"));
    switch (item.kind) {
      case "tab":
        markTabUsed(item.open.tab.tab_id);
        if (item.open.source === "agent") navigate(tabTarget(item.open.tab));
        else run(browser.command({ command: "activate", tab_id: item.open.tab.tab_id }));
        return;
      case "chat":
        navigate(`/sessions/${encodeURIComponent(item.session.id)}`);
        return;
      case "closed":
        run(client.restorePersonRecentlyClosed(item.entry.session_id));
        return;
      case "history":
      case "bookmark":
        run(openUrl(item.url));
        return;
      case "extension":
        run(openUrl("chrome://extensions"));
        return;
    }
  };

  const onKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (event.nativeEvent.isComposing || event.keyCode === 229) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (items.length === 0) return;
      const step = event.key === "ArrowDown" ? 1 : -1;
      setActive((Math.max(current, 0) + step + items.length) % items.length);
    } else if (event.key === "Enter") {
      event.preventDefault();
      activate(items[current]);
    }
  };

  const loading = sessions === null || closed === null || history === null || bookmarks === null || extensions === null;
  let index = 0;
  return (
    <>
      <div className="search-palette-field">
        <Search className="search-palette-icon" size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
        <input
          className="search-palette-input"
          type="text"
          role="combobox"
          aria-label={t("shell.search.inputLabel")}
          aria-expanded="true"
          aria-controls={listId}
          aria-autocomplete="list"
          aria-activedescendant={current >= 0 ? optionId(current) : undefined}
          placeholder={t("shell.search.placeholder")}
          autoComplete="off"
          spellCheck={false}
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
            // A new query starts from the first result again.
            setActive(0);
          }}
          onKeyDown={onKeyDown}
        />
        <kbd className="key-chip search-palette-key">{keyLabel(shortcutKeys("search"))}</kbd>
      </div>
      <div className="search-palette-results thin-scroll" id={listId} role="listbox" aria-label={t("shell.search.results")}>
        {SECTIONS.map(({ kind, heading }) => {
          const group = items.filter((item) => item.kind === kind);
          if (group.length === 0) return null;
          const headingId = `${listId}-${kind}`;
          return (
            <div key={kind} role="group" aria-labelledby={headingId} className="search-palette-group">
              <div id={headingId} className="search-palette-heading" role="presentation">
                {t(heading)}
              </div>
              {group.map((item) => {
                const position = index++;
                return <ResultRow key={item.key} id={optionId(position)} item={item} selected={position === current} onHover={() => setActive(position)} onChoose={() => activate(item)} />;
              })}
            </div>
          );
        })}
        {items.length === 0 ? (
          <p className="search-palette-empty" role="status">
            {loading ? t("shell.search.searching") : trimmed ? t("shell.search.noMatch", { query: trimmed }) : t("shell.search.empty")}
          </p>
        ) : null}
      </div>
    </>
  );
}

function Action({ label }: { label: string }) {
  return (
    <>
      {label}
      <ArrowUpRight className="search-palette-arrow" size={10} strokeWidth={ICON_STROKE} aria-hidden="true" />
    </>
  );
}

function lead(item: Item): ReactNode {
  switch (item.kind) {
    case "tab":
      return item.page ? <BrandMark size={16} /> : <Favicon url={item.url} favicon={item.favicon} />;
    case "chat":
      return <MessagesSquare size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />;
    case "closed":
      return item.entry.kind === "window" ? <AppWindow size={16} strokeWidth={ICON_STROKE} aria-hidden="true" /> : <Favicon url={item.entry.url ?? ""} favicon={item.entry.favicon_url} />;
    case "history":
    case "bookmark":
      return <Favicon url={item.url} />;
    case "extension":
      return <Puzzle size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />;
  }
}

function trailing(item: Item): ReactNode {
  switch (item.kind) {
    case "tab":
      return <Action label={t("shell.search.switchToTab")} />;
    case "chat":
      return item.time;
    case "closed":
      return <Action label={t("shell.search.reopen")} />;
    case "history":
      return item.time;
    case "bookmark":
      return <Action label={t("shell.search.open")} />;
    case "extension":
      return <Action label={t("shell.search.manage")} />;
  }
}

function ResultRow({ id, item, selected, onHover, onChoose }: { id: string; item: Item; selected: boolean; onHover: () => void; onChoose: () => void }) {
  const detail = item.kind === "history" || item.kind === "bookmark" || item.kind === "extension" ? item.detail : "";
  const after = trailing(item);
  return (
    <div
      id={id}
      role="option"
      aria-selected={selected}
      className={cx("search-palette-row", selected && "active")}
      data-kind={item.kind}
      // Keep focus in the input so typing and arrow keys keep working after a pointer move.
      onMouseDown={(event) => event.preventDefault()}
      onMouseMove={selected ? undefined : onHover}
      onClick={onChoose}
    >
      <span className="search-palette-lead">{lead(item)}</span>
      <span className="search-palette-text">
        <span className="search-palette-title">{item.kind === "tab" && item.page ? <PageLabel label={item.title} /> : item.title}</span>
        {detail ? <span className="search-palette-detail">{detail}</span> : null}
      </span>
      {after ? <span className="search-palette-trailing">{after}</span> : null}
    </div>
  );
}

/**
 * Mod+Shift+A (or the sidebar's Search button): one field over open tabs (the agents' and the
 * person's own), recent chats, recently closed tabs and, once something is typed, browsing history,
 * bookmarks and extensions. Centred in the window on wide screens, a bottom sheet on phones; Base
 * UI owns focus trapping, Escape, outside press and scroll lock.
 */
export function SearchPalette({ open, onClose, tabs, pageLabel }: { open: boolean; onClose: () => void; tabs: readonly OwnedTab[]; pageLabel: (pathname: string) => string | null }) {
  return (
    <BaseDialog.Root
      open={open}
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
    >
      <BaseDialog.Portal>
        <BaseDialog.Backdrop className="search-palette-backdrop" />
        <BaseDialog.Viewport className="search-palette-viewport">
          <BaseDialog.Popup className="search-palette" aria-label={t("common.search")}>
            {/* Mounted only while open, so every opening starts from an empty field. */}
            {open ? <Results agentTabs={tabs} pageLabel={pageLabel} onDone={onClose} /> : null}
          </BaseDialog.Popup>
        </BaseDialog.Viewport>
      </BaseDialog.Portal>
    </BaseDialog.Root>
  );
}
