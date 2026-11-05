import { Check, Globe, Maximize2, Minimize2, Square, TriangleAlert, X } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { useSearchParams } from "react-router";
import { AGENT_VIEWPORT as VIEWPORT } from "../hooks/useBrowserInput";
import { previewNotice, useBrowserPreview } from "../hooks/useBrowserPreview";
import { pendingTickets, type BrowserStateView, type SessionView, type TimelineItem } from "../state/session";
import { ICON_STROKE } from "../ui";
import { useAgentTabsPanel } from "./agentTabsStore";
import { useBrowserDock, type BrowserDock } from "./browserDock";
import { BrowserFrame } from "./BrowserFrame";
import { BrowserPip } from "./BrowserPip";
import { BrowserSheet, useSessionTabs } from "./BrowserSheet";
import { TakeoverView } from "./TakeoverView";
import { firstSentence } from "./taskStatus";
import { opensPage, safeFavicon } from "./toolCopy";
import { lastHost, statusPhrase } from "./toolStatus";
import "./browser.css";

/**
 * A run shorter than this is not announced as done: replaying a conversation's history on
 * load flips the status through running and back within milliseconds.
 */
const MIN_RUN_MS = 1000;
/** How long the done chip shows on the open card before it folds to the slim pill. */
const COLLAPSE_MS = 4000;
const RIPPLE_MS = 700;

type ToolItem = Extract<TimelineItem, { type: "tool" }>;
type Click = { id: string; x: number; y: number };

/** Tool calls since the person's latest message: the run the card narrates. */
function currentTools(items: readonly TimelineItem[]): ToolItem[] {
  const tools: ToolItem[] = [];
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const item = items[index]!;
    if (item.type === "user") break;
    if (item.type === "tool") tools.unshift(item);
  }
  return tools;
}

/**
 * The message the live card sits under: the person's message in front of the agent's latest
 * browser call, so a later browsing turn brings the card down to its own message. `null` when
 * the log has no such message.
 */
export function browserAnchor(items: readonly TimelineItem[]): string | null {
  const start = items.findLastIndex((item) => item.type === "tool" && item.name.toLowerCase().startsWith("browser"));
  for (let index = start; index >= 0; index -= 1) {
    const item = items[index]!;
    if (item.type === "user" && item.author_kind != null && item.author_kind !== "internal") return item.id;
  }
  return null;
}

/** A coordinate click the agent is making right now; `browser_act` targets refs and has no point. */
function runningClick(items: readonly TimelineItem[]): Click | null {
  const tool = items.findLast((item): item is ToolItem => item.type === "tool" && item.name === "browser_pointer");
  if (!tool || tool.done || !tool.args || typeof tool.args !== "object") return null;
  const { action, x, y } = tool.args as Record<string, unknown>;
  return action === "click" && typeof x === "number" && typeof y === "number" ? { id: tool.id, x, y } : null;
}

export interface BrowserPreviewProps {
  sessionId: string;
  view: SessionView;
  /** Brings the request waiting on the person into view. */
  onShowPending: () => void;
  /** Asks to stop the running agent (the caller confirms first). */
  onCancel: () => void;
}

/**
 * The live browser of an agent that browses itself: an inline card (the tab's read-only
 * screencast, what the agent is doing, "Action needed" while a request waits, a done line after
 * the run, which then folds the card to a slim pill), a floating PiP while that card is out of
 * view, and the browser sheet. Background Tasks show their own tab in their Task unit
 * (`TaskUnit`). Private sessions never show it. `?browser=open` opens the sheet once.
 */
export function BrowserPreview(props: BrowserPreviewProps) {
  // A private session never touches the dock, so its stored state stays untouched too.
  return props.view.incognito ? null : <Docked {...props} />;
}

function Docked({ sessionId, view, onShowPending, onCancel }: BrowserPreviewProps) {
  const dock = useBrowserDock(sessionId);
  const panel = useAgentTabsPanel(sessionId);
  const browserCalls = useMemo(() => view.items.filter((item) => item.type === "tool" && item.name.toLowerCase().startsWith("browser")).length, [view.items]);
  // Closing the idle card hides it until the agent touches the browser again.
  const [dismissedAt, setDismissedAt] = useState(-1);

  const [params, setParams] = useSearchParams();
  // Waits for the first events: until then `incognito` is unknown and a private session's link
  // would open its sheet.
  const linked = params.get("browser") === "open" && view.lastSeq > 0;
  const { setSheet } = dock;
  useEffect(() => {
    if (!linked) return;
    setSheet(true);
    // Dropped once handled, so Back and a reload show the conversation.
    setParams(
      (current) => {
        current.delete("browser");
        return current;
      },
      { replace: true },
    );
  }, [linked, setSheet, setParams]);

  const cardShown = browserCalls > dismissedAt && browserCalls > 0;
  // The PiP stands in for the front agent's card. Where only Tasks browse, each Task unit already
  // shows its tab inline, so the PiP waits for Show browser instead of floating over the thread.
  // The desktop Agent Tabs column already shows the tabs, so the PiP waits while it is open.
  const floatable =
    dock.pip !== "closed" && !panel.shown && (cardShown || (dock.summoned && Object.keys(view.browserStates).length > 0));
  // Nothing to show: keep the preview socket and the tab polling closed.
  if (!cardShown && !floatable && !dock.sheet) return null;
  return (
    <Live
      sessionId={sessionId}
      view={view}
      onShowPending={onShowPending}
      onCancel={onCancel}
      dock={dock}
      cardShown={cardShown}
      floatable={floatable}
      onDismiss={() => {
        setDismissedAt(browserCalls);
        // Closing the card closes the browser; the header's Show browser brings the PiP back.
        dock.setPip("closed");
      }}
    />
  );
}

