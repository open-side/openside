import { act, render, renderHook, screen } from "@testing-library/react";
import { createElement } from "react";
import { MemoryRouter, useLocation } from "react-router";
import { afterEach, describe, expect, it } from "vitest";
import { createClient } from "../api/client";
import { ClientContext } from "../context";
import { emptySessionView } from "../state/session";
import { json } from "../test/fakeDaemon";
import { useBrowserDock } from "./browserDock";
import { BrowserPreview } from "./BrowserPreview";

afterEach(() => sessionStorage.clear());

describe("useBrowserDock", () => {
  it("starts with the PiP open and the sheet shut, and keeps each conversation's PiP in sessionStorage", () => {
    const one = renderHook(() => useBrowserDock("s1"));
    expect(one.result.current).toMatchObject({ pip: "open", sheet: false });

    act(() => one.result.current.setPip("minimized"));
    act(() => one.result.current.setSheet(true));
    expect(one.result.current).toMatchObject({ pip: "minimized", sheet: true });
    expect(JSON.parse(sessionStorage.getItem("openside.browserDock.s1")!)).toEqual({ pip: "minimized" });
    // Another conversation keeps its own dock.
    expect(renderHook(() => useBrowserDock("s2")).result.current).toMatchObject({ pip: "open", sheet: false });
    // Coming back to the conversation (or reloading it) restores the PiP; the sheet closed with it.
    one.unmount();
    expect(renderHook(() => useBrowserDock("s1")).result.current).toMatchObject({ pip: "minimized", sheet: false });
  });

  it("ignores a sheet flag stored by an earlier version", () => {
    sessionStorage.setItem("openside.browserDock.s4", JSON.stringify({ pip: "closed", sheet: true }));
    expect(renderHook(() => useBrowserDock("s4")).result.current).toMatchObject({ pip: "closed", sheet: false });
  });

  it("tells a chosen PiP from the default one, even when the choice is the default value", () => {
    const dock = renderHook(() => useBrowserDock("s5"));
    expect(dock.result.current).toMatchObject({ pip: "open", summoned: false });
    act(() => dock.result.current.setPip("open"));
    expect(dock.result.current).toMatchObject({ pip: "open", summoned: true });
    dock.unmount();
    expect(renderHook(() => useBrowserDock("s5")).result.current).toMatchObject({ pip: "open", summoned: true });
  });

  it("keeps separately mounted users of one conversation in step", () => {
    const header = renderHook(() => useBrowserDock("s1"));
    const card = renderHook(() => useBrowserDock("s1"));
    act(() => card.result.current.setPip("closed"));
    expect(header.result.current.pip).toBe("closed");
    act(() => header.result.current.setPip("open"));
    expect(card.result.current.pip).toBe("open");
  });

  it("falls back to the default for a corrupt or foreign entry", () => {
    sessionStorage.setItem("openside.browserDock.s1", "{not json");
    expect(renderHook(() => useBrowserDock("s1")).result.current).toMatchObject({ pip: "open", sheet: false });
    sessionStorage.setItem("openside.browserDock.s3", JSON.stringify({ pip: "huge", sheet: "yes" }));
    expect(renderHook(() => useBrowserDock("s3")).result.current).toMatchObject({ pip: "open", sheet: false });
  });

  it("closes the sheet once nothing shows the conversation, and keeps the PiP choice", () => {
    const header = renderHook(() => useBrowserDock("gone"));
    const card = renderHook(() => useBrowserDock("gone"));
    act(() => card.result.current.setPip("minimized"));
    act(() => card.result.current.setSheet(true));
    // The header still shows the conversation, so the sheet stays open for it.
    card.unmount();
    expect(header.result.current).toMatchObject({ pip: "minimized", sheet: true });
    // Leaving the conversation closes the sheet; coming back restores only the PiP.
    header.unmount();
    expect(renderHook(() => useBrowserDock("gone")).result.current).toMatchObject({ pip: "minimized", sheet: false });
  });
});

describe("?browser=open", () => {
  let search = "";
  function Location() {
    search = useLocation().search;
    return null;
  }
  function renderLinked(patch: Partial<typeof emptySessionView>) {
    const client = createClient({
      token: () => "t",
      fetch: (async () => json(200, { tabs: [] })) as typeof fetch,
      webSocket: () => ({ close() {} }) as unknown as WebSocket,
    });
    const view = {
      ...emptySessionView,
      lastSeq: 4,
      items: [{ type: "tool" as const, id: "t1", name: "browser_open_tab", args: { url: "https://shop.test/" }, updates: [], done: true }],
      ...patch,
    };
    return render(
      createElement(
        MemoryRouter,
        { initialEntries: ["/sessions/s1?browser=open&tab=x"] },
        createElement(
          ClientContext.Provider,
          { value: client },
          createElement(BrowserPreview, { sessionId: "s1", view, onShowPending: () => {}, onCancel: () => {} }),
          createElement(Location),
        ),
      ),
    );
  }

  it("opens the browser sheet once and drops the parameter", async () => {
    renderLinked({});
    expect(await screen.findByRole("dialog", { name: "shop.test" })).toBeInTheDocument();
    expect(search).toBe("?tab=x");
    expect(renderHook(() => useBrowserDock("s1")).result.current.sheet).toBe(true);
  });

  it("leaves a private conversation's dock alone, and waits for the conversation to load", () => {
    renderLinked({ incognito: true });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(renderHook(() => useBrowserDock("s1")).result.current.sheet).toBe(false);
    // Before its first event a conversation's privacy is unknown, so the link waits.
    renderLinked({ lastSeq: 0 });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(renderHook(() => useBrowserDock("s1")).result.current.sheet).toBe(false);
  });
});
