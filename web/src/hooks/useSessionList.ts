import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError, type Client } from "../api/client";
import type { Session } from "../api/types";

const SYNC_MS = 5000;

export interface SessionListState {
  /** `null` until the first response. */
  sessions: Session[] | null;
  error: string | null;
  query: string;
  setQuery: (query: string) => void;
  archived: boolean;
  setArchived: (archived: boolean) => void;
  refresh: () => void;
}

/**
 * The sidebar's conversation list. The daemon filters and orders (pinned first), so typing is
 * debounced; while the page is visible the list re-syncs every few seconds so titles and run
 * status from other devices show up without a reload.
 */
export function useSessionList(client: Client): SessionListState {
  const [query, setQuery] = useState("");
  const [archived, setArchived] = useState(false);
  const [sessions, setSessions] = useState<Session[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const generation = useRef(0);

  const load = useCallback(async () => {
    const mine = ++generation.current;
    try {
      const list = await client.listSessions({ q: query, archived });
      if (mine !== generation.current) return;
      setSessions(list.sessions);
      setError(null);
    } catch (cause) {
      if (mine === generation.current) setError(cause instanceof ApiError ? cause.message : "Request failed");
    }
  }, [client, query, archived]);

  useEffect(() => {
    // Not a DOM ref: bumping the counter drops a response for the previous filter.
    const responses = generation;
    const timer = window.setTimeout(() => void load(), 200);
    return () => {
      window.clearTimeout(timer);
      responses.current++;
    };
  }, [load]);

  useEffect(() => {
    const sync = () => {
      if (document.visibilityState === "visible") void load();
    };
    const timer = window.setInterval(sync, SYNC_MS);
    document.addEventListener("visibilitychange", sync);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", sync);
    };
  }, [load]);

  const refresh = useCallback(() => void load(), [load]);

  return { sessions, error, query, setQuery, archived, setArchived, refresh };
}
