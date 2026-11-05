import { useCallback, useSyncExternalStore } from "react";

type Pip = "open" | "minimized" | "closed";
interface Dock {
  pip: Pip;
  /** The person picked `pip` (Show browser, minimize, close) rather than it being the default. */
  chosen: boolean;
  sheet: boolean;
  /** The tab the sheet opened on, when the opener named one; null follows the agent's tab. */
  sheetTab: string | null;
}

const DEFAULT: Dock = { pip: "open", chosen: false, sheet: false, sheetTab: null };
const PIPS: readonly string[] = ["open", "minimized", "closed"];

// The header button (`BrowserButton`) and the live card (`BrowserPreview`) are mounted apart, so
// they share one per-session store; the parsed value is cached by its raw string to keep the
// snapshot stable between renders, as `useSyncExternalStore` requires.
//
// Only the PiP choice is stored. The sheet is a modal: it stays open across the header and the
// card but closes once the conversation is left, so coming back to it (or any full page load of
// it) shows the conversation rather than reopening the sheet over it.
const listeners = new Map<string, Set<() => void>>();
const parsed = new Map<string, { raw: string | null; dock: Dock }>();

function rawOf(sessionId: string): string | null {
  try {
    return sessionStorage.getItem(`openside.browserDock.${sessionId}`);
  } catch {
    return null;
  }
}

function read(sessionId: string): Dock {
  const raw = rawOf(sessionId);
  const cached = parsed.get(sessionId);
  if (cached && cached.raw === raw) return cached.dock;
  let pip = DEFAULT.pip;
  let chosen = false;
  try {
    const value: unknown = JSON.parse(raw ?? "null");
    if (value && typeof value === "object") {
      const stored = (value as Record<string, unknown>).pip;
      if (typeof stored === "string" && PIPS.includes(stored)) {
        pip = stored as Pip;
        chosen = true;
      }
    }
  } catch {
    // Corrupt entry: start from the default dock.
  }
  const dock = { pip, chosen, sheet: cached?.dock.sheet ?? DEFAULT.sheet, sheetTab: cached?.dock.sheetTab ?? DEFAULT.sheetTab };
  parsed.set(sessionId, { raw, dock });
  return dock;
}

function write(sessionId: string, patch: Partial<Pick<Dock, "pip" | "sheet" | "sheetTab">>) {
  const current = read(sessionId);
  const next = { ...current, ...patch, chosen: current.chosen || patch.pip !== undefined };
  if (next.pip === current.pip && next.chosen === current.chosen && next.sheet === current.sheet && next.sheetTab === current.sheetTab) return;
  let raw = rawOf(sessionId);
  if (next.pip !== current.pip || next.chosen !== current.chosen) {
    try {
      sessionStorage.setItem(`openside.browserDock.${sessionId}`, JSON.stringify({ pip: next.pip }));
      raw = rawOf(sessionId);
    } catch {
      // Storage unavailable: keep the choice for this page only. `rawOf` keeps answering null,
      // so the cache entry stays keyed on that.
    }
  }
  parsed.set(sessionId, { raw, dock: next });
  for (const listener of listeners.get(sessionId) ?? []) listener();
}

function subscribe(sessionId: string, listener: () => void) {
  let set = listeners.get(sessionId);
  if (!set) listeners.set(sessionId, (set = new Set()));
  set.add(listener);
  return () => {
    set.delete(listener);
    // Nothing shows this conversation any more: forget it, so visiting many conversations does
    // not grow the maps, and an open sheet closes. sessionStorage keeps the PiP choice.
    if (set.size === 0) {
      listeners.delete(sessionId);
      parsed.delete(sessionId);
    }
  };
}

export interface BrowserDock {
  pip: Pip;
  /** Whether `pip` was set in this browser tab, so the PiP was asked for rather than defaulted. */
  summoned: boolean;
  setPip(next: Pip): void;
  sheet: boolean;
  sheetTab: string | null;
  /** Opens the sheet (on `tabId` when given) or closes it. */
  setSheet(open: boolean, tabId?: string): void;
}

/**
 * Where a conversation's browser is shown: the floating PiP (`open`, `minimized` to a pill, or
 * `closed` until the header button summons it), remembered per browser tab in sessionStorage,
 * and whether the browser sheet is open, for as long as the conversation is on screen.
 */
export function useBrowserDock(sessionId: string): BrowserDock {
  const dock = useSyncExternalStore(
    useCallback((listener: () => void) => subscribe(sessionId, listener), [sessionId]),
    () => read(sessionId),
  );
  const setPip = useCallback((pip: Pip) => write(sessionId, { pip }), [sessionId]);
  const setSheet = useCallback((sheet: boolean, tabId?: string) => write(sessionId, { sheet, sheetTab: sheet ? (tabId ?? null) : null }), [sessionId]);
  return { pip: dock.pip, summoned: dock.chosen, setPip, sheet: dock.sheet, sheetTab: dock.sheetTab, setSheet };
}
