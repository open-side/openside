import "fake-indexeddb/auto";
import { describe, expect, it } from "vitest";
import { idbPut, SHARE_STORE } from "./idb";
import { needsYouTarget, parseLaunch } from "./launch";
import { fitFiles, shareDraftText, takeShare, type SharedItem } from "./share";

describe("launch intents", () => {
  it("reads manifest shortcuts and the share-target redirect", () => {
    expect(parseLaunch("?launch=new")).toEqual({ kind: "new" });
    expect(parseLaunch("?launch=needs-you")).toEqual({ kind: "needs-you" });
    expect(parseLaunch("?share=3f2a9c1e-77aa-4b1d-9d11-0a0b0c0d0e0f")).toEqual({ kind: "share", id: "3f2a9c1e-77aa-4b1d-9d11-0a0b0c0d0e0f" });
    expect(parseLaunch("?share=failed")).toEqual({ kind: "share-failed" });
  });

  it("ignores everything else, including share ids that are not ones the worker mints", () => {
    expect(parseLaunch("")).toBeNull();
    expect(parseLaunch("?launch=elsewhere")).toBeNull();
    expect(parseLaunch("?share=../../etc")).toBeNull();
    expect(parseLaunch("?share=x")).toBeNull();
  });

  it("sends Needs you to the conversation whose ticket has waited longest", () => {
    const tickets = [
      { session_id: "newer", status: "pending", created_at: "2026-10-12T10:05:00Z" },
      { session_id: "older", status: "pending", created_at: "2026-10-12T10:00:00Z" },
      { session_id: "older", status: "pending", created_at: "2026-10-12T10:01:00Z" },
      { session_id: "done", status: "resolved", created_at: "2026-10-12T09:00:00Z" },
    ] as const;
    expect(needsYouTarget(tickets)).toEqual({ sessionId: "older", conversations: 2 });
    expect(needsYouTarget([tickets[3]])).toBeNull();
    expect(needsYouTarget([])).toBeNull();
  });
});

describe("shared content", () => {
  it.each([
    [{ title: "Page title", text: "https://example.com/a", url: "" }, "Page title\nhttps://example.com/a"],
    [{ title: "Page title", text: "", url: "https://example.com/a" }, "Page title\nhttps://example.com/a"],
    [{ title: "T", text: "Read https://example.com/a please", url: "https://example.com/a" }, "T\nRead https://example.com/a please"],
    [{ title: "same", text: "same", url: "" }, "same"],
    [{ title: " ", text: "\n", url: "" }, ""],
  ])("turns %j into the draft text", (parts, expected) => {
    expect(shareDraftText(parts)).toBe(expected);
  });

  const item = (over: Partial<SharedItem> = {}): SharedItem => ({
    id: "share-aaaa-bbbb",
    title: "Title",
    text: "",
    url: "https://example.com",
    files: [],
    received_at: Date.now(),
    ...over,
  });

  it("hands a parked share over once, as files, and deletes it", async () => {
    await idbPut(SHARE_STORE, item({ files: [{ name: "shot.png", type: "image/png", data: new TextEncoder().encode("bytes").buffer as ArrayBuffer }] }));
    const draft = await takeShare("share-aaaa-bbbb");
    expect(draft).toMatchObject({ id: "share-aaaa-bbbb", text: "Title\nhttps://example.com" });
    expect(draft!.files).toHaveLength(1);
    expect(draft!.files[0]).toMatchObject({ name: "shot.png", type: "image/png", size: 5 });
    expect(await takeShare("share-aaaa-bbbb")).toBeNull();
  });

  it("drops a share nobody opened within the hour", async () => {
    await idbPut(SHARE_STORE, item({ id: "share-old-0001", received_at: 1000 }));
    expect(await takeShare("share-old-0001", 1000 + 61 * 60 * 1000)).toBeNull();
    expect(await takeShare("share-old-0001")).toBeNull();
  });

  it("returns files that already fit, and non-images, untouched", async () => {
    const small = [new File(["x"], "a.txt", { type: "text/plain" })];
    expect(await fitFiles(small)).toBe(small);
    const big = [new File([new Uint8Array(3000)], "log.txt", { type: "text/plain" })];
    expect(await fitFiles(big, 1000)).toEqual(big);
  });
});
