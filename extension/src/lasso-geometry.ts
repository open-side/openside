/** Pure geometry for the lasso overlay and the capture crop, in CSS pixels unless named otherwise. */
export interface Point { x: number; y: number }
export interface Rect { x: number; y: number; width: number; height: number }
export interface Size { width: number; height: number }

/** A stroke smaller than this on either side is a click, not a selection. */
export const MIN_SELECTION = 8;

/** The box around the drawn path, kept inside the viewport; null for an empty or click-sized path. */
export function selectionRect(points: readonly Point[], viewport: Size): Rect | null {
  if (!points.length) return null;
  const xs = points.map(point => Math.min(Math.max(point.x, 0), viewport.width));
  const ys = points.map(point => Math.min(Math.max(point.y, 0), viewport.height));
  const x = Math.min(...xs);
  const y = Math.min(...ys);
  const width = Math.max(...xs) - x;
  const height = Math.max(...ys) - y;
  return width < MIN_SELECTION || height < MIN_SELECTION ? null : { x, y, width, height };
}

/**
 * The selection in the captured image's pixels. The capture covers exactly the viewport, so the
 * scale is the image size over the viewport size (device pixel ratio times zoom); edges round
 * outwards so nothing the person circled is cut, then stay inside the image.
 */
export function captureRect(selection: Rect, viewport: Size, image: Size): Rect {
  const scaleX = image.width / viewport.width;
  const scaleY = image.height / viewport.height;
  const left = Math.max(0, Math.floor(selection.x * scaleX));
  const top = Math.max(0, Math.floor(selection.y * scaleY));
  const right = Math.min(image.width, Math.ceil((selection.x + selection.width) * scaleX));
  const bottom = Math.min(image.height, Math.ceil((selection.y + selection.height) * scaleY));
  return { x: left, y: top, width: Math.max(1, right - left), height: Math.max(1, bottom - top) };
}

/** An image size within `maxPixels`, keeping the aspect ratio; never scales up. */
export function fitPixels(size: Size, maxPixels: number): Size {
  const pixels = size.width * size.height;
  if (pixels <= maxPixels) return size;
  const scale = Math.sqrt(maxPixels / pixels);
  return { width: Math.max(1, Math.floor(size.width * scale)), height: Math.max(1, Math.floor(size.height * scale)) };
}

/**
 * Where a popover of `size` goes next to `anchor`: below its end edge, else above, and always
 * `margin` inside the viewport.
 */
export function popoverPosition(anchor: Rect, size: Size, viewport: Size, gap = 8, margin = 8): Point {
  const below = anchor.y + anchor.height + gap;
  const above = anchor.y - gap - size.height;
  const y = below + size.height <= viewport.height - margin ? below : above >= margin ? above : viewport.height - margin - size.height;
  const x = anchor.x + anchor.width - size.width;
  return {
    x: Math.round(Math.min(Math.max(x, margin), Math.max(margin, viewport.width - margin - size.width))),
    y: Math.round(Math.max(margin, y)),
  };
}

/** An SVG path through the drawn points, closed back to the start like a lasso. */
export function lassoPath(points: readonly Point[]): string {
  if (!points.length) return "";
  const [first, ...rest] = points;
  return `M${first!.x} ${first!.y}${rest.map(point => `L${point.x} ${point.y}`).join("")}Z`;
}

export type Modifier = "alt" | "control" | "shift";
const KEY_OF: Record<Modifier, string> = { alt: "Alt", control: "Control", shift: "Shift" };

/**
 * Detects the modifier pressed twice on its own within `windowMs` (Aside's ⌥ + ⌥). Any other key
 * in between, a held key (auto-repeat) or a chord resets it, so typing a capital letter or a
 * shortcut never starts a lasso.
 */
export function doublePress(modifier: () => Modifier, windowMs = 400): (event: Pick<KeyboardEvent, "type" | "key" | "repeat" | "timeStamp">) => boolean {
  let releasedAt = -Infinity;
  let clean = false;
  return event => {
    const key = KEY_OF[modifier()];
    if (event.key !== key) { clean = false; releasedAt = -Infinity; return false; }
    if (event.repeat) { clean = false; return false; }
    if (event.type === "keydown") {
      if (clean && event.timeStamp - releasedAt <= windowMs) { clean = false; releasedAt = -Infinity; return true; }
      clean = true;
      return false;
    }
    if (event.type === "keyup" && clean) releasedAt = event.timeStamp;
    return false;
  };
}
