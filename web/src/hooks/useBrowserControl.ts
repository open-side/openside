import { useCallback, useEffect, useRef, useState } from "react";
import type { TakeoverInput, TakeoverServerMessage } from "../api/types";
import { useClient } from "../context";

export type ControlFrame = Extract<TakeoverServerMessage, { type: "frame" }>;
export type ControlPage = Extract<TakeoverServerMessage, { type: "page" }>;
export type ControlStatus = "idle" | "connecting" | "open" | "ended" | "failed";

/** Input sent before the socket opens waits here; a burst beyond it is dropped, not buffered. */
const MAX_QUEUED = 32;

export interface BrowserControl {
  status: ControlStatus;
  /** Connecting or open: the agent is locked out of the tab. */
  active: boolean;
  frame: ControlFrame | null;
  page: ControlPage | null;
  withheld: boolean;
  /** Static code of the last refused input (`invalid_url`, `policy_denied`, …). */
  inputError: string | null;
  /** Why the daemon ended control, when it said. */
  ended: string | null;
  start(): void;
  /** Hands the tab back to the agent. */
  release(): void;
  /** Sends input, opening control first when it is not active. */
  send(input: TakeoverInput): void;
}

/**
 * The person's control socket for one agent tab (`GET …/browser/tabs/{tab}/control`), opened
 * only when asked: the agent is locked out of the tab for as long as it is open. Switching tabs,
 * leaving the page or unmounting closes it, which hands the tab back.
 */
export function useBrowserControl(sessionId: string, tabId: string | null): BrowserControl {
  const client = useClient();
  const socket = useRef<WebSocket | null>(null);
  // Input waiting for the socket to open; null once it is open and input goes straight out.
  const queue = useRef<string[] | null>([]);
  const [status, setStatus] = useState<ControlStatus>("idle");
  const [frame, setFrame] = useState<ControlFrame | null>(null);
  const [page, setPage] = useState<ControlPage | null>(null);
  const [withheld, setWithheld] = useState(false);
  const [inputError, setInputError] = useState<string | null>(null);
  const [ended, setEnded] = useState<string | null>(null);

  const release = useCallback(() => {
    const ws = socket.current;
    socket.current = null;
    queue.current = [];
    setStatus("idle");
    setFrame(null);
    setPage(null);
    setWithheld(false);
    ws?.close();
  }, []);

  const start = useCallback(() => {
    if (socket.current || !tabId) return;
    const ws = client.browserControl(sessionId, tabId);
    socket.current = ws;
    setStatus("connecting");
    setEnded(null);
    setInputError(null);
    ws.onopen = () => {
      if (socket.current !== ws) return;
      setStatus("open");
      for (const message of queue.current ?? []) ws.send(message);
      queue.current = null;
    };
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
        case "page":
          setPage(message);
          break;
        case "input_error":
          setInputError(message.code);
          break;
        case "ended":
          // The daemon reads nothing after this; later input opens a new lease instead.
          if (socket.current === ws) {
            socket.current = null;
            queue.current = [];
            setFrame(null);
            setPage(null);
            setWithheld(false);
            setStatus("ended");
          }
          setEnded(message.reason);
          ws.close();
          break;
      }
    };
    ws.onclose = () => {
      // A release or an `ended` message already reset the state; only an unannounced close
      // from the daemon or the network lands here.
      if (socket.current !== ws) return;
      socket.current = null;
      queue.current = [];
      setFrame(null);
      setPage(null);
      setWithheld(false);
      // A refused upgrade closes before it opens; the browser hides its HTTP status.
      setStatus((current) => (current === "open" ? "ended" : "failed"));
    };
  }, [client, sessionId, tabId]);

  const send = useCallback(
    (input: TakeoverInput) => {
      const message = JSON.stringify(input);
      const ws = socket.current;
      if (ws && queue.current === null) {
        if (ws.readyState === WebSocket.OPEN) ws.send(message);
        return;
      }
      if (!ws) start();
      if (queue.current && queue.current.length < MAX_QUEUED) queue.current.push(message);
    },
    [start],
  );

  // Another tab or conversation is another lease; hand this one back first.
  useEffect(() => {
    setStatus("idle");
    setEnded(null);
    setInputError(null);
    return release;
  }, [sessionId, tabId, release]);

  // A page leaving (or entering the back/forward cache) must not keep the agent locked out.
  useEffect(() => {
    window.addEventListener("pagehide", release);
    return () => window.removeEventListener("pagehide", release);
  }, [release]);

  return { status, active: status === "connecting" || status === "open", frame, page, withheld, inputError, ended, start, release, send };
}
