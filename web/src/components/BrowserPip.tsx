import { ChevronUp, Maximize2, Minus, X } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type PointerEvent, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { cx, ICON_STROKE } from "../ui";
import "./browser.css";

const POSITION_KEY = "openside.pipPosition";
const KEY_STEP = 20;
/** Pointer travel below this is a tap on the title, not a drag. */
const DRAG_THRESHOLD = 4;

type Offset = { x: number; y: number };

function loadOffset(): Offset {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(POSITION_KEY) ?? "null");
    if (value && typeof value === "object") {
      const { x, y } = value as Record<string, unknown>;
      // Offsets are measured from the bottom-right anchor, so valid ones are never positive.
      if (typeof x === "number" && typeof y === "number" && Number.isFinite(x) && Number.isFinite(y) && x <= 0 && y <= 0) return { x, y };
    }
  } catch {
    // Storage unavailable or corrupt: start at the anchor.
  }
  return { x: 0, y: 0 };
}

function saveOffset(next: Offset) {
  try {
    localStorage.setItem(POSITION_KEY, JSON.stringify(next));
  } catch {
    // Position memory is a convenience only.
  }
}

/**
 * Limits `next` so the card stays inside the viewport and clear of the notch; `from` is the
 * offset `element` is drawn at. The right and bottom edges are the anchor, which browser.css
 * already keeps inside the safe area; the top and left insets come from the element's
 * `scroll-margin`, which browser.css resolves from `env(safe-area-inset-*)`.
 */
function clamp(element: HTMLElement | null, next: Offset, from: Offset): Offset {
  const rect = element?.getBoundingClientRect();
  if (!element || !rect?.width) return { x: Math.min(0, next.x), y: Math.min(0, next.y) };
  const style = getComputedStyle(element);
  const top = parseFloat(style.scrollMarginTop) || 8;
  const left = parseFloat(style.scrollMarginLeft) || 8;
  // The anchored (zero-offset) box is `rect` shifted back by `from`.
  return {
    x: Math.min(0, Math.max(left - (rect.left - from.x), next.x)),
    y: Math.min(0, Math.max(top - (rect.top - from.y), next.y)),
  };
}

export interface BrowserPipProps {
  /** Accessible name of the floating region. */
  label: string;
  title: string;
  status: string;
  /** Frames are arriving; the title's dot shows live. */
  live: boolean;
  /** Folded to a 48px pill that shows `thumb` and the title. */
  minimized: boolean;
  onMinimizedChange: (minimized: boolean) => void;
  /** Small picture of the page for the minimized pill: the frame or the site's icon. */
  thumb?: ReactNode;
  /** Picture or status text shown in the screen area. */
  children: ReactNode;
  onExpand: () => void;
  onClose: () => void;
  className?: string;
}

/**
 * Floating picture-in-picture card (aside-relay BrowserLive): a draggable title bar with
 * expand, minimize and close, over a read-only screen. It never forwards input to the tab.
 * Its position is remembered across conversations and reloads and re-clamped when the window
 * shrinks.
 */
