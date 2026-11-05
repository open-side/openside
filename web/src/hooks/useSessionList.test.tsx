import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError, type Client } from "../api/client";
import type { SessionList } from "../api/types";
import { useSessionList } from "./useSessionList";

interface Pending {
  q: string | undefined;
  signal: AbortSignal | undefined;
  resolve: (list: SessionList) => void;
  reject: (error: unknown) => void;
}

/** A client whose searches stay open until the test answers them. */
function client() {
  const pending: Pending[] = [];
  const listSessions = vi.fn((query: { q?: string } = {}, signal?: AbortSignal) => {
    const { promise, resolve, reject } = Promise.withResolvers<SessionList>();
    pending.push({ q: query.q, signal, resolve, reject });
    signal?.addEventListener("abort", () => reject(new DOMException("Aborted", "AbortError")));
    return promise;
  });
  return { client: { listSessions } as unknown as Client, pending };
}

const list = (title: string): SessionList => ({ sessions: [{ id: title, title } as SessionList["sessions"][number]] }) as SessionList;

afterEach(() => vi.useRealTimers());

describe("useSessionList", () => {
  it("aborts the request for a term already typed past, and on unmount", async () => {
    vi.useFakeTimers();
    const { client: fake, pending } = client();
    const { result, unmount } = renderHook(() => useSessionList(fake));
    act(() => result.current.setQuery("vi"));
    await act(async () => vi.advanceTimersByTime(250));
    const first = pending.find((request) => request.q === "vi");
    expect(first?.signal?.aborted).toBe(false);

    act(() => result.current.setQuery("visa"));
    expect(first?.signal?.aborted).toBe(true);
    await act(async () => vi.advanceTimersByTime(250));
    const second = pending.find((request) => request.q === "visa");
    await act(async () => second?.resolve(list("Visa form")));
    expect(result.current.sessions?.[0]?.title).toBe("Visa form");
    expect(result.current.answered).toBe("visa");

    act(() => result.current.refresh());
    const third = pending.at(-1);
    unmount();
    expect(third?.signal?.aborted).toBe(true);
  });

  it("drops the previous term's error as soon as the query changes", async () => {
    const { client: fake, pending } = client();
    const { result } = renderHook(() => useSessionList(fake));
    act(() => result.current.setQuery("zebra"));
    await waitFor(() => expect(pending.some((request) => request.q === "zebra")).toBe(true));
    await act(async () => pending.find((request) => request.q === "zebra")?.reject(new ApiError(0, "network_error")));
    expect(result.current.offline).toBe(true);
    expect(result.current.error).not.toBeNull();

    act(() => result.current.setQuery("zebras"));
    expect(result.current.error).toBeNull();
    expect(result.current.offline).toBe(false);
    expect(result.current.answered).not.toBe("zebras");
  });
});
