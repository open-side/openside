import { useEffect, useSyncExternalStore } from "react";
import type { Client } from "../api/client";
import { outbox, type FlushResult, type OutboxEntry } from "./outbox";

const RETRY_MS = 10_000;

export function useOutboxEntries(): readonly OutboxEntry[] {
  return useSyncExternalStore(outbox.subscribe, outbox.snapshot);
}

/**
 * Delivers queued messages as soon as the daemon is reachable again: on the browser's `online`
 * event, when the app returns to the foreground, and on a slow timer while anything waits (a
 * tailnet can drop without the browser ever noticing it is offline).
 */
export function useOutboxFlush(client: Client, onFlushed: (result: FlushResult) => void): void {
  const waiting = useOutboxEntries().some((entry) => !entry.failure);
  useEffect(() => {
    const run = () => {
      if (document.visibilityState !== "visible") return;
      outbox.flush(client).then((result) => {
        if (result.sent || result.failed) onFlushed(result);
      }, () => undefined);
    };
    run();
    window.addEventListener("online", run);
    document.addEventListener("visibilitychange", run);
    const timer = waiting ? window.setInterval(run, RETRY_MS) : undefined;
    return () => {
      window.removeEventListener("online", run);
      document.removeEventListener("visibilitychange", run);
      window.clearInterval(timer);
    };
  }, [client, waiting, onFlushed]);
}
