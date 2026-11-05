import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { BrowserPip } from "./BrowserPip";

const POSITION_KEY = "openside.pipPosition";

function renderPip() {
  render(
    <BrowserPip label="Live browser" title="Checkout" status="Clicking" live minimized={false} onMinimizedChange={() => {}} onExpand={() => {}} onClose={() => {}}>
      <span>screen</span>
    </BrowserPip>,
  );
  const pip = screen.getByRole("complementary", { name: "Live browser" });
  return { pip, handle: screen.getByRole("button", { name: "Open live browser" }) };
}

const position = (pip: HTMLElement) => ({ x: pip.style.getPropertyValue("--pip-x"), y: pip.style.getPropertyValue("--pip-y") });

/** The anchored card sits at (1000, 300) and moves with its drawn offset, like the real one. */
function layOut() {
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
    const x = parseFloat(this.style.getPropertyValue("--pip-x")) || 0;
    const y = parseFloat(this.style.getPropertyValue("--pip-y")) || 0;
    return { left: 1000 + x, top: 300 + y, width: 280, height: 200, right: 1280 + x, bottom: 500 + y, x: 1000 + x, y: 300 + y, toJSON: () => ({}) } as DOMRect;
  });
}

afterEach(() => {
  vi.restoreAllMocks();
  localStorage.clear();
});

describe("BrowserPip position", () => {
  it("draws its offset through custom properties, not a style attribute value", () => {
    localStorage.setItem(POSITION_KEY, JSON.stringify({ x: -40, y: -60 }));
    const { pip } = renderPip();
    expect(position(pip)).toEqual({ x: "-40px", y: "-60px" });
    expect(pip.style.translate).toBe("");
  });

  it.each([["corrupt JSON", "{not json"], ["a foreign shape", JSON.stringify({ x: "far", y: null })], ["an offset past the anchor", JSON.stringify({ x: 30, y: -10 })]])(
    "starts at the anchor when the stored position is %s",
    (_, stored) => {
      localStorage.setItem(POSITION_KEY, stored);
      const { pip } = renderPip();
      expect(position(pip)).toEqual({ x: "0px", y: "0px" });
    },
  );

  it("stores where a drag ends even when the last move has not been drawn yet", () => {
    const { handle } = renderPip();
    fireEvent.pointerDown(handle, { button: 0, pointerId: 1, clientX: 500, clientY: 500 });
    act(() => {
      fireEvent.pointerMove(handle, { pointerId: 1, clientX: 400, clientY: 450 });
      fireEvent.pointerMove(handle, { pointerId: 1, clientX: 300, clientY: 400 });
      fireEvent.pointerUp(handle, { pointerId: 1, clientX: 300, clientY: 400 });
    });
    expect(JSON.parse(localStorage.getItem(POSITION_KEY)!)).toEqual({ x: -200, y: -100 });
  });

  it("keeps clear of the safe area on the top and left edges", () => {
    layOut();
    const { pip, handle } = renderPip();
    // browser.css resolves these from env(safe-area-inset-*); jsdom reads them inline.
    pip.style.scrollMarginTop = "44px";
    pip.style.scrollMarginLeft = "20px";
    for (let step = 0; step < 20; step += 1) fireEvent.keyDown(handle, { key: "ArrowUp" });
    for (let step = 0; step < 60; step += 1) fireEvent.keyDown(handle, { key: "ArrowLeft" });
    expect(position(pip)).toEqual({ x: "-980px", y: "-256px" });
    expect(JSON.parse(localStorage.getItem(POSITION_KEY)!)).toEqual({ x: -980, y: -256 });
  });

  it("pulls a position saved on a larger window back on screen", () => {
    layOut();
    localStorage.setItem(POSITION_KEY, JSON.stringify({ x: -1400, y: -900 }));
    const { pip } = renderPip();
    expect(position(pip)).toEqual({ x: "-992px", y: "-292px" });
  });
});
