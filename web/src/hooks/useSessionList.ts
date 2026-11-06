import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError, type Client } from "../api/client";
import type { Session } from "../api/types";
import { t } from "../i18n";

const SYNC_MS = 5000;

export interface SessionListState {
  /** `null` until the first response. */
  sessions: Session[] | null;
  error: string | null;
  /** The last sync could not reach the daemon at all (as opposed to an error it returned). */
  offline: boolean;
  query: string;
  /** The query `sessions` answers; it trails `query` while a new term's request is pending. */
  answered: string | null;
  /** Also drops the previous term's error, which says nothing about the new one. */
  setQuery: (query: string) => void;
  archived: boolean;
  setArchived: (archived: boolean) => void;
  refresh: () => void;
}

/**
 * A conversation list. The daemon filters and orders (pinned first), so typing is debounced and
 * a request for a term already typed past is aborted; while the page is visible the list
 * re-syncs every few seconds so titles and run status from other devices show up without a
 * reload. `agent: "none"` keeps conversations from before the team.
 */
export function useSessionList(client: Client, agent?: string): SessionListState {
  const [query, setQueryState] = useState("");
  const [archived, setArchived] = useState(false);
  const [sessions, setSessions] = useState<Session[] | null>(null);
  const [answered, setAnswered] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [offline, setOffline] = useState(false);
  const generation = useRef(0);
  const inflight = useRef<AbortController | null>(null);

  const load = useCallback(async () => {
    const mine = ++generation.current;
    inflight.current?.abort();
    const controller = new AbortController();
    inflight.current = controller;
    try {
      const list = await client.listSessions({ q: query, archived, agent }, controller.signal);
      if (mine !== generation.current) return;
      setSessions(list.sessions);
      setAnswered(query);
      setError(null);
      setOffline(false);
    } catch (cause) {
      // An aborted request always belongs to a superseded generation, so it lands here too.
      if (mine !== generation.current) return;
      setError(cause instanceof ApiError ? cause.message : t("browser.requestFailed"));
      setOffline(cause instanceof ApiError && cause.status === 0);
    } finally {
      if (inflight.current === controller) inflight.current = null;
    }
  }, [client, query, archived, agent]);

  useEffect(() => {
    // Not DOM refs: bumping the counter drops a response for the previous filter, and the
    // controller cancels it on the wire (also on unmount).
    const responses = generation;
    const requests = inflight;
    const timer = window.setTimeout(() => void load(), 200);
    return () => {
      window.clearTimeout(timer);
      responses.current++;
      requests.current?.abort();
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

  const setQuery = useCallback((next: string) => {
    setQueryState(next);
    setError(null);
    setOffline(false);
  }, []);
  const refresh = useCallback(() => void load(), [load]);

  return { sessions, error, offline, query, answered, setQuery, archived, setArchived, refresh };
}
