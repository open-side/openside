import { Dialog as BaseDialog } from "@base-ui/react/dialog";
import { ChevronLeft, X } from "lucide-react";
import { useRef, type PointerEvent as ReactPointerEvent, type ReactNode, type RefObject } from "react";
import { cx, ICON_STROKE, IconButton } from "./Button";

export interface DialogProps {
  open: boolean;
  onClose: () => void;
  title: string;
  description?: ReactNode;
  /** Back control before the heading, for dialogs that page through sections. */
  back?: { label: string; onClick: () => void };
  /** `wide` fits grids and tables (640px instead of 420px); phones always get a full-width sheet. */
  size?: "default" | "wide";
  children?: ReactNode;
  /** Element focused when the dialog opens; defaults to the first focusable control. */
  initialFocus?: RefObject<HTMLElement | null>;
  className?: string;
}

// Matches the sheet breakpoint in ui.css.
const SHEET_QUERY = "(max-width: 700px)";
// aside-relay's thresholds: 96px down, or a flick faster than 700px/s.
const CLOSE_DISTANCE = 96;
const CLOSE_VELOCITY = 0.7;
const DRAWER_CLOSE_DISTANCE = 80;

/**
 * Phone sheets close when their handle is dragged down far or fast enough; anything less
 * springs back. Only the handle starts a drag, so scrolling the content never does, and
 * reduced-motion users keep the Close button and Escape.
 */
