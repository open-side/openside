import { ArrowLeft, ArrowRight, CornerDownLeft, ExternalLink, Globe, Hand, Maximize2, RotateCw, X } from "lucide-react";
import { useEffect, useRef, useState, type CSSProperties, type FormEvent } from "react";
import { ApiError } from "../api/client";
import type { BrowserTab, MyDevice } from "../api/types";
import { useClient } from "../context";
import { useBrowserControl } from "../hooks/useBrowserControl";
import { AGENT_VIEWPORT, MAX_TEXT_BYTES, useBrowserInput, utf8Chunks } from "../hooks/useBrowserInput";
import { originOf, useFavicon } from "../hooks/useFavicon";
import { Button, Dialog, ICON_STROKE, IconButton } from "../ui";
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
  sessionId: string;
  /** The agent is working: control starts only from Take control. */
  running: boolean;
  tabs: SessionTabs;
  /** Tab the live stream shows, when known. */
  liveTabId: string | null;
  /** Tab the opener asked for; null follows the agent's tab. */
  selectedTabId?: string | null;
  /** Base64 JPEG of the latest frame, or null with `state` explaining why. */
  frame: string | null;
  state: string | null;
  /** Title and host of the agent's page from its browser state, used until the tab ledger answers. */
  fallback: { title: string; host: string };
  /** The pending takeover request the person can finish here; absent when none is pending. */
  takeover?: { origin: string | null; onStart: () => void } | undefined;
  onClose: () => void;
}

type Access = { kind: "checking" } | { kind: "allowed" } | { kind: "blocked"; reason: string };

/**
 * This device's record, or "unknown" when it cannot be looked up (an older daemon, a network
 * error): controls then stay offered, the daemon still refuses, and the sheet says so.
 */
function useMyDevice(): MyDevice | "unknown" | null {
  const client = useClient();
  const [device, setDevice] = useState<MyDevice | "unknown" | null>(null);
  useEffect(() => {
    let current = true;
    client.myDevice().then(
      (me) => current && setDevice(me),
      () => current && setDevice("unknown"),
    );
    return () => {
      current = false;
    };
  }, [client]);
  return device;
}

/**
 * Whether this device may finish a takeover request (a person's device with `human_step`) or
 * drive a tab itself (a person's device with `steer`); `admin` covers both. An agent client or a
 * narrower pairing is told why instead of being offered a button that would only fail.
 */
function accessOf(device: MyDevice | "unknown" | null, scope: "human_step" | "steer"): Access {
  if (device === null) return { kind: "checking" };
  if (device === "unknown") return { kind: "allowed" };
  if (device.kind === "agent_client") return { kind: "blocked", reason: "Only a device you paired as yourself can take control of the browser." };
  if (Array.isArray(device.scopes) && !device.scopes.includes(scope) && !device.scopes.includes("admin"))
    return { kind: "blocked", reason: "This device wasn't given permission to take control of the browser." };
  return { kind: "allowed" };
}

const END_LABEL: Record<string, string> = {
  idle: "Handed back to the agent after inactivity.",
  revoked: "This device's access was revoked.",
  tab_unavailable: "The tab was closed.",
};

const INPUT_ERROR: Record<string, string> = {
  invalid_url: "Only http and https addresses can be opened here.",
  policy_denied: "Your browsing policy blocks that site.",
  approval_required: "That site needs approval. Type its address to open it.",
  connection_denied: "That site isn't allowed in this conversation.",
  navigation_failed: "The page didn't load.",
  no_history: "There's no page to go to.",
};

/** An address as typed: a bare host gets `https://`; anything but http(s) is refused here. */
function addressOf(text: string): string | null {
  const value = text.trim();
  if (!value) return null;
  const url = /^[a-z][a-z0-9+.-]*:\/\//i.test(value) ? value : `https://${value}`;
  return /^https?:\/\//i.test(url) ? url : null;
}

function TabRow({
  tab,
  live,
  selected,
  onSelect,
  onCloseTab,
}: {
  tab: BrowserTab;
  live: boolean;
  selected: boolean;
  onSelect: () => void;
  onCloseTab: (() => void) | null;
}) {
  const icon = useFavicon(originOf(tab.url));
  const name = tab.title || tab.url;
  return (
    <li aria-current={selected || undefined}>
      <button type="button" className="browser-sheet-tab" aria-pressed={selected} aria-label={`Show ${name}`} onClick={onSelect}>
        <span className="browser-sheet-favicon" aria-hidden="true">
          {icon ? <img src={icon} alt="" width={16} height={16} /> : <Globe size={16} strokeWidth={ICON_STROKE} />}
        </span>
        <span className="browser-sheet-tab-text">
          <strong>{name}</strong>
          <small>{tab.url}</small>
        </span>
        {live ? (
          <span className="browser-sheet-live">
            <span className="live-dot" aria-hidden="true" />
            Live
          </span>
        ) : null}
      </button>
      {onCloseTab ? (
        <IconButton label={`Close ${name}`} onClick={onCloseTab}>
          <X size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </IconButton>
      ) : null}
    </li>
  );
}

