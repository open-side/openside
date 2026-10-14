import { Check, CircleAlert, Minimize2, TriangleAlert, X } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { useClient } from "../context";
import { previewNotice, useBrowserPreview } from "../hooks/useBrowserPreview";
import type { BrowserStateView, SubagentView } from "../state/session";
import { ICON_STROKE, useToast } from "../ui";
import { BrowserFrame } from "./BrowserFrame";
import { StopTaskDialog } from "./StopTaskDialog";
import { taskLine, taskSteps } from "./taskStatus";
import { browserLine } from "./toolStatus";
import "./browser.css";
import "./tasks.css";

type Tone = "running" | "warn" | "done" | "failed";

function Dots() {
  return (
    <span className="browser-card-dots" aria-hidden="true">
      <span />
      <span />
      <span />
    </span>
  );
}

function ToneMark({ tone }: { tone: Tone }) {
  if (tone === "running") return <Dots />;
  if (tone === "warn") return <TriangleAlert size={16} strokeWidth={2} aria-hidden="true" />;
  if (tone === "failed") return <CircleAlert size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />;
  return <Check size={16} strokeWidth={2.4} aria-hidden="true" />;
}

function toneOf(task: SubagentView, tabWaiting: boolean): Tone {
  if (task.status === "completed") return "done";
  if (task.status === "failed" || task.status === "cancelled") return "failed";
  return task.status === "waiting_human" || tabWaiting ? "warn" : "running";
}

/** The Task's last picture of this tab, fetched once per thumbnail id; it may be withheld. */
function useThumbnail(sessionId: string, thumbnailId: string | undefined, enabled: boolean): string | null {
  const client = useClient();
  const [thumbnail, setThumbnail] = useState<{ id: string; url: string } | null>(null);
  useEffect(() => {
    if (!thumbnailId || !enabled) return;
    const controller = new AbortController();
    let url: string | undefined;
    void client
      .browserThumbnail(sessionId, thumbnailId, controller.signal)
      .then((blob) => {
        if (controller.signal.aborted) return;
        url = URL.createObjectURL(blob);
        setThumbnail({ id: thumbnailId, url });
      })
      .catch(() => {
        /* A thumbnail may expire or be withheld after a secret fill. */
      });
    return () => {
      controller.abort();
      if (url) URL.revokeObjectURL(url);
    };
  }, [client, sessionId, thumbnailId, enabled]);
  return thumbnail && thumbnail.id === thumbnailId ? thumbnail.url : null;
}

/** Up to two earlier lines, newest first, faint under the current one. */
function Trail({ lines }: { lines: string[] }) {
  if (!lines.length) return null;
  return (
    <ol className="task-browser-trail" aria-label="Earlier steps">
      {lines.map((entry, index) => (
        <li key={`${index}-${entry}`}>{entry}</li>
      ))}
    </ol>
  );
}

/** The small step bubble: a state mark and the current line. It acts only when it has something to open. */
function StepBubble({ tone, line, label, onClick }: { tone: Tone; line: string; label: string; onClick?: () => void }) {
  const content = (
    <>
      <span className="task-browser-mark">
        <ToneMark tone={tone} />
      </span>
      <span className={tone === "running" ? "task-browser-line status-line-text" : "task-browser-line"} key={line}>
        {line}
      </span>
    </>
  );
  return (
    <div className="task-browser-pill">
      {onClick ? (
        <button type="button" className="task-browser-bubble" aria-label={label} onClick={onClick}>
          {content}
        </button>
      ) : (
        <p className="task-browser-bubble" role="status" aria-label={label}>
          {content}
        </p>
      )}
    </div>
  );
}

export interface TaskUnitProps {
  sessionId: string;
  task: SubagentView;
  /** The latest `browser.state` of the Task's tab, when it browses. */
  state?: BrowserStateView | undefined;
  /** The newest Task still at work: its browser opens on its own while nobody chose otherwise. */
  newest: boolean;
  /** Brings the card that asks the person into view. */
  onShowPending: () => void;
}

/**
 * One Task, as its own small unit under the message that started it: the step it is on with a
 * state mark and, faint beneath, the steps before; once it ends, how it ended. Steering happens
 * by talking to the assistant, so the unit carries no controls of its own beyond its browser.
 */
export function TaskUnit(props: TaskUnitProps) {
  return props.state ? <BrowserTask {...props} state={props.state} /> : <StepTask {...props} />;
}

function StepTask({ task, onShowPending }: TaskUnitProps) {
  const steps = taskSteps(task.progress);
  const tone = toneOf(task, false);
  const running = tone === "running";
  const line = running ? (steps.at(-1)?.text ?? (task.progress.length ? "Thinking" : "Starting")) : tone === "warn" ? "Waiting for you" : taskLine(task);
  // A call still open when the Task ended was never finished; its present-tense phrase must not outlive the Task.
  const earlier = (running ? steps.slice(0, -1) : steps.filter((step) => step.state !== "running"))
    .slice(-2)
    .map((step) => step.text)
    .filter((entry) => entry !== line)
    .reverse();
  return (
    <div className="task-browser" data-tone={tone} data-task={task.id} data-status={task.status}>
      <StepBubble tone={tone} line={line} label={`${task.task}: ${line}`} {...(tone === "warn" ? { onClick: onShowPending } : {})} />
      <Trail lines={earlier} />
    </div>
  );
}

