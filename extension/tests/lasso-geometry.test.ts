import { describe, expect, it } from "vitest";
import { captureRect, doublePress, fitPixels, lassoPath, MIN_SELECTION, popoverPosition, selectionRect } from "../src/lasso-geometry";

const viewport = { width: 1000, height: 800 };

describe("selectionRect", () => {
  it("boxes the drawn path", () => {
    expect(selectionRect([{ x: 100, y: 120 }, { x: 300, y: 80 }, { x: 250, y: 400 }], viewport)).toEqual({ x: 100, y: 80, width: 200, height: 320 });
  });

  it("clamps a stroke that leaves the viewport", () => {
    expect(selectionRect([{ x: -50, y: -20 }, { x: 1200, y: 900 }], viewport)).toEqual({ x: 0, y: 0, width: 1000, height: 800 });
  });

  it("treats an empty path or a click-sized stroke as no selection", () => {
    expect(selectionRect([], viewport)).toBeNull();
    expect(selectionRect([{ x: 10, y: 10 }, { x: 10 + MIN_SELECTION - 1, y: 300 }], viewport)).toBeNull();
    expect(selectionRect([{ x: 10, y: 10 }, { x: 300, y: 10 + MIN_SELECTION - 1 }], viewport)).toBeNull();
    expect(selectionRect([{ x: 10, y: 10 }, { x: 10 + MIN_SELECTION, y: 10 + MIN_SELECTION }], viewport)).not.toBeNull();
  });
});

describe("captureRect", () => {
  it("scales CSS pixels to the capture at device pixel ratio 2", () => {
    expect(captureRect({ x: 10, y: 20, width: 100, height: 50 }, viewport, { width: 2000, height: 1600 })).toEqual({ x: 20, y: 40, width: 200, height: 100 });
  });

  it("rounds outwards at fractional zoom so nothing circled is cut", () => {
    // 125% zoom at DPR 1: 1000 CSS px wide shows as 1250 image px.
    expect(captureRect({ x: 10.3, y: 10.3, width: 20.2, height: 20.2 }, viewport, { width: 1250, height: 1000 })).toEqual({ x: 12, y: 12, width: 27, height: 27 });
  });

  it("stays inside the image", () => {
    expect(captureRect({ x: 990, y: 790, width: 50, height: 50 }, viewport, { width: 1000, height: 800 })).toEqual({ x: 990, y: 790, width: 10, height: 10 });
    expect(captureRect({ x: 1000, y: 800, width: 10, height: 10 }, viewport, { width: 1000, height: 800 })).toEqual({ x: 1000, y: 800, width: 1, height: 1 });
  });
});

describe("fitPixels", () => {
  it("keeps a small size and scales a large one down with its aspect ratio", () => {
    expect(fitPixels({ width: 100, height: 100 }, 20_000)).toEqual({ width: 100, height: 100 });
    const fitted = fitPixels({ width: 4000, height: 2000 }, 2_000_000);
    expect(fitted.width * fitted.height).toBeLessThanOrEqual(2_000_000);
    expect(fitted.width / fitted.height).toBeCloseTo(2, 2);
  });
});

describe("popoverPosition", () => {
  const size = { width: 280, height: 120 };

  it("goes below the anchor, aligned to its end edge", () => {
    expect(popoverPosition({ x: 300, y: 100, width: 200, height: 50 }, size, viewport)).toEqual({ x: 220, y: 158 });
  });

  it("flips above when there is no room below", () => {
    expect(popoverPosition({ x: 300, y: 600, width: 200, height: 150 }, size, viewport)).toEqual({ x: 220, y: 472 });
  });

  it("pins to the bottom margin when it fits neither below nor above", () => {
    expect(popoverPosition({ x: 300, y: 50, width: 200, height: 700 }, size, viewport)).toEqual({ x: 220, y: 672 });
  });

  it("stays inside the viewport sideways", () => {
    expect(popoverPosition({ x: 0, y: 100, width: 40, height: 20 }, size, viewport).x).toBe(8);
    expect(popoverPosition({ x: 900, y: 100, width: 400, height: 20 }, size, viewport).x).toBe(1000 - 8 - 280);
  });
});

describe("lassoPath", () => {
  it("closes the stroke back to its start", () => {
    expect(lassoPath([])).toBe("");
    expect(lassoPath([{ x: 1, y: 2 }, { x: 3, y: 4 }, { x: 5, y: 6 }])).toBe("M1 2L3 4L5 6Z");
  });
});

describe("doublePress", () => {
  type Key = { type: "keydown" | "keyup"; key: string; repeat?: boolean; timeStamp: number };
  const run = (keys: Key[], modifier: "alt" | "control" | "shift" = "alt") => {
    const pressed = doublePress(() => modifier);
    return keys.map(key => pressed({ repeat: false, ...key }));
  };
  const tap = (key: string, at: number): Key[] => [{ type: "keydown", key, timeStamp: at }, { type: "keyup", key, timeStamp: at + 50 }];

  it("fires on the second press of the modifier on its own", () => {
    expect(run([...tap("Alt", 0), { type: "keydown", key: "Alt", timeStamp: 200 }]).at(-1)).toBe(true);
    expect(run([...tap("Shift", 0), { type: "keydown", key: "Shift", timeStamp: 200 }], "shift").at(-1)).toBe(true);
  });

  it("ignores the other modifiers", () => {
    expect(run([...tap("Control", 0), { type: "keydown", key: "Control", timeStamp: 200 }]).some(Boolean)).toBe(false);
  });

  it("does not fire when the presses are too far apart", () => {
    expect(run([...tap("Alt", 0), { type: "keydown", key: "Alt", timeStamp: 1000 }]).some(Boolean)).toBe(false);
  });

  it("resets on a chord, so a shortcut never starts a lasso", () => {
    expect(run([
      { type: "keydown", key: "Alt", timeStamp: 0 }, { type: "keydown", key: "a", timeStamp: 20 }, { type: "keyup", key: "a", timeStamp: 40 }, { type: "keyup", key: "Alt", timeStamp: 60 },
      { type: "keydown", key: "Alt", timeStamp: 150 },
    ]).some(Boolean)).toBe(false);
  });

  it("resets when another key comes between the presses", () => {
    expect(run([...tap("Alt", 0), ...tap("b", 100), { type: "keydown", key: "Alt", timeStamp: 250 }]).some(Boolean)).toBe(false);
  });

  it("does not count a held, auto-repeating key", () => {
    expect(run([
      { type: "keydown", key: "Alt", timeStamp: 0 }, { type: "keydown", key: "Alt", repeat: true, timeStamp: 30 }, { type: "keyup", key: "Alt", timeStamp: 60 },
      { type: "keydown", key: "Alt", timeStamp: 120 },
    ]).some(Boolean)).toBe(false);
  });

  it("needs two fresh presses after firing", () => {
    expect(run([...tap("Alt", 0), ...tap("Alt", 200), { type: "keydown", key: "Alt", timeStamp: 400 }])).toEqual([false, false, true, false, false]);
  });
});
