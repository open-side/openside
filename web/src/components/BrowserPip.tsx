import { ChevronUp, Maximize2, Minus, X } from "lucide-react";
import { useLayoutEffect, useRef, useState, type KeyboardEvent, type PointerEvent, type ReactNode } from "react";
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

/** Limits `next` so the card stays inside the viewport; `from` is the offset `element` is drawn at. */
function clamp(element: HTMLElement | null, next: Offset, from: Offset): Offset {
  const rect = element?.getBoundingClientRect();
  if (!rect || !rect.width) return { x: Math.min(0, next.x), y: Math.min(0, next.y) };
  const margin = 8;
  // The anchored (zero-offset) box is `rect` shifted back by `from`.
  return {
    x: Math.min(0, Math.max(margin - (rect.left - from.x), next.x)),
    y: Math.min(0, Math.max(margin - (rect.top - from.y), next.y)),
  };
}

export interface BrowserPipProps {
  /** Accessible name of the floating region. */
  label: string;
  title: string;
  status: string;
  /** Frames are arriving; the title's dot turns green. */
  live: boolean;
  /** Picture or status text shown in the screen area. */
  children: ReactNode;
  onExpand: () => void;
  onClose: () => void;
  className?: string;
}

/**
 * Floating picture-in-picture card (aside-relay BrowserLive): a draggable title bar with
 * expand, minimize and close, over a read-only screen. It never forwards input to the tab.
 */
export function BrowserPip({ label, title, status, live, children, onExpand, onClose, className }: BrowserPipProps) {
  const card = useRef<HTMLElement>(null);
  const drag = useRef<{ x: number; y: number; start: Offset; moved: boolean } | null>(null);
  const dragged = useRef(false);
  const [offset, setOffset] = useState(loadOffset);
  const [minimized, setMinimized] = useState(false);

  // Keep the card on screen when the viewport shrank since the position was saved.
  useLayoutEffect(() => {
    setOffset((current) => clamp(card.current, current, current));
  }, [minimized]);

  function save(next: Offset) {
    try {
      localStorage.setItem(POSITION_KEY, JSON.stringify(next));
    } catch {
      // Position memory is a convenience only.
    }
  }

  function pointerDown(event: PointerEvent<HTMLButtonElement>) {
    if (event.button !== 0) return;
    event.currentTarget.setPointerCapture?.(event.pointerId);
    drag.current = { x: event.clientX, y: event.clientY, start: offset, moved: false };
  }

  function pointerMove(event: PointerEvent<HTMLButtonElement>) {
    const state = drag.current;
    if (!state) return;
    const dx = event.clientX - state.x;
    const dy = event.clientY - state.y;
    if (!state.moved && Math.hypot(dx, dy) < DRAG_THRESHOLD) return;
    state.moved = true;
    setOffset(clamp(card.current, { x: state.start.x + dx, y: state.start.y + dy }, offset));
  }

  function pointerUp() {
    const state = drag.current;
    drag.current = null;
    dragged.current = !!state?.moved;
    if (state?.moved) save(offset);
  }

  function handleClick() {
    // A drag ends with a click on the handle; only a tap opens the preview.
    if (dragged.current) {
      dragged.current = false;
      return;
    }
    onExpand();
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
    const next = clamp(card.current, { x: offset.x + step.x, y: offset.y + step.y }, offset);
    setOffset(next);
    save(next);
  }

  return createPortal(
    <aside
      ref={card}
      className={cx("pip", className)}
      aria-label={label}
      data-status={status}
      data-minimized={minimized || undefined}
      style={{ translate: `${offset.x}px ${offset.y}px` }}
    >
      <div className="pip-bar">
        <button
          type="button"
          className="pip-handle"
          aria-label={`Open ${label.toLowerCase()}`}
          aria-description="Drag to move, or use the arrow keys."
          onPointerDown={pointerDown}
          onPointerMove={pointerMove}
          onPointerUp={pointerUp}
          onPointerCancel={pointerUp}
          onKeyDown={handleKey}
          onClick={handleClick}
        >
          <span className="live-dot" data-live={live || undefined} aria-hidden="true" />
          <span className="pip-title">{title}</span>
        </button>
        <button type="button" className="pip-control" aria-label="Expand" title="Expand" onClick={onExpand}>
          <Maximize2 size={15} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </button>
        <button
          type="button"
          className="pip-control"
          aria-label={minimized ? "Restore" : "Minimize"}
          title={minimized ? "Restore" : "Minimize"}
          aria-expanded={!minimized}
          onClick={() => setMinimized(!minimized)}
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
