import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createClient } from "../api/client";
import type { Ticket } from "../api/types";
import { ClientContext } from "../context";
import { emptySessionView, type SessionView, type TimelineItem } from "../state/session";
import { json } from "../test/fakeDaemon";
import { BrowserPreview, browserAnchor } from "./BrowserPreview";

class FakeSocket {
  static last: FakeSocket | null = null;
  closed = false;
  sent: string[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  constructor(readonly url: string, readonly protocols: string[]) { FakeSocket.last = this; }
  send(data: string) { this.sent.push(data); }
  close() { this.closed = true; this.onclose?.(); }
  receive(data: unknown) { this.onmessage?.({ data: JSON.stringify(data) }); }
}
const TABS = {
  tabs: [
    { tab_id: "tab1", url: "https://shop.test/checkout", title: "Checkout · Shop", opened_by: "agent", owner_session_id: "s1" },
    { tab_id: "tab2", url: "https://docs.test/guide", title: "Setup guide", opened_by: "agent", owner_session_id: "s1" },
    { tab_id: "tab9", url: "https://other.test/", title: "Another conversation", opened_by: "agent", owner_session_id: "s2" },
  ],
};
let tabsResponse: () => Response = () => json(200, TABS);
let tabRequests: string[] = [];

const OPENED: TimelineItem = { type: "tool", id: "t1", name: "browser_open_tab", args: { url: "https://shop.test/checkout" }, updates: [], result: { tab_id: "tab1" }, done: true };
const FRAME = { type: "frame", data: "/9j/first", width: 720, height: 450, tab_id: "tab1" };

function viewOf(patch: Partial<SessionView> = {}): SessionView {
  return { ...emptySessionView, items: [OPENED], ...patch };
}

function setup(initial: SessionView = viewOf()) {
  FakeSocket.last = null;
  tabRequests = [];
  const onShowPending = vi.fn();
  const onCancel = vi.fn();
  const client = createClient({
    token: () => "device-token",
    fetch: (async (input: RequestInfo | URL) => {
      tabRequests.push(new URL(String(input), "http://x").pathname);
      return tabsResponse();
    }) as typeof fetch,
    webSocket: (url, protocols) => new FakeSocket(url, protocols) as unknown as WebSocket,
  });
  const ui = (view: SessionView) => (
    <ClientContext.Provider value={client}>
      <BrowserPreview sessionId="s1" view={view} onShowPending={onShowPending} onCancel={onCancel} />
    </ClientContext.Provider>
  );
  const result = render(ui(initial));
  return { ...result, onShowPending, onCancel, update: (view: SessionView) => result.rerender(ui(view)) };
}

function open() {
  const socket = FakeSocket.last!;
  act(() => { socket.onopen?.(); socket.receive(FRAME); });
  return socket;
}

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("BrowserPreview", () => {
  it("docks once the browser is used, titles the idle card from the ledger, minimizes, expands and stays read-only", async () => {
    const user = userEvent.setup();
    const { update } = setup(viewOf({ items: [] }));
    expect(FakeSocket.last).toBeNull();
    expect(screen.queryByRole("region", { name: "Live browser" })).toBeNull();
    update(viewOf());
    const socket = open();
    expect(socket.url).toBe("ws://localhost:3000/v1/sessions/s1/browser/preview");
    expect(socket.protocols).toEqual(["openside.v1", "openside.bearer.device-token"]);
    const card = screen.getByRole("region", { name: "Live browser" });
    expect(within(card).getByAltText("Remote browser tab")).toHaveAttribute("src", "data:image/jpeg;base64,/9j/first");
    // Idle, the pill names the live tab from the tab ledger, matched by the streamed tab id.
    await waitFor(() => expect(within(card).getByRole("status")).toHaveTextContent("Checkout · Shop"));
    expect(tabRequests[0]).toBe("/v1/browser/tabs");

    await user.click(within(card).getByRole("button", { name: "Minimize" }));
    expect(card).toHaveAttribute("data-minimized");
    expect(within(card).getByRole("button", { name: "Restore" })).toHaveAttribute("aria-expanded", "false");
    // The slim pill keeps a live thumbnail.
    expect(within(card).getByAltText("Remote browser tab")).toBeInTheDocument();
    await user.click(within(card).getByRole("button", { name: "Restore" }));
    expect(card).not.toHaveAttribute("data-minimized");

    await user.click(within(card).getByRole("button", { name: "Expand Checkout · Shop" }));
    const sheet = screen.getByRole("dialog", { name: "Checkout · Shop" });
    expect(within(sheet).getByText("https://shop.test/checkout", { selector: ".browser-sheet-url" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Live browser" })).toBeNull();
    expect(within(sheet).getByRole("link", { name: "Open in your browser" })).toHaveAttribute("href", "https://shop.test/checkout");
    const list = within(sheet).getByRole("region", { name: "Tabs in this conversation" });
    expect(within(list).getAllByRole("listitem").map((item) => item.querySelector("strong")?.textContent)).toEqual(["Checkout · Shop", "Setup guide"]);
    expect(within(list).getAllByRole("listitem")[0]).toHaveAttribute("aria-current", "true");

    // The agent moves to its other tab: the sheet follows the stream.
    act(() => socket.receive({ ...FRAME, data: "/9j/second", tab_id: "tab2" }));
    expect(within(sheet).getByRole("heading", { name: "Setup guide" })).toBeInTheDocument();
    expect(within(sheet).getByAltText("Remote browser tab")).toHaveAttribute("src", "data:image/jpeg;base64,/9j/second");

    await user.click(within(sheet).getByRole("button", { name: "Zoom browser preview" }));
    const viewer = screen.getByRole("dialog", { name: "Live browser preview" });
    const image = within(viewer).getByAltText("Remote browser tab");
    await user.click(within(viewer).getByRole("button", { name: "Zoom in" }));
    expect(within(viewer).getByLabelText("Zoom level")).toHaveTextContent("150%");
    fireEvent.pointerDown(image); fireEvent.keyDown(image, { key: "a" }); fireEvent.wheel(image, { deltaY: 20 });
    expect(socket.sent).toEqual([]);
    await user.click(within(viewer).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Live browser preview" })).toBeNull());
    await user.click(within(screen.getByRole("dialog", { name: "Setup guide" })).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(screen.getByRole("region", { name: "Live browser" })).toBeInTheDocument();
    expect(socket.closed).toBe(false);

    // Closing the idle card hides it until the agent touches the browser again.
    await user.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("region", { name: "Live browser" })).toBeNull();
    expect(socket.closed).toBe(true);
    update(viewOf({ items: [OPENED, { ...OPENED, id: "t2", name: "browser_snapshot", args: { tab_id: "tab1" } }] }));
    expect(screen.getByRole("region", { name: "Live browser" })).toBeInTheDocument();
  });

  it("narrates the run, ripples coordinate clicks, asks for the person and cancels through the caller", async () => {
    const user = userEvent.setup();
    const click: TimelineItem = { type: "tool", id: "c1", name: "browser_pointer", args: { tab_id: "tab1", action: "click", x: 720, y: 225 }, updates: [], done: false };
    const { update, onCancel, onShowPending } = setup(viewOf({ status: "running", items: [{ type: "user", id: "u1", text: "Buy it", author_kind: "person", from_agent: null }, OPENED, click] }));
    open();
    const card = screen.getByRole("region", { name: "Live browser" });
    expect(within(card).getByRole("status")).toHaveTextContent("Clicking on shop.test…");
    const ripple = card.querySelector<HTMLElement>(".browser-card-ripple")!;
    expect(ripple.style.left).toBe("50%");
    expect(ripple.style.top).toBe("25%");

    // Running: a labelled stop, never a bare X; Close only appears once the run is over.
    expect(within(card).queryByRole("button", { name: "Close" })).toBeNull();
    await user.click(within(card).getByRole("button", { name: "Stop" }));
    expect(onCancel).toHaveBeenCalledOnce();
    expect(card).toBeInTheDocument();

    const ticket = { id: "k1", status: "pending", kind: "secret_input" } as Ticket;
    update(viewOf({ status: "running", items: [OPENED, { ...click, done: true }], tickets: { k1: ticket } }));
    expect(within(card).queryByRole("status")).toBeNull();
    await user.click(within(card).getByRole("button", { name: "Action needed" }));
    expect(onShowPending).toHaveBeenCalledOnce();
  });

  it("announces a finished run with the reply's first sentence, then folds to the slim pill", () => {
    vi.useFakeTimers();
    const running = viewOf({ status: "running", items: [{ type: "user", id: "u1", text: "Sign in", author_kind: "person", from_agent: null }, OPENED] });
    const { update } = setup(running);
    open();
    act(() => vi.advanceTimersByTime(1500));
    update(viewOf({ items: [...running.items, { type: "assistant", id: "a1", text: "**Signed in** to Shop. You have 3 orders.", final: true }] }));
    const card = screen.getByRole("region", { name: "Live browser" });
    expect(within(card).getByRole("status")).toHaveTextContent("Signed in to Shop.");
    expect(card).toHaveAttribute("data-tone", "done");
    expect(card).not.toHaveAttribute("data-minimized");
    act(() => vi.advanceTimersByTime(4000));
    expect(card).toHaveAttribute("data-minimized");
    expect(within(card).getByRole("button", { name: "Close" })).toBeInTheDocument();
  });

  it("does not announce a run that only flashed by while history replayed", () => {
    const { update } = setup(viewOf({ status: "running" }));
    open();
    update(viewOf({ items: [OPENED, { type: "assistant", id: "a1", text: "Done.", final: true }] }));
    expect(screen.getByRole("region", { name: "Live browser" })).toHaveAttribute("data-tone", "idle");
  });

  it("says when the browser is disconnected and keeps the live picture", async () => {
    const user = userEvent.setup();
    tabsResponse = () => json(503, { code: "browser_disconnected", message: "browser disconnected" });
    try {
      setup();
      open();
      // Without the ledger the card falls back to the host the agent opened.
      await user.click(screen.getByRole("button", { name: "Expand shop.test" }));
      const sheet = screen.getByRole("dialog", { name: "Browser" });
      await waitFor(() => expect(within(sheet).getByText("The browser is disconnected.")).toBeInTheDocument());
      expect(within(sheet).queryByRole("link")).toBeNull();
      expect(within(sheet).getByAltText("Remote browser tab")).toBeInTheDocument();
    } finally {
      tabsResponse = () => json(200, TABS);
    }
  });

  it("clears stale pixels for takeover, secret withholding and terminal messages, and hides without a tab", () => {
    setup();
    const socket = open();
    act(() => socket.receive({ type: "paused", reason: "takeover", tab_id: "tab1" }));
    expect(screen.queryByAltText("Remote browser tab")).toBeNull();
    expect(screen.getByText(/paused while a person controls/)).toBeInTheDocument();
    act(() => socket.receive(FRAME));
    expect(screen.getByAltText("Remote browser tab")).toBeInTheDocument();
    act(() => socket.receive({ type: "withheld", code: "screenshot_unavailable_after_secret_fill", tab_id: "tab1" }));
    expect(screen.queryByAltText("Remote browser tab")).toBeNull();
    expect(screen.getByText(/held a secret/)).toBeInTheDocument();
    act(() => socket.receive({ type: "no_tab" }));
    expect(screen.queryByRole("region", { name: "Live browser" })).toBeNull();
    act(() => socket.receive({ type: "ended", reason: "revoked" }));
    expect(screen.getByText(/access was revoked/)).toBeInTheDocument();
    expect(socket.closed).toBe(true);
    act(() => socket.receive(FRAME));
    expect(screen.queryByAltText("Remote browser tab")).toBeNull();
  });

  it("never shows private sessions and closes the socket on unmount", () => {
    const privateView = setup(viewOf({ incognito: true }));
    expect(screen.queryByRole("region", { name: "Live browser" })).toBeNull();
    expect(FakeSocket.last).toBeNull(); privateView.unmount();
    const view = setup();
    const socket = FakeSocket.last!; view.unmount(); expect(socket.closed).toBe(true);
  });

  it("explains unavailable streams without keeping stale pixels", () => {
    setup();
    const socket = open();
    act(() => socket.onclose?.());
    expect(screen.queryByAltText("Remote browser tab")).toBeNull();
    expect(screen.getByText(/browse or steer device token/)).toBeInTheDocument();
  });
});

describe("browserAnchor", () => {
  const user = (id: string): Extract<TimelineItem, { type: "user" }> => ({ type: "user", id, text: id, author_kind: "person", from_agent: null });
  const browse = (id: string, name = "browser_navigate"): TimelineItem => ({ type: "tool", id, name, args: {}, updates: [], done: true });
  const read: TimelineItem = { type: "tool", id: "r", name: "read", args: {}, updates: [], done: true };

  it("is the person's message in front of the latest browser call, so a later browsing turn moves the card down", () => {
    expect(browserAnchor([user("m1"), browse("t1"), user("m2"), read, user("m3"), browse("t2")])).toBe("m3");
    expect(browserAnchor([user("m1"), browse("t1"), user("m2"), read])).toBe("m1");
  });

  it("skips internal messages such as Task reports", () => {
    expect(browserAnchor([user("m1"), browse("t1"), { ...user("plain-report"), author_kind: "internal" }])).toBe("m1");
    expect(browserAnchor([user("task-report-person"), browse("t1")])).toBe("task-report-person");
  });

  it("is null without a browsing call or a message to hang the card on", () => {
    expect(browserAnchor([user("m1"), read])).toBeNull();
    expect(browserAnchor([browse("t1")])).toBeNull();
  });
});
