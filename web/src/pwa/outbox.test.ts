import "fake-indexeddb/auto";
import { describe, expect, it, vi } from "vitest";
import { ApiError } from "../api/client";
import type { CreateSessionRequest, Session, SubmitMessageRequest } from "../api/types";
import { idbStore, OUTBOX_STORE, type RecordStore } from "./idb";
import { createOutbox, entriesFor, type FlushClient, type OutboxEntry } from "./outbox";

function memoryStore(): RecordStore<OutboxEntry> & { rows: Map<string, OutboxEntry> } {
  const rows = new Map<string, OutboxEntry>();
  return {
    rows,
    all: async () => [...rows.values()],
    put: async (value) => void rows.set(value.message_id, structuredClone(value)),
    delete: async (key) => void rows.delete(key),
  };
}

function entry(id: string, over: Partial<OutboxEntry> = {}): OutboxEntry {
  return { message_id: id, session_id: "s1", text: `text ${id}`, attachments: [], queued_at: Number(id.replace(/\D/g, "")) || 0, ...over };
}

function client(over: Partial<FlushClient> = {}) {
  const submitMessage = vi.fn(async (_id: string, _request: SubmitMessageRequest) => ({}) as never);
  const createSession = vi.fn(async (_request: CreateSessionRequest) => ({ id: "created" }) as Session);
  return { createSession, submitMessage, ...over } as FlushClient & { createSession: typeof createSession; submitMessage: typeof submitMessage };
}

const offline = () => new ApiError(0, "network_error");

describe("outbox", () => {
  it("delivers queued messages oldest first and empties the store", async () => {
    const store = memoryStore();
    const box = createOutbox(store);
    await box.enqueue(entry("m2"));
    await box.enqueue(entry("m1"));
    const api = client();
    const result = await box.flush(api);
    expect(result).toEqual({ sent: 2, failed: 0, created: [] });
    expect(api.submitMessage.mock.calls.map(([, request]) => request.message_id)).toEqual(["m1", "m2"]);
    expect(api.submitMessage.mock.calls[0]![1]).toMatchObject({ mode: "queue", text: "text m1" });
    expect(store.rows.size).toBe(0);
    expect(box.snapshot()).toEqual([]);
  });

  it("keeps everything while the daemon is unreachable and resends under the same message id", async () => {
    const store = memoryStore();
    const box = createOutbox(store);
    await box.enqueue(entry("m1"));
    await box.enqueue(entry("m2"));
    const down = client({ submitMessage: vi.fn(async (_id: string, _request: SubmitMessageRequest) => Promise.reject(offline())) });
    expect(await box.flush(down)).toEqual({ sent: 0, failed: 0, created: [] });
    // Stops at the first failure instead of hammering a dead daemon once per message.
    expect(down.submitMessage).toHaveBeenCalledTimes(1);
    expect(store.rows.size).toBe(2);

    const up = client();
    await box.flush(up);
    expect(up.submitMessage.mock.calls.map(([, request]) => request.message_id)).toEqual(["m1", "m2"]);
    expect(down.submitMessage.mock.calls[0]![1].message_id).toBe("m1");
  });

  it.each([401, 429, 503])("treats HTTP %i as try-again-later, not as a refusal", async (status) => {
    const box = createOutbox(memoryStore());
    await box.enqueue(entry("m1"));
    const api = client({ submitMessage: vi.fn(async (_id: string, _request: SubmitMessageRequest) => Promise.reject(new ApiError(status, "x"))) });
    await box.flush(api);
    expect(box.snapshot()[0]).toMatchObject({ message_id: "m1" });
    expect(box.snapshot()[0]!.failure).toBeUndefined();
  });

  it("marks a refused message failed, keeps it visible and carries on with the rest", async () => {
    const store = memoryStore();
    const box = createOutbox(store);
    await box.enqueue(entry("m1"));
    await box.enqueue(entry("m2"));
    const api = client({
      submitMessage: vi.fn(async (_id: string, request: SubmitMessageRequest) => {
        if (request.message_id === "m1") throw new ApiError(413, "attachments_too_large");
        return {} as never;
      }),
    });
    const result = await box.flush(api);
    expect(result).toMatchObject({ sent: 1, failed: 1 });
    expect(store.rows.get("m1")?.failure).toBeTruthy();
    expect(store.rows.has("m2")).toBe(false);
    // A failed entry is not retried on the next flush.
    await box.flush(api);
    expect(api.submitMessage).toHaveBeenCalledTimes(2);
    await box.discard("m1");
    expect(store.rows.size).toBe(0);
  });

  it("creates the chat of a new-chat entry exactly once even when the send is lost afterwards", async () => {
    const store = memoryStore();
    const box = createOutbox(store);
    await box.enqueue(entry("m1", { session_id: null, create: { title: "Hello" } }));
    const lost = client({ submitMessage: vi.fn(async (_id: string, _request: SubmitMessageRequest) => Promise.reject(offline())) });
    await box.flush(lost);
    expect(lost.createSession).toHaveBeenCalledWith({ title: "Hello" });
    expect(store.rows.get("m1")?.session_id).toBe("created");

    const up = client();
    const result = await box.flush(up);
    expect(up.createSession).not.toHaveBeenCalled();
    expect(up.submitMessage).toHaveBeenCalledWith("created", expect.objectContaining({ message_id: "m1" }));
    // Still reported, so the app can open it for a person who is waiting on the empty screen.
    expect(result).toEqual({ sent: 1, failed: 0, created: ["created"] });
  });

  it("reports the chat a flush created", async () => {
    const box = createOutbox(memoryStore());
    await box.enqueue(entry("m1", { session_id: null, create: { title: "Hello" } }));
    expect((await box.flush(client())).created).toEqual(["created"]);
  });

  it("sends attachments only when present and notifies subscribers", async () => {
    const box = createOutbox(memoryStore());
    const seen = vi.fn();
    box.subscribe(seen);
    await box.enqueue(entry("m1", { attachments: [{ name: "a.txt", data_base64: "QQ==" }] }));
    await box.enqueue(entry("m2"));
    const api = client();
    await box.flush(api);
    expect(api.submitMessage.mock.calls[0]![1]).toMatchObject({ attachments: [{ name: "a.txt", data_base64: "QQ==" }] });
    expect(api.submitMessage.mock.calls[1]![1]).not.toHaveProperty("attachments");
    expect(seen).toHaveBeenCalled();
  });

  it("scopes entries to the conversation they belong to", () => {
    const all = [entry("m1", { session_id: null }), entry("m2", { session_id: "s1" }), entry("m3", { session_id: "s2" })];
    expect(entriesFor(all, null).map((e) => e.message_id)).toEqual(["m1"]);
    expect(entriesFor(all, "s1").map((e) => e.message_id)).toEqual(["m2"]);
  });
});

describe("IndexedDB store", () => {
  it("survives a reload: entries written by one outbox are delivered by a fresh one", async () => {
    const first = createOutbox(idbStore<OutboxEntry>(OUTBOX_STORE));
    await first.enqueue(entry("idb1", { attachments: [{ name: "a.png", data_base64: "QQ==" }] }));
    const second = createOutbox(idbStore<OutboxEntry>(OUTBOX_STORE));
    const api = client();
    expect((await second.flush(api)).sent).toBe(1);
    expect(api.submitMessage).toHaveBeenCalledWith("s1", expect.objectContaining({ message_id: "idb1", attachments: [{ name: "a.png", data_base64: "QQ==" }] }));
    expect(await idbStore<OutboxEntry>(OUTBOX_STORE).all()).toEqual([]);
  });
});
