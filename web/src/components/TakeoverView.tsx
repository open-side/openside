import { Globe, Minimize2, X } from "lucide-react";
import { useEffect, useRef, useState, type CSSProperties, type KeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { ApiError } from "../api/client";
import { whilePageShown } from "../api/pageLifecycle";
import type { TakeoverInput, TakeoverServerMessage } from "../api/types";
import { useClient } from "../context";
import { useBrowserInput } from "../hooks/useBrowserInput";
import { t, tMaybe } from "../i18n";
import { Button, ICON_STROKE, IconButton } from "../ui";
import { BrowserFrame } from "./BrowserFrame";
import { BrowserPip } from "./BrowserPip";
import "./browser.css";

type Frame = Extract<TakeoverServerMessage, { type: "frame" }>;
type Status = "connecting" | "open" | "ended" | "failed";

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
  const [frame, setFrame] = useState<Frame | null>(null);
  const [status, setStatus] = useState<Status>("connecting");
  const [ended, setEnded] = useState<string | null>(null);
  const [withheld, setWithheld] = useState(false);
  const [inputError, setInputError] = useState<string | null>(null);
  const [pip, setPip] = useState(false);
  const [pipMinimized, setPipMinimized] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const dialog = useRef<HTMLDivElement>(null);
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

  const input = useBrowserInput<HTMLDivElement>({ send, viewport: frame });

  async function finish(action: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (cause) {
      setError(cause instanceof ApiError ? cause.message : t("browser.requestFailed"));
    } finally {
      setBusy(false);
    }
  }

  const image = frame ? <BrowserFrame data={frame.data} /> : null;
  const state =
    status === "connecting"
      ? t("browser.takeover.connecting")
      : status === "failed"
        ? t("browser.takeover.unavailable")
        : status === "ended"
          ? (tMaybe(`browser.takeover.ended.${ended ?? ""}`) ?? t("browser.takeover.ended"))
          : withheld
            ? t("browser.takeover.withheld")
            : null;

  if (pip) {
    return (
      <BrowserPip
        label={t("browser.preview.label")}
        title={ticket.summary}
        status={status}
        live={!!image && status === "open" && !withheld}
        minimized={pipMinimized}
        onMinimizedChange={setPipMinimized}
        thumb={image ?? <Globe size={16} strokeWidth={ICON_STROKE} />}
        onExpand={() => setPip(false)}
        onClose={onClose}
      >
        {image ?? <p className="pip-state">{state ?? t("browser.takeover.waitingFrame")}</p>}
      </BrowserPip>
    );
  }

  // The remote screen forwards every key, Tab and Escape included, to the page; elsewhere in the
  // view Escape shrinks it to the PiP and Tab stays inside, as in a modal dialog.
  function dialogKey(event: KeyboardEvent<HTMLDivElement>) {
    if (event.target === input.surface || !dialog.current) return;
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
      aria-label={t("browser.takeover.title")}
      data-status={status}
      tabIndex={-1}
      onKeyDown={dialogKey}
    >
      <header className="takeover-head">
        <IconButton label={t("browser.pip.minimize")} onClick={() => setPip(true)}>
          <Minimize2 size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </IconButton>
        <h2>{t("browser.takeover.title")}</h2>
        <IconButton label={t("common.close")} onClick={onClose}>
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
        <p className="takeover-note">{t("browser.takeover.note")}</p>
        {state ? (
          <p className="takeover-state" role="status">
            {state}
          </p>
        ) : null}
        <div
          {...input.props}
          className="takeover-screen"
          tabIndex={0}
          aria-label={t("browser.takeover.screenLabel")}
          data-frame-width={frame?.width}
          data-frame-height={frame?.height}
          style={frame ? ({ "--frame-aspect": frame.width / frame.height } as CSSProperties) : undefined}
        >
          {image}
        </div>
        {inputError ? <p className="takeover-note">{t("browser.takeover.inputRefused", { code: inputError })}</p> : null}
        <div className="takeover-actions">
          <Button variant="primary" disabled={busy || status === "ended"} onClick={() => finish(() => client.ticketDone(ticket.id))}>
            {t("common.done")}
          </Button>
          <Button disabled={busy || status === "ended"} onClick={() => finish(() => client.rejectTicket(ticket.id))}>
            {t("browser.takeover.reject")}
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
