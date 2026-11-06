import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useRef } from "react";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createClient } from "../api/client";
import { ClientContext } from "../context";
import { AgentTabsContext, AgentTabsSettledContext, type OwnedTab } from "../hooks/useAgentTabs";
import { emptySessionView, type BrowserStateView, type SessionView } from "../state/session";
import { json } from "../test/fakeDaemon";
import { AgentTabsPanel, AgentTabsToggle } from "./AgentTabsPanel";
import { BrowserPreview } from "./BrowserPreview";
import { AGENT_TABS_MIN_WIDTH } from "./agentTabsStore";
import { useBrowserDock } from "./browserDock";

class FakeSocket {
  static last: FakeSocket | null = null;
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  constructor(readonly url: string) {
    FakeSocket.last = this;
  }
  close() {
    this.onclose?.();
  }
  receive(data: unknown) {
    this.onmessage?.({ data: JSON.stringify(data) });
  }
}

let width = AGENT_TABS_MIN_WIDTH;
const observers = new Set<() => void>();
class FakeResizeObserver {
  constructor(private readonly callback: () => void) {}
  observe() {
    observers.add(this.callback);
    this.callback();
  }
  disconnect() {
    observers.delete(this.callback);
  }
}

const tab = (tab_id: string, title: string, owner = "s1"): OwnedTab => ({ tab_id, title, url: `https://${tab_id}.test/page`, opened_by: "agent", owner_session_id: owner });
const SHOP = tab("shop", "Checkout · Shop");
const DOCS = tab("docs", "Setup guide");
const OTHER = tab("other", "Another conversation", "s2");

const thumbnailRequests: string[] = [];
const client = createClient({
  token: () => "device-token",
  fetch: (async (input: RequestInfo | URL) => {
    const path = new URL(String(input), "http://x").pathname;
    if (path.includes("/browser/thumbnails/")) {
      thumbnailRequests.push(path);
      return new Response(new Blob(["jpeg"], { type: "image/jpeg" }), { status: 200, headers: { "Content-Type": "image/jpeg" } });
    }
    return json(404, { error: { code: "not_found", message: "none" } });
  }) as typeof fetch,
  webSocket: (url) => new FakeSocket(url) as unknown as WebSocket,
});

const stateOf = (tab_id: string, seq: number, thumbnail?: string): BrowserStateView =>
  ({ tab_id, title: tab_id, url: `https://${tab_id}.test/page`, host: `${tab_id}.test`, page_kind: "other", phase: "reading", facts: {}, thumbnail, seq, trail: [] }) as BrowserStateView;
const opening = (id: string, tab_id: string) => ({ type: "tool" as const, id, name: "browser_open_tab", args: {}, updates: [], result: { tab_id }, done: true });

function DockProbe() {
  const dock = useBrowserDock("s1");
  return <output data-testid="dock">{dock.sheet ? `sheet:${dock.sheetTab ?? "live"}` : "closed"}</output>;
}

function Screen({ tabs, view, settled = true, preview = false }: { tabs: OwnedTab[]; view: SessionView; settled?: boolean; preview?: boolean }) {
  const main = useRef<HTMLElement>(null);
  return (
    <MemoryRouter>
      <ClientContext.Provider value={client}>
        <AgentTabsContext.Provider value={tabs}>
          <AgentTabsSettledContext.Provider value={settled}>
          <main ref={main}>
            <AgentTabsToggle sessionId="s1" view={view} />
            {preview ? <BrowserPreview sessionId="s1" view={view} onShowPending={() => {}} onCancel={() => {}} /> : null}
            <AgentTabsPanel sessionId="s1" view={view} container={main} />
            <DockProbe />
          </main>
          </AgentTabsSettledContext.Provider>
        </AgentTabsContext.Provider>
      </ClientContext.Provider>
    </MemoryRouter>
  );
}

/** A conversation whose log has loaded: the panel ranks and prunes only then. */
const LOADED: SessionView = { ...emptySessionView, lastSeq: 1 };

function setup(tabs: OwnedTab[], view: SessionView = LOADED) {
  const result = render(<Screen tabs={tabs} view={view} />);
  return {
    ...result,
    update: (next: OwnedTab[], nextView: SessionView = view, settled = true) => result.rerender(<Screen tabs={next} view={nextView} settled={settled} />),
  };
}

const cards = () => within(screen.getByRole("complementary", { name: "Agent tabs" })).getAllByRole("article");
const titles = () => cards().map((card) => card.querySelector(".agent-tab-title")?.textContent);

beforeEach(() => {
  width = AGENT_TABS_MIN_WIDTH;
  FakeSocket.last = null;
  vi.stubGlobal("ResizeObserver", FakeResizeObserver);
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(() => ({ width }) as DOMRect);
  vi.stubGlobal("URL", Object.assign(URL, { createObjectURL: () => "blob:thumb", revokeObjectURL: () => {} }));
  thumbnailRequests.length = 0;
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  observers.clear();
  sessionStorage.clear();
});

