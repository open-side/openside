import { useCallback, useContext, useMemo, useSyncExternalStore } from "react";
import { AgentTabsContext, type OwnedTab } from "../hooks/useAgentTabs";

/** Thread column (≈680) + gap + panel (300) + right inset: below this the panel gives way to the PiP. */
export const AGENT_TABS_MIN_WIDTH = 680 + 16 + 300 + 16;

interface PanelState {
  /** Tab ids, oldest first: the order the conversation opened them (see `syncAgentTabs`). */
  order: readonly string[];
  /** Cards the person hid; the daemon tab stays open. */
  dismissed: ReadonlySet<string>;
  /** The person turned the whole panel off from the header. */
  hidden: boolean;
  /** The conversation is wide enough for the column beside the thread (measured by the panel). */
  fits: boolean;
}

const EMPTY: PanelState = { order: [], dismissed: new Set(), hidden: false, fits: false };

// The header toggle, the panel and the live card (which gives up its PiP to the panel) mount
// apart, so they share one store per conversation, as `browserDock` does. The person's choices
// (and the order, so a reload keeps it) live in sessionStorage: they last for this browser tab,
// across navigation and reloads, and no longer.
const states = new Map<string, PanelState>();
const listeners = new Map<string, Set<() => void>>();

const storageKey = (sessionId: string) => `openside.agentTabs.${sessionId}`;

function load(sessionId: string): PanelState {
  try {
    const value: unknown = JSON.parse(sessionStorage.getItem(storageKey(sessionId)) ?? "null");
    if (!value || typeof value !== "object") return EMPTY;
    const { order, dismissed, hidden } = value as Record<string, unknown>;
    const strings = (list: unknown) => (Array.isArray(list) ? list.filter((id): id is string => typeof id === "string") : []);
    return { order: strings(order), dismissed: new Set(strings(dismissed)), hidden: hidden === true, fits: false };
  } catch {
    // Corrupt entry or storage unavailable: start from the default.
    return EMPTY;
  }
}

function read(sessionId: string): PanelState {
  let state = states.get(sessionId);
  if (!state) states.set(sessionId, (state = load(sessionId)));
  return state;
}

function update(sessionId: string, next: PanelState) {
  const current = read(sessionId);
  if (next === current) return;
  states.set(sessionId, next);
  if (next.order !== current.order || next.dismissed !== current.dismissed || next.hidden !== current.hidden) {
    try {
      sessionStorage.setItem(storageKey(sessionId), JSON.stringify({ order: next.order, dismissed: [...next.dismissed], hidden: next.hidden }));
    } catch {
      // Storage unavailable: the choice lasts for this page only.
    }
  }
  for (const listener of listeners.get(sessionId) ?? []) listener();
}

function subscribe(sessionId: string, listener: () => void) {
  let set = listeners.get(sessionId);
  if (!set) listeners.set(sessionId, (set = new Set()));
  set.add(listener);
  return () => {
    set.delete(listener);
    // Nothing shows this conversation any more: forget it in memory, so the maps do not grow;
    // sessionStorage keeps the choices for coming back.
    if (set.size === 0) {
      listeners.delete(sessionId);
      states.delete(sessionId);
    }
  };
}

/**
 * Folds the latest authoritative tab list into the store: a tab id never seen before brings the
 * panel back (it appears by itself when the agent opens a tab), and ids that left the list are
 * forgotten, so no hidden or stale card outlives its tab.
 *
 * Order: the daemon exposes no creation time, and lists tabs by target id, which says nothing
 * about age. Tabs seen together (the list at page load) are ranked by the conversation's own log,
 * `opened` (tab ids in the order its browser calls opened them); tabs the log does not show count
 * as older, last-listed newest. A tab that appears later is newer than everything before it.
 */
export function syncAgentTabs(sessionId: string, ids: readonly string[], opened: readonly string[]) {
  const current = read(sessionId);
  const listed = new Set(ids);
  const kept = current.order.filter((id) => listed.has(id));
  const rank = (id: string) => opened.indexOf(id);
  const added = ids.filter((id) => !current.order.includes(id)).sort((a, b) => rank(a) - rank(b));
  const dismissed = new Set([...current.dismissed].filter((id) => listed.has(id)));
  if (added.length === 0 && kept.length === current.order.length && dismissed.size === current.dismissed.size) return;
  update(sessionId, { ...current, order: [...kept, ...added], dismissed, hidden: added.length > 0 ? false : current.hidden });
}

export function setAgentTabsFit(sessionId: string, fits: boolean) {
  const current = read(sessionId);
  if (current.fits !== fits) update(sessionId, { ...current, fits });
}

export interface AgentTabsPanel {
  /** The conversation's agent tabs not hidden by the person, newest first. */
  cards: OwnedTab[];
  /** The conversation has agent tabs and room for the column, so the header toggle applies. */
  available: boolean;
  /** The column is on screen. */
  shown: boolean;
  toggle(): void;
  dismiss(tabId: string): void;
}

/** The desktop Agent Tabs column of one conversation, filtered from the shell's tab poller. */
export function useAgentTabsPanel(sessionId: string): AgentTabsPanel {
  const tabs = useContext(AgentTabsContext);
  const state = useSyncExternalStore(
    useCallback((listener: () => void) => subscribe(sessionId, listener), [sessionId]),
    () => read(sessionId),
  );
  const owned = useMemo(() => tabs.filter((tab) => tab.owner_session_id === sessionId), [tabs, sessionId]);
  const cards = useMemo(() => {
    // Ids not folded in yet (the first render after a poll) count as the newest.
    const rank = (id: string) => {
      const index = state.order.indexOf(id);
      return index === -1 ? Number.MAX_SAFE_INTEGER : index;
    };
    return owned.filter((tab) => !state.dismissed.has(tab.tab_id)).sort((a, b) => rank(b.tab_id) - rank(a.tab_id));
  }, [owned, state.order, state.dismissed]);
  const available = state.fits && owned.length > 0;
  const shown = available && !state.hidden && cards.length > 0;
  const toggle = useCallback(() => {
    const current = read(sessionId);
    // Turning the panel back on also brings back the cards hidden one by one.
    update(sessionId, shown ? { ...current, hidden: true } : { ...current, hidden: false, dismissed: new Set() });
  }, [sessionId, shown]);
  const dismiss = useCallback((tabId: string) => {
    const current = read(sessionId);
    update(sessionId, { ...current, dismissed: new Set([...current.dismissed, tabId]) });
  }, [sessionId]);
  return { cards, available, shown, toggle, dismiss };
}