function Live({
  sessionId,
  view,
  onShowPending,
  onCancel,
  dock,
  cardShown,
  floatable,
  onDismiss,
}: BrowserPreviewProps & { dock: BrowserDock; cardShown: boolean; floatable: boolean; onDismiss: () => void }) {
  const stream = useBrowserPreview(sessionId);
  const [minimized, setMinimized] = useState(false);
  const [done, setDone] = useState(false);
  const [ripple, setRipple] = useState<Click | null>(null);
  const [element, setElement] = useState<HTMLElement | null>(null);
  const [away, setAway] = useState(false);
  const [takingOver, setTakingOver] = useState(false);
  const tabs = useSessionTabs(sessionId);
  const running = view.status === "running";
  const pending = pendingTickets(view);
  const waiting = pending.length > 0;
  const takeoverTicket = pending.find((ticket) => ticket.kind === "takeover");
  const runStart = useRef<number | null>(running ? Date.now() : null);

  useEffect(() => {
    if (running) {
      runStart.current ??= Date.now();
      setDone(false);
      return;
    }
    const started = runStart.current;
    runStart.current = null;
    if (started === null || view.status !== "idle" || Date.now() - started < MIN_RUN_MS) return;
    setDone(true);
    const timer = window.setTimeout(() => setMinimized(true), COLLAPSE_MS);
    return () => window.clearTimeout(timer);
    // Only the run boundary matters; the status and clock are read at that moment.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [running]);

  const click = runningClick(view.items);
  useEffect(() => {
    if (!click) return;
    setRipple(click);
    const timer = window.setTimeout(() => setRipple(null), RIPPLE_MS);
    return () => window.clearTimeout(timer);
    // A click is announced once, when its call starts.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [click?.id]);

  // Scrolled away from the card: the picture follows as the floating PiP. While the agent works
  // and the card is scrolled past downward it stays sticky instead (browser.css).
  useEffect(() => {
    if (!element || typeof IntersectionObserver === "undefined") return;
    const observer = new IntersectionObserver(([entry]) => setAway(!!entry && !entry.isIntersecting), {
      root: element.closest(".conversation-scroll"),
      rootMargin: "0px 0px -120px 0px",
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [element]);

  const { message, status } = stream;
  const state = previewNotice(stream) ?? (status === "connecting" || status === "idle" ? "Connecting…" : message ? null : "Waiting for a frame…");
  const frame = message?.type === "frame" && status === "open" ? message : null;
  const liveTabId = message && "tab_id" in message ? message.tab_id : null;
  const liveTab = tabs.tabs?.find((tab) => tab.tab_id === liveTabId);
  // The page the card shows: the agent's own latest `browser.state`.
  const pageState = Object.values(view.browserStates)
    .filter((candidate) => !candidate.task_id)
    .reduce<BrowserStateView | undefined>((latest, candidate) => (!latest || latest.seq < candidate.seq ? candidate : latest), undefined);
  const pageHost = pageState?.host || lastHost(view.items);
  const pageTitle = (liveTab?.title || pageState?.title)?.trim() ?? "";
  const title = pageTitle || pageHost || "Browser";
  const favicon = pageState ? safeFavicon(pageState.favicon) : null;
  const siteIcon = favicon ? <img src={favicon} alt="" width={16} height={16} /> : <Globe size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />;
  const loading = currentTools(view.items).some((tool) => !tool.done && opensPage(tool));

  const takeover =
    takingOver && takeoverTicket ? (
      <TakeoverView ticket={{ id: takeoverTicket.id, summary: takeoverTicket.summary, origin: takeoverTicket.target?.origin ?? null }} onClose={() => setTakingOver(false)} />
    ) : null;

  if (dock.sheet) {
    return (
      <BrowserSheet
        sessionId={sessionId}
        running={running}
        tabs={tabs}
        liveTabId={liveTabId}
        selectedTabId={dock.sheetTab}
        frame={frame?.data ?? null}
        state={state}
        fallback={{ title: pageTitle, host: pageHost ?? "" }}
        takeover={
          takeoverTicket
            ? {
                origin: takeoverTicket.target?.origin ?? null,
                onStart: () => {
                  dock.setSheet(false);
                  setTakingOver(true);
                },
              }
            : undefined
        }
        onClose={() => dock.setSheet(false)}
      />
    );
  }
  // The session has no tab open (closed, or not opened yet): nothing to show until one is.
  if (message?.type === "no_tab" && status === "open") return takeover;

  const tone = waiting ? "warn" : running ? "running" : done ? "done" : "idle";
  const activity = view.thinking ? "Thinking" : statusPhrase(currentTools(view.items), lastHost(view.items));
  const reply = view.items.findLast((item): item is Extract<TimelineItem, { type: "assistant" }> => item.type === "assistant" && item.final)?.text;
  const summary = (reply ? firstSentence(reply) : null) ?? "Done";
  // Idle, the open card already names the page beside its icon, so the status adds the site;
  // the slim pill has no title of its own and names the page.
  const idleText = minimized ? title : pageTitle && pageHost ? pageHost : null;
  const statusText = tone === "running" ? `${activity}…` : tone === "done" ? summary : idleText;

  const statusLine =
    tone === "warn" ? (
      <button type="button" className="browser-card-action" onClick={onShowPending}>
        <TriangleAlert size={16} strokeWidth={2} aria-hidden="true" />
        Action needed
      </button>
    ) : statusText ? (
      <p className="browser-card-status" data-tone={tone} role="status">
        {tone === "running" ? (
          <span className="browser-card-dots" aria-hidden="true">
            <span />
            <span />
            <span />
          </span>
        ) : tone === "done" ? (
          <Check size={15} strokeWidth={2.4} aria-hidden="true" />
        ) : (
          <span className="live-dot" data-live={!!frame || undefined} aria-hidden="true" />
        )}
        <span key={tone === "running" ? activity : tone} className="browser-card-text">
          {statusText}
        </span>
      </p>
    ) : (
      <span className="browser-card-status" aria-hidden="true" />
    );

  const controls = (
    <span className="browser-card-controls">
      <button
        type="button"
        className="browser-card-control"
        aria-label={minimized ? "Restore" : "Minimize"}
        title={minimized ? "Restore" : "Minimize"}
        aria-expanded={!minimized}
        onClick={() => setMinimized(!minimized)}
      >
        {minimized ? (
          <Maximize2 size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
        ) : (
          <Minimize2 size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
        )}
      </button>
      {running ? (
        <button type="button" className="browser-card-stop" onClick={onCancel}>
          <Square size={12} strokeWidth={ICON_STROKE} aria-hidden="true" />
          Stop
        </button>
      ) : (
        <button type="button" className="browser-card-control" aria-label="Close" title="Close" onClick={onDismiss}>
          <X size={17} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </button>
      )}
    </span>
  );

  const screen = frame ? (
    <span className="browser-card-frame" style={{ "--frame-ratio": frame.width / frame.height } as CSSProperties}>
      <BrowserFrame data={frame.data} />
      {ripple ? (
        <span
          key={ripple.id}
          className="browser-card-ripple"
          aria-hidden="true"
          style={{ left: `${(ripple.x / VIEWPORT.width) * 100}%`, top: `${(ripple.y / VIEWPORT.height) * 100}%` }}
        />
      ) : null}
    </span>
  ) : (
    <span className="browser-card-state" role="status">
      {state}
    </span>
  );

  const card = cardShown ? (
    <section
      ref={setElement}
      className="browser-card"
      aria-label="Live browser"
      data-tone={tone}
      data-minimized={minimized || undefined}
      data-loading={(loading && running) || undefined}
    >
      {minimized ? (
        <div className="browser-card-bar">
          <button type="button" className="browser-card-thumb" aria-label={`Expand ${title}`} onClick={() => dock.setSheet(true)}>
            {frame ? <BrowserFrame data={frame.data} /> : siteIcon}
          </button>
          {statusLine}
          {controls}
        </div>
      ) : (
        <>
          <button type="button" className="browser-card-screen" aria-label={`Expand ${title}`} onClick={() => dock.setSheet(true)}>
            {screen}
          </button>
          <div className="browser-card-bar">
            <span className="browser-card-site">
              {siteIcon}
              <span className="browser-card-site-title">{title}</span>
            </span>
            {statusLine}
            {controls}
          </div>
        </>
      )}
    </section>
  ) : null;

  // The PiP stands in for the card whenever the card is not on screen: scrolled away, closed,
  // or never drawn because only the session's Tasks browsed (once summoned). While a request
  // waits for the person it only floats when they asked for it, so it never covers the
  // request's buttons by itself.
  const pip =
    floatable && !(cardShown && !away) && (!waiting || dock.summoned) ? (
      <BrowserPip
        label="Live browser"
        title={title}
        status={tone}
        live={!!frame}
        minimized={dock.pip === "minimized"}
        onMinimizedChange={(next) => dock.setPip(next ? "minimized" : "open")}
        thumb={frame ? <BrowserFrame data={frame.data} /> : siteIcon}
        onExpand={() => dock.setSheet(true)}
        onClose={() => dock.setPip("closed")}
      >
        {frame ? <BrowserFrame data={frame.data} /> : <p className="pip-state">{state}</p>}
      </BrowserPip>
    ) : null;

  return (
    <>
      {card}
      {pip}
      {takeover}
    </>
  );
}
