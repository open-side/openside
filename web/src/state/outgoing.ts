import { useSyncExternalStore } from "react";
import { ApiError, type Client } from "../api/client";
import type { AttachmentUpload } from "../api/types";

/**
 * A message this device has sent whose `message.queued` event has not come back yet. It is
 * what lets the transcript show "Sending", "Sent" and "Not sent" before the daemon's event log
 * can speak for the message; once the event arrives the log takes over and the entry goes.
 */
export interface OutgoingMessage {
  message_id: string;
  session_id: string;
  text: string;
  attachments: AttachmentUpload[];
  /** `sent`: the daemon accepted the POST. `failed`: it did not; the person can retry. */
  state: "sending" | "sent" | "failed";
  error?: string;
}

let entries: readonly OutgoingMessage[] = [];
const listeners = new Set<() => void>();

function publish(next: readonly OutgoingMessage[]): void {
  entries = next;
  listeners.forEach((listener) => listener());
}

function patch(id: string, change: Partial<OutgoingMessage>): void {
  publish(entries.map((entry) => (entry.message_id === id ? { ...entry, ...change } : entry)));
}

const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => void listeners.delete(listener);
};

export const outgoing = {
  snapshot: (): readonly OutgoingMessage[] => entries,
  subscribe,
  forget(id: string): void {
    if (entries.some((entry) => entry.message_id === id)) publish(entries.filter((entry) => entry.message_id !== id));
  },
  /** Drops every entry of `sessionId` whose message the event log already knows. */
  prune(sessionId: string, known: ReadonlySet<string>): void {
    const keep = entries.filter((entry) => entry.session_id !== sessionId || !known.has(entry.message_id));
    if (keep.length !== entries.length) publish(keep);
  },
  /**
   * Posts the message (again, on retry) under its original id, so a retry after a lost
   * response is a no-op on the daemon. Resolves to the failure, or `null` when accepted.
   */
  async submit(client: Pick<Client, "submitMessage">, message: Omit<OutgoingMessage, "state" | "error">): Promise<ApiError | null> {
    const entry: OutgoingMessage = { ...message, state: "sending" };
    publish(entries.some((e) => e.message_id === entry.message_id) ? entries.map((e) => (e.message_id === entry.message_id ? entry : e)) : [...entries, entry]);
    try {
      await client.submitMessage(message.session_id, {
        message_id: message.message_id,
        text: message.text,
        mode: "queue",
        ...(message.attachments.length ? { attachments: message.attachments } : {}),
      });
      patch(message.message_id, { state: "sent" });
      return null;
    } catch (cause) {
      const failure = cause instanceof ApiError ? cause : new ApiError(0, "request_failed", "Request failed");
      patch(message.message_id, { state: "failed", error: failure.status === 0 ? "Can’t reach openside" : failure.message });
      return failure;
    }
  },
};

export function useOutgoing(sessionId: string): readonly OutgoingMessage[] {
  const all = useSyncExternalStore(subscribe, outgoing.snapshot);
  return all.filter((entry) => entry.session_id === sessionId);
}