/**
 * The browser sheet (aside-relay BrowserPanel): a tab of this conversation, titled with its page
 * title and site, its live picture (zoomable), and the other tabs it opened. A person's device
 * with `steer` drives the tab here: an address bar, back, forward, reload, switching and closing
 * tabs, and taps, scrolls and typing on the picture. Driving takes the tab from the agent until
 * Hand back (or inactivity); while the agent runs, Take control comes first. A pending takeover
 * request is finished in the full-screen remote control instead.
 */
export function BrowserSheet({ sessionId, running, tabs, liveTabId, selectedTabId = null, frame, state, fallback, takeover, onClose }: BrowserSheetProps) {
  const [zoomed, setZoomed] = useState(false);
  const [picked, setPicked] = useState<string | null>(selectedTabId);
  // An opener naming another tab while the sheet is open (a tab card) switches to it.
  const [opened, setOpened] = useState(selectedTabId);
  if (selectedTabId !== opened) {
    setOpened(selectedTabId);
    if (selectedTabId) setPicked(selectedTabId);
  }
  const [address, setAddress] = useState<string | null>(null);
  const [addressError, setAddressError] = useState<string | null>(null);
  const [typed, setTyped] = useState("");
  const list = tabs.tabs ?? [];
  // A picked tab that closed falls back to the agent's.
  const tabId = (picked && (tabs.tabs === null || list.some((tab) => tab.tab_id === picked)) ? picked : null) ?? liveTabId;
  const shown = list.find((tab) => tab.tab_id === tabId) ?? null;
  const device = useMyDevice();
  const takeoverAccess: Access = takeover && (!takeover.origin || originOf(takeover.origin) === null) ? { kind: "blocked", reason: "This request doesn't say which page to take over." } : accessOf(device, "human_step");
  const controlAccess = accessOf(device, "steer");
  const control = useBrowserControl(sessionId, tabId);
  // A takeover request owns its tab; the daemon refuses control of it until the request ends.
  const controllable = !takeover && controlAccess.kind === "allowed" && !!tabId && !tabs.disconnected;
  const interactive = controllable && (control.active || !running);
  // The preview's last picture stands in until the control stream's first frame arrives.
  const picture = control.frame?.data ?? (tabId === liveTabId ? frame : null);
  const viewport = control.frame ? { width: control.frame.width, height: control.frame.height } : AGENT_VIEWPORT;
  const input = useBrowserInput<HTMLDivElement>({ send: interactive && picture ? control.send : null, viewport });
  // What to do once the sheet has switched to a tab: open control there, or close it.
  const pending = useRef<{ tab: string; close: boolean } | null>(null);
  const { start, send } = control;
  useEffect(() => {
    const next = pending.current;
    if (!next || next.tab !== tabId) return;
    pending.current = null;
    if (next.close) send({ type: "close_tab" });
    else start();
  }, [tabId, start, send]);

  const url = control.page?.url ?? shown?.url ?? null;
  const title = control.page?.title || shown?.title || (tabId === liveTabId ? fallback.title || fallback.host : "") || "Browser";
  const host = (url ? hostOf(url) : null) ?? fallback.host;
  const src = picture ? `data:image/jpeg;base64,${picture}` : null;

  function select(tab: string) {
    setAddress(null);
    setAddressError(null);
    if (tab === tabId) return;
    if (interactive && !running) pending.current = { tab, close: false };
    setPicked(tab);
  }

  function closeTab(tab: string) {
    if (tab === tabId) {
      control.send({ type: "close_tab" });
      return;
    }
    pending.current = { tab, close: true };
    setPicked(tab);
  }

  function navigate(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const target = addressOf(address ?? url ?? "");
    if (!target) {
      setAddressError(INPUT_ERROR.invalid_url ?? null);
      return;
    }
    setAddressError(null);
    setAddress(null);
    control.send({ type: "navigate", url: target });
  }

  function sendText(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    for (const chunk of utf8Chunks(typed, MAX_TEXT_BYTES)) control.send({ type: "text", text: chunk });
    setTyped("");
  }

  const note = tabs.disconnected
    ? "The browser is disconnected."
    : control.status === "connecting"
      ? "Taking control…"
      : control.status === "open"
        ? "You're in control. The agent waits until you hand the tab back."
        : control.status === "failed"
          ? "Couldn't connect to take control. Another device may be using this tab, the agent may be waiting on a request for you, or this conversation browses privately."
          : control.status === "ended"
            ? (END_LABEL[control.ended ?? ""] ?? "Control ended. The agent can use the tab again.")
            : controllable && running
              ? "The agent is using this tab. Take control to use it yourself; the agent waits until you hand it back."
              : interactive
                ? "Tap the page or use the address bar. The agent waits while you browse."
                : "Read-only live view. The agent keeps control.";
  const refused = control.inputError ? (INPUT_ERROR[control.inputError] ?? "That didn't work.") : null;
  const canGoBack = interactive && (control.page ? control.page.can_go_back : true);
  const canGoForward = interactive && (control.page ? control.page.can_go_forward : true);

  return (
    <Dialog open size="wide" title={title} description={host && host !== title ? host : undefined} onClose={onClose} className="browser-sheet">
      <div className="browser-sheet-body">
        {controllable ? (
          <form className="browser-sheet-toolbar" onSubmit={navigate} aria-label="Address bar">
            <IconButton label="Back" disabled={!canGoBack} onClick={() => control.send({ type: "back" })}>
              <ArrowLeft size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
            </IconButton>
            <IconButton label="Forward" disabled={!canGoForward} onClick={() => control.send({ type: "forward" })}>
              <ArrowRight size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
            </IconButton>
            <IconButton label="Reload" disabled={!interactive} onClick={() => control.send({ type: "reload" })}>
              <RotateCw size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
            </IconButton>
            <input
              className="browser-sheet-address"
              type="text"
              inputMode="url"
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
              enterKeyHint="go"
              aria-label="Address"
              placeholder="Search or type an address"
              disabled={!interactive}
              value={address ?? url ?? ""}
              onChange={(event) => setAddress(event.target.value)}
              onBlur={() => address === url && setAddress(null)}
            />
          </form>
        ) : url ? (
          <p className="browser-sheet-url" title={url}>
            {url}
          </p>
        ) : null}
        {addressError ?? refused ? (
          <p className="browser-sheet-takeover-note" role="alert">
            {addressError ?? refused}
          </p>
        ) : null}
        <div className="browser-sheet-stage">
          <div
            {...input.props}
            className="browser-sheet-screen"
            data-interactive={(interactive && !!picture) || undefined}
            tabIndex={interactive && picture ? 0 : -1}
            aria-label={interactive ? "Remote browser screen; tap to interact, type to send keys" : "Browser preview"}
            style={control.frame ? ({ "--frame-aspect": control.frame.width / control.frame.height } as CSSProperties) : undefined}
          >
            {picture ? (
              <BrowserFrame data={picture} />
            ) : (
              <span className="pip-state" role="status">
                {control.active ? "Waiting for a frame…" : control.withheld ? null : tabId === liveTabId ? state : "Take control to see this tab."}
              </span>
            )}
          </div>
          <IconButton label="Zoom browser preview" className="browser-sheet-zoom" disabled={!src} onClick={() => setZoomed(true)}>
            <Maximize2 size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
          </IconButton>
        </div>
        {control.withheld ? <p className="browser-sheet-takeover-note">The screen is hidden because the page holds a secret value. Input still works.</p> : null}
        {control.status === "open" ? (
          <form className="browser-sheet-type" onSubmit={sendText} aria-label="Type into the page">
            <input
              type="text"
              aria-label="Text to type"
              placeholder="Type into the page"
              autoCapitalize="off"
              autoCorrect="off"
              value={typed}
              onChange={(event) => setTyped(event.target.value)}
            />
            <Button type="submit" className="browser-sheet-action" disabled={!typed}>
              Type
            </Button>
            <IconButton
              label="Press Enter"
              onClick={() => {
                control.send({ type: "key", event: "down", key: "Enter", code: "Enter", modifiers: 0 });
                control.send({ type: "key", event: "up", key: "Enter", code: "Enter", modifiers: 0 });
              }}
            >
              <CornerDownLeft size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
            </IconButton>
          </form>
        ) : null}
        <div className="browser-sheet-meta">
          <p className="browser-sheet-note" role="status" data-live={(src && !tabs.disconnected) || control.status === "open" ? "" : undefined}>
            {note}
          </p>
          {takeover && takeoverAccess.kind === "allowed" ? (
            <Button variant="primary" className="browser-sheet-action" onClick={takeover.onStart}>
              <Hand size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
              Take control
            </Button>
          ) : null}
          {takeover && takeoverAccess.kind === "blocked" ? <p className="browser-sheet-takeover-note">{takeoverAccess.reason}</p> : null}
          {controllable && running && !control.active ? (
            <Button variant="primary" className="browser-sheet-action" onClick={control.start}>
              <Hand size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
              Take control
            </Button>
          ) : null}
          {control.active ? (
            <Button variant="primary" className="browser-sheet-action" onClick={control.release}>
              Hand back
            </Button>
          ) : null}
          {!takeover && controlAccess.kind === "blocked" ? <p className="browser-sheet-takeover-note">{controlAccess.reason}</p> : null}
          {url && /^https?:\/\//i.test(url) ? (
            <a className="ui-button ui-button--secondary browser-sheet-action browser-sheet-open" href={url} target="_blank" rel="noopener noreferrer external">
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
                <TabRow
                  key={tab.tab_id}
                  tab={tab}
                  live={tab.tab_id === liveTabId}
                  selected={tab.tab_id === tabId}
                  onSelect={() => select(tab.tab_id)}
                  onCloseTab={interactive && (!running || tab.tab_id === tabId) ? () => closeTab(tab.tab_id) : null}
                />
              ))}
            </ul>
          </section>
        ) : null}
      </div>
      {zoomed && src ? (
        <ImageViewer label="Live browser preview" title={title} subtitle={url ?? undefined} src={src} alt="Remote browser tab" onClose={() => setZoomed(false)} />
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
