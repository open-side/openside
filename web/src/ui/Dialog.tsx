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
            <header className="ui-dialog-header">
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

/** Modal side drawer (the phone conversation menu). */
export function Drawer({ open, onClose, label, children }: DrawerProps) {
  return (
    <BaseDialog.Root
      open={open}
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
    >
      <BaseDialog.Portal>
        <BaseDialog.Backdrop className="ui-backdrop ui-drawer-backdrop" />
        <BaseDialog.Popup className="ui-drawer" aria-label={label}>
          {children}
        </BaseDialog.Popup>
      </BaseDialog.Portal>
    </BaseDialog.Root>
  );
}
