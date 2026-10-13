import { Globe, Minimize2, X } from "lucide-react";
import { useEffect, useRef, useState, type CSSProperties, type KeyboardEvent, type PointerEvent, type WheelEvent } from "react";
import { createPortal } from "react-dom";
import { ApiError } from "../api/client";
import { whilePageShown } from "../api/pageLifecycle";
import type { TakeoverInput, TakeoverServerMessage } from "../api/types";
import { useClient } from "../context";
import { Button, ICON_STROKE, IconButton } from "../ui";
import { BrowserFrame } from "./BrowserFrame";
import { BrowserPip } from "./BrowserPip";
import "./browser.css";

type Frame = Extract<TakeoverServerMessage, { type: "frame" }>;
type Status = "connecting" | "open" | "ended" | "failed";

const END_LABEL: Record<string, string> = {
  done: "Step completed.",
  rejected: "The step was rejected.",
  expired: "The step expired.",
  idle: "Disconnected after inactivity. Reopen to continue.",
  revoked: "This device's access was revoked.",
  tab_unavailable: "The browser tab is gone.",
};

/** CDP modifier mask: Alt=1, Ctrl=2, Meta=4, Shift=8. */
function modifiers(event: { altKey: boolean; ctrlKey: boolean; metaKey: boolean; shiftKey: boolean }): number {
  return (event.altKey ? 1 : 0) | (event.ctrlKey ? 2 : 0) | (event.metaKey ? 4 : 0) | (event.shiftKey ? 8 : 0);
}

const BUTTONS = ["left", "middle", "right"] as const;
/** Pointer moves without a pressed button are sampled; the daemon needs positions, not every pixel. */
const MOVE_INTERVAL_MS = 50;
/** The daemon's `text` input limit (takeover.rs `MAX_TEXT`). */
const MAX_TEXT_BYTES = 4096;

