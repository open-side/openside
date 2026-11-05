import { useEffect, useLayoutEffect, useRef, useState, type ClipboardEvent, type KeyboardEvent, type PointerEvent } from "react";
import type { TakeoverInput } from "../api/types";

// Gesture handling adapted from mym0404/aside-relay web/src/useBrowserInput.ts (MIT; see NOTICE):
// a touch that stays put is a tap, one that travels scrolls the page; the wheel listener is
// non-passive so the sheet itself never scrolls under the remote screen.

/** A finger that moves farther than this (CSS px) scrolls instead of tapping. */
const SCROLL_THRESHOLD = 10;
/** A finger drag scrolls the page this many viewport pixels per screen pixel. */
const DRAG_SCROLL_SCALE = 3;
const WHEEL_LINE_HEIGHT = 16;
/** Largest wheel delta per event, as aside-relay clamps it. */
const MAX_SCROLL_DELTA = 2000;
/** Pointer moves without a pressed button are sampled; the daemon needs positions, not every pixel. */
const MOVE_INTERVAL_MS = 50;
/** The daemon's `text` input limit (takeover.rs `MAX_TEXT`). */
export const MAX_TEXT_BYTES = 4096;

const BUTTONS = ["left", "middle", "right"] as const;

export interface Viewport {
  width: number;
  height: number;
}

/** The agent tab's fixed viewport (openside-browser `pointer::VIEWPORT`); input coordinates are CSS pixels in it. */
export const AGENT_VIEWPORT: Viewport = { width: 1440, height: 900 };

/** CDP modifier mask: Alt=1, Ctrl=2, Meta=4, Shift=8. */
export function modifiers(event: { altKey: boolean; ctrlKey: boolean; metaKey: boolean; shiftKey: boolean }): number {
  return (event.altKey ? 1 : 0) | (event.ctrlKey ? 2 : 0) | (event.metaKey ? 4 : 0) | (event.shiftKey ? 8 : 0);
}

/** Splits text into pieces of at most `limit` UTF-8 bytes, never inside a code point. */
export function utf8Chunks(text: string, limit: number): string[] {
  const chunks: string[] = [];
  let current = "";
  let bytes = 0;
  for (const char of text) {
    const point = char.codePointAt(0) ?? 0;
    const size = point < 0x80 ? 1 : point < 0x800 ? 2 : point < 0x10000 ? 3 : 4;
    if (bytes + size > limit) {
      chunks.push(current);
      current = "";
      bytes = 0;
    }
    current += char;
    bytes += size;
  }
  if (current) chunks.push(current);
  return chunks;
}

function clampDelta(value: number): number {
  return Math.max(-MAX_SCROLL_DELTA, Math.min(MAX_SCROLL_DELTA, Math.round(value)));
}

/**
 * Maps a client point to viewport CSS pixels of the remote tab. The picture may be letterboxed
 * inside its box (`object-fit: contain`), so the drawn area is computed from its natural size;
 * points in the bars map to nothing.
 */
export function viewportPoint(surface: Element, clientX: number, clientY: number, viewport: Viewport): { x: number; y: number } | null {
  const picture = surface.querySelector("img") ?? surface;
  const bounds = picture.getBoundingClientRect();
  if (!bounds.width || !bounds.height) return null;
  let { left, top, width, height } = bounds;
  if (picture instanceof HTMLImageElement && picture.naturalWidth && picture.naturalHeight) {
    const scale = Math.min(bounds.width / picture.naturalWidth, bounds.height / picture.naturalHeight);
    width = picture.naturalWidth * scale;
    height = picture.naturalHeight * scale;
    left += (bounds.width - width) / 2;
    top += (bounds.height - height) / 2;
  }
  if (clientX < left || clientX > left + width || clientY < top || clientY > top + height) return null;
  return {
    x: Math.min(viewport.width - 1, Math.max(0, Math.round(((clientX - left) / width) * viewport.width))),
    y: Math.min(viewport.height - 1, Math.max(0, Math.round(((clientY - top) / height) * viewport.height))),
  };
}

interface Gesture {
  id: number;
  x: number;
  y: number;
  lastX: number;
  lastY: number;
  at: { x: number; y: number };
  scrolling: boolean;
}

/**
 * Pointer, wheel, keyboard and paste handlers for a remote browser screen, sending
 * takeover-protocol input in the tab's viewport pixels. A mouse or pen is forwarded as it is
 * (press, release, sampled moves); a touch is a tap, or a scroll when the finger travels. With
 * `send` null the screen is inert.
 */
