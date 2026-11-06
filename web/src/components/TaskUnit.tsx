import { Check, CircleAlert, Minimize2, TriangleAlert, X } from "lucide-react";
import { useState, type ReactNode } from "react";
import { useClient } from "../context";
import { previewNotice, useBrowserPreview } from "../hooks/useBrowserPreview";
import { useTaskThumbnail } from "../hooks/useTaskThumbnail";
import { t } from "../i18n";
import type { BrowserStateView, SubagentView } from "../state/session";
import { ICON_STROKE, useToast } from "../ui";
import { BrowserFrame } from "./BrowserFrame";
import { StopTaskDialog } from "./StopTaskDialog";
import { taskLine, taskStateLabel, taskSteps } from "./taskStatus";
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

/** Up to two earlier lines, newest first, faint under the current one. */
function Trail({ lines }: { lines: string[] }) {
  if (!lines.length) return null;
  return (
    <ol className="task-browser-trail" aria-label={t("timeline.task.earlierSteps")}>
      {lines.map((entry, index) => (
        <li key={`${index}-${entry}`}>{entry}</li>
      ))}
    </ol>
  );
}

/** ✕ for a Task still at work; it stops the Task only after a confirmation. */
function StopTask({ sessionId, task, className }: { sessionId: string; task: SubagentView; className: string }) {
  const client = useClient();
  const toast = useToast();
  const [open, setOpen] = useState(false);
  return (
    <>
      <button type="button" className={className} aria-label={t("timeline.task.stop")} title={t("timeline.task.stop")} onClick={() => setOpen(true)}>
        <X size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
      </button>
      <StopTaskDialog
        open={open}
        scope={{ kind: "task", name: task.task.slice(0, 80) }}
        onClose={() => setOpen(false)}
        onConfirm={() => {
          setOpen(false);
          void client.cancelSubagent(sessionId, task.id).catch(() => toast.notify(t("timeline.task.stopFailed"), "error"));
        }}
      />
    </>
  );
}

/**
 * The Task's card at rest: where it stands, what it was asked, the step it is on and, faint
 * beneath, the steps before. It acts only when it has something to open.
 */
