import { act, fireEvent, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Client } from "../api/client";
import { pendingTicketIds, setAppBadge, usePendingTicketBadge } from "./badge";
import { useEdgeSwipe } from "./edgeSwipe";
import { haptic } from "./haptics";
import { currentOffer } from "./install";

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("app badge", () => {
  it("sets the count, and clears at zero", () => {
    const setBadge = vi.fn(async () => undefined);
    const clearBadge = vi.fn(async () => undefined);
    Object.assign(navigator, { setAppBadge: setBadge, clearAppBadge: clearBadge });
    setAppBadge(3);
    expect(setBadge).toHaveBeenCalledWith(3);
    setAppBadge(0);
    expect(clearBadge).toHaveBeenCalled();
  });

  it("does nothing where the Badging API is missing", () => {
    Object.assign(navigator, { setAppBadge: undefined, clearAppBadge: undefined });
    expect(() => setAppBadge(2)).not.toThrow();
  });

  it("counts only pending tickets", () => {
    expect(
      pendingTicketIds([
        { id: "a", status: "pending" },
        { id: "b", status: "resolved" },
        { id: "c", status: "needs_desktop" },
        { id: "d", status: "pending" },
      ]),
    ).toEqual(["a", "d"]);
  });

  describe("usePendingTicketBadge", () => {
    const setBadge = vi.fn(async () => undefined);
    const clearBadge = vi.fn(async () => undefined);
    const vibrate = vi.fn(() => true);
    beforeEach(() => {
      vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
      setBadge.mockClear();
      clearBadge.mockClear();
      vibrate.mockClear();
      Object.assign(navigator, { setAppBadge: setBadge, clearAppBadge: clearBadge, vibrate });
    });
    afterEach(() => vi.useRealTimers());

    const ticket = (id: string, status = "pending") => ({ id, status });
    function clientWith(lists: { id: string; status: string }[][]) {
      const listTickets = vi.fn(async () => ({ tickets: lists.shift() ?? [] }));
      return { listTickets } as unknown as Client;
    }

    it("mirrors the pending count, nudges only for tickets that appear later, and clears on unmount", async () => {
      const client = clientWith([[ticket("t1")], [ticket("t1"), ticket("t2")], []]);
      const { unmount } = renderHook(() => usePendingTicketBadge(client));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(setBadge).toHaveBeenLastCalledWith(1);
      expect(vibrate).not.toHaveBeenCalled();

      await act(async () => {
        await vi.advanceTimersByTimeAsync(15_000);
      });
      expect(setBadge).toHaveBeenLastCalledWith(2);
      expect(vibrate).toHaveBeenCalledTimes(1);

      await act(async () => {
        await vi.advanceTimersByTimeAsync(15_000);
      });
      expect(clearBadge).toHaveBeenCalled();
      expect(vibrate).toHaveBeenCalledTimes(1);

      clearBadge.mockClear();
      unmount();
      expect(clearBadge).toHaveBeenCalled();
    });

    it("keeps the badge when the daemon cannot be reached", async () => {
      const client = { listTickets: vi.fn(async () => Promise.reject(new Error("down"))) } as unknown as Client;
      renderHook(() => usePendingTicketBadge(client));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(setBadge).not.toHaveBeenCalled();
      expect(clearBadge).not.toHaveBeenCalled();
    });
  });
});

describe("haptics", () => {
  it("vibrates with a distinct pattern per kind and is silent without the API", () => {
    const vibrate = vi.fn((_pattern: VibratePattern) => true);
    Object.assign(navigator, { vibrate });
    haptic("approve");
    haptic("reject");
    expect(vibrate.mock.calls[0]![0]).not.toEqual(vibrate.mock.calls[1]![0]);
    Object.assign(navigator, { vibrate: undefined });
    expect(() => haptic("approve")).not.toThrow();
  });
});

describe("edge swipe", () => {
  function touch(type: "touchStart" | "touchMove", x: number, y: number) {
    fireEvent[type](document, { touches: [{ clientX: x, clientY: y }] });
  }
  function phone(matches = true) {
    vi.stubGlobal("matchMedia", (query: string) => ({ matches: matches && query.includes("820px"), media: query }));
  }

  it("opens on a mostly-horizontal swipe that starts at the left edge", () => {
    phone();
    const open = vi.fn();
    renderHook(() => useEdgeSwipe(open));
    touch("touchStart", 8, 300);
    touch("touchMove", 40, 304);
    expect(open).not.toHaveBeenCalled();
    touch("touchMove", 80, 310);
    expect(open).toHaveBeenCalledTimes(1);
    touch("touchMove", 120, 310);
    expect(open).toHaveBeenCalledTimes(1);
  });

  it.each([
    ["starts away from the edge", 60, 300, 140, 300],
    ["is mostly vertical (a scroll)", 8, 300, 70, 420],
  ])("ignores a swipe that %s", (_name, x0, y0, x1, y1) => {
    phone();
    const open = vi.fn();
    renderHook(() => useEdgeSwipe(open));
    touch("touchStart", x0, y0);
    touch("touchMove", x1, y1);
    expect(open).not.toHaveBeenCalled();
  });

  it("stays off on wide screens and while a dialog is open", () => {
    const open = vi.fn();
    renderHook(() => useEdgeSwipe(open));
    phone(false);
    touch("touchStart", 8, 300);
    touch("touchMove", 100, 300);
    phone();
    document.body.innerHTML = '<div role="dialog"></div>';
    touch("touchStart", 8, 300);
    touch("touchMove", 100, 300);
    document.body.innerHTML = "";
    expect(open).not.toHaveBeenCalled();
  });
});

describe("install offer", () => {
  function standalone(on: boolean) {
    vi.stubGlobal("matchMedia", (query: string) => ({ matches: on && query.includes("standalone"), media: query }));
  }

  it("offers the iOS hint once in Safari and never in the installed app", () => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)");
    standalone(false);
    expect(currentOffer()).toEqual({ kind: "ios" });
    localStorage.setItem("openside.install.dismissed", String(Date.now()));
    expect(currentOffer()).toBeNull();
    localStorage.clear();
    standalone(true);
    expect(currentOffer()).toBeNull();
  });

  it("offers the browser's own prompt after beforeinstallprompt, and re-offers 14 days after a dismissal", async () => {
    standalone(false);
    const prompt = vi.fn(async () => undefined);
    const event = Object.assign(new Event("beforeinstallprompt", { cancelable: true }), {
      prompt,
      userChoice: Promise.resolve({ outcome: "accepted" as const }),
    });
    window.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    const offer = currentOffer();
    expect(offer?.kind).toBe("android");

    localStorage.setItem("openside.install.dismissed", String(Date.now()));
    expect(currentOffer()).toBeNull();
    expect(currentOffer(Date.now() + 15 * 24 * 60 * 60 * 1000)?.kind).toBe("android");
    localStorage.clear();

    if (offer?.kind === "android") await offer.install();
    expect(prompt).toHaveBeenCalledTimes(1);
    expect(currentOffer()).toBeNull();
  });
});