/** Splits text into pieces of at most `limit` UTF-8 bytes, never inside a code point. */
function utf8Chunks(text: string, limit: number): string[] {
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

export interface TakeoverTarget {
  id: string;
  summary: string;
  origin: string | null;
}

/**
 * Remote control of the ticket's browser tab over the takeover WebSocket: JPEG frames in,
 * pointer/keyboard input out (viewport CSS pixels). Minimized, it stays a read-only
 * picture-in-picture preview of the same frames.
 */
export function TakeoverView({ ticket, onClose }: { ticket: TakeoverTarget; onClose: () => void }) {
  const client = useClient();
  const socket = useRef<WebSocket | null>(null);
  const lastMove = useRef(0);
  const [frame, setFrame] = useState<Frame | null>(null);
  const [status, setStatus] = useState<Status>("connecting");
  const [ended, setEnded] = useState<string | null>(null);
  const [withheld, setWithheld] = useState(false);
  const [inputError, setInputError] = useState<string | null>(null);
  const [pip, setPip] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const dialog = useRef<HTMLDivElement>(null);
  const screen = useRef<HTMLDivElement>(null);
  // The control that opened the view gets focus back when it closes or shrinks to the PiP.
  const opener = useRef(document.activeElement);

  // Move focus into the full-screen view so keyboard users start inside it.
  useEffect(() => {
    if (!pip) dialog.current?.focus();
    else if (opener.current instanceof HTMLElement) opener.current.focus();
  }, [pip]);
  useEffect(
    () => () => {
      if (opener.current instanceof HTMLElement && opener.current.isConnected) opener.current.focus();
    },
    [],
  );
  useEffect(
    () =>
      whilePageShown(() => {
        const ws = client.takeover(ticket.id);
        socket.current = ws;
        let finished = false;
        ws.onopen = () => setStatus("open");
        ws.onmessage = (event: MessageEvent) => {
          let message: TakeoverServerMessage;
          try {
            message = JSON.parse(String(event.data)) as TakeoverServerMessage;
          } catch {
            return;
          }
          switch (message.type) {
            case "frame":
              setFrame(message);
              setWithheld(false);
              break;
            case "withheld":
              setWithheld(true);
              break;
            case "input_error":
              setInputError(message.code);
              break;
            case "ended":
              finished = true;
              setEnded(message.reason);
              setStatus("ended");
              ws.close();
              break;
          }
        };
        ws.onclose = () => {
          if (!finished) setStatus((current) => (current === "open" ? "ended" : "failed"));
        };
        // The daemon admits one controller per ticket; a page leaving for the back/forward cache
        // hands the takeover back so the page on screen can open it.
        return () => {
          finished = true;
          socket.current = null;
          ws.close();
        };
      }),
    [client, ticket.id],
  );

  function send(input: TakeoverInput) {
    if (socket.current?.readyState === WebSocket.OPEN) socket.current.send(JSON.stringify(input));
  }

  function point(event: { clientX: number; clientY: number; currentTarget: Element }) {
    if (!frame) return null;
    // The frame is letterboxed inside the screen; map against the picture itself.
    const picture = event.currentTarget.querySelector("img") ?? event.currentTarget;
    const rect = picture.getBoundingClientRect();
    if (!rect.width || !rect.height) return null;
    const x = ((event.clientX - rect.left) / rect.width) * frame.width;
    const y = ((event.clientY - rect.top) / rect.height) * frame.height;
    return {
      x: Math.min(frame.width - 1, Math.max(0, Math.round(x))),
      y: Math.min(frame.height - 1, Math.max(0, Math.round(y))),
    };
  }

  function pointer(phase: "down" | "up" | "move", event: PointerEvent<HTMLElement>) {
    const at = point(event);
    if (!at) return;
    if (phase === "move") {
      const now = performance.now();
      if (!event.buttons && now - lastMove.current < MOVE_INTERVAL_MS) return;
      lastMove.current = now;
    } else {
      event.preventDefault();
      event.currentTarget.focus();
    }
    send({
      type: "mouse",
      event: phase,
      ...at,
      button: phase === "move" ? (event.buttons ? "left" : "none") : (BUTTONS[event.button] ?? "left"),
      modifiers: modifiers(event),
    });
  }

  function wheel(event: WheelEvent<HTMLElement>) {
    const at = point(event);
    if (at) send({ type: "wheel", ...at, delta_x: event.deltaX, delta_y: event.deltaY });
  }

  function key(phase: "down" | "up", event: KeyboardEvent<HTMLElement>) {
    event.preventDefault();
    const printable = event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey;
    if (printable) {
      if (phase === "down") send({ type: "text", text: event.key });
      return;
    }
    send({ type: "key", event: phase, key: event.key, code: event.code, modifiers: modifiers(event) });
  }

  async function finish(action: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (cause) {
      setError(cause instanceof ApiError ? cause.message : "Request failed");
    } finally {
      setBusy(false);
    }
  }

  const image = frame ? <BrowserFrame data={frame.data} /> : null;
  const state =
    status === "connecting"
      ? "Connecting…"
      : status === "failed"
        ? "The remote screen is not available. Finish on the desktop, then mark the step done."
        : status === "ended"
          ? (END_LABEL[ended ?? ""] ?? "Disconnected.")
          : withheld
            ? "The screen is hidden because the page holds a secret value. Input still works."
            : null;

  if (pip) {
    return (
      <BrowserPip
        label="Browser preview"
        title={ticket.summary}
        status={status}
        live={!!image && status === "open" && !withheld}
        onExpand={() => setPip(false)}
        onClose={onClose}
      >
        {image ?? <p className="pip-state">{state ?? "Waiting for a frame…"}</p>}
      </BrowserPip>
    );
  }

  // The remote screen forwards every key, Tab and Escape included, to the page; elsewhere in the
  // view Escape shrinks it to the PiP and Tab stays inside, as in a modal dialog.
  function dialogKey(event: KeyboardEvent<HTMLDivElement>) {
    if (event.target === screen.current || !dialog.current) return;
    if (event.key === "Escape") {
      event.preventDefault();
      setPip(true);
      return;
    }
    if (event.key !== "Tab") return;
    const items = Array.from(dialog.current.querySelectorAll<HTMLElement>("button:not(:disabled), [tabindex='0']"));
    const first = items[0];
    const last = items[items.length - 1];
    if (!first || !last) return;
    const active = document.activeElement;
    if (event.shiftKey && (active === first || active === dialog.current)) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && active === last) {
      event.preventDefault();
      first.focus();
    }
  }

  // Portaled so no transcript stacking context can sit above the full-screen view.
  return createPortal(
    <div
      ref={dialog}
      className="takeover"
      role="dialog"
      aria-modal="true"
      aria-label="Remote control"
      data-status={status}
      tabIndex={-1}
      onKeyDown={dialogKey}
    >
      <header className="takeover-head">
        <IconButton label="Minimize" onClick={() => setPip(true)}>
          <Minimize2 size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </IconButton>
        <h2>Remote control</h2>
        <IconButton label="Close" onClick={onClose}>
          <X size={22} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </IconButton>
      </header>
      <div className="takeover-body">
        <p className="takeover-summary">{ticket.summary}</p>
        {ticket.origin ? (
          <div className="takeover-origin">
            <Globe size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
            <span>{ticket.origin}</span>
          </div>
        ) : null}
        <p className="takeover-note">The agent stays paused on this tab until you choose Done or Reject.</p>
        {state ? (
          <p className="takeover-state" role="status">
            {state}
          </p>
        ) : null}
        <div
          ref={screen}
          className="takeover-screen"
          tabIndex={0}
          aria-label="Remote browser screen; click to interact, type to send keys"
          data-frame-width={frame?.width}
          data-frame-height={frame?.height}
          style={frame ? ({ "--frame-aspect": frame.width / frame.height } as CSSProperties) : undefined}
          onPointerDown={(e) => pointer("down", e)}
          onPointerUp={(e) => pointer("up", e)}
          onPointerMove={(e) => pointer("move", e)}
          onWheel={wheel}
          onKeyDown={(e) => key("down", e)}
          onKeyUp={(e) => key("up", e)}
          onPaste={(e) => {
            const text = e.clipboardData.getData("text");
            for (const chunk of utf8Chunks(text, MAX_TEXT_BYTES)) send({ type: "text", text: chunk });
          }}
          onContextMenu={(e) => e.preventDefault()}
        >
          {image}
        </div>
        {inputError ? <p className="takeover-note">Input refused: {inputError}</p> : null}
        <div className="takeover-actions">
          <Button variant="primary" disabled={busy || status === "ended"} onClick={() => finish(() => client.ticketDone(ticket.id))}>
            Done
          </Button>
          <Button disabled={busy || status === "ended"} onClick={() => finish(() => client.rejectTicket(ticket.id))}>
            Reject
          </Button>
        </div>
        {error ? (
          <p role="alert" className="takeover-error">
            {error}
          </p>
        ) : null}
      </div>
    </div>,
    document.body,
  );
}
