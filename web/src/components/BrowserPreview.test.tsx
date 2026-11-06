import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createClient } from "../api/client";
import type { BrowserState, Ticket } from "../api/types";
import { ClientContext } from "../context";
import { emptySessionView, type BrowserStateView, type SessionView, type TimelineItem } from "../state/session";
import { json } from "../test/fakeDaemon";
import { BrowserButton } from "./BrowserButton";
import { BrowserPreview, browserAnchor } from "./BrowserPreview";

class FakeSocket {
  static last: FakeSocket | null = null;
  closed = false;
  readyState: number = WebSocket.OPEN;
  sent: string[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  constructor(readonly url: string, readonly protocols: string[]) { FakeSocket.last = this; }
  send(data: string) { this.sent.push(data); }
  close() { this.closed = true; this.readyState = WebSocket.CLOSED; this.onclose?.(); }
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
const PERSON = { device_id: "d1", name: "Phone", kind: "person_device", scopes: ["read", "steer", "approve", "human_step"] };
let myDevice: object = PERSON;

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
      const path = new URL(String(input), "http://x").pathname;
      if (path === "/v1/devices/me") return json(200, myDevice);
      tabRequests.push(path);
      return tabsResponse();
    }) as typeof fetch,
    webSocket: (url, protocols) => new FakeSocket(url, protocols) as unknown as WebSocket,
  });
  const ui = (view: SessionView) => (
    <MemoryRouter>
      <ClientContext.Provider value={client}>
        <BrowserButton sessionId="s1" view={view} />
        <BrowserPreview sessionId="s1" view={view} onShowPending={onShowPending} onCancel={onCancel} />
      </ClientContext.Provider>
    </MemoryRouter>
  );
  const result = render(ui(initial));
  return { ...result, onShowPending, onCancel, update: (view: SessionView) => result.rerender(ui(view)) };
}

function open() {
  const socket = FakeSocket.last!;
  act(() => { socket.onopen?.(); socket.receive(FRAME); });
  return socket;
}

