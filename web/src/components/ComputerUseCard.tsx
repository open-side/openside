import { AppWindow, Square } from "lucide-react";
import { useEffect, useState, type CSSProperties } from "react";
import { openComputerUseStream } from "../api/sse";
import type { ComputerUseConnection, ComputerUseFrame } from "../api/types";
import { useClient } from "../context";
import { t } from "../i18n";
import type { SessionView } from "../state/session";
import { ICON_STROKE } from "../ui";
import "./browser.css";

/**
 * The desktop app this chat is connected to: the frame the agent last saw, its pointer, and a
 * Stop that ends the connection at once. Shown only while an app is connected.
 */
export function ComputerUseCard({ sessionId, view }: { sessionId: string; view: SessionView }) {
  const client = useClient();
  // Private chats never see the desktop; the daemon refuses them too.
  const active = !view.incognito && view.items.some((item) => item.type === "tool" && item.name.startsWith("computer_use_"));
  const [connection, setConnection] = useState<ComputerUseConnection | null>(null);
  const [frame, setFrame] = useState<ComputerUseFrame | null>(null);
  const [stopping, setStopping] = useState(false);

  useEffect(() => {
    if (!active) return;
    const stream = openComputerUseStream({
      client,
      sessionId,
      onEvent: (event) => {
        if (event.type === "state") {
          setConnection(event.connection ?? null);
          if (!event.connection) setFrame(null);
        } else if (event.type === "frame" && event.mime === "image/png" && event.width > 0 && event.height > 0 && /^[A-Za-z0-9+/]+=*$/.test(event.data)) {
          setFrame(event);
        }
      },
    });
    return () => stream.close();
  }, [active, client, sessionId]);

  useEffect(() => setStopping(false), [connection?.id]);

  if (!connection || view.incognito) return null;
  const shown = frame && frame.connection === connection.id ? frame : null;
  const cursor = shown && connection.cursor ? connection.cursor : null;
  const phase = t(`thread.computerUse.phase.${connection.phase}`);

  return (
    <section className="browser-card computer-use-card" aria-label={t("thread.computerUse.label", { app: connection.app })} data-tone="running">
      <div className="browser-card-screen">
        {shown ? (
          <span className="browser-card-frame" style={{ "--frame-ratio": shown.width / shown.height } as CSSProperties}>
            <img src={`data:${shown.mime};base64,${shown.data}`} alt={t("thread.computerUse.frame", { app: connection.app })} draggable={false} />
            {cursor ? (
              <span
                className="computer-use-cursor"
                data-held={cursor.held || undefined}
                aria-hidden="true"
                style={{ left: `${(cursor.x / shown.width) * 100}%`, top: `${(cursor.y / shown.height) * 100}%` }}
              />
            ) : null}
          </span>
        ) : (
          <span className="browser-card-state" role="status">
            {t("thread.computerUse.waiting")}
          </span>
        )}
      </div>
      <div className="browser-card-bar">
        <span className="browser-card-site">
          <AppWindow size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
          <span className="browser-card-site-title" title={connection.title}>
            {connection.app}
          </span>
        </span>
        <p className="browser-card-status" data-tone="running" role="status">
          <span className="live-dot" data-live={!!shown || undefined} aria-hidden="true" />
          <span className="browser-card-text">{phase}</span>
        </p>
        <span className="browser-card-controls">
          <button
            type="button"
            className="browser-card-stop"
            disabled={stopping}
            onClick={() => {
              setStopping(true);
              client.stopComputerUse(sessionId).then(
                (result) => {
                  if (result.stopped) setConnection(null);
                },
                () => setStopping(false),
              );
            }}
          >
            <Square size={12} strokeWidth={ICON_STROKE} aria-hidden="true" />
            {t("thread.computerUse.stop")}
          </button>
        </span>
      </div>
    </section>
  );
}