function useSheetDrag(popup: RefObject<HTMLDivElement | null>, onClose: () => void) {
  return (event: ReactPointerEvent<HTMLElement>) => {
    const element = popup.current;
    if (!element || event.button !== 0) return;
    if (!window.matchMedia?.(SHEET_QUERY).matches || window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    event.preventDefault();
    const handle = event.currentTarget;
    handle.setPointerCapture(event.pointerId);
    const startY = event.clientY;
    let lastY = startY;
    let lastTime = event.timeStamp;
    let velocity = 0;
    element.style.transition = "none";

    const move = (next: PointerEvent) => {
      const dy = Math.max(0, next.clientY - startY);
      const dt = next.timeStamp - lastTime;
      if (dt > 0) velocity = (next.clientY - lastY) / dt;
      lastY = next.clientY;
      lastTime = next.timeStamp;
      element.style.transform = `translateY(${dy}px)`;
    };
    const end = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", end);
      handle.removeEventListener("pointercancel", end);
      element.style.transition = "";
      if (lastY - startY > CLOSE_DISTANCE || velocity > CLOSE_VELOCITY) {
        // Continue from the dragged offset to fully hidden; Base UI waits for this transition.
        element.style.transform = "translateY(100%)";
        onClose();
      } else {
        element.style.transform = "";
      }
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", end);
    handle.addEventListener("pointercancel", end);
  };
}

/**
 * Centered modal on wide screens, bottom sheet on phones (CSS only). Base UI owns focus
 * trapping, Escape, outside press and scroll lock.
 */
export function Dialog({ open, onClose, title, description, back, size = "default", children, initialFocus, className }: DialogProps) {
  const popup = useRef<HTMLDivElement>(null);
  const startDrag = useSheetDrag(popup, onClose);
  return (
    <BaseDialog.Root
      open={open}
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
    >
      <BaseDialog.Portal>
        <BaseDialog.Backdrop className="ui-backdrop" />
        <BaseDialog.Viewport className="ui-dialog-viewport">
          <BaseDialog.Popup
            ref={popup}
            className={cx("ui-dialog", size === "wide" && "ui-dialog--wide", className)}
            initialFocus={initialFocus}
          >
            <div className="ui-dialog-handle" aria-hidden="true" onPointerDown={startDrag}>
              <span />
            </div>
            <header
              className="ui-dialog-header"
              onPointerDown={(event) => {
                if (!(event.target as Element).closest("button, a, input, select, textarea")) startDrag(event);
              }}
            >
              {back && (
                <IconButton label={back.label} size="small" onClick={back.onClick}>
                  <ChevronLeft size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
                </IconButton>
              )}
              <div className="ui-dialog-heading">
                <BaseDialog.Title className="ui-dialog-title">{title}</BaseDialog.Title>
                {description && <BaseDialog.Description className="ui-dialog-description">{description}</BaseDialog.Description>}
              </div>
              <BaseDialog.Close
                render={
                  <IconButton label="Close" size="small">
                    <X size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
                  </IconButton>
                }
              />
            </header>
            <div className="ui-dialog-body">{children}</div>
          </BaseDialog.Popup>
        </BaseDialog.Viewport>
      </BaseDialog.Portal>
    </BaseDialog.Root>
  );
}

export function DialogActions({ children }: { children: ReactNode }) {
  return <div className="ui-dialog-actions">{children}</div>;
}

export interface DrawerProps {
  open: boolean;
  onClose: () => void;
  /** Accessible name of the drawer. */
  label: string;
  children: ReactNode;
}

/**
 * Swiping the drawer back toward the left edge closes it (the mirror of the edge swipe that
 * opens it). Vertical scrolling of the list wins; only a mostly-horizontal leftward move drags.
 */
function useDrawerSwipe(onClose: () => void) {
  const popup = useRef<HTMLDivElement>(null);
  const gesture = useRef<{ id: number; x: number; y: number; dragging: boolean; lastX: number; lastTime: number; velocity: number } | null>(null);

  const reset = (element: HTMLElement) => {
    gesture.current = null;
    element.style.transition = "";
    element.style.transform = "";
  };
  return {
    ref: popup,
    onPointerDown(event: ReactPointerEvent<HTMLElement>) {
      if (event.pointerType !== "touch") return;
      gesture.current = { id: event.pointerId, x: event.clientX, y: event.clientY, dragging: false, lastX: event.clientX, lastTime: event.timeStamp, velocity: 0 };
    },
    onPointerMove(event: ReactPointerEvent<HTMLElement>) {
      const g = gesture.current;
      const element = popup.current;
      if (!g || !element || g.id !== event.pointerId) return;
      const dx = event.clientX - g.x;
      if (!g.dragging) {
        const dy = Math.abs(event.clientY - g.y);
        if (dy > 10 && dy > Math.abs(dx)) gesture.current = null;
        else if (dx < -10 && -dx > dy * 1.5) {
          g.dragging = true;
          element.setPointerCapture(event.pointerId);
          element.style.transition = "none";
        }
        if (!g.dragging) return;
      }
      const dt = event.timeStamp - g.lastTime;
      if (dt > 0) g.velocity = (event.clientX - g.lastX) / dt;
      g.lastX = event.clientX;
      g.lastTime = event.timeStamp;
      element.style.transform = `translateX(${Math.min(0, dx)}px)`;
    },
    onPointerUp(event: ReactPointerEvent<HTMLElement>) {
      const g = gesture.current;
      const element = popup.current;
      if (!g || !element || g.id !== event.pointerId) return;
      if (g.dragging && (event.clientX - g.x < -DRAWER_CLOSE_DISTANCE || g.velocity < -CLOSE_VELOCITY)) {
        gesture.current = null;
        element.style.transition = "";
        element.style.transform = "translateX(-100%)";
        onClose();
      } else reset(element);
    },
    onPointerCancel() {
      if (popup.current) reset(popup.current);
    },
  };
}

/** Modal side drawer (the phone conversation menu). */
export function Drawer({ open, onClose, label, children }: DrawerProps) {
  const { ref, ...swipe } = useDrawerSwipe(onClose);
  return (
    <BaseDialog.Root
      open={open}
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
    >
      <BaseDialog.Portal>
        <BaseDialog.Backdrop className="ui-backdrop ui-drawer-backdrop" />
        <BaseDialog.Popup ref={ref} className="ui-drawer" aria-label={label} {...swipe}>
          {children}
        </BaseDialog.Popup>
      </BaseDialog.Portal>
    </BaseDialog.Root>
  );
}
