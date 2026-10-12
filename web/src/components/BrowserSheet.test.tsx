import { act, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createClient } from "../api/client";
import { ClientContext } from "../context";
import { json } from "../test/fakeDaemon";
import { useSessionTabs, type SessionTabs } from "./BrowserSheet";

const tab = (id: string, owner: string) => ({ tab_id: id, url: `https://${id}.test/`, title: id, opened_by: "agent", owner_session_id: owner });

let visibility: DocumentVisibilityState = "visible";
function setVisibility(next: DocumentVisibilityState) {
  visibility = next;
  act(() => void document.dispatchEvent(new Event("visibilitychange")));
}

/** Every request stays pending until the test resolves it. */
function deferredClient() {
  const requests: ((body: unknown) => void)[] = [];
  const client = createClient({
    token: () => "t",
    fetch: (() => new Promise<Response>((resolve) => requests.push((body) => resolve(json(200, body))))) as typeof fetch,
  });
  return { client, requests };
}

let seen: SessionTabs | null = null;
function Probe({ sessionId }: { sessionId: string }) {
  seen = useSessionTabs(sessionId);
  return null;
}

/** Resolves a recorded request; a missing one means the hook never sent it. */
async function settle(resolve: ((body: unknown) => void) | undefined, body: unknown) {
  expect(resolve).toBeDefined();
  await act(async () => {
    resolve!(body);
    await vi.advanceTimersByTimeAsync(0);
  });
}

describe("useSessionTabs", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    visibility = "visible";
    seen = null;
    vi.spyOn(document, "visibilityState", "get").mockImplementation(() => visibility);
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("keeps one request in flight across a visibility round trip and clears its timer", async () => {
    const { client, requests } = deferredClient();
    const view = render(<ClientContext.Provider value={client}><Probe sessionId="s1" /></ClientContext.Provider>);
    expect(requests).toHaveLength(1);

    // Hidden and back while the first request is still unresolved: no second request.
    setVisibility("hidden");
    setVisibility("visible");
    expect(requests).toHaveLength(1);

    await settle(requests[0], { tabs: [tab("a", "s1"), tab("b", "s2")] });
    expect(seen?.tabs?.map((t) => t.tab_id)).toEqual(["a"]);
    expect(requests).toHaveLength(1);
    await act(() => vi.advanceTimersByTimeAsync(2000));
    expect(requests).toHaveLength(2);

    // Hidden when the request settles: the loop stops until the page is visible again.
    setVisibility("hidden");
    await settle(requests[1], { tabs: [] });
    await act(() => vi.advanceTimersByTimeAsync(10_000));
    expect(requests).toHaveLength(2);
    setVisibility("visible");
    expect(requests).toHaveLength(3);
    setVisibility("hidden");
    setVisibility("visible");
    expect(requests).toHaveLength(3);

    // Unmount with a timer armed: nothing fires afterwards.
    await settle(requests[2], { tabs: [] });
    view.unmount();
    await act(() => vi.advanceTimersByTimeAsync(10_000));
    expect(requests).toHaveLength(3);
  });

  it("drops a late answer from the previous session's loop", async () => {
    const { client, requests } = deferredClient();
    const view = render(<ClientContext.Provider value={client}><Probe sessionId="s1" /></ClientContext.Provider>);
    view.rerender(<ClientContext.Provider value={client}><Probe sessionId="s2" /></ClientContext.Provider>);
    expect(requests).toHaveLength(2);
    await settle(requests[1], { tabs: [tab("new", "s2")] });
    await settle(requests[0], { tabs: [tab("old", "s1")] });
    expect(seen?.tabs?.map((t) => t.tab_id)).toEqual(["new"]);
    // Only the current loop rescheduled.
    await act(() => vi.advanceTimersByTimeAsync(2000));
    expect(requests).toHaveLength(3);
  });
});
