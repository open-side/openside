import { Check, Globe, Maximize2, Minimize2, Square, TriangleAlert, X } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { whilePageShown } from "../api/pageLifecycle";
import type { PreviewServerMessage } from "../api/types";
import { useClient } from "../context";
import { pendingTickets, type BrowserStateView, type SessionView, type SubagentView, type TimelineItem } from "../state/session";
import { ICON_STROKE, useToast } from "../ui";
import { BrowserFrame } from "./BrowserFrame";
import { BrowserPip } from "./BrowserPip";
import { BrowserSheet, useSessionTabs } from "./BrowserSheet";
import { StopTaskDialog } from "./StopTaskDialog";
import { opensPage, safeFavicon } from "./toolCopy";
import { lastHost, statusPhrase } from "./toolStatus";
import "./browser.css";

const ENDED: Record<string, string> = {
  revoked: "This device's access was revoked.",
  session_stopped: "The session stopped.",
  session_deleted: "The session was deleted.",
};

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
 * The message the live card sits under: the person's message in front of the latest browser
 * activity (a browser call, or a Task started to browse), so a later browsing turn brings the
 * card down to its own message. `null` when the log has no such message.
 */
export function browserAnchor(items: readonly TimelineItem[]): string | null {
  const start = items.findLastIndex((item) => item.type === "tool" && (item.name.toLowerCase().startsWith("browser") || item.name === "task_start"));
  for (let index = start; index >= 0; index -= 1) {
    const item = items[index]!;
    if (item.type === "user" && item.author_kind != null && item.author_kind !== "internal") return item.id;
  }
  return null;
}