function taskState(patch: Partial<BrowserState> = {}): Record<string, BrowserStateView> {
  return {
    tab9: {
      tab_id: "tab9",
      task_id: "task1",
      title: "Flights",
      url: "https://air.test/",
      host: "air.test",
      page_kind: "other",
      phase: "reading",
      facts: {},
      seq: 3,
      trail: [],
      ...patch,
    } as BrowserStateView,
  };
}

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  sessionStorage.clear();
  myDevice = PERSON;
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
    // Idle, the footer names the live tab from the tab ledger, matched by the streamed tab id,
    // and the status adds the site.
    await waitFor(() => expect(card.querySelector(".browser-card-site")).toHaveTextContent("Checkout · Shop"));
    expect(within(card).getByRole("status")).toHaveTextContent("shop.test");
    expect(tabRequests[0]).toBe("/v1/browser/tabs");

    await user.click(within(card).getByRole("button", { name: "Minimize" }));
    expect(card).toHaveAttribute("data-minimized");
    expect(within(card).getByRole("button", { name: "Restore" })).toHaveAttribute("aria-expanded", "false");
    // The slim pill keeps a live thumbnail and names the page.
    expect(within(card).getByRole("status")).toHaveTextContent("Checkout · Shop");
    expect(within(card).getByAltText("Remote browser tab")).toBeInTheDocument();
    await user.click(within(card).getByRole("button", { name: "Restore" }));
    expect(card).not.toHaveAttribute("data-minimized");

    await user.click(within(card).getByRole("button", { name: "Expand Checkout · Shop" }));
    const sheet = screen.getByRole("dialog", { name: "Checkout · Shop" });
    expect(within(sheet).getByRole("textbox", { name: "Address" })).toHaveValue("https://shop.test/checkout");
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

    // Closing the idle card hides it, and the floating preview with it, until asked for again.
    await user.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("region", { name: "Live browser" })).toBeNull();
    expect(screen.queryByRole("complementary", { name: "Live browser" })).toBeNull();
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
      const sheet = screen.getByRole("dialog", { name: "shop.test" });
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

  it("keeps a Task-only session's PiP until Show browser, then minimises it to a pill and remembers the choice", async () => {
    const user = userEvent.setup();
    const { unmount } = setup(viewOf({ items: [], browserStates: taskState() }));
    // No inline card without the agent's own browser calls, and the Task unit already shows its
    // tab: nothing floats over the thread and no stream opens until the person asks.
    expect(screen.queryByRole("region", { name: "Live browser" })).toBeNull();
    expect(screen.queryByRole("complementary", { name: "Live browser" })).toBeNull();
    expect(FakeSocket.last).toBeNull();
    await user.click(screen.getByRole("button", { name: "Show browser" }));
    open();
    const pip = screen.getByRole("complementary", { name: "Live browser" });
    expect(within(pip).getByAltText("Remote browser tab")).toBeInTheDocument();

    await user.click(within(pip).getByRole("button", { name: "Minimize" }));
    expect(pip).toHaveAttribute("data-minimized");
    expect(pip.querySelector(".pip-thumb img")).toHaveAttribute("src", "data:image/jpeg;base64,/9j/first");
    expect(JSON.parse(sessionStorage.getItem("openside.browserDock.s1")!)).toMatchObject({ pip: "minimized" });
    unmount();

    // A reload of the same conversation comes back minimised; tapping the pill restores it.
    setup(viewOf({ items: [], browserStates: taskState() }));
    open();
    const again = screen.getByRole("complementary", { name: "Live browser" });
    expect(again).toHaveAttribute("data-minimized");
    await user.click(within(again).getByRole("button", { name: "Restore live browser" }));
    expect(again).not.toHaveAttribute("data-minimized");

    await user.click(within(again).getByRole("button", { name: "Expand" }));
    expect(await screen.findByRole("dialog", { name: "Checkout · Shop" })).toBeInTheDocument();
    expect(screen.queryByRole("complementary", { name: "Live browser" })).toBeNull();
  });

  it("stays closed after Close until Show browser in the header summons it", async () => {
    const user = userEvent.setup();
    const { update } = setup(viewOf({ items: [], browserStates: taskState() }));
    await user.click(screen.getByRole("button", { name: "Show browser" }));
    let socket = open();
    await user.click(within(screen.getByRole("complementary", { name: "Live browser" })).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("complementary", { name: "Live browser" })).toBeNull();
    expect(socket.closed).toBe(true);
    // Later browser states do not bring it back by themselves.
    update(viewOf({ items: [], browserStates: taskState({ phase: "acting" }) }));
    expect(screen.queryByRole("complementary", { name: "Live browser" })).toBeNull();

    await user.click(screen.getByRole("button", { name: "Show browser" }));
    socket = open();
    expect(screen.getByRole("complementary", { name: "Live browser" })).toBeInTheDocument();
    expect(socket.closed).toBe(false);
  });

  it("stands in for a scrolled-away card, but not over a request waiting for the person unless asked", async () => {
    const user = userEvent.setup();
    vi.stubGlobal(
      "IntersectionObserver",
      class {
        constructor(private readonly callback: IntersectionObserverCallback) {}
        observe() {
          this.callback([{ isIntersecting: false } as IntersectionObserverEntry], this as unknown as IntersectionObserver);
        }
        disconnect() {}
      },
    );
    try {
      const { update } = setup(viewOf({ status: "running", browserStates: taskState() }));
      open();
      expect(screen.getByRole("complementary", { name: "Live browser" })).toBeInTheDocument();
      const confirm = { k3: { id: "k3", status: "pending", kind: "confirm", summary: "Approve the order" } as Ticket };
      update(viewOf({ status: "running", browserStates: taskState(), tickets: confirm }));
      expect(screen.queryByRole("complementary", { name: "Live browser" })).toBeNull();
      await user.click(screen.getByRole("button", { name: "Show browser" }));
      expect(screen.getByRole("complementary", { name: "Live browser" })).toBeInTheDocument();
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("opens the sheet from Show browser on a phone", async () => {
    const user = userEvent.setup();
    vi.stubGlobal("matchMedia", (query: string) => ({ matches: query === "(max-width: 820px)" }) as MediaQueryList);
    try {
      setup(viewOf({ items: [], browserStates: taskState() }));
      await user.click(screen.getByRole("button", { name: "Show browser" }));
      open();
      expect(await screen.findByRole("dialog", { name: "Checkout · Shop" })).toBeInTheDocument();
    } finally {
      vi.unstubAllGlobals();
    }
  });

  describe("taking over from the sheet", () => {
    const takeover = (target: Ticket["target"]) => ({ k2: { id: "k2", status: "pending", kind: "takeover", summary: "Solve the puzzle", target } as Ticket });

    it("offers a person's device the pending takeover", async () => {
      const user = userEvent.setup();
      setup(viewOf({ tickets: takeover({ origin: "https://shop.test" } as Ticket["target"]) }));
      open();
      await user.click(screen.getByRole("button", { name: /^Expand / }));
      await user.click(await within(screen.getByRole("dialog")).findByRole("button", { name: "Take control" }));
      expect(screen.getByRole("dialog", { name: "Remote control" })).toHaveTextContent("Solve the puzzle");
    });

    it.each([
      ["an agent client", { ...PERSON, kind: "agent_client" }, "Only a device you paired as yourself can take control of the browser."],
      ["a device without human_step", { ...PERSON, scopes: ["read", "steer"] }, "This device wasn't given permission to take control of the browser."],
    ])("tells %s why it cannot take control", async (_, device, reason) => {
      const user = userEvent.setup();
      myDevice = device;
      setup(viewOf({ tickets: takeover({ origin: "https://shop.test" } as Ticket["target"]) }));
      open();
      await user.click(screen.getByRole("button", { name: /^Expand / }));
      const dialog = screen.getByRole("dialog");
      expect(await within(dialog).findByText(reason)).toBeInTheDocument();
      expect(within(dialog).queryByRole("button", { name: "Take control" })).toBeNull();
    });

    it("explains a request without a page to take over", async () => {
      const user = userEvent.setup();
      setup(viewOf({ tickets: takeover(undefined) }));
      open();
      await user.click(screen.getByRole("button", { name: /^Expand / }));
      const dialog = screen.getByRole("dialog");
      expect(within(dialog).getByText("This request doesn't say which page to take over.")).toBeInTheDocument();
      expect(within(dialog).queryByRole("button", { name: "Take control" })).toBeNull();
    });
  });

  describe("controlling a tab from the sheet", () => {
    async function openSheet(user: UserEvent) {
      await user.click(screen.getByRole("button", { name: /^Expand / }));
      return screen.getByRole("dialog");
    }
    /** The picture is drawn at half size: 720x450 CSS px from (10, 20). */
    function layOut(sheet: HTMLElement) {
      const screenArea = within(sheet).getByLabelText(/Remote browser screen/);
      vi.spyOn(screenArea.querySelector("img")!, "getBoundingClientRect").mockReturnValue({
        left: 10, top: 20, width: 720, height: 450, right: 730, bottom: 470, x: 10, y: 20, toJSON: () => ({}),
      });
      return screenArea;
    }
    const sent = (socket: FakeSocket) => socket.sent.map((data) => JSON.parse(data) as Record<string, unknown>);

    it("drives an idle tab: a tap opens control, the address bar navigates, and Hand back releases it", async () => {
      const user = userEvent.setup();
      setup();
      const preview = open();
      const sheet = await openSheet(user);
      const screenArea = layOut(sheet);
      fireEvent.pointerDown(screenArea, { clientX: 60, clientY: 70, button: 0 });
      fireEvent.pointerUp(screenArea, { clientX: 60, clientY: 70, button: 0 });
      const control = FakeSocket.last!;
      expect(control).not.toBe(preview);
      expect(control.url).toBe("ws://localhost:3000/v1/sessions/s1/browser/tabs/tab1/control");
      expect(control.protocols).toEqual(["openside.v1", "openside.bearer.device-token"]);
      expect(control.sent).toEqual([]);
      act(() => control.onopen?.());
      // Queued until the socket opened, in the tab's 1440x900 viewport.
      expect(sent(control)).toEqual([
        { type: "mouse", event: "down", x: 100, y: 100, button: "left", modifiers: 0 },
        { type: "mouse", event: "up", x: 100, y: 100, button: "left", modifiers: 0 },
      ]);
      act(() => {
        control.receive({ type: "frame", data: "/9j/control", width: 1440, height: 900 });
        control.receive({ type: "page", url: "https://shop.test/cart", title: "Cart", can_go_back: true, can_go_forward: false });
      });
      expect(within(sheet).getByAltText("Remote browser tab")).toHaveAttribute("src", "data:image/jpeg;base64,/9j/control");
      expect(within(sheet).getByRole("textbox", { name: "Address" })).toHaveValue("https://shop.test/cart");
      expect(within(sheet).getByText("You're in control. The agent waits until you hand the tab back.")).toBeInTheDocument();
      expect(within(sheet).getByRole("button", { name: "Forward" })).toBeDisabled();

      control.sent = [];
      await user.click(within(sheet).getByRole("button", { name: "Back" }));
      await user.click(within(sheet).getByRole("button", { name: "Reload" }));
      const address = within(sheet).getByRole("textbox", { name: "Address" });
      await user.clear(address);
      await user.type(address, "example.com/path{Enter}");
      await user.type(within(sheet).getByRole("textbox", { name: "Text to type" }), "héllo{Enter}");
      await user.click(within(sheet).getByRole("button", { name: "Press Enter" }));
      expect(sent(control)).toEqual([
        { type: "back" },
        { type: "reload" },
        { type: "navigate", url: "https://example.com/path" },
        { type: "text", text: "héllo" },
        { type: "key", event: "down", key: "Enter", code: "Enter", modifiers: 0 },
        { type: "key", event: "up", key: "Enter", code: "Enter", modifiers: 0 },
      ]);
      // Any scheme the person types is sent as typed; a bare host with a port still gets https.
      control.sent = [];
      for (const typedAddress of ["file:///etc/passwd", "about:blank", "data:text/html,hi", "javascript:void(0)", "localhost:3000/app"]) {
        await user.clear(address);
        await user.type(address, `${typedAddress}{Enter}`);
      }
      expect(sent(control)).toEqual([
        { type: "navigate", url: "file:///etc/passwd" },
        { type: "navigate", url: "about:blank" },
        { type: "navigate", url: "data:text/html,hi" },
        { type: "navigate", url: "javascript:void(0)" },
        { type: "navigate", url: "https://localhost:3000/app" },
      ]);
      expect(within(sheet).queryByRole("alert")).toBeNull();
      // The daemon's refusals are explained.
      act(() => control.receive({ type: "input_error", code: "invalid_url" }));
      expect(within(sheet).getByRole("alert")).toHaveTextContent("That address isn't valid.");
      act(() => control.receive({ type: "input_error", code: "policy_denied" }));
      await user.clear(address);
      await user.type(address, "https://ok.test{Enter}");
      act(() => control.receive({ type: "input_error", code: "policy_denied" }));
      expect(within(sheet).getByRole("alert")).toHaveTextContent("Your browsing policy blocks that site.");

      await user.click(within(sheet).getByRole("button", { name: "Hand back" }));
      expect(control.closed).toBe(true);
      expect(preview.closed).toBe(false);
      expect(within(sheet).getByAltText("Remote browser tab")).toHaveAttribute("src", "data:image/jpeg;base64,/9j/first");
    });

    it("sends nothing once the daemon ended control or the socket is no longer open", async () => {
      const user = userEvent.setup();
      setup();
      open();
      const sheet = await openSheet(user);
      const screenArea = layOut(sheet);
      const tap = () => {
        fireEvent.pointerDown(screenArea, { clientX: 60, clientY: 70, button: 0 });
        fireEvent.pointerUp(screenArea, { clientX: 60, clientY: 70, button: 0 });
      };
      tap();
      const control = FakeSocket.last!;
      act(() => control.onopen?.());
      control.sent = [];
      control.readyState = WebSocket.CLOSING;
      tap();
      expect(control.sent).toEqual([]);
      control.readyState = WebSocket.OPEN;
      act(() => control.receive({ type: "ended", reason: "idle" }));
      expect(control.closed).toBe(true);
      expect(within(sheet).getByText("Handed back to the agent after inactivity.")).toBeInTheDocument();
      // Input after the end asks for a new lease instead of writing to the finished one.
      tap();
      expect(control.sent).toEqual([]);
      expect(FakeSocket.last).not.toBe(control);
    });

    it("asks for Take control first while the agent runs, and scrolls with a finger drag", async () => {
      const user = userEvent.setup();
      class TouchPointer extends MouseEvent {
        readonly pointerType: string;
        readonly isPrimary: boolean;
        readonly pointerId: number;
        constructor(type: string, init: PointerEventInit = {}) {
          super(type, init);
          this.pointerType = init.pointerType ?? "";
          this.isPrimary = init.isPrimary ?? false;
          this.pointerId = init.pointerId ?? 1;
        }
      }
      vi.stubGlobal("PointerEvent", TouchPointer);
      try {
        setup(viewOf({ status: "running" }));
        const preview = open();
        const sheet = await openSheet(user);
        expect(within(sheet).getByRole("textbox", { name: "Address" })).toBeDisabled();
        expect(within(sheet).getByRole("button", { name: "Back" })).toBeDisabled();
        const inert = within(sheet).getByLabelText("Browser preview");
        fireEvent.pointerDown(inert, { clientX: 60, clientY: 70, button: 0 });
        fireEvent.pointerUp(inert, { clientX: 60, clientY: 70, button: 0 });
        expect(FakeSocket.last).toBe(preview);
        expect(within(sheet).queryByRole("button", { name: /^Close / })).toBeNull();

        await user.click(within(sheet).getByRole("button", { name: "Take control" }));
        const control = FakeSocket.last!;
        expect(control.url).toMatch(/\/browser\/tabs\/tab1\/control$/);
        act(() => {
          control.onopen?.();
          control.receive({ type: "frame", data: "/9j/control", width: 1440, height: 900 });
        });
        const screenArea = layOut(sheet);
        const touch = { pointerType: "touch", isPrimary: true, pointerId: 7 };
        fireEvent.pointerDown(screenArea, { ...touch, clientX: 60, clientY: 200 });
        fireEvent.pointerMove(screenArea, { ...touch, clientX: 60, clientY: 150 });
        fireEvent.pointerUp(screenArea, { ...touch, clientX: 60, clientY: 150 });
        fireEvent.pointerDown(screenArea, { ...touch, clientX: 60, clientY: 70 });
        fireEvent.pointerUp(screenArea, { ...touch, clientX: 62, clientY: 71 });
        expect(sent(control)).toEqual([
          { type: "wheel", x: 100, y: 360, delta_x: 0, delta_y: 150 },
          { type: "mouse", event: "down", x: 104, y: 102, button: "left", modifiers: 0 },
          { type: "mouse", event: "up", x: 104, y: 102, button: "left", modifiers: 0 },
        ]);
        act(() => control.receive({ type: "ended", reason: "idle" }));
        act(() => control.onclose?.());
        expect(within(sheet).getByText("Handed back to the agent after inactivity.")).toBeInTheDocument();
        expect(within(sheet).getByRole("button", { name: "Take control" })).toBeInTheDocument();
      } finally {
        vi.unstubAllGlobals();
      }
    });

    it("switches and closes tabs through the control of the tab picked", async () => {
      const user = userEvent.setup();
      setup();
      open();
      const sheet = await openSheet(user);
      const list = within(sheet).getByRole("region", { name: "Tabs in this conversation" });
      await waitFor(() => expect(within(list).getAllByRole("listitem")).toHaveLength(2));
      await user.click(within(list).getByRole("button", { name: "Show Setup guide" }));
      const second = FakeSocket.last!;
      expect(second.url).toMatch(/\/browser\/tabs\/tab2\/control$/);
      expect(within(list).getByRole("button", { name: "Show Setup guide" })).toHaveAttribute("aria-pressed", "true");
      expect(within(sheet).getByRole("heading", { name: "Setup guide" })).toBeInTheDocument();
      act(() => second.onopen?.());
      await user.click(within(list).getByRole("button", { name: "Close Setup guide" }));
      expect(sent(second)).toEqual([{ type: "close_tab" }]);

      // Closing another tab moves control there first.
      await user.click(within(list).getByRole("button", { name: "Close Checkout · Shop" }));
      expect(second.closed).toBe(true);
      const first = FakeSocket.last!;
      expect(first.url).toMatch(/\/browser\/tabs\/tab1\/control$/);
      act(() => first.onopen?.());
      expect(sent(first)).toEqual([{ type: "close_tab" }]);
    });

    it.each([
      ["an agent client", { ...PERSON, kind: "agent_client" }, "Only a device you paired as yourself can take control of the browser."],
      ["a device without steer", { ...PERSON, scopes: ["read", "human_step"] }, "This device wasn't given permission to take control of the browser."],
    ])("keeps the sheet read-only for %s", async (_, device, reason) => {
      const user = userEvent.setup();
      myDevice = device;
      setup();
      const preview = open();
      const sheet = await openSheet(user);
      expect(await within(sheet).findByText(reason)).toBeInTheDocument();
      expect(within(sheet).queryByRole("textbox", { name: "Address" })).toBeNull();
      expect(within(sheet).getByText("https://shop.test/checkout", { selector: ".browser-sheet-url" })).toBeInTheDocument();
      fireEvent.pointerDown(within(sheet).getByLabelText("Browser preview"), { clientX: 60, clientY: 70, button: 0 });
      expect(FakeSocket.last).toBe(preview);
    });
  });

  it("hides the header button for sessions without a browser state and for private ones", () => {
    const first = setup(viewOf());
    expect(screen.queryByRole("button", { name: "Show browser" })).toBeNull();
    first.unmount();
    setup(viewOf({ incognito: true, browserStates: taskState() }));
    expect(screen.queryByRole("button", { name: "Show browser" })).toBeNull();
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