/**
 * A Task that browses. While it works (and it is the newest one at work) the unit is its tab's
 * live picture with the current line as a status pill, Collapse, and ✕ to stop the Task after a
 * confirmation; otherwise, and once it ends, it is the step bubble, which opens the tab again.
 * The person's own open/fold choice sticks. The session's preview follows the tab its agents
 * used last, so another Task's tab shows its last picture, never labelled live.
 */
function BrowserTask({ sessionId, task, state, newest, onShowPending }: TaskUnitProps & { state: BrowserStateView }) {
  const client = useClient();
  const toast = useToast();
  const [choice, setChoice] = useState<boolean | null>(null);
  const [stopping, setStopping] = useState(false);
  const active = task.status === "running" || task.status === "waiting_human";
  const expanded = choice ?? (active && newest);
  const stream = useBrowserPreview(sessionId, expanded);
  const thumbnailUrl = useThumbnail(sessionId, state.thumbnail ?? undefined, expanded);

  const tone = toneOf(task, state.phase === "waiting_human");
  const line = tone === "running" ? browserLine(state) : tone === "warn" ? "Action needed" : taskLine(task);
  // Its browser steps are phrased in the present tense, so a Task that has ended keeps only the result line.
  const ended = tone === "done" || tone === "failed";
  const earlier = (ended ? [] : state.trail).filter((entry) => entry !== line).reverse();
  const title = state.title.trim() || state.host || "Browser";
  // Once the Task has left the page to write its answer, stopping it would only lose the answer.
  const stoppable = active && state.phase !== "done";

  let body: ReactNode;
  if (!expanded) {
    body = <StepBubble tone={tone} line={line} label={`Show the browser: ${title}. ${line}`} onClick={() => setChoice(true)} />;
  } else {
    const frame = stream.message?.type === "frame" && stream.message.tab_id === state.tab_id ? stream.message : null;
    // A notice about another tab (paused, withheld) says nothing about this one.
    const notice = stream.message && "tab_id" in stream.message && stream.message.tab_id !== state.tab_id ? null : previewNotice(stream);
    const screen = frame ? (
      <span className="browser-card-frame">
        <BrowserFrame data={frame.data} />
      </span>
    ) : thumbnailUrl && !notice ? (
      <span className="browser-card-frame">
        <img src={thumbnailUrl} alt={`Last picture of ${title}`} draggable={false} />
      </span>
    ) : (
      <span className="task-browser-skeleton" role="status">
        <span className="task-browser-skeleton-bar" aria-hidden="true" />
        <span className="task-browser-skeleton-bar" aria-hidden="true" />
        <span className="task-browser-skeleton-bar" aria-hidden="true" />
        <span className="task-browser-skeleton-text">{notice ?? "Loading the browser"}</span>
      </span>
    );
    body = (
      <section
        className="browser-card task-browser-card"
        aria-label={`Browser: ${title}`}
        data-tone={tone}
        data-live={frame ? true : undefined}
        data-loading={(state.phase === "loading" && tone === "running") || undefined}
      >
        <div className="browser-card-screen task-browser-screen">{screen}</div>
        <div className="browser-card-bar">
          {tone === "warn" ? (
            <button type="button" className="browser-card-pill browser-card-status" data-tone="warn" onClick={onShowPending}>
              <ToneMark tone="warn" />
              <span className="browser-card-text">{line}</span>
            </button>
          ) : (
            <p className="browser-card-pill browser-card-status" data-tone={tone} role="status">
              <ToneMark tone={tone} />
              <span className="browser-card-text" key={line}>
                {line}
              </span>
            </p>
          )}
          <button type="button" className="browser-card-pill browser-card-control" aria-label="Collapse" title="Collapse" onClick={() => setChoice(false)}>
            <Minimize2 size={17} strokeWidth={ICON_STROKE} aria-hidden="true" />
          </button>
          {stoppable ? (
            <button type="button" className="browser-card-pill browser-card-control" aria-label="Stop this task" title="Stop this task" onClick={() => setStopping(true)}>
              <X size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
            </button>
          ) : null}
        </div>
      </section>
    );
  }

  return (
    <div className="task-browser" data-tone={tone} data-task={task.id} data-status={task.status} data-expanded={expanded}>
      {body}
      <Trail lines={earlier} />
      <StopTaskDialog
        open={stopping}
        scope={{ kind: "task", name: task.task.slice(0, 80) }}
        onClose={() => setStopping(false)}
        onConfirm={() => void client.cancelSubagent(sessionId, task.id).catch(() => toast.notify("Couldn’t stop the task. Try again.", "error"))}
      />
    </div>
  );
}
