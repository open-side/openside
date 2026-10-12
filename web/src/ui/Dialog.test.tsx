import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Dialog, Drawer } from "./Dialog";

/** jsdom stamps events with the wall clock; the gestures here are timed by the test. */
class TestPointerEvent extends MouseEvent {
  pointerId: number;
  pointerType: string;
  #time: number;
  constructor(type: string, init: MouseEventInit & { pointerId?: number; pointerType?: string; timeStamp?: number } = {}) {
    super(type, init);
    this.pointerId = init.pointerId ?? 1;
    this.pointerType = init.pointerType ?? "mouse";
    this.#time = init.timeStamp ?? 0;
  }
  override get timeStamp() {
    return this.#time;
  }
}

const original = globalThis.PointerEvent;
beforeEach(() => {
  globalThis.PointerEvent = TestPointerEvent as unknown as typeof PointerEvent;
  vi.stubGlobal("matchMedia", (query: string) => ({ matches: query.includes("700px"), media: query }));
  Element.prototype.setPointerCapture = vi.fn();
});
afterEach(() => {
  globalThis.PointerEvent = original;
  vi.unstubAllGlobals();
});

function sheet() {
  const onClose = vi.fn();
  render(
    <Dialog open onClose={onClose} title="Settings">
      <p>body</p>
    </Dialog>,
  );
  return { onClose, popup: screen.getByRole("dialog") };
}

/** Pointer capture routes the rest of the gesture to the element that started it. */
function drag(target: Element, from: number, to: number, time = 400) {
  fireEvent.pointerDown(target, { clientY: from, button: 0, timeStamp: 0 });
  fireEvent.pointerMove(target, { clientY: to, timeStamp: time });
  fireEvent.pointerUp(target, { clientY: to, timeStamp: time });
}

describe("bottom sheet", () => {
  it("closes when dragged down far enough from the header, not only from the handle", () => {
    const { onClose, popup } = sheet();
    drag(screen.getByRole("heading", { name: "Settings" }).closest("header")!, 100, 260);
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(popup.style.transform).toBe("translateY(100%)");
  });

  it("springs back after a short drag", () => {
    const { onClose, popup } = sheet();
    drag(screen.getByRole("heading", { name: "Settings" }).closest("header")!, 100, 140);
    expect(onClose).not.toHaveBeenCalled();
    expect(popup.style.transform).toBe("");
  });

  it("does not start a drag on the header's own buttons", () => {
    const { onClose, popup } = sheet();
    drag(screen.getByRole("button", { name: "Close" }), 100, 400);
    expect(onClose).not.toHaveBeenCalled();
    expect(popup.style.transform).toBe("");
  });

  it("stays put for reduced-motion users", () => {
    vi.stubGlobal("matchMedia", (query: string) => ({ matches: true, media: query }));
    const { onClose } = sheet();
    drag(screen.getByRole("heading", { name: "Settings" }).closest("header")!, 100, 400);
    expect(onClose).not.toHaveBeenCalled();
  });
});

describe("drawer", () => {
  function drawer() {
    const onClose = vi.fn();
    render(
      <Drawer open onClose={onClose} label="Conversations menu">
        <ul>
          <li>one</li>
        </ul>
      </Drawer>,
    );
    return { onClose, popup: screen.getByRole("dialog", { name: "Conversations menu" }) };
  }
  const touch = (target: Element, type: "pointerDown" | "pointerMove" | "pointerUp", x: number, y: number, time = 0) =>
    fireEvent[type](target, { clientX: x, clientY: y, pointerType: "touch", timeStamp: time });

  it("closes on a leftward swipe", () => {
    const { onClose, popup } = drawer();
    touch(popup, "pointerDown", 200, 300);
    touch(popup, "pointerMove", 150, 304, 100);
    expect(popup.style.transform).toBe("translateX(-50px)");
    touch(popup, "pointerMove", 90, 306, 200);
    touch(popup, "pointerUp", 90, 306, 200);
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(popup.style.transform).toBe("translateX(-100%)");
  });

  it("leaves a vertical scroll, a short swipe and a mouse drag alone", () => {
    const { onClose, popup } = drawer();
    touch(popup, "pointerDown", 200, 300);
    touch(popup, "pointerMove", 190, 380, 100);
    touch(popup, "pointerUp", 190, 380, 100);

    touch(popup, "pointerDown", 200, 300);
    touch(popup, "pointerMove", 170, 300, 600);
    touch(popup, "pointerUp", 170, 300, 600);
    expect(popup.style.transform).toBe("");

    fireEvent.pointerDown(popup, { clientX: 200, clientY: 300, pointerType: "mouse" });
    fireEvent.pointerMove(popup, { clientX: 20, clientY: 300, pointerType: "mouse", timeStamp: 100 });
    fireEvent.pointerUp(popup, { clientX: 20, clientY: 300, pointerType: "mouse", timeStamp: 100 });
    expect(onClose).not.toHaveBeenCalled();
  });
});
