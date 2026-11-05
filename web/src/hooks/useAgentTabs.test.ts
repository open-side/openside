import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { BrowserTab } from "../api/types";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { TABS_MAX_BACKOFF_MS, TABS_POLL_MS, useAgentTabs } from "./useAgentTabs";

const tab = (over: Partial<BrowserTab>): BrowserTab => ({ tab_id: "t", title: "Page", url: "https://shop.example/", opened_by: "agent", owner_session_id: "s1", ...over });

let visibility: DocumentVisibilityState = "visible";
function setVisibility(next: DocumentVisibilityState) {
  visibility = next;
  document.dispatchEvent(new Event("visibilitychange"));
}

beforeEach(() => {
  vi.useFakeTimers();
  visibility = "visible";
  Object.defineProperty(document, "visibilityState", { configurable: true, get: () => visibility });
});

afterEach(() => {
  vi.useRealTimers();
  Reflect.deleteProperty(document, "visibilityState");
});

const flush = () => act(() => vi.advanceTimersByTimeAsync(0));
const wait = (ms: number) => act(() => vi.advanceTimersByTimeAsync(ms));

function setup(handler: (call: Call) => Response | Promise<Response>) {
  const daemon = fakeClient(handler);
  const hook = renderHook(() => useAgentTabs(daemon.client));
  const requests = () => daemon.calls.filter((call) => call.path === "/v1/browser/tabs").length;
  return { ...hook, requests };
}

describe("useAgentTabs", () => {
  it("keeps only tabs that belong to a conversation and asks again 5 s after each answer", async () => {
    const { result, requests } = setup(() =>
      json(200, { tabs: [tab({ tab_id: "a" }), tab({ tab_id: "b", opened_by: "user" }), tab({ tab_id: "c", owner_session_id: null })] }),
    );
    await flush();
    expect(result.current.tabs.map((t) => t.tab_id)).toEqual(["a"]);
    expect(result.current.settled).toBe(true);
    expect(requests()).toBe(1);
    await wait(TABS_POLL_MS - 1);
    expect(requests()).toBe(1);
    await wait(1);
    expect(requests()).toBe(2);
  });

  it("never has two requests out: the next waits for a slow answer", async () => {
    const answer = Promise.withResolvers<Response>();
    const { requests } = setup(() => answer.promise);
    await flush();
    await wait(TABS_POLL_MS * 4);
    expect(requests()).toBe(1);
    await act(async () => answer.resolve(json(200, { tabs: [] })));
    await wait(TABS_POLL_MS);
    expect(requests()).toBe(2);
  });

  it("backs off on failures up to a minute and returns to 5 s after a success", async () => {
    let failing = true;
    const { requests } = setup(() => (failing ? json(500, { code: "internal", message: "boom" }) : json(200, { tabs: [] })));
    await flush();
    const gaps = [2, 4, 8, 12, 12].map((n) => n * TABS_POLL_MS);
    for (const [index, gap] of gaps.entries()) {
      await wait(gap - 1);
      expect(requests()).toBe(index + 1);
      await wait(1);
      expect(requests()).toBe(index + 2);
    }
    expect(gaps.at(-1)).toBe(TABS_MAX_BACKOFF_MS);
    failing = false;
    const before = requests();
    await wait(TABS_MAX_BACKOFF_MS);
    expect(requests()).toBe(before + 1);
    await wait(TABS_POLL_MS);
    expect(requests()).toBe(before + 2);
  });

  it("asks nothing while the page is hidden and asks at once when it is shown again", async () => {
    const { requests } = setup(() => json(200, { tabs: [] }));
    await flush();
    setVisibility("hidden");
    await wait(TABS_POLL_MS * 10);
    expect(requests()).toBe(1);
    setVisibility("visible");
    await flush();
    expect(requests()).toBe(2);
    await wait(TABS_POLL_MS);
    expect(requests()).toBe(3);
  });

  it("stops for good once the token is refused", async () => {
    const { result, requests } = setup(() => json(403, { code: "forbidden", message: "missing scope" }));
    await flush();
    expect(result.current).toEqual({ tabs: [], settled: false });
    await wait(TABS_MAX_BACKOFF_MS * 3);
    setVisibility("hidden");
    setVisibility("visible");
    await flush();
    expect(requests()).toBe(1);
  });

  it("ignores an answer that lands after unmount and schedules nothing more", async () => {
    const answer = Promise.withResolvers<Response>();
    const { result, unmount, requests } = setup(() => answer.promise);
    await flush();
    unmount();
    await act(async () => answer.resolve(json(200, { tabs: [tab({ tab_id: "late" })] })));
    await wait(TABS_MAX_BACKOFF_MS);
    expect(result.current).toEqual({ tabs: [], settled: false });
    expect(requests()).toBe(1);
  });
});
