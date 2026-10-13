import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { createClient } from "../api/client";
import type { Card } from "../api/types";
import { fixture } from "../cards/testing";
import { ClientContext } from "../context";
import { json } from "../test/fakeDaemon";
import { ConversationCard } from "./ConversationCard";

class FakeSocket {
  static last: FakeSocket | null = null;
  readyState = 0;
  sent: unknown[] = [];
  closed = false;
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  constructor(
    readonly url: string,
    readonly protocols: string[],
  ) {
    FakeSocket.last = this;
  }
  send(data: string) {
    this.sent.push(JSON.parse(data));
  }
  close() {
    this.closed = true;
    this.readyState = 3;
    this.onclose?.();
  }
  open() {
    this.readyState = 1;
    this.onopen?.();
  }
  receive(message: unknown) {
    this.onmessage?.({ data: JSON.stringify(message) });
  }
}

const card: Card = { ...fixture("ticket:take1"), id: "ticket:k9", ticket_id: "k9", title: "Solve the CAPTCHA" };

function setup(value: Card = card) {
  const calls: { method: string; path: string }[] = [];
  const client = createClient({
    token: () => "tok_123",
    fetch: (async (input: RequestInfo | URL, init?: RequestInit) => {
      calls.push({ method: init?.method ?? "GET", path: new URL(String(input), "http://x").pathname });
      return json(200, { id: "k9", status: "resolved" });
    }) as typeof fetch,
    webSocket: (url, protocols) => new FakeSocket(url, protocols) as unknown as WebSocket,
  });
  render(
    <ClientContext.Provider value={client}>
      <ConversationCard card={value} defaultOpen />
    </ClientContext.Provider>,
  );
  return { calls };
}

const FRAME = { type: "frame", data: "/9j/AAAA", width: 1440, height: 900 };
// The card has its own polite live region; the stream's state line belongs to the remote-control dialog.
const dialogStatus = () => document.querySelector<HTMLElement>(".takeover-state")!;

