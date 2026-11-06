import { useEffect, useState } from "react";
import { whilePageShown } from "../api/pageLifecycle";
import type { PreviewServerMessage } from "../api/types";
import { useClient } from "../context";
import { t, tMaybe } from "../i18n";

export type PreviewStatus = "idle" | "connecting" | "open" | "failed" | "ended";

export interface BrowserPreviewStream {
  status: PreviewStatus;
  message: PreviewServerMessage | null;
}

/**
 * The session's read-only screencast (`/browser/preview`): the latest server message while the
 * page is shown. It follows the tab the session's agents used last. `enabled` false keeps the
 * socket closed, so a collapsed card costs nothing.
 */
export function useBrowserPreview(sessionId: string, enabled = true): BrowserPreviewStream {
  const client = useClient();
  const [stream, setStream] = useState<BrowserPreviewStream>({ status: "idle", message: null });
  useEffect(() => {
    if (!enabled) {
      setStream({ status: "idle", message: null });
      return;
    }
    return whilePageShown(() => {
      const socket = client.browserPreview(sessionId);
      let finished = false;
      setStream((current) => ({ ...current, status: "connecting" }));
      socket.onopen = () => {
        if (!finished) setStream((current) => ({ ...current, status: "open" }));
      };
      socket.onmessage = (event: MessageEvent) => {
        if (finished) return;
        let next: PreviewServerMessage;
        try {
          next = JSON.parse(String(event.data)) as PreviewServerMessage;
        } catch {
          return;
        }
        if (next.type === "ended") {
          finished = true;
          setStream({ status: "ended", message: next });
          socket.close();
          return;
        }
        setStream({ status: "open", message: next });
      };
      socket.onclose = () => {
        if (!finished) setStream((current) => ({ ...current, status: "failed" }));
      };
      return () => {
        finished = true;
        socket.close();
      };
    });
  }, [client, sessionId, enabled]);
  return stream;
}

/** Why there is no picture right now, in words; null while frames arrive. */
export function previewNotice({ status, message }: BrowserPreviewStream): string | null {
  if (status === "failed") return t("browser.preview.unavailable");
  if (status === "connecting" || status === "idle") return null;
  switch (message?.type) {
    case "paused":
      return t("browser.preview.paused");
    case "withheld":
      return t("browser.preview.withheld");
    case "ended":
      return tMaybe(`browser.preview.ended.${message.reason}`) ?? t("browser.preview.ended");
    default:
      return null;
  }
}
