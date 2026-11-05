import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { MenuItem, ToastProvider } from "../ui";
import { MessageActions, MessageMenu, quoteText } from "./MessageMenu";

function mount(onQuote?: (text: string) => void, extra?: ReactNode) {
  return render(
    <ToastProvider>
      <ol>
        <MessageMenu className="bubble assistant" text={"It rained.\n\nTake an umbrella."} onQuote={onQuote} extra={extra}>
          <p>It rained.</p>
          <a href="https://example.com">a link</a>
        </MessageMenu>
      </ol>
    </ToastProvider>,
  );
}

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("MessageMenu", () => {
  it("opens Copy and Quote on a right click, and Copy copies the whole message", async () => {
    // userEvent installs its own clipboard on setup; spy on that one.
    const user = userEvent.setup();
    vi.stubGlobal("isSecureContext", true);
    const writeText = vi.spyOn(navigator.clipboard, "writeText");
    mount(() => undefined);
    fireEvent.contextMenu(screen.getByText("It rained."));
    const menu = await screen.findByRole("menu", { name: "Message actions" });
    expect(menu).toBeInTheDocument();
    await user.click(screen.getByRole("menuitem", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith("It rained.\n\nTake an umbrella.");
    expect(await screen.findByText("Copied")).toBeInTheDocument();
  });

  it("quotes the message into the composer through onQuote", async () => {
    const user = userEvent.setup();
    const onQuote = vi.fn();
    mount(onQuote);
    fireEvent.contextMenu(screen.getByText("It rained."));
    await user.click(await screen.findByRole("menuitem", { name: "Quote" }));
    expect(onQuote).toHaveBeenCalledWith("It rained.\n\nTake an umbrella.");
  });

  it("offers no Quote where nothing can be written, and adds a message's own items", async () => {
    mount(undefined, <MenuItem onClick={() => undefined}>Edit message</MenuItem>);
    fireEvent.contextMenu(screen.getByText("It rained."));
    await screen.findByRole("menu", { name: "Message actions" });
    expect(screen.queryByRole("menuitem", { name: "Quote" })).toBeNull();
    expect(screen.getByRole("menuitem", { name: "Edit message" })).toBeInTheDocument();
  });

  it("leaves a link's right click to the browser", () => {
    mount(() => undefined);
    const event = fireEvent.contextMenu(screen.getByRole("link", { name: "a link" }));
    expect(event).toBe(true);
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("opens on a 500 ms touch press, and not when the finger moves", () => {
    // The shared jsdom PointerEvent stand-in carries no pointerType, which the press depends on.
    class TouchPointer extends MouseEvent {
      readonly pointerType: string;
      readonly isPrimary: boolean;
      constructor(type: string, init: PointerEventInit = {}) {
        super(type, init);
        this.pointerType = init.pointerType ?? "";
        this.isPrimary = init.isPrimary ?? false;
      }
    }
    vi.stubGlobal("PointerEvent", TouchPointer);
    vi.useFakeTimers();
    mount(() => undefined);
    const text = screen.getByText("It rained.");
    fireEvent.pointerDown(text, { pointerType: "touch", isPrimary: true, clientX: 10, clientY: 10 });
    fireEvent.pointerMove(text, { pointerType: "touch", clientX: 10, clientY: 30 });
    act(() => vi.advanceTimersByTime(600));
    expect(screen.queryByRole("menu")).toBeNull();

    fireEvent.pointerDown(text, { pointerType: "touch", isPrimary: true, clientX: 10, clientY: 10 });
    act(() => vi.advanceTimersByTime(499));
    expect(screen.queryByRole("menu")).toBeNull();
    act(() => vi.advanceTimersByTime(1));
    expect(screen.getByRole("menu", { name: "Message actions" })).toBeInTheDocument();
  });

  it("formats a quote as Markdown lines followed by a blank line", () => {
    expect(quoteText("First\n\nSecond\n")).toBe("> First\n>\n> Second\n\n");
  });

  it("copies from the hover row under a finished reply", async () => {
    // userEvent installs its own clipboard on setup; spy on that one.
    const user = userEvent.setup();
    vi.stubGlobal("isSecureContext", true);
    const writeText = vi.spyOn(navigator.clipboard, "writeText");
    render(
      <ToastProvider>
        <MessageActions text="Done." />
      </ToastProvider>,
    );
    await user.click(screen.getByRole("button", { name: "Copy message" }));
    expect(writeText).toHaveBeenCalledWith("Done.");
  });
});

describe("MessageMenu on a touch screen", () => {
  class TouchPointer extends MouseEvent {
    readonly pointerType: string;
    readonly isPrimary: boolean;
    constructor(type: string, init: PointerEventInit = {}) {
      super(type, init);
      this.pointerType = init.pointerType ?? "";
      this.isPrimary = init.isPrimary ?? false;
    }
  }
  const touch = { pointerType: "touch", isPrimary: true, clientX: 10, clientY: 10 };
  function coarse() {
    vi.stubGlobal("PointerEvent", TouchPointer);
    vi.stubGlobal("matchMedia", (query: string) => ({ matches: query === "(pointer: coarse)", media: query, addEventListener: () => undefined, removeEventListener: () => undefined }));
  }

  it("leaves a long press on a link to the browser", () => {
    coarse();
    vi.useFakeTimers();
    mount(() => undefined);
    fireEvent.pointerDown(screen.getByRole("link", { name: "a link" }), touch);
    act(() => vi.advanceTimersByTime(600));
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("Select text hands the message to native selection until a tap lands outside it", async () => {
    coarse();
    mount(() => undefined);
    const bubble = screen.getByRole("listitem");
    fireEvent.contextMenu(screen.getByText("It rained."));
    fireEvent.click(await screen.findByRole("menuitem", { name: "Select text" }));
    await waitFor(() => expect(bubble).toHaveAttribute("data-selectable"));
    await waitFor(() => expect(window.getSelection()?.toString()).toContain("It rained."));

    // While selectable, a press inside selects text instead of opening the menu.
    fireEvent.contextMenu(screen.getByText("It rained."));
    expect(screen.queryByRole("menu")).toBeNull();
    fireEvent.pointerDown(bubble, touch);
    expect(bubble).toHaveAttribute("data-selectable");

    fireEvent.pointerDown(document.body, touch);
    await waitFor(() => expect(bubble).not.toHaveAttribute("data-selectable"));
  });

  it("offers no Select text where a pointer can already select", async () => {
    mount(() => undefined);
    fireEvent.contextMenu(screen.getByText("It rained."));
    await screen.findByRole("menu", { name: "Message actions" });
    expect(screen.queryByRole("menuitem", { name: "Select text" })).toBeNull();
  });
});
