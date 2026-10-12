import { Dialog as BaseDialog } from "@base-ui/react/dialog";
import { Minus, Plus, X } from "lucide-react";
import { useRef, useState, type KeyboardEvent, type PointerEvent, type ReactNode } from "react";
import { ICON_STROKE, IconButton } from "../ui";
import "./browser.css";

const ZOOM_STEPS = [1, 1.5, 2, 3, 4, 5];
const PAN_STEP = 48;

interface Pointer {
  id: number;
  x: number;
  y: number;
}

export interface ImageViewerProps {
  /** Accessible name of the dialog. */
  label: string;
  title: string;
  subtitle?: string;
  /** Image to show; without one, `placeholder` fills the viewport. */
  src?: string | null;
  alt: string;
  placeholder?: ReactNode;
  onClose: () => void;
}

/**
 * Full-screen zoomable viewer (aside-relay ImagePreview). Zoom is a CSS scale and pan a CSS
 * translate, so buttons, keys, drag and pinch all work without browser zoom.
 */
export function ImageViewer({ label, title, subtitle, src, alt, placeholder, onClose }: ImageViewerProps) {
  const [step, setStep] = useState(0);
  const [offset, setOffset] = useState({ x: 0, y: 0 });
  const [panning, setPanning] = useState(false);
  const pointers = useRef<Pointer[]>([]);
  const viewport = useRef<HTMLDivElement>(null);
  const gesture = useRef<{ x: number; y: number; distance: number; step: number; offset: { x: number; y: number } } | null>(null);
  const scale = ZOOM_STEPS[step] ?? 1;

  function zoomTo(next: number) {
    const clamped = Math.max(0, Math.min(ZOOM_STEPS.length - 1, next));
    setStep(clamped);
    if (clamped === 0) setOffset({ x: 0, y: 0 });
  }

  function reset() {
    setStep(0);
    setOffset({ x: 0, y: 0 });
  }

  function centroid() {
    const [first, second = first] = pointers.current;
    if (!first || !second) return { x: 0, y: 0, distance: 0 };
    return { x: (first.x + second.x) / 2, y: (first.y + second.y) / 2, distance: Math.hypot(first.x - second.x, first.y - second.y) };
  }

  function begin() {
    gesture.current = pointers.current.length ? { ...centroid(), step, offset } : null;
  }

  function down(event: PointerEvent<HTMLDivElement>) {
    if (event.button !== 0 || pointers.current.length >= 2) return;
    event.currentTarget.setPointerCapture?.(event.pointerId);
    pointers.current.push({ id: event.pointerId, x: event.clientX, y: event.clientY });
    setPanning(true);
    begin();
  }

  function move(event: PointerEvent<HTMLDivElement>) {
    const pointer = pointers.current.find((p) => p.id === event.pointerId);
    const start = gesture.current;
    if (!pointer || !start) return;
    pointer.x = event.clientX;
    pointer.y = event.clientY;
    const now = centroid();
    if (pointers.current.length === 2 && start.distance) {
      // Pinch snaps to the nearest zoom step so the level readout stays meaningful.
      const wanted = (ZOOM_STEPS[start.step] ?? 1) * (now.distance / start.distance);
      let nearest = 0;
      ZOOM_STEPS.forEach((value, index) => {
        if (Math.abs(value - wanted) < Math.abs((ZOOM_STEPS[nearest] ?? 1) - wanted)) nearest = index;
      });
      zoomTo(nearest);
      return;
    }
    if (step === 0) return;
    setOffset({ x: start.offset.x + now.x - start.x, y: start.offset.y + now.y - start.y });
  }

  function up(event: PointerEvent<HTMLDivElement>) {
    pointers.current = pointers.current.filter((p) => p.id !== event.pointerId);
    if (!pointers.current.length) setPanning(false);
    begin();
  }

  function key(event: KeyboardEvent<HTMLDivElement>) {
    const pan: Record<string, [number, number]> = {
      ArrowLeft: [PAN_STEP, 0],
      ArrowRight: [-PAN_STEP, 0],
      ArrowUp: [0, PAN_STEP],
      ArrowDown: [0, -PAN_STEP],
    };
    if (event.key === "+" || event.key === "=") zoomTo(step + 1);
    else if (event.key === "-") zoomTo(step - 1);
    else if (event.key === "0") reset();
    else if (pan[event.key] && step > 0) {
      const [dx, dy] = pan[event.key]!;
      setOffset({ x: offset.x + dx, y: offset.y + dy });
    } else return;
    event.preventDefault();
  }

  return (
    <BaseDialog.Root
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <BaseDialog.Portal>
        <BaseDialog.Backdrop className="viewer-backdrop" />
        <BaseDialog.Popup className="viewer" aria-label={label} initialFocus={viewport}>
          <header className="viewer-header">
            <div className="viewer-identity">
              <h2 className="viewer-title">{title}</h2>
              {subtitle ? <span className="viewer-subtitle">{subtitle}</span> : null}
            </div>
            <IconButton label="Close" onClick={onClose}>
              <X size={22} strokeWidth={ICON_STROKE} aria-hidden="true" />
            </IconButton>
          </header>
          <div
            ref={viewport}
            className="viewer-viewport"
            role="region"
            aria-label="Zoomable view"
            aria-description="Drag to move. Use plus and minus to zoom, arrow keys to move, or zero to reset."
            tabIndex={0}
            data-panning={panning || undefined}
            onPointerDown={down}
            onPointerMove={move}
            onPointerUp={up}
            onPointerCancel={up}
            onKeyDown={key}
          >
            {src ? (
              <img
                src={src}
                alt={alt}
                draggable={false}
                style={{ transform: `scale(${scale})`, translate: `${offset.x}px ${offset.y}px` }}
              />
            ) : (
              placeholder
            )}
          </div>
          <footer className="viewer-controls">
            <IconButton label="Zoom out" disabled={step === 0} onClick={() => zoomTo(step - 1)}>
              <Minus size={22} strokeWidth={ICON_STROKE} aria-hidden="true" />
            </IconButton>
            <button type="button" className="viewer-reset" aria-label="Zoom level" title="Reset zoom" onClick={reset}>
              {Math.round(scale * 100)}%
            </button>
            <IconButton label="Zoom in" disabled={step === ZOOM_STEPS.length - 1} onClick={() => zoomTo(step + 1)}>
              <Plus size={22} strokeWidth={ICON_STROKE} aria-hidden="true" />
            </IconButton>
          </footer>
        </BaseDialog.Popup>
      </BaseDialog.Portal>
    </BaseDialog.Root>
  );
}
