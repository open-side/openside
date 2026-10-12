import { ApiError, type Client } from "../api/client";
import type { AttachmentUpload, CreateSessionRequest } from "../api/types";
import { idbStore, OUTBOX_STORE, type RecordStore } from "./idb";

/**
 * A message typed while the daemon was unreachable. It keeps its `message_id` for life, so a
 * retry after a lost response is a no-op on the daemon (it dedups submissions by id).
 */
export interface OutboxEntry {
  message_id: string;
  /** `null` for a new chat: the conversation is created when the entry is delivered. */
  session_id: string | null;
  /** Only meaningful while `session_id` is `null`. */
  create?: CreateSessionRequest;
  text: string;
  attachments: AttachmentUpload[];
  queued_at: number;
  /** The daemon refused the message for good (a 4xx); it stays visible until discarded. */
  failure?: string;
}

export type FlushClient = Pick<Client, "createSession" | "submitMessage">;

export interface FlushResult {
  sent: number;
  failed: number;
  /** Conversations of new-chat entries delivered in this run, even if an earlier run created them. */
  created: string[];
}

const LOCK = "openside-outbox";
const CHANNEL = "openside-outbox";

/** A failure that says "try again later" rather than "this message is wrong". */
function transient(cause: unknown): boolean {
  if (!(cause instanceof ApiError)) return true;
  return cause.status === 0 || cause.status === 401 || cause.status === 429 || cause.status >= 500;
}

export interface Outbox {
  subscribe(listener: () => void): () => void;
  snapshot(): readonly OutboxEntry[];
  enqueue(entry: OutboxEntry): Promise<void>;
  discard(id: string): Promise<void>;
  /**
   * Sends everything that is waiting, oldest first, and stops at the first sign that the daemon
   * is still unreachable. The lock keeps two tabs from delivering the same entry together.
   */
  flush(client: FlushClient): Promise<FlushResult>;
}

export function createOutbox(store: RecordStore<OutboxEntry>, channel?: BroadcastChannel): Outbox {
  let entries: readonly OutboxEntry[] = [];
  let loaded: Promise<void> | null = null;
  const listeners = new Set<() => void>();

  const publish = (next: readonly OutboxEntry[]) => {
    entries = [...next].sort((a, b) => a.queued_at - b.queued_at);
    listeners.forEach((listener) => listener());
  };
  const reload = async () => {
    try {
      publish(await store.all());
    } catch {
      // Storage unavailable (private mode): the outbox simply stays empty.
    }
  };
  const load = () => (loaded ??= reload());
  if (channel) channel.onmessage = () => void reload();

  async function save(entry: OutboxEntry) {
    await store.put(entry);
    publish([...entries.filter((other) => other.message_id !== entry.message_id), entry]);
    channel?.postMessage(0);
  }

  async function drop(id: string) {
    await store.delete(id);
    publish(entries.filter((other) => other.message_id !== id));
    channel?.postMessage(0);
  }

  async function deliver(client: FlushClient, entry: OutboxEntry, result: FlushResult): Promise<"next" | "stop"> {
    let target = entry.session_id;
    try {
      if (target === null) {
        const session = await client.createSession(entry.create ?? {});
        target = session.id;
        // Remember the conversation before submitting: a lost response must not create a second one.
        await save({ ...entry, session_id: target });
      }
      await client.submitMessage(target, {
        message_id: entry.message_id,
        text: entry.text,
        mode: "queue",
        ...(entry.attachments.length ? { attachments: entry.attachments } : {}),
      });
    } catch (cause) {
      if (transient(cause)) return "stop";
      await save({ ...entry, failure: cause instanceof ApiError ? cause.message : "Request failed" });
      result.failed++;
      return "next";
    }
    await drop(entry.message_id);
    result.sent++;
    // `create` marks an entry from the new-chat screen, whose conversation the person has not seen yet.
    if (entry.create && target !== null) result.created.push(target);
    return "next";
  }

  return {
    subscribe(listener: () => void) {
      listeners.add(listener);
      void load();
      return () => void listeners.delete(listener);
    },
    snapshot: () => entries,

    async enqueue(entry: OutboxEntry) {
      await load();
      await save(entry);
    },

    async discard(id: string) {
      await drop(id);
    },

    async flush(client: FlushClient): Promise<FlushResult> {
      const result: FlushResult = { sent: 0, failed: 0, created: [] };
      const run = async () => {
        await reload();
        for (const entry of entries.filter((candidate) => !candidate.failure)) {
          if ((await deliver(client, entry, result)) === "stop") break;
        }
        return result;
      };
      return typeof navigator !== "undefined" && navigator.locks ? navigator.locks.request(LOCK, run) : run();
    },
  };
}

export const outbox: Outbox = createOutbox(
  idbStore<OutboxEntry>(OUTBOX_STORE),
  typeof BroadcastChannel === "function" ? new BroadcastChannel(CHANNEL) : undefined,
);

/** What belongs on one screen: a conversation's own entries, or the new-chat screen's (`null`). */
export function entriesFor(all: readonly OutboxEntry[], sessionId: string | null): OutboxEntry[] {
  return all.filter((entry) => entry.session_id === sessionId);
}