describe("AgentTabsPanel", () => {
  it("appears by itself when the agent opens a tab, stacks newer tabs first and keeps other conversations out", () => {
    const { update } = setup([]);
    expect(screen.queryByRole("complementary", { name: "Agent tabs" })).toBeNull();
    expect(screen.queryByRole("button", { name: /agent tabs/ })).toBeNull();

    update([SHOP, OTHER]);
    expect(titles()).toEqual(["Checkout · Shop"]);
    // The daemon lists tabs by target id, not age: a later tab still goes on top.
    update([DOCS, SHOP, OTHER]);
    expect(titles()).toEqual(["Setup guide", "Checkout · Shop"]);
    expect(screen.getByRole("button", { name: "Hide agent tabs" })).toHaveAttribute("aria-pressed", "true");

    // A tab gone from the authoritative list loses its card.
    update([DOCS]);
    expect(titles()).toEqual(["Setup guide"]);
  });

  it("shows the live frame on the agent's tab and a blank card under the title band elsewhere; a card opens the sheet on its tab", async () => {
    const user = userEvent.setup();
    setup([SHOP, DOCS]);
    const socket = FakeSocket.last!;
    expect(socket.url).toContain("/v1/sessions/s1/browser/preview");
    act(() => {
      socket.onopen?.();
      socket.receive({ type: "frame", data: "/9j/frame", width: 720, height: 450, tab_id: "shop" });
    });
    const [docs, shop] = cards();
    // The picture fills the card and the title band sits over it, not above it.
    expect(within(shop!).getByRole("button", { name: "Open Checkout · Shop" })).toContainElement(within(shop!).getByRole("img", { name: "Remote browser tab" }));
    expect(within(shop!).getByRole("img", { name: "Remote browser tab" })).toHaveAttribute("src", "data:image/jpeg;base64,/9j/frame");
    expect(shop!.querySelector(".agent-tab-bar")).toHaveTextContent("Checkout · Shop");
    expect(shop).not.toHaveAttribute("data-empty");
    // No picture of its own yet: the same card with a blank body and only the title band.
    expect(within(docs!).queryByRole("img", { name: "Remote browser tab" })).toBeNull();
    expect(docs).toHaveAttribute("data-empty");
    expect(docs!.querySelector(".agent-tab-screen")).toBeEmptyDOMElement();
    expect(docs).toHaveTextContent(/^Setup guide$/);

    await user.click(screen.getByRole("button", { name: "Open Setup guide" }));
    expect(screen.getByTestId("dock")).toHaveTextContent("sheet:docs");
  });

  it("uses the conversation's page favicon when it has one", () => {
    const state = { tab_id: "shop", title: "Checkout", url: SHOP.url, host: "shop.test", page_kind: "other", phase: "reading", facts: {}, favicon: "data:image/png;base64,iVBORw0KGgo=", seq: 4, trail: [] } as BrowserStateView;
    const { container } = setup([SHOP], { ...LOADED, browserStates: { shop: state } });
    expect(container.querySelector(".agent-tab-favicon")).toHaveAttribute("src", "data:image/png;base64,iVBORw0KGgo=");
  });

  it("shows a tab's newest picture from the log, even when its latest state carries none", async () => {
    const view = { ...LOADED, browserStates: { "front:docs": stateOf("docs", 7), "task1:docs": stateOf("docs", 5, "thumb-5") } };
    setup([DOCS], view);
    await waitFor(() => expect(cards()[0]!.querySelector(".agent-tab-screen img")).toHaveAttribute("src", "blob:thumb"));
    expect(thumbnailRequests).toEqual(["/v1/sessions/s1/browser/thumbnails/thumb-5"]);
    expect(cards()[0]).not.toHaveAttribute("data-empty");
  });

  it("ranks tabs that arrive before the log only once the log has loaded", () => {
    const { update } = setup([SHOP, DOCS], emptySessionView);
    update([SHOP, DOCS], { ...LOADED, items: [opening("t1", "docs"), opening("t2", "shop")] });
    expect(titles()).toEqual(["Checkout · Shop", "Setup guide"]);
  });

  it("orders tabs already open at load by when the log opened them, unknown ones as older with the last listed newest", () => {
    const NEWS = tab("news", "Headlines");
    const MAIL = tab("mail", "Inbox");
    const view = { ...LOADED, items: [opening("t1", "news"), opening("t2", "shop")] };
    const { update } = setup([SHOP, DOCS, NEWS, MAIL], view);
    expect(titles()).toEqual(["Checkout · Shop", "Headlines", "Inbox", "Setup guide"]);
    // A tab that appears later goes on top, whatever the log or list order says.
    update([tab("aaa", "First by id"), SHOP, DOCS, NEWS, MAIL], { ...view, items: [opening("t0", "aaa"), ...view.items] });
    expect(titles()[0]).toBe("First by id");
  });

  it("keeps hidden cards and a hidden column for the conversation across a remount, and prunes cards of closed tabs", async () => {
    const user = userEvent.setup();
    const first = setup([SHOP, DOCS]);
    await user.click(screen.getByRole("button", { name: "Hide Setup guide" }));
    first.unmount();

    // A remount (navigation back, or a reload) reads the choices from sessionStorage.
    const second = setup([SHOP, DOCS]);
    expect(titles()).toEqual(["Checkout · Shop"]);
    await user.click(screen.getByRole("button", { name: "Hide agent tabs" }));
    second.unmount();

    const third = setup([SHOP, DOCS]);
    expect(screen.queryByRole("complementary", { name: "Agent tabs" })).toBeNull();
    expect(screen.getByRole("button", { name: "Show agent tabs" })).toHaveAttribute("aria-pressed", "false");

    // Docs closes: its hidden card is forgotten with it.
    third.update([SHOP]);
    expect(JSON.parse(sessionStorage.getItem("openside.agentTabs.s1")!)).toEqual({ order: ["shop"], dismissed: [], hidden: true });
  });

  it("forgets everything when the last tab closes, but not on a failed poll or before the first answer", async () => {
    const user = userEvent.setup();
    const stored = () => JSON.parse(sessionStorage.getItem("openside.agentTabs.s1")!);
    const { update } = setup([SHOP, DOCS]);
    await user.click(screen.getByRole("button", { name: "Hide Setup guide" }));
    expect(stored()).toEqual({ order: ["shop", "docs"], dismissed: ["docs"], hidden: false });

    // A failed poll (or none yet) also leaves an empty list; it proves nothing.
    update([], LOADED, false);
    expect(stored()).toEqual({ order: ["shop", "docs"], dismissed: ["docs"], hidden: false });

    // The daemon answered: no tabs are open.
    update([], LOADED, true);
    expect(stored()).toEqual({ order: [], dismissed: [], hidden: false });
    expect(screen.queryByRole("complementary", { name: "Agent tabs" })).toBeNull();
  });

  it("hides a card without closing the tab, the header toggle hides and restores the column, and a new tab brings it back", async () => {
    const user = userEvent.setup();
    const { update } = setup([SHOP, DOCS]);
    await user.click(screen.getByRole("button", { name: "Hide Setup guide" }));
    expect(titles()).toEqual(["Checkout · Shop"]);

    await user.click(screen.getByRole("button", { name: "Hide agent tabs" }));
    expect(screen.queryByRole("complementary", { name: "Agent tabs" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Show agent tabs" }));
    expect(titles()).toEqual(["Setup guide", "Checkout · Shop"]);

    await user.click(screen.getByRole("button", { name: "Hide agent tabs" }));
    update([SHOP, DOCS, tab("news", "Headlines")]);
    expect(titles()[0]).toBe("Headlines");
  });

  it("gives way where the conversation is too narrow, and never shows in a private conversation", () => {
    width = AGENT_TABS_MIN_WIDTH - 1;
    const { update } = setup([SHOP]);
    expect(screen.queryByRole("complementary", { name: "Agent tabs" })).toBeNull();
    expect(screen.queryByRole("button", { name: /agent tabs/ })).toBeNull();
    expect(FakeSocket.last).toBeNull();

    width = AGENT_TABS_MIN_WIDTH;
    act(() => observers.forEach((callback) => callback()));
    expect(titles()).toEqual(["Checkout · Shop"]);

    update([SHOP], { ...LOADED, incognito: true });
    expect(screen.queryByRole("complementary", { name: "Agent tabs" })).toBeNull();
    expect(screen.queryByRole("button", { name: /agent tabs/ })).toBeNull();
  });

  it("takes over from the conversation's inline browser card while the column is shown, and gives it back where the column does not fit", () => {
    width = AGENT_TABS_MIN_WIDTH - 1;
    render(<Screen tabs={[SHOP]} view={{ ...LOADED, items: [opening("t1", "shop")] }} preview />);
    expect(screen.queryByRole("complementary", { name: "Agent tabs" })).toBeNull();
    expect(screen.getByRole("region", { name: "Live browser" })).toBeInTheDocument();

    width = AGENT_TABS_MIN_WIDTH;
    act(() => observers.forEach((callback) => callback()));
    expect(titles()).toEqual(["Checkout · Shop"]);
    expect(screen.queryByRole("region", { name: "Live browser" })).toBeNull();

    width = AGENT_TABS_MIN_WIDTH - 1;
    act(() => observers.forEach((callback) => callback()));
    expect(screen.queryByRole("complementary", { name: "Agent tabs" })).toBeNull();
    expect(screen.getByRole("region", { name: "Live browser" })).toBeInTheDocument();
  });
});
