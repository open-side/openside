import { Check, ExternalLink } from "lucide-react";
import { useEffect, useState } from "react";
import { ApiError } from "../api/client";
import type { BrowserTab } from "../api/types";
import { useClient } from "../context";
import { Dialog, ICON_STROKE } from "../ui";
import { BrowserFrame } from "./BrowserFrame";
import { ImageViewer } from "./ImageViewer";
import "./browser.css";

/** Delay between completed tab-list requests, as in aside-relay's browser panel. */
const TABS_INTERVAL_MS = 2000;

export interface SessionTabs {
  /** This session's agent tabs; null until the first answer. */
  tabs: BrowserTab[] | null;
  disconnected: boolean;
}

/**
 * Polls `GET /v1/browser/tabs` while the page is visible and keeps the tabs this session opened.
 * At most one request is in flight: the next one is scheduled only after the previous settles,
 * and becoming visible again starts one only when nothing is pending. Each effect run owns its
 * loop, so a response that lands after unmount or a session switch is dropped by `stopped`.
 * Failed polls keep the last list; the next poll retries.
 */
export function useSessionTabs(sessionId: string): SessionTabs {
  const client = useClient();
  const [state, setState] = useState<SessionTabs>({ tabs: null, disconnected: false });
  useEffect(() => {
    let stopped = false;
    let pending = false;
    let timer: number | undefined;
    async function poll() {
      timer = undefined;
      pending = true;
      try {
        const { tabs } = await client.browserTabs();
        if (!stopped) setState({ tabs: tabs.filter((tab) => tab.owner_session_id === sessionId), disconnected: false });
      } catch (cause) {
        if (!stopped && cause instanceof ApiError && cause.code === "browser_disconnected") setState({ tabs: [], disconnected: true });
      } finally {
        pending = false;
      }
      // A hidden page stops here; `onVisibility` restarts the loop.
      if (!stopped && document.visibilityState !== "hidden") timer = window.setTimeout(() => void poll(), TABS_INTERVAL_MS);
    }
    const onVisibility = () => {
      if (document.visibilityState === "hidden") {
        window.clearTimeout(timer);
        timer = undefined;
      } else if (!pending && timer === undefined) {
        void poll();
      }
    };
    if (document.visibilityState !== "hidden") void poll();
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      stopped = true;
      window.clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [client, sessionId]);
  return state;
}

export interface BrowserSheetProps {
  tabs: SessionTabs;
  /** Tab the live stream shows, when known. */
  liveTabId: string | null;
  /** Base64 JPEG of the latest frame, or null with `state` explaining why. */
  frame: string | null;
  state: string | null;
  onClose: () => void;
}

/**
 * The browser sheet (aside-relay BrowserPanel): the tab the agent is on, titled with its page
 * title and URL, its live read-only picture (tap to zoom), and the other tabs this conversation
 * opened. Only actions with a real path exist: opening the page in the user's own browser.
 */
export function BrowserSheet({ tabs, liveTabId, frame, state, onClose }: BrowserSheetProps) {
  const [zoomed, setZoomed] = useState(false);
  const list = tabs.tabs ?? [];
  const live = list.find((tab) => tab.tab_id === liveTabId) ?? null;
  const title = live?.title || "Browser";
  const src = frame ? `data:image/jpeg;base64,${frame}` : null;

  return (
    <Dialog
      open
      size="wide"
      title={title}
      description={live ? <span className="browser-sheet-url" title={live.url}>{live.url}</span> : undefined}
      onClose={onClose}
      className="browser-sheet"
    >
      <div className="browser-sheet-body">
        <button type="button" className="browser-sheet-screen" aria-label="Zoom browser preview" disabled={!src} onClick={() => setZoomed(true)}>
          {frame ? (
            <BrowserFrame data={frame} />
          ) : (
            <span className="pip-state" role="status">
              {state}
            </span>
          )}
        </button>
        <div className="browser-sheet-meta">
          <p className="browser-sheet-note" role="status" data-live={src && !tabs.disconnected ? "" : undefined}>
            {tabs.disconnected ? "The browser is disconnected." : "Read-only live view. The agent keeps control."}
          </p>
          {live && /^https?:\/\//i.test(live.url) ? (
            <a className="ui-button ui-button--secondary browser-sheet-open" href={live.url} target="_blank" rel="noopener noreferrer external">
              <ExternalLink size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
              Open in your browser
            </a>
          ) : null}
        </div>
        {list.length ? (
          <section className="browser-sheet-tabs" aria-labelledby="browser-sheet-tabs-heading">
            <h4 id="browser-sheet-tabs-heading">Tabs in this conversation</h4>
            <ul>
              {list.map((tab) => (
                <li key={tab.tab_id} aria-current={tab.tab_id === liveTabId || undefined}>
                  <span className="browser-sheet-tab-text">
                    <strong>{tab.title || tab.url}</strong>
                    <small>{tab.url}</small>
                  </span>
                  {tab.tab_id === liveTabId ? (
                    <span className="browser-sheet-live">
                      <Check size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
                      Live
                    </span>
                  ) : null}
                </li>
              ))}
            </ul>
          </section>
        ) : null}
      </div>
      {zoomed && src ? (
        <ImageViewer label="Live browser preview" title={title} subtitle={live?.url} src={src} alt="Remote browser tab" onClose={() => setZoomed(false)} />
      ) : null}
    </Dialog>
  );
}