describe("TakeoverView (HS-6)", () => {
  it("authenticates by subprotocol, shows frames and forwards pointer and keyboard input", async () => {
    const user = userEvent.setup();
    setup();
    await user.click(screen.getByRole("button", { name: "Open remote screen" }));
    const socket = FakeSocket.last!;
    expect(socket.url).toBe("ws://localhost:3000/v1/tickets/k9/takeover");
    expect(socket.protocols).toEqual(["openside.v1", "openside.bearer.tok_123"]);
    expect(dialogStatus()).toHaveTextContent("Connecting");

    act(() => {
      socket.open();
      socket.receive(FRAME);
    });
    const image = screen.getByAltText("Remote browser tab");
    expect(image).toHaveAttribute("src", "data:image/jpeg;base64,/9j/AAAA");
    // The picture is shown at half size: 720x450 CSS px from (10, 20).
    vi.spyOn(image, "getBoundingClientRect").mockReturnValue({
      left: 10, top: 20, width: 720, height: 450, right: 730, bottom: 470, x: 10, y: 20, toJSON: () => ({}),
    });
    const screenArea = screen.getByLabelText(/Remote browser screen/);
    fireEvent.pointerDown(screenArea, { clientX: 60, clientY: 70, button: 0 });
    fireEvent.pointerUp(screenArea, { clientX: 60, clientY: 70, button: 0 });
    fireEvent.keyDown(screenArea, { key: "a", code: "KeyA" });
    fireEvent.keyUp(screenArea, { key: "a", code: "KeyA" });
    fireEvent.keyDown(screenArea, { key: "Enter", code: "Enter" });
    fireEvent.wheel(screenArea, { clientX: 730, clientY: 470, deltaY: 120 });
    expect(socket.sent).toEqual([
      { type: "mouse", event: "down", x: 100, y: 100, button: "left", modifiers: 0 },
      { type: "mouse", event: "up", x: 100, y: 100, button: "left", modifiers: 0 },
      { type: "text", text: "a" },
      { type: "key", event: "down", key: "Enter", code: "Enter", modifiers: 0 },
      { type: "wheel", x: 1439, y: 899, delta_x: 0, delta_y: 120 },
    ]);
  });

  it("splits a paste into text inputs of at most 4096 UTF-8 bytes", async () => {
    const user = userEvent.setup();
    setup();
    await user.click(screen.getByRole("button", { name: "Open remote screen" }));
    const socket = FakeSocket.last!;
    act(() => {
      socket.open();
      socket.receive(FRAME);
    });
    const pasted = `${"é".repeat(2049)}a`;
    fireEvent.paste(screen.getByLabelText(/Remote browser screen/), { clipboardData: { getData: () => pasted } });
    expect(socket.sent).toEqual([
      { type: "text", text: "é".repeat(2048) },
      { type: "text", text: "éa" },
    ]);
    fireEvent.paste(screen.getByLabelText(/Remote browser screen/), { clipboardData: { getData: () => "x".repeat(4096) } });
    expect(socket.sent).toHaveLength(3);
  });

  it("finishes with Done and reports how the stream ended", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await user.click(screen.getByRole("button", { name: "Open remote screen" }));
    const socket = FakeSocket.last!;
    act(() => socket.open());
    const dialog = screen.getByRole("dialog", { name: "Remote control" });
    await user.click(screen.getAllByRole("button", { name: "Done" }).find((b) => dialog.contains(b))!);
    expect(calls.filter((call) => call.method === "POST")).toEqual([{ method: "POST", path: "/v1/tickets/k9/done" }]);
    act(() => socket.receive({ type: "ended", reason: "done" }));
    expect(dialogStatus()).toHaveTextContent("Step completed.");
    expect(socket.closed).toBe(true);
  });

  it("explains withheld frames and refused input", async () => {
    const user = userEvent.setup();
    setup();
    await user.click(screen.getByRole("button", { name: "Open remote screen" }));
    const socket = FakeSocket.last!;
    act(() => {
      socket.open();
      socket.receive({ type: "withheld", code: "screenshot_unavailable_after_secret_fill" });
      socket.receive({ type: "input_error", code: "out_of_bounds" });
    });
    expect(dialogStatus()).toHaveTextContent("hidden because the page holds a secret");
    expect(screen.getByText("Input refused: out_of_bounds")).toBeInTheDocument();
  });

  it("minimizes into a read-only picture-in-picture preview of the same frames", async () => {
    const user = userEvent.setup();
    setup();
    await user.click(screen.getByRole("button", { name: "Open remote screen" }));
    const socket = FakeSocket.last!;
    act(() => socket.open());
    await user.click(screen.getByRole("button", { name: "Minimize" }));
    const pip = screen.getByRole("complementary", { name: "Browser preview" });
    act(() => socket.receive({ ...FRAME, data: "/9j/BBBB" }));
    const picture = screen.getByAltText("Remote browser tab");
    expect(pip).toContainElement(picture);
    expect(picture).toHaveAttribute("src", "data:image/jpeg;base64,/9j/BBBB");
    fireEvent.pointerDown(picture, { clientX: 5, clientY: 5 });
    expect(socket.sent).toEqual([]);
    await user.click(screen.getByRole("button", { name: "Expand" }));
    expect(screen.getByRole("dialog", { name: "Remote control" })).toBeInTheDocument();
    expect(socket.closed).toBe(false);
  });

  it("keeps keyboard focus inside, gives the screen every key, and Escape elsewhere shrinks it", async () => {
    const user = userEvent.setup();
    setup();
    const opener = screen.getByRole("button", { name: "Open remote screen" });
    await user.click(opener);
    const socket = FakeSocket.last!;
    act(() => socket.open());
    const dialog = screen.getByRole("dialog", { name: "Remote control" });
    expect(dialog).toHaveFocus();
    // Tab wraps from the last control back to the first instead of leaving for the page below.
    const reject = screen.getAllByRole("button", { name: "Reject" }).find((b) => dialog.contains(b))!;
    reject.focus();
    await user.tab();
    expect(screen.getByRole("button", { name: "Minimize" })).toHaveFocus();
    await user.tab({ shift: true });
    expect(reject).toHaveFocus();
    // On the remote screen Escape is the page's.
    fireEvent.keyDown(screen.getByLabelText(/Remote browser screen/), { key: "Escape", code: "Escape" });
    expect(socket.sent).toEqual([{ type: "key", event: "down", key: "Escape", code: "Escape", modifiers: 0 }]);
    expect(screen.getByRole("dialog", { name: "Remote control" })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.getByRole("complementary", { name: "Browser preview" })).toBeInTheDocument();
    expect(opener).toHaveFocus();
  });

  it("a takeover card the daemon sent without a screen action offers no remote screen, only Done and Cancel", () => {
    setup({ ...card, actions: card.actions.filter((action) => action.target.type !== "takeover") });
    expect(screen.queryByRole("button", { name: "Open remote screen" })).toBeNull();
    expect(screen.getByRole("button", { name: "Done" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeInTheDocument();
  });
});
