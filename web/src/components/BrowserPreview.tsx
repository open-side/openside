import { Check, Globe, Maximize2, Minimize2, Square, TriangleAlert, X } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { previewNotice, useBrowserPreview } from "../hooks/useBrowserPreview";
import { pendingTickets, type BrowserStateView, type SessionView, type TimelineItem } from "../state/session";
import { ICON_STROKE } from "../ui";
import { BrowserFrame } from "./BrowserFrame";
import { BrowserPip } from "./BrowserPip";
import { BrowserSheet, useSessionTabs } from "./BrowserSheet";
import { firstSentence } from "./taskStatus";
import { opensPage, safeFavicon } from "./toolCopy";
import { lastHost, statusPhrase } from "./toolStatus";
import "./browser.css";

/** The agent tab's fixed viewport (openside-browser `pointer::VIEWPORT`); click coordinates are CSS pixels in it. */
const VIEWPORT = { width: 1440, height: 900 };
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
 * The live browser card of an agent that browses itself: the tab's read-only screencast, a
 * status pill saying what the agent is doing, "Action needed" while a request waits, and a done
 * chip after the run, which then folds the card to a slim pill. Background Tasks show their own
 * tab in their Task unit (`TaskUnit`). Private sessions never show it.
 */
export function BrowserPreview({ sessionId, view, onShowPending, onCancel }: BrowserPreviewProps) {
  const browserCalls = useMemo(() => view.items.filter((item) => item.type === "tool" && item.name.toLowerCase().startsWith("browser")).length, [view.items]);
  // Closing the idle card hides it until the agent touches the browser again.
  const [dismissedAt, setDismissedAt] = useState(-1);
  if (view.incognito || browserCalls === 0 || browserCalls <= dismissedAt) return null;
  return <Card sessionId={sessionId} view={view} onShowPending={onShowPending} onCancel={onCancel} onDismiss={() => setDismissedAt(browserCalls)} />;
}

function Card({ sessionId, view, onShowPending, onCancel, onDismiss }: BrowserPreviewProps & { onDismiss: () => void }) {
  const stream = useBrowserPreview(sessionId);
  const [expanded, setExpanded] = useState(false);
  const [minimized, setMinimized] = useState(false);
  const [done, setDone] = useState(false);
  const [ripple, setRipple] = useState<Click | null>(null);
  const [element, setElement] = useState<HTMLElement | null>(null);
  const [away, setAway] = useState(false);
  const [pipClosed, setPipClosed] = useState(false);
  const tabs = useSessionTabs(sessionId);
  const running = view.status === "running";
  const waiting = pendingTickets(view).length > 0;
  const runStart = useRef<number | null>(running ? Date.now() : null);

  useEffect(() => {
    if (running) {
      runStart.current ??= Date.now();
      setDone(false);
      setPipClosed(false);
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

  // Scrolled away from the card while the agent still works: the picture follows as a floating
  // preview. While the card is still on screen or scrolled past downward it stays sticky (browser.css).
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
  const loading = currentTools(view.items).some((tool) => !tool.done && opensPage(tool));

  if (expanded) {
    return <BrowserSheet tabs={tabs} liveTabId={liveTabId} frame={frame?.data ?? null} state={state} onClose={() => setExpanded(false)} />;
  }
  // The session has no tab open (closed, or not opened yet): nothing to show until one is.
  if (message?.type === "no_tab" && status === "open") return null;

  const tone = waiting ? "warn" : running ? "running" : done ? "done" : "idle";
  const activity = view.thinking ? "Thinking" : statusPhrase(currentTools(view.items), lastHost(view.items));
  const reply = view.items.findLast((item): item is Extract<TimelineItem, { type: "assistant" }> => item.type === "assistant" && item.final)?.text;
  const summary = (reply ? firstSentence(reply) : null) ?? "Done";

  const statusPill =
    tone === "warn" ? (
      <button type="button" className="browser-card-pill browser-card-status" data-tone="warn" onClick={onShowPending}>
        <TriangleAlert size={18} strokeWidth={2} aria-hidden="true" />
        <span className="browser-card-text">Action needed</span>
      </button>
    ) : (
      <p className="browser-card-pill browser-card-status" data-tone={tone} role="status">
        {tone === "running" ? (
          <span className="browser-card-dots" aria-hidden="true">
            <span />
            <span />
            <span />
          </span>
        ) : tone === "done" ? (
          <Check size={18} strokeWidth={2.4} aria-hidden="true" />
        ) : (
          <span className="live-dot" data-live={!!frame || undefined} aria-hidden="true" />
        )}
        <span key={tone === "running" ? activity : tone} className="browser-card-text">
          {tone === "running" ? `${activity}…` : tone === "done" ? summary : title}
        </span>
      </p>
    );

  const controls = (
    <>
      <button
        type="button"
        className="browser-card-pill browser-card-control"
        aria-label={minimized ? "Restore" : "Minimize"}
        title={minimized ? "Restore" : "Minimize"}
        aria-expanded={!minimized}
        onClick={() => setMinimized(!minimized)}
      >
        {minimized ? (
          <Maximize2 size={17} strokeWidth={ICON_STROKE} aria-hidden="true" />
        ) : (
          <Minimize2 size={17} strokeWidth={ICON_STROKE} aria-hidden="true" />
        )}
      </button>
      {running ? (
        <button type="button" className="browser-card-pill browser-card-stop" onClick={onCancel}>
          <Square size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
          Stop
        </button>
      ) : (
        <button type="button" className="browser-card-pill browser-card-control" aria-label="Close" title="Close" onClick={onDismiss}>
          <X size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </button>
      )}
    </>
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

  return (
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
          <button type="button" className="browser-card-thumb" aria-label={`Expand ${title}`} onClick={() => setExpanded(true)}>
            {frame ? <BrowserFrame data={frame.data} /> : null}
          </button>
          {statusPill}
          {controls}
        </div>
      ) : (
        <>
          <button type="button" className="browser-card-screen" aria-label={`Expand ${title}`} onClick={() => setExpanded(true)}>
            {screen}
          </button>
          {pageTitle || pageHost ? (
            <span className="browser-card-chip">
              {favicon ? <img src={favicon} alt="" width={16} height={16} /> : <Globe size={15} strokeWidth={ICON_STROKE} aria-hidden="true" />}
              <span className="browser-card-chip-title">{pageTitle || pageHost}</span>
              {pageTitle && pageHost ? <span className="browser-card-chip-host">{pageHost}</span> : null}
            </span>
          ) : null}
          <div className="browser-card-bar">
            {statusPill}
            {controls}
          </div>
        </>
      )}
      {tone === "running" && away && !pipClosed && frame ? (
        <BrowserPip label="Live browser" title={title} status={tone} live onExpand={() => setExpanded(true)} onClose={() => setPipClosed(true)}>
          <BrowserFrame data={frame.data} />
        </BrowserPip>
      ) : null}
    </section>
  );
}
