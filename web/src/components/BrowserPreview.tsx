import { MonitorPlay } from "lucide-react";
import { useEffect, useState } from "react";
import type { PreviewServerMessage } from "../api/types";
import { useClient } from "../context";
import { ICON_STROKE } from "../ui";
import { BrowserFrame } from "./BrowserFrame";
import { BrowserPip } from "./BrowserPip";
import { BrowserSheet, useSessionTabs } from "./BrowserSheet";
import "./browser.css";

const ENDED: Record<string, string> = {
  revoked: "This device's access was revoked.",
  session_stopped: "The session stopped.",
  session_deleted: "The session was deleted.",
};

export function BrowserPreview({ sessionId, privateSession = false }: { sessionId: string; privateSession?: boolean }) {
  const [visible, setVisible] = useState(false);
  if (privateSession) return null;
  return (
    <>
      <button type="button" className="tool-pill" aria-pressed={visible} onClick={() => setVisible(!visible)}>
        <MonitorPlay size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
        <span className="tool-pill-label">Browser preview</span>
      </button>
      {visible ? <Preview sessionId={sessionId} onClose={() => setVisible(false)} /> : null}
    </>
  );
}

function Preview({ sessionId, onClose }: { sessionId: string; onClose: () => void }) {
  const client = useClient();
  const [message, setMessage] = useState<PreviewServerMessage | null>(null);
  const [status, setStatus] = useState("connecting");
  const [expanded, setExpanded] = useState(false);
  const tabs = useSessionTabs(sessionId);
  useEffect(() => {
    const socket = client.browserPreview(sessionId);
    let finished = false;
    socket.onopen = () => {
      if (!finished) setStatus("open");
    };
    socket.onmessage = (event: MessageEvent) => {
      if (finished) return;
      let next: PreviewServerMessage;
      try {
        next = JSON.parse(String(event.data)) as PreviewServerMessage;
      } catch {
        return;
      }
      setMessage(next);
      if (next.type === "ended") {
        finished = true;
        setStatus("ended");
        socket.close();
      }
    };
    socket.onclose = () => {
      if (!finished) setStatus("failed");
    };
    return () => {
      finished = true;
      socket.close();
    };
  }, [client, sessionId]);
  const state =
    status === "failed"
      ? "Preview unavailable. A browse or steer device token is required."
      : status === "connecting"
        ? "Connecting…"
        : message?.type === "paused"
          ? "Preview paused while a person controls the browser."
          : message?.type === "withheld"
            ? "The screen is hidden because this tab has held a secret value."
            : message?.type === "ended"
              ? ENDED[message.reason]
              : message?.type === "no_tab"
                ? "No browser tab is active in this session."
                : message
                  ? null
                  : "Waiting for a frame…";
  const frame = message?.type === "frame" && status === "open" ? message : null;
  const liveTabId = message && "tab_id" in message ? message.tab_id : null;
  const liveTitle = tabs.tabs?.find((tab) => tab.tab_id === liveTabId)?.title;

  if (expanded) {
    return <BrowserSheet tabs={tabs} liveTabId={liveTabId} frame={frame?.data ?? null} state={state ?? null} onClose={() => setExpanded(false)} />;
  }
  return (
    <BrowserPip label="Live browser preview" title={liveTitle || "Browser"} status={status} onExpand={() => setExpanded(true)} onClose={onClose} className="browser-preview">
      {frame ? (
        <BrowserFrame data={frame.data} />
      ) : (
        <p className="pip-state" role="status">
          {state}
        </p>
      )}
    </BrowserPip>
  );
}