function TaskCard({
  task,
  tone,
  line,
  label,
  earlier,
  onClick,
  stop,
}: {
  task: SubagentView;
  tone: Tone;
  line: string;
  label: string;
  earlier: string[];
  onClick?: () => void;
  stop?: ReactNode;
}) {
  const content = (
    <>
      <span className="task-browser-state">
        <span className="task-browser-mark">
          <ToneMark tone={tone} />
        </span>
        {/* A tab waiting on the person outranks the Task's own status. */}
        {tone === "warn" ? t("timeline.task.state.needsYou") : taskStateLabel(task)}
      </span>
      <span className="task-browser-title">{task.task}</span>
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
      {stop}
      <Trail lines={earlier} />
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
 * One Task, as its own card under the message that started it: its status, what it was asked,
 * the step it is on and, faint beneath, the steps before; once it ends, how it ended. Steering
 * happens by talking to the assistant, so the card's only controls are its browser and ✕.
 */
export function TaskUnit(props: TaskUnitProps) {
  return props.state ? <BrowserTask {...props} state={props.state} /> : <StepTask {...props} />;
}

function StepTask({ sessionId, task, onShowPending }: TaskUnitProps) {
  const steps = taskSteps(task.progress);
  const tone = toneOf(task, false);
  const running = tone === "running";
  const line = running
    ? (steps.at(-1)?.text ?? t(task.progress.length ? "timeline.status.thinking" : "timeline.task.starting"))
    : tone === "warn"
      ? t("timeline.status.waiting")
      : taskLine(task);
  // A call still open when the Task ended was never finished; its present-tense phrase must not outlive the Task.
  const earlier = (running ? steps.slice(0, -1) : steps.filter((step) => step.state !== "running"))
    .slice(-2)
    .map((step) => step.text)
    .filter((entry) => entry !== line)
    .reverse();
  const active = task.status === "running" || task.status === "waiting_human";
  return (
    <div className="task-browser" data-tone={tone} data-task={task.id} data-status={task.status}>
      <TaskCard
        task={task}
        tone={tone}
        line={line}
        label={t("timeline.task.rowLabel", { task: task.task, line })}
        earlier={earlier}
        {...(tone === "warn" ? { onClick: onShowPending } : {})}
        {...(active ? { stop: <StopTask sessionId={sessionId} task={task} className="task-browser-stop" /> } : {})}
      />
    </div>
  );
}

/**
 * A Task that browses. While it works (and it is the newest one at work) the unit is its tab's
 * live picture with the current line as a status pill, Collapse, and ✕ to stop the Task after a
 * confirmation; otherwise, and once it ends, it is the Task card, which opens the tab again.
 * The person's own open/fold choice sticks. The session's preview follows the tab its agents
 * used last, so another Task's tab shows its last picture, never labelled live.
 */
function BrowserTask({ sessionId, task, state, newest, onShowPending }: TaskUnitProps & { state: BrowserStateView }) {
  const [choice, setChoice] = useState<boolean | null>(null);
  const active = task.status === "running" || task.status === "waiting_human";
  const expanded = choice ?? (active && newest);
  const stream = useBrowserPreview(sessionId, expanded);
  const thumbnailUrl = useTaskThumbnail(sessionId, state.thumbnail ?? undefined, expanded);

  const tone = toneOf(task, state.phase === "waiting_human");
  const line = tone === "running" ? browserLine(state) : tone === "warn" ? t("timeline.task.actionNeeded") : taskLine(task);
  // Its browser steps are phrased in the present tense, so a Task that has ended keeps only the result line.
  const ended = tone === "done" || tone === "failed";
  const earlier = (ended ? [] : state.trail).filter((entry) => entry !== line).reverse();
  const title = state.title.trim() || state.host || t("timeline.task.browserFallback");
  // Once the Task has left the page to write its answer, stopping it would only lose the answer.
  const stoppable = active && state.phase !== "done";

  if (!expanded) {
    return (
      <div className="task-browser" data-tone={tone} data-task={task.id} data-status={task.status} data-expanded={false}>
        <TaskCard
          task={task}
          tone={tone}
          line={line}
          label={t("timeline.task.showBrowser", { title, line })}
          earlier={earlier}
          onClick={() => setChoice(true)}
          {...(stoppable ? { stop: <StopTask sessionId={sessionId} task={task} className="task-browser-stop" /> } : {})}
        />
      </div>
    );
  }

  const frame = stream.message?.type === "frame" && stream.message.tab_id === state.tab_id ? stream.message : null;
  // A notice about another tab (paused, withheld) says nothing about this one.
  const notice = stream.message && "tab_id" in stream.message && stream.message.tab_id !== state.tab_id ? null : previewNotice(stream);
  const screen = frame ? (
    <span className="browser-card-frame">
      <BrowserFrame data={frame.data} />
    </span>
  ) : thumbnailUrl && !notice ? (
    <span className="browser-card-frame">
      <img src={thumbnailUrl} alt={t("timeline.task.lastPicture", { title })} draggable={false} />
    </span>
  ) : (
    <span className="task-browser-skeleton" role="status">
      <span className="task-browser-skeleton-bar" aria-hidden="true" />
      <span className="task-browser-skeleton-bar" aria-hidden="true" />
      <span className="task-browser-skeleton-bar" aria-hidden="true" />
      <span className="task-browser-skeleton-text">{notice ?? t("timeline.task.loadingBrowser")}</span>
    </span>
  );
  return (
    <div className="task-browser" data-tone={tone} data-task={task.id} data-status={task.status} data-expanded={true}>
      <section
        className="browser-card task-browser-card"
        aria-label={t("timeline.task.browserRegion", { title })}
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
          <button type="button" className="browser-card-pill browser-card-control" aria-label={t("timeline.task.collapse")} title={t("timeline.task.collapse")} onClick={() => setChoice(false)}>
            <Minimize2 size={17} strokeWidth={ICON_STROKE} aria-hidden="true" />
          </button>
          {stoppable ? <StopTask sessionId={sessionId} task={task} className="browser-card-pill browser-card-control" /> : null}
        </div>
      </section>
      <Trail lines={earlier} />
    </div>
  );
}
