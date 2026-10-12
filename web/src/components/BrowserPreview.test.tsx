import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { createClient } from "../api/client";
import { ClientContext } from "../context";
import { json } from "../test/fakeDaemon";
import { BrowserPreview } from "./BrowserPreview";

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

function setup(privateSession = false) {
  FakeSocket.last = null;
  tabRequests = [];
  const client = createClient({
    token: () => "device-token",
    fetch: (async (input: RequestInfo | URL) => {
      tabRequests.push(new URL(String(input), "http://x").pathname);
      return tabsResponse();
    }) as typeof fetch,
    webSocket: (url, protocols) => new FakeSocket(url, protocols) as unknown as WebSocket,
  });
  return render(<ClientContext.Provider value={client}><BrowserPreview sessionId="s1" privateSession={privateSession} /></ClientContext.Provider>);
}
const FRAME = { type: "frame", data: "/9j/first", width: 720, height: 450, tab_id: "tab1" };

describe("BrowserPreview", () => {
  it("toggles the authenticated PiP, updates images, and remains read-only expanded or minimized", async () => {
    const user = userEvent.setup();
    setup();
    expect(FakeSocket.last).toBeNull();
    await user.click(screen.getByRole("button", { name: "Browser preview" }));
    const socket = FakeSocket.last!;
    expect(socket.url).toBe("ws://localhost:3000/v1/sessions/s1/browser/preview");
    expect(socket.protocols).toEqual(["openside.v1", "openside.bearer.device-token"]);
    act(() => { socket.onopen?.(); socket.receive(FRAME); });
    const pip = screen.getByRole("complementary", { name: "Live browser preview" });
    expect(within(pip).getByAltText("Remote browser tab")).toHaveAttribute("src", "data:image/jpeg;base64,/9j/first");
    // The card is titled from the tab ledger, matched by the streamed tab id.
    await waitFor(() => expect(within(pip).getByText("Checkout · Shop")).toBeInTheDocument());
    expect(tabRequests[0]).toBe("/v1/browser/tabs");

    await user.click(within(pip).getByRole("button", { name: "Minimize" }));
    expect(screen.queryByAltText("Remote browser tab")).toBeNull();
    expect(within(pip).getByRole("button", { name: "Restore" })).toHaveAttribute("aria-expanded", "false");
    await user.click(within(pip).getByRole("button", { name: "Restore" }));
    expect(within(pip).getByAltText("Remote browser tab")).toBeInTheDocument();

    await user.click(within(pip).getByRole("button", { name: "Expand" }));
    const sheet = screen.getByRole("dialog", { name: "Browser" });
    expect(screen.queryByRole("complementary")).toBeNull();
    expect(within(sheet).getByRole("heading", { name: "Checkout · Shop" })).toBeInTheDocument();
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
    expect(image).toHaveAttribute("src", "data:image/jpeg;base64,/9j/second");
    await user.click(within(viewer).getByRole("button", { name: "Zoom in" }));
    expect(within(viewer).getByLabelText("Zoom level")).toHaveTextContent("150%");
    fireEvent.pointerDown(image); fireEvent.keyDown(image, { key: "a" }); fireEvent.wheel(image, { deltaY: 20 });
    expect(socket.sent).toEqual([]);
    await user.click(within(viewer).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Live browser preview" })).toBeNull());
    await user.click(within(screen.getByRole("dialog", { name: "Browser" })).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(screen.getByRole("complementary", { name: "Live browser preview" })).toBeInTheDocument();
    expect(socket.closed).toBe(false);
    await user.click(screen.getByRole("button", { name: "Browser preview" }));
    expect(socket.closed).toBe(true);
    expect(screen.queryByRole("complementary")).toBeNull();
  });

  it("says when the browser is disconnected and keeps the live picture", async () => {
    const user = userEvent.setup();
    tabsResponse = () => json(503, { code: "browser_disconnected", message: "browser disconnected" });
    try {
      setup();
      await user.click(screen.getByRole("button", { name: "Browser preview" }));
      const socket = FakeSocket.last!;
      act(() => { socket.onopen?.(); socket.receive(FRAME); });
      await user.click(screen.getByRole("button", { name: "Expand" }));
      const sheet = screen.getByRole("dialog", { name: "Browser" });
      await waitFor(() => expect(within(sheet).getByText("The browser is disconnected.")).toBeInTheDocument());
      expect(within(sheet).queryByRole("link")).toBeNull();
      expect(within(sheet).getByAltText("Remote browser tab")).toBeInTheDocument();
    } finally {
      tabsResponse = () => json(200, TABS);
    }
  });

  it("moves with the arrow keys and remembers the position", async () => {
    const user = userEvent.setup();
    localStorage.removeItem("openside.pipPosition");
    setup();
    await user.click(screen.getByRole("button", { name: "Browser preview" }));
    const handle = screen.getByRole("button", { name: "Open live browser preview" });
    handle.focus();
    await user.keyboard("{ArrowLeft}{ArrowUp}");
    expect(JSON.parse(localStorage.getItem("openside.pipPosition")!)).toEqual({ x: -20, y: -20 });
    // The bottom-right anchor is the limit: the card never moves past it.
    await user.keyboard("{ArrowRight}{ArrowRight}");
    expect(JSON.parse(localStorage.getItem("openside.pipPosition")!)).toEqual({ x: 0, y: -20 });
    localStorage.removeItem("openside.pipPosition");
  });

  it("clears stale pixels for takeover, secret withholding, no tab and terminal messages", async () => {
    const user = userEvent.setup(); setup();
    await user.click(screen.getByRole("button", { name: "Browser preview" }));
    const socket = FakeSocket.last!;
    act(() => { socket.onopen?.(); socket.receive(FRAME); });
    act(() => socket.receive({ type: "paused", reason: "takeover", tab_id: "tab1" }));
    expect(screen.queryByAltText("Remote browser tab")).toBeNull();
    expect(screen.getByRole("status")).toHaveTextContent("paused while a person controls");
    act(() => socket.receive(FRAME));
    expect(screen.getByAltText("Remote browser tab")).toBeInTheDocument();
    act(() => socket.receive({ type: "withheld", code: "screenshot_unavailable_after_secret_fill", tab_id: "tab1" }));
    expect(screen.queryByAltText("Remote browser tab")).toBeNull();
    expect(screen.getByRole("status")).toHaveTextContent("held a secret");
    act(() => socket.receive({ type: "no_tab" }));
    expect(screen.getByRole("status")).toHaveTextContent("No browser tab");
    act(() => socket.receive({ type: "ended", reason: "revoked" }));
    expect(screen.getByRole("status")).toHaveTextContent("access was revoked");
    expect(socket.closed).toBe(true);
    act(() => socket.receive(FRAME));
    expect(screen.queryByAltText("Remote browser tab")).toBeNull();
  });

  it("hides the toggle for private sessions and closes the socket on unmount", async () => {
    const user = userEvent.setup();
    const privateView = setup(true);
    expect(screen.queryByRole("button", { name: "Browser preview" })).toBeNull();
    expect(FakeSocket.last).toBeNull(); privateView.unmount();
    const view = setup(); await user.click(screen.getByRole("button", { name: "Browser preview" }));
    const socket = FakeSocket.last!; view.unmount(); expect(socket.closed).toBe(true);
  });

  it("explains unavailable streams without keeping stale pixels", async () => {
    const user = userEvent.setup(); setup(); await user.click(screen.getByRole("button", { name: "Browser preview" }));
    const socket = FakeSocket.last!;
    act(() => { socket.onopen?.(); socket.receive(FRAME); socket.onclose?.(); });
    expect(screen.queryByAltText("Remote browser tab")).toBeNull();
    expect(screen.getByRole("status")).toHaveTextContent("browse or steer device token");
  });
});
