import { useSyncExternalStore } from "react";

/**
 * Per-device read cursor for each agent's home thread. The daemon keeps no read state, so a
 * thread is unread on this device while its latest event sequence is past what was shown here.
 */
const KEY = "openside.seen.v1";

export type SeenCursors = Readonly<Record<string, number>>;

interface Stored {
  /** Set after the first roster this device saw, so existing history does not all start unread. */
  seeded: boolean;
  cursors: Record<string, number>;
}

function load(): Stored {
  try {
    const parsed = JSON.parse(localStorage.getItem(KEY) ?? "null") as Partial<Stored> | null;
    if (parsed && typeof parsed === "object" && parsed.cursors && typeof parsed.cursors === "object") {
      return { seeded: parsed.seeded === true, cursors: { ...parsed.cursors } };
    }
  } catch {
    // Unreadable storage behaves like a fresh device.
  }
  return { seeded: false, cursors: {} };
}

let state = load();
const listeners = new Set<() => void>();

function commit(next: Stored) {
  state = next;
  try {
    localStorage.setItem(KEY, JSON.stringify(next));
  } catch {
    // Private mode or a full quota keeps the cursor for this page only.
  }
  for (const listener of listeners) listener();
}

export const seen = {
  cursors: (): SeenCursors => state.cursors,
  seeded: () => state.seeded,
  /** Moves the cursor forward only: an older view arriving late never marks newer events unread. */
  mark(agentId: string, seq: number) {
    if ((state.cursors[agentId] ?? -1) >= seq) return;
    commit({ ...state, cursors: { ...state.cursors, [agentId]: seq } });
  },
  /** First roster on this device: everything already there counts as read. */
  seed(latest: Record<string, number>) {
    if (state.seeded) return;
    commit({ seeded: true, cursors: { ...latest, ...state.cursors } });
  },
  subscribe(listener: () => void) {
    listeners.add(listener);
    return () => listeners.delete(listener);
  },
  /** Tests start from a fresh device. */
  reset() {
    commit({ seeded: false, cursors: {} });
  },
};

export function useSeen(): SeenCursors {
  return useSyncExternalStore(seen.subscribe, seen.cursors, seen.cursors);
}
