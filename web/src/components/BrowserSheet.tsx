import { ArrowLeft, ArrowRight, CornerDownLeft, ExternalLink, Globe, Hand, Maximize2, RotateCw, X } from "lucide-react";
import { useEffect, useRef, useState, type CSSProperties, type FormEvent } from "react";
import { ApiError } from "../api/client";
import type { BrowserTab, MyDevice } from "../api/types";
import { useClient } from "../context";
import { useBrowserControl } from "../hooks/useBrowserControl";
import { AGENT_VIEWPORT, MAX_TEXT_BYTES, useBrowserInput, utf8Chunks } from "../hooks/useBrowserInput";
import { originOf, useFavicon } from "../hooks/useFavicon";
import { t, tMaybe } from "../i18n";
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
  if (device.kind === "agent_client") return { kind: "blocked", reason: t("browser.sheet.access.agentClient") };
  if (Array.isArray(device.scopes) && !device.scopes.includes(scope) && !device.scopes.includes("admin"))
    return { kind: "blocked", reason: t("browser.sheet.access.noScope") };
  return { kind: "allowed" };
}

/** An address as typed: a bare host (with or without a port) gets `https://`; any other scheme is sent as typed. */
export function addressOf(text: string): string | null {
  const value = text.trim();
  if (!value) return null;
  const schemed = /^[a-z][a-z0-9+.-]*:/i.test(value) && !/^[^/:]+:\d+(?:[/?#]|$)/.test(value);
  return schemed ? value : `https://${value}`;
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
      <button type="button" className="browser-sheet-tab" aria-pressed={selected} aria-label={t("browser.sheet.tab.show", { name })} onClick={onSelect}>
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
            {t("browser.sheet.tab.live")}
          </span>
        ) : null}
      </button>
      {onCloseTab ? (
        <IconButton label={t("browser.sheet.tab.close", { name })} onClick={onCloseTab}>
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
  const takeoverAccess: Access = takeover && (!takeover.origin || originOf(takeover.origin) === null) ? { kind: "blocked", reason: t("browser.sheet.access.noOrigin") } : accessOf(device, "human_step");
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
  const title = control.page?.title || shown?.title || (tabId === liveTabId ? fallback.title || fallback.host : "") || t("browser.card.fallbackTitle");
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
      setAddressError(t("browser.sheet.inputError.invalid_url"));
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
    ? t("browser.sheet.note.disconnected")
    : control.status === "connecting"
      ? t("browser.sheet.note.connecting")
      : control.status === "open"
        ? t("browser.sheet.note.inControl")
        : control.status === "failed"
          ? t("browser.sheet.note.failed")
          : control.status === "ended"
            ? (tMaybe(`browser.sheet.ended.${control.ended ?? ""}`) ?? t("browser.sheet.ended"))
            : controllable && running
              ? t("browser.sheet.note.agentUsing")
              : interactive
                ? t("browser.sheet.note.interactive")
                : t("browser.sheet.note.readOnly");
  const refused = control.inputError ? (tMaybe(`browser.sheet.inputError.${control.inputError}`) ?? t("browser.sheet.inputError")) : null;
  const canGoBack = interactive && (control.page ? control.page.can_go_back : true);
  const canGoForward = interactive && (control.page ? control.page.can_go_forward : true);

  return (
    <Dialog open size="wide" title={title} description={host && host !== title ? host : undefined} onClose={onClose} className="browser-sheet">
      <div className="browser-sheet-body">
        {controllable ? (
          <form className="browser-sheet-toolbar" onSubmit={navigate} aria-label={t("browser.sheet.addressBar")}>
            <IconButton label={t("common.back")} disabled={!canGoBack} onClick={() => control.send({ type: "back" })}>
              <ArrowLeft size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
            </IconButton>
            <IconButton label={t("browser.sheet.forward")} disabled={!canGoForward} onClick={() => control.send({ type: "forward" })}>
              <ArrowRight size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
            </IconButton>
            <IconButton label={t("browser.sheet.reload")} disabled={!interactive} onClick={() => control.send({ type: "reload" })}>
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
              aria-label={t("browser.sheet.address")}
              placeholder={t("browser.sheet.addressPlaceholder")}
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
            aria-label={interactive ? t("browser.sheet.screenLabel") : t("browser.preview.label")}
            style={control.frame ? ({ "--frame-aspect": control.frame.width / control.frame.height } as CSSProperties) : undefined}
          >
            {picture ? (
              <BrowserFrame data={picture} />
            ) : (
              <span className="pip-state" role="status">
                {control.active ? t("browser.takeover.waitingFrame") : control.withheld ? null : tabId === liveTabId ? state : t("browser.sheet.takeControlToSee")}
              </span>
            )}
          </div>
          <IconButton label={t("browser.sheet.zoom")} className="browser-sheet-zoom" disabled={!src} onClick={() => setZoomed(true)}>
            <Maximize2 size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
          </IconButton>
        </div>
        {control.withheld ? <p className="browser-sheet-takeover-note">{t("browser.takeover.withheld")}</p> : null}
        {control.status === "open" ? (
          <form className="browser-sheet-type" onSubmit={sendText} aria-label={t("browser.sheet.typeForm")}>
            <input
              type="text"
              aria-label={t("browser.sheet.typeLabel")}
              placeholder={t("browser.sheet.typeForm")}
              autoCapitalize="off"
              autoCorrect="off"
              value={typed}
              onChange={(event) => setTyped(event.target.value)}
            />
            <Button type="submit" className="browser-sheet-action" disabled={!typed}>
              {t("browser.sheet.type")}
            </Button>
            <IconButton
              label={t("browser.sheet.pressEnter")}
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
              {t("browser.sheet.takeControl")}
            </Button>
          ) : null}
          {takeover && takeoverAccess.kind === "blocked" ? <p className="browser-sheet-takeover-note">{takeoverAccess.reason}</p> : null}
          {controllable && running && !control.active ? (
            <Button variant="primary" className="browser-sheet-action" onClick={control.start}>
              <Hand size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
              {t("browser.sheet.takeControl")}
            </Button>
          ) : null}
          {control.active ? (
            <Button variant="primary" className="browser-sheet-action" onClick={control.release}>
              {t("browser.sheet.handBack")}
            </Button>
          ) : null}
          {!takeover && controlAccess.kind === "blocked" ? <p className="browser-sheet-takeover-note">{controlAccess.reason}</p> : null}
          {url && /^https?:\/\//i.test(url) ? (
            <a className="ui-button ui-button--secondary browser-sheet-action browser-sheet-open" href={url} target="_blank" rel="noopener noreferrer external">
              <ExternalLink size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
              {t("browser.sheet.openExternal")}
            </a>
          ) : null}
        </div>
        {list.length ? (
          <section className="browser-sheet-tabs" aria-labelledby="browser-sheet-tabs-heading">
            <h4 id="browser-sheet-tabs-heading" className="section-heading">
              {t("browser.sheet.tabsHeading")}
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
        <ImageViewer label={t("browser.sheet.zoomLabel")} title={title} subtitle={url ?? undefined} src={src} alt={t("browser.sheet.zoomAlt")} onClose={() => setZoomed(false)} />
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
