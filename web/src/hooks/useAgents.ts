import { createContext, useCallback, useContext, useEffect, useRef, useState } from "react";
import { ApiError, type Client } from "../api/client";
import type { AgentSummary } from "../api/types";
import { seen, type SeenCursors } from "../state/seen";

const SYNC_MS = 5000;
export const CHIEF_ID = "chief";

export interface AgentsState {
  /** `null` until the first response. */
  agents: AgentSummary[] | null;
  error: string | null;
  /** The last sync could not reach the daemon at all. */
  offline: boolean;
  refresh: () => void;
}

/**
 * The team roster for the sidebar and thread headers. The registry is not event-sourced, so it
 * re-syncs on the same cadence as the conversation list while the page is visible.
 */
export function useAgents(client: Client): AgentsState {
  const [agents, setAgents] = useState<AgentSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [offline, setOffline] = useState(false);
  const generation = useRef(0);

  const load = useCallback(async () => {
    const mine = ++generation.current;
    try {
      const list = await client.listAgents();
      if (mine !== generation.current) return;
      seen.seed(Object.fromEntries(list.agents.map((agent) => [agent.id, agent.last_seq])));
      setAgents(list.agents);
      setError(null);
      setOffline(false);
    } catch (cause) {
      if (mine !== generation.current) return;
      setError(cause instanceof ApiError ? cause.message : "Request failed");
      setOffline(cause instanceof ApiError && cause.status === 0);
    }
  }, [client]);

  useEffect(() => {
    void load();
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
  return { agents, error, offline, refresh };
}

export const AgentsContext = createContext<AgentsState>({ agents: null, error: null, offline: false, refresh: () => {} });

export function useTeam(): AgentsState {
  return useContext(AgentsContext);
}

export interface People {
  chief: AgentSummary | null;
  /** Active, visible teammates: pinned first, then the most recent conversation first. */
  teammates: AgentSummary[];
  /** Removed from the sidebar but still working. */
  hidden: AgentSummary[];
}

const recency = (agent: AgentSummary) => {
  const at = agent.last_message_at ? Date.parse(agent.last_message_at) : Number.NaN;
  return Number.isNaN(at) ? 0 : at;
};

/** Sidebar order. Retired teammates live in History, never here. */
export function people(agents: readonly AgentSummary[]): People {
  const active = agents.filter((agent) => agent.status === "active");
  const chief = active.find((agent) => agent.kind === "chief") ?? null;
  const teammates = active
    .filter((agent) => agent.kind === "teammate")
    .sort((a, b) => Number(b.pinned) - Number(a.pinned) || recency(b) - recency(a) || a.name.localeCompare(b.name));
  return { chief, teammates: teammates.filter((agent) => !agent.hidden), hidden: teammates.filter((agent) => agent.hidden) };
}

/** Unread on this device: the thread moved past the cursor, and the person is not looking at it. */
export function isUnread(agent: AgentSummary, cursors: SeenCursors, openAgentId?: string | null): boolean {
  if (agent.id === openAgentId) return false;
  return agent.last_seq > (cursors[agent.id] ?? 0);
}