export function useBrowserInput<T extends HTMLElement>({ send, viewport }: { send: ((input: TakeoverInput) => void) | null; viewport: Viewport | null }) {
  // A state ref, so a screen that remounts (a view leaving the PiP) gets its listeners again.
  const [surface, setSurface] = useState<T | null>(null);
  const lastMove = useRef(0);
  const gesture = useRef<Gesture | null>(null);
  const enabled = !!send && !!viewport;
  // The native wheel listener lives across renders; it reads the current target from here.
  const latest = useRef({ send, viewport });
  useLayoutEffect(() => {
    latest.current = { send, viewport };
  });

  useEffect(() => {
    if (!surface || !enabled) return;
    const onWheel = (event: WheelEvent) => {
      const { send, viewport } = latest.current;
      if (!send || !viewport || event.ctrlKey) return;
      event.preventDefault();
      event.stopPropagation();
      const at = viewportPoint(surface, event.clientX, event.clientY, viewport);
      if (!at) return;
      const unit = event.deltaMode === 1 ? WHEEL_LINE_HEIGHT : event.deltaMode === 2 ? surface.clientHeight : 1;
      send({ type: "wheel", ...at, delta_x: clampDelta(event.deltaX * unit), delta_y: clampDelta(event.deltaY * unit) });
    };
    // The page under the finger scrolls remotely; the sheet around it must not.
    const onTouchMove = (event: TouchEvent) => {
      if (event.cancelable) event.preventDefault();
    };
    surface.addEventListener("wheel", onWheel, { passive: false });
    surface.addEventListener("touchmove", onTouchMove, { passive: false });
    return () => {
      surface.removeEventListener("wheel", onWheel);
      surface.removeEventListener("touchmove", onTouchMove);
      gesture.current = null;
    };
  }, [surface, enabled]);

  function point(event: { clientX: number; clientY: number; currentTarget: Element }) {
    return viewport ? viewportPoint(event.currentTarget, event.clientX, event.clientY, viewport) : null;
  }

  function mouse(phase: "down" | "up" | "move", event: PointerEvent<T>) {
    if (!send) return;
    const at = point(event);
    if (!at) return;
    if (phase === "move") {
      const now = performance.now();
      if (!event.buttons && now - lastMove.current < MOVE_INTERVAL_MS) return;
      lastMove.current = now;
    } else {
      event.preventDefault();
      event.currentTarget.focus({ preventScroll: true });
    }
    send({
      type: "mouse",
      event: phase,
      ...at,
      button: phase === "move" ? (event.buttons ? "left" : "none") : (BUTTONS[event.button] ?? "left"),
      modifiers: modifiers(event),
    });
  }

  function onPointerDown(event: PointerEvent<T>) {
    if (event.pointerType !== "touch") return mouse("down", event);
    if (!send || !event.isPrimary) return;
    event.preventDefault();
    event.currentTarget.focus({ preventScroll: true });
    event.currentTarget.setPointerCapture?.(event.pointerId);
    const at = point(event);
    gesture.current = at ? { id: event.pointerId, x: event.clientX, y: event.clientY, lastX: event.clientX, lastY: event.clientY, at, scrolling: false } : null;
  }

  function onPointerMove(event: PointerEvent<T>) {
    if (event.pointerType !== "touch") return mouse("move", event);
    const g = gesture.current;
    if (!send || !g || g.id !== event.pointerId) return;
    if (!g.scrolling && Math.hypot(event.clientX - g.x, event.clientY - g.y) <= SCROLL_THRESHOLD) return;
    g.scrolling = true;
    const dx = event.clientX - g.lastX;
    const dy = event.clientY - g.lastY;
    g.lastX = event.clientX;
    g.lastY = event.clientY;
    // Content follows the finger: dragging up scrolls down.
    if (dx || dy) send({ type: "wheel", ...g.at, delta_x: clampDelta(-dx * DRAG_SCROLL_SCALE), delta_y: clampDelta(-dy * DRAG_SCROLL_SCALE) });
  }

  function onPointerUp(event: PointerEvent<T>) {
    if (event.pointerType !== "touch") return mouse("up", event);
    const g = gesture.current;
    gesture.current = null;
    if (!send || !g || g.id !== event.pointerId || g.scrolling) return;
    const at = point(event) ?? g.at;
    send({ type: "mouse", event: "down", ...at, button: "left", modifiers: 0 });
    send({ type: "mouse", event: "up", ...at, button: "left", modifiers: 0 });
  }

  function onPointerCancel() {
    gesture.current = null;
  }

  function key(phase: "down" | "up", event: KeyboardEvent<T>) {
    if (!send || event.nativeEvent.isComposing) return;
    event.preventDefault();
    event.stopPropagation();
    const printable = event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey;
    if (printable) {
      if (phase === "down") send({ type: "text", text: event.key });
      return;
    }
    send({ type: "key", event: phase, key: event.key, code: event.code, modifiers: modifiers(event) });
  }

  function onPaste(event: ClipboardEvent<T>) {
    if (!send) return;
    event.preventDefault();
    for (const chunk of utf8Chunks(event.clipboardData.getData("text"), MAX_TEXT_BYTES)) send({ type: "text", text: chunk });
  }

  return {
    surface,
    props: {
      ref: setSurface,
      onPointerDown,
      onPointerMove,
      onPointerUp,
      onPointerCancel,
      onLostPointerCapture: onPointerCancel,
      onKeyDown: (event: KeyboardEvent<T>) => key("down", event),
      onKeyUp: (event: KeyboardEvent<T>) => key("up", event),
      onPaste,
      onContextMenu: (event: { preventDefault: () => void }) => event.preventDefault(),
    },
  };
}