/** First sentence of `text` without Markdown marks. */
function firstSentence(text: string): string | null {
  const plain = text.replace(/[*_`#>]+/g, "").replace(/\s+/g, " ").trim();
  return plain.split(/(?<=[.!?。])\s/)[0] || null;
}

/** A background Task that has used the browser, with its latest `browser.state`. */
interface TaskCard {
  task: SubagentView;
  state: BrowserStateView;
}

/** Tasks with browser activity in start order; the front agent's own reads are not a Task. */
function taskCards(view: SessionView): TaskCard[] {
  const latest = new Map<string, BrowserStateView>();
  for (const state of Object.values(view.browserStates)) {
    if (!state.task_id) continue;
    const seen = latest.get(state.task_id);
    if (!seen || seen.seq < state.seq) latest.set(state.task_id, state);
  }
  return view.subagents.flatMap((task) => {
    const state = latest.get(task.id);
    return state ? [{ task, state }] : [];
  });
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
  /** Asks to stop the running task (the caller confirms first). */
  onCancel: () => void;
}

/**
 * The live browser card docked at the top of the conversation: the agent
 * tab's read-only screencast, a status pill saying what the agent is doing, "Action needed"
 * while a request waits, and a done chip after the run, which then folds the card to a slim
 * pill. Background Tasks narrate it from their `browser.state`; with several, a strip picks
 * the one shown. It connects once the conversation has used the browser; private sessions
 * never show it.
 */
export function BrowserPreview({ sessionId, view, onShowPending, onCancel }: BrowserPreviewProps) {
  const browserCalls = useMemo(() => view.items.filter((item) => item.type === "tool" && item.name.toLowerCase().startsWith("browser")).length, [view.items]);
  // A Task's own browser calls stay in its progress stream; its states mark the activity.
  const lastState = useMemo(() => Math.max(0, ...Object.values(view.browserStates).map((state) => state.seq)), [view.browserStates]);
  const activity = browserCalls + lastState;
  // Closing the idle card hides it until the agent touches the browser again.
  const [dismissedAt, setDismissedAt] = useState(-1);
  if (view.incognito || activity === 0 || activity <= dismissedAt) return null;
  return <Card sessionId={sessionId} view={view} onShowPending={onShowPending} onCancel={onCancel} onDismiss={() => setDismissedAt(activity)} />;
}

function Card({ sessionId, view, onShowPending, onCancel, onDismiss }: BrowserPreviewProps & { onDismiss: () => void }) {
  const client = useClient();
  const toast = useToast();
  const [message, setMessage] = useState<PreviewServerMessage | null>(null);
  const [status, setStatus] = useState("connecting");
  const [expanded, setExpanded] = useState(false);
  const [minimized, setMinimized] = useState(false);
  const [done, setDone] = useState(false);
  const [ripple, setRipple] = useState<Click | null>(null);
  const [picked, setPicked] = useState<string | null>(null);
  const [cancelling, setCancelling] = useState<string | null>(null);
  const [thumbnail, setThumbnail] = useState<{ id: string; url: string } | null>(null);
  const [element, setElement] = useState<HTMLElement | null>(null);
  const [away, setAway] = useState(false);
  const [pipClosed, setPipClosed] = useState(false);
  const tabs = useSessionTabs(sessionId);
  const tasks = taskCards(view);
  // Follows the Task that acted last until the person picks one in the strip.
  const task =
    tasks.find((card) => card.task.id === picked) ??
    tasks.reduce<TaskCard | undefined>((latest, card) => (!latest || latest.state.seq < card.state.seq ? card : latest), undefined);
  const taskWaiting = task?.task.status === "waiting_human" || task?.state.phase === "waiting_human";
  const running = task ? task.task.status === "running" || taskWaiting : view.status === "running";
  const waiting = pendingTickets(view).length > 0 || taskWaiting;
  const runStart = useRef<number | null>(running ? Date.now() : null);
  const thumbnailId = task?.state.thumbnail;
  useEffect(() => {
    if (!thumbnailId) return;
    const controller = new AbortController();
    let url: string | undefined;
    void client.browserThumbnail(sessionId, thumbnailId, controller.signal).then((blob) => {
      if (controller.signal.aborted) return;
      url = URL.createObjectURL(blob);
      setThumbnail({ id: thumbnailId, url });
    }).catch(() => { /* A thumbnail may expire or be withheld after a secret fill. */ });
    return () => {
      controller.abort();
      if (url) URL.revokeObjectURL(url);
    };
  }, [client, sessionId, thumbnailId]);
  const thumbnailUrl = thumbnail && thumbnail.id === thumbnailId ? thumbnail.url : null;

  useEffect(
    () =>
      whilePageShown(() => {
        const socket = client.browserPreview(sessionId);
        let finished = false;
        setStatus("connecting");
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
      }),
    [client, sessionId],
  );

  useEffect(() => {
    if (running) {
      runStart.current ??= Date.now();
      setDone(false);
      setPipClosed(false);
      return;
    }
    const started = runStart.current;
    runStart.current = null;
    // A Task that failed or was cancelled is not announced as done.
    if (started === null || (task ? task.task.status !== "completed" : view.status !== "idle") || Date.now() - started < MIN_RUN_MS) return;
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
              : message
                ? null
                : "Waiting for a frame…";
  const frame = message?.type === "frame" && status === "open" ? message : null;
  const liveTabId = message && "tab_id" in message ? message.tab_id : null;
  const liveTab = tabs.tabs?.find((tab) => tab.tab_id === liveTabId);
  const title = task ? task.state.title || task.state.host || "Browser" : liveTab?.title || lastHost(view.items) || "Browser";
  // The page the card shows: a Task's state, else the session's latest front-agent state.
  const pageState =
    task?.state ??
    Object.values(view.browserStates)
      .filter((state) => !state.task_id)
      .reduce<BrowserStateView | undefined>((latest, state) => (!latest || latest.seq < state.seq ? state : latest), undefined);
  const pageHost = pageState?.host || lastHost(view.items);
  const pageTitle = (task ? task.state.title : liveTab?.title || pageState?.title)?.trim() ?? "";
  const favicon = pageState ? safeFavicon(pageState.favicon) : null;
  const loading = task ? task.state.phase === "loading" : currentTools(view.items).some((tool) => !tool.done && opensPage(tool));

  if (expanded) {
    return <BrowserSheet tabs={tabs} liveTabId={liveTabId} frame={frame?.data ?? null} state={state ?? null} onClose={() => setExpanded(false)} />;
  }
  // The session has no tab open (closed, or not opened yet): nothing to show until one is.
  if (message?.type === "no_tab" && status === "open" && !task) return null;

  // The preview follows the session's last-used tab; another Task's tab shows what its page says.
  const facts = task && task.state.tab_id !== liveTabId ? task.state : null;
  const stopped = task && (task.task.status === "failed" || task.task.status === "cancelled");
  const tone = waiting ? "warn" : running ? "running" : done ? "done" : "idle";
  const action = task?.state.action;
  const activity = task
    ? action
      ? [action.phrase, action.target].filter(Boolean).join(" ")
      : task.state.phase === "loading"
        ? `Opening ${task.state.host || "a page"}`
        : `Reading ${task.state.host || "a page"}`
    : view.thinking
      ? "Thinking"
      : statusPhrase(currentTools(view.items), lastHost(view.items));
  const reply = task
    ? task.task.output
    : view.items.findLast((item): item is Extract<TimelineItem, { type: "assistant" }> => item.type === "assistant" && item.final)?.text;
  const summary = (reply ? firstSentence(reply) : null) ?? "Done";
  const idleText = stopped
    ? task.state.facts.error_text || (task.task.status === "cancelled" ? "Task cancelled" : "Task stopped")
    : task?.state.phase === "error"
      ? task.state.facts.error_text || title
      : title;

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
          {tone === "running" ? `${activity}…` : tone === "done" ? summary : idleText}
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
        <button
          type="button"
          className="browser-card-pill browser-card-stop"
          onClick={task ? () => setCancelling(task.task.id) : onCancel}
        >
          <Square size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
          {task ? "Cancel task" : "Stop"}
        </button>
      ) : (
        <button type="button" className="browser-card-pill browser-card-control" aria-label="Close" title="Close" onClick={onDismiss}>
          <X size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </button>
      )}
    </>
  );

  const thumb = frame && !facts ? <BrowserFrame data={frame.data} /> : thumbnailUrl ? <img src={thumbnailUrl} alt="" /> : null;
  const screen = facts ? (
    <span className="browser-card-facts">
      {thumbnailUrl ? <img className="browser-card-thumbnail" src={thumbnailUrl} alt={`Preview of ${title}`} width={360} height={225} /> : null}
      <strong>{facts.facts.heading || facts.title || facts.host || "Browser"}</strong>
      {facts.host ? <span className="browser-card-host">{facts.host}</span> : null}
      {facts.facts.fields?.length ? (
        <span className="browser-card-fields">
          {facts.facts.fields.map((field) => (
            <span key={`${field.kind}:${field.label}`}>{field.label}</span>
          ))}
        </span>
      ) : null}
      {facts.facts.result_count != null ? <span>{facts.facts.result_count === 1 ? "1 result" : `${facts.facts.result_count} results`}</span> : null}
      {facts.facts.error_text ? <span data-tone="error">{facts.facts.error_text}</span> : null}
    </span>
  ) : frame ? (
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
            {thumb}
          </button>
          {statusPill}
          {controls}
        </div>
      ) : (
        <>
          <button type="button" className="browser-card-screen" aria-label={`Expand ${title}`} onClick={() => setExpanded(true)}>
            {screen}
          </button>
          {tasks.length > 1 ? (
            <div className="browser-card-tasks" role="tablist" aria-label="Tasks">
              {tasks.map((card) => (
                <button
                  key={card.task.id}
                  type="button"
                  role="tab"
                  className="browser-card-pill browser-card-task"
                  aria-selected={card.task.id === task?.task.id}
                  data-status={card.task.status}
                  title={card.task.task}
                  onClick={() => setPicked(card.task.id)}
                >
                  <span className="live-dot" data-live={card.task.status === "running" || undefined} aria-hidden="true" />
                  <span className="browser-card-text">{card.task.task}</span>
                </button>
              ))}
            </div>
          ) : null}
          {!facts && tasks.length <= 1 && (pageTitle || pageHost) ? (
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
      <StopTaskDialog
        open={cancelling !== null}
        scope={{ kind: "task", name: tasks.find((card) => card.task.id === cancelling)?.task.task.slice(0, 80) }}
        onClose={() => setCancelling(null)}
        onConfirm={() => {
          if (cancelling) void client.cancelSubagent(sessionId, cancelling).catch(() => toast.notify("Could not cancel the task", "error"));
        }}
      />
      {tone === "running" && away && !pipClosed && frame ? (
        <BrowserPip
          label="Live browser"
          title={pageTitle || pageHost || "Browser"}
          status={tone}
          live
          onExpand={() => setExpanded(true)}
          onClose={() => setPipClosed(true)}
        >
          <BrowserFrame data={frame.data} />
        </BrowserPip>
      ) : null}
    </section>
  );
}
