import { act, renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { fakeClient, json } from "../test/fakeDaemon";
import { bookmarkableUrl, usePersonBrowserState } from "./usePersonBrowser";

describe("bookmarkableUrl", () => {
  it("keeps web addresses and refuses everything else", () => {
    expect(bookmarkableUrl(" https://docs.example/a ")).toBe("https://docs.example/a");
    expect(bookmarkableUrl("http://docs.example")).toBe("http://docs.example/");
    for (const url of ["javascript:alert(1)", "JAVASCRIPT:alert(1)", "data:text/html,x", "file:///etc/passwd", "blob:https://a.example/1", "chrome://settings", "https://user:pw@a.example/", "https://user@a.example/", "not a url", ""]) {
      expect(bookmarkableUrl(url), url).toBeNull();
    }
  });
});

describe("usePersonBrowserState bookmarks", () => {
  it("reads one tree at a time and runs a refresh asked for meanwhile once afterwards", async () => {
    const pending: ((response: Response) => void)[] = [];
    const tree = { connected: true, roots: [{ id: "1", parent_id: null, title: "Bar", url: null, children: [] }] };
    const { client, calls } = fakeClient((call) => {
      if (call.path !== "/v1/browser/person/bookmarks") return json(200, { connected: false, focused_window_id: null, tabs: [], groups: [] });
      const { promise, resolve } = Promise.withResolvers<Response>();
      pending.push(resolve);
      return promise;
    });
    const { result, unmount } = renderHook(() => usePersonBrowserState(client));
    const reads = () => calls.filter((call) => call.path === "/v1/browser/person/bookmarks").length;
    await waitFor(() => expect(reads()).toBe(1));
    act(() => {
      result.current.refreshBookmarks();
      result.current.refreshBookmarks();
      window.dispatchEvent(new Event("focus"));
    });
    expect(reads()).toBe(1);
    await act(async () => pending.shift()?.(json(200, tree)));
    await waitFor(() => expect(reads()).toBe(2));
    await act(async () => pending.shift()?.(json(200, tree)));
    expect(result.current.bookmarkBar).toBe("1");
    unmount();
    act(() => window.dispatchEvent(new Event("focus")));
    expect(reads()).toBe(2);
  });
});