export function BrowserPip({ label, title, status, live, minimized, onMinimizedChange, thumb, children, onExpand, onClose, className }: BrowserPipProps) {
  const card = useRef<HTMLElement>(null);
  const drag = useRef<{ x: number; y: number; start: Offset; moved: boolean } | null>(null);
  const dragged = useRef(false);
  const [offset, setOffset] = useState(loadOffset);
  // The position a drag ends at may not have rendered yet when pointerup fires; persisting from
  // this ref instead of the render's `offset` stores where the card actually stopped.
  const latest = useRef(offset);
  function place(next: Offset) {
    latest.current = next;
    setOffset(next);
  }

  // The daemon's CSP forbids style attributes, so the position is set as custom properties
  // through the CSSOM, as `--composer-height` is (browser.css reads them in `translate`).
  useLayoutEffect(() => {
    card.current?.style.setProperty("--pip-x", `${offset.x}px`);
    card.current?.style.setProperty("--pip-y", `${offset.y}px`);
  }, [offset]);

  // Keep the card on screen when the viewport shrank since the position was saved, and when
  // the pill and the full card trade places.
  useLayoutEffect(() => {
    place(clamp(card.current, latest.current, latest.current));
  }, [minimized]);
  useEffect(() => {
    const reclamp = () => place(clamp(card.current, latest.current, latest.current));
    window.addEventListener("resize", reclamp);
    return () => window.removeEventListener("resize", reclamp);
  }, []);

  function pointerDown(event: PointerEvent<HTMLButtonElement>) {
    if (event.button !== 0) return;
    event.currentTarget.setPointerCapture?.(event.pointerId);
    drag.current = { x: event.clientX, y: event.clientY, start: latest.current, moved: false };
  }

  function pointerMove(event: PointerEvent<HTMLButtonElement>) {
    const state = drag.current;
    if (!state) return;
    const dx = event.clientX - state.x;
    const dy = event.clientY - state.y;
    if (!state.moved && Math.hypot(dx, dy) < DRAG_THRESHOLD) return;
    state.moved = true;
    place(clamp(card.current, { x: state.start.x + dx, y: state.start.y + dy }, offset));
  }

  function pointerUp() {
    const state = drag.current;
    drag.current = null;
    dragged.current = !!state?.moved;
    if (state?.moved) saveOffset(latest.current);
  }

  function handleClick() {
    // A drag ends with a click on the handle; only a tap acts.
    if (dragged.current) {
      dragged.current = false;
      return;
    }
    if (minimized) onMinimizedChange(false);
    else onExpand();
  }

  function handleKey(event: KeyboardEvent<HTMLButtonElement>) {
    const moves: Record<string, Offset> = {
      ArrowLeft: { x: -KEY_STEP, y: 0 },
      ArrowRight: { x: KEY_STEP, y: 0 },
      ArrowUp: { x: 0, y: -KEY_STEP },
      ArrowDown: { x: 0, y: KEY_STEP },
    };
    const step = moves[event.key];
    if (!step) return;
    event.preventDefault();
    const next = clamp(card.current, { x: latest.current.x + step.x, y: latest.current.y + step.y }, offset);
    place(next);
    saveOffset(next);
  }

  return createPortal(
    <aside
      ref={card}
      className={cx("pip", className)}
      aria-label={label}
      data-status={status}
      data-minimized={minimized || undefined}
    >
      <div className="pip-bar">
        <button
          type="button"
          className="pip-handle"
          aria-label={minimized ? `Restore ${label.toLowerCase()}` : `Open ${label.toLowerCase()}`}
          aria-description="Drag to move, or use the arrow keys."
          onPointerDown={pointerDown}
          onPointerMove={pointerMove}
          onPointerUp={pointerUp}
          onPointerCancel={pointerUp}
          onKeyDown={handleKey}
          onClick={handleClick}
        >
          {minimized ? (
            <span className="pip-thumb" aria-hidden="true">
              {thumb}
            </span>
          ) : null}
          <span className="live-dot" data-live={live || undefined} aria-hidden="true" />
          <span className="pip-title">{title}</span>
        </button>
        {minimized ? null : (
          <button type="button" className="pip-control" aria-label="Expand" title="Expand" onClick={onExpand}>
            <Maximize2 size={15} strokeWidth={ICON_STROKE} aria-hidden="true" />
          </button>
        )}
        <button
          type="button"
          className="pip-control"
          aria-label={minimized ? "Restore" : "Minimize"}
          title={minimized ? "Restore" : "Minimize"}
          aria-expanded={!minimized}
          onClick={() => onMinimizedChange(!minimized)}
        >
          {minimized ? (
            <ChevronUp size={17} strokeWidth={ICON_STROKE} aria-hidden="true" />
          ) : (
            <Minus size={17} strokeWidth={ICON_STROKE} aria-hidden="true" />
          )}
        </button>
        <button type="button" className="pip-control" aria-label="Close" title="Close" onClick={onClose}>
          <X size={17} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </button>
      </div>
      {minimized ? null : (
        // Mouse shortcut only; keyboard users have the Expand button above.
        <div className="pip-screen" onClick={onExpand}>
          {children}
        </div>
      )}
    </aside>,
    document.body,
  );
}
