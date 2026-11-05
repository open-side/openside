import { createContext, useEffect, useState } from "react";
import { ApiError, type Client } from "../api/client";
import type { BrowserTab } from "../api/types";

export const TABS_POLL_MS = 5000;
export const TABS_MAX_BACKOFF_MS = 60_000;

export type OwnedTab = BrowserTab & { owner_session_id: string };

/** The shell's `useAgentTabs` list, for screens under the layout (the conversation's tab panel). */
export const AgentTabsContext = createContext<readonly OwnedTab[]>([]);
/**
 * Whether that list is the daemon's latest successful answer. It is empty, and this false, before
 * the first answer and after a failed poll, so an empty list is only proof of no tabs when true.
 */
export const AgentTabsSettledContext = createContext(false);

export interface AgentTabsPoll {
  tabs: OwnedTab[];
  settled: boolean;
}

/**
 * The agents' open tabs for the sidebar, polled by one owner for the whole shell (the desktop
 * column and the phone drawer both render it). One request at a time, the next one scheduled
 * only after the previous settles; failures back off up to a minute and a success resets it.
 * Nothing is asked while the page is hidden, and nothing ever again once the token is refused.
 */
export function useAgentTabs(client: Client): AgentTabsPoll {
  const [state, setState] = useState<AgentTabsPoll>({ tabs: [], settled: false });
  useEffect(() => {
    let stopped = false;
    let inFlight = false;
    let timer: number | undefined;
    let delay = TABS_POLL_MS;
    const schedule = () => {
      window.clearTimeout(timer);
      timer = undefined;
      if (!stopped && document.visibilityState === "visible") timer = window.setTimeout(() => void poll(), delay);
    };
    const poll = async () => {
      timer = undefined;
      if (stopped || inFlight || document.visibilityState !== "visible") return;
      inFlight = true;
      try {
        const list = await client.browserTabs();
        if (stopped) return;
        // The person's own tabs stay theirs; only tabs that belong to a conversation are listed.
        setState({ tabs: list.tabs.filter((tab): tab is OwnedTab => tab.opened_by !== "user" && !!tab.owner_session_id), settled: true });
        delay = TABS_POLL_MS;
      } catch (cause) {
        if (stopped) return;
        setState({ tabs: [], settled: false });
        if (cause instanceof ApiError && (cause.status === 401 || cause.status === 403)) {
          stopped = true;
          return;
        }
        delay = Math.min(delay * 2, TABS_MAX_BACKOFF_MS);
      } finally {
        inFlight = false;
      }
      schedule();
    };
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
  return state;
}
