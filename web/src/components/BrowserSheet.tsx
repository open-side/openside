import { ExternalLink, Globe, Hand } from "lucide-react";
import { useEffect, useState } from "react";
import { ApiError } from "../api/client";
import type { BrowserTab } from "../api/types";
import { useClient } from "../context";
import { originOf, useFavicon } from "../hooks/useFavicon";
import { Button, Dialog, ICON_STROKE } from "../ui";
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
  /** Title and host of the agent's page from its browser state, used until the tab ledger answers. */
  fallback: { title: string; host: string };
  /** The pending takeover request the person can finish here; absent when none is pending. */
  takeover?: { origin: string | null; onStart: () => void } | undefined;
  onClose: () => void;
}

type TakeoverAccess = { kind: "checking" } | { kind: "allowed" } | { kind: "blocked"; reason: string };

/**
 * Whether this device may take over the tab. The daemon requires a person's own device with
 * `human_step` (or `admin`); an agent client or a narrower pairing is told why instead of being
 * offered a button that would only fail. When the device cannot be looked up (an older daemon,
 * a network error) the button stays: the daemon still refuses, and the remote control says so.
 */
function useTakeoverAccess(origin: string | null | undefined): TakeoverAccess {
  const client = useClient();
  const wanted = origin !== undefined;
  const valid = !!origin && originOf(origin) !== null;
  const [device, setDevice] = useState<TakeoverAccess>({ kind: "checking" });
  useEffect(() => {
    if (!wanted || !valid) return;
    let current = true;
    client.myDevice().then(
      (me) => {
        if (!current) return;
        if (me.kind === "agent_client") setDevice({ kind: "blocked", reason: "Only a device you paired as yourself can take control of the browser." });
        else if (Array.isArray(me.scopes) && !me.scopes.includes("human_step") && !me.scopes.includes("admin"))
          setDevice({ kind: "blocked", reason: "This device wasn't given permission to take control of the browser." });
        else setDevice({ kind: "allowed" });
      },
      () => current && setDevice({ kind: "allowed" }),
    );
    return () => {
      current = false;
    };
  }, [client, wanted, valid]);
  if (wanted && !valid) return { kind: "blocked", reason: "This request doesn't say which page to take over." };
  return device;
}

function TabRow({ tab, live }: { tab: BrowserTab; live: boolean }) {
  const icon = useFavicon(originOf(tab.url));
  return (
    <li aria-current={live || undefined}>
      <span className="browser-sheet-favicon" aria-hidden="true">
        {icon ? <img src={icon} alt="" width={16} height={16} /> : <Globe size={16} strokeWidth={ICON_STROKE} />}
      </span>
      <span className="browser-sheet-tab-text">
        <strong>{tab.title || tab.url}</strong>
        <small>{tab.url}</small>
      </span>
      {live ? (
        <span className="browser-sheet-live">
          <span className="live-dot" aria-hidden="true" />
          Live
        </span>
      ) : null}
    </li>
  );
}

/**
 * The browser sheet (aside-relay BrowserPanel): the tab the agent is on, titled with its page
 * title and site, its URL, its live read-only picture (tap to zoom), and the other tabs this
 * conversation opened. Only actions with a real path exist: opening the page in the user's own
 * browser and taking over a tab the agent asked the person to finish. There is no address bar
 * or back/forward: the daemon has no endpoint to drive the tab from here.
 */
export function BrowserSheet({ tabs, liveTabId, frame, state, fallback, takeover, onClose }: BrowserSheetProps) {
  const [zoomed, setZoomed] = useState(false);
  const list = tabs.tabs ?? [];
  const live = list.find((tab) => tab.tab_id === liveTabId) ?? null;
  const title = live?.title || fallback.title || fallback.host || "Browser";
  const host = (live ? hostOf(live.url) : null) ?? fallback.host;
  const src = frame ? `data:image/jpeg;base64,${frame}` : null;
  const access = useTakeoverAccess(takeover?.origin);

  return (
    <Dialog open size="wide" title={title} description={host && host !== title ? host : undefined} onClose={onClose} className="browser-sheet">
      <div className="browser-sheet-body">
        {live ? (
          <p className="browser-sheet-url" title={live.url}>
            {live.url}
          </p>
        ) : null}
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
          {takeover && access.kind === "allowed" ? (
            <Button variant="primary" className="browser-sheet-action" onClick={takeover.onStart}>
              <Hand size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
              Take control
            </Button>
          ) : null}
          {takeover && access.kind === "blocked" ? <p className="browser-sheet-takeover-note">{access.reason}</p> : null}
          {live && /^https?:\/\//i.test(live.url) ? (
            <a className="ui-button ui-button--secondary browser-sheet-action browser-sheet-open" href={live.url} target="_blank" rel="noopener noreferrer external">
              <ExternalLink size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
              Open in your browser
            </a>
          ) : null}
        </div>
        {list.length ? (
          <section className="browser-sheet-tabs" aria-labelledby="browser-sheet-tabs-heading">
            <h4 id="browser-sheet-tabs-heading" className="section-heading">
              Tabs in this conversation
            </h4>
            <ul>
              {list.map((tab) => (
                <TabRow key={tab.tab_id} tab={tab} live={tab.tab_id === liveTabId} />
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

function hostOf(url: string): string | null {
  try {
    return new URL(url).host || null;
  } catch {
    return null;
  }
}
