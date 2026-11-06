import { Ban, Bot, Check, ChevronRight, CircleAlert } from "lucide-react";
import { useState } from "react";
import { t, type MessageKey } from "../i18n";
import type { SubagentView } from "../state/session";
import { Dialog, ICON_STROKE, Spinner } from "../ui";
import { taskLine, taskSteps } from "./taskStatus";
import "./browser.css";
import "./tasks.css";

export const STATUS_LABEL: Record<SubagentView["status"], MessageKey> = {
  running: "timeline.agents.status.running",
  waiting_human: "timeline.agents.status.waiting_human",
  completed: "timeline.agents.status.completed",
  failed: "timeline.agents.status.failed",
  cancelled: "timeline.agents.status.cancelled",
};

export function StatusMark({ status }: { status: SubagentView["status"] }) {
  switch (status) {
    case "running":
      return <Spinner size={16} />;
    case "waiting_human":
      return <span className="attention-dot" aria-hidden="true" />;
    case "completed":
      return <Check size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />;
    case "failed":
      return <CircleAlert size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />;
    case "cancelled":
      return <Ban size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />;
  }
}

/** The calls a child made, one past-tense line each; the running one spins. */
function TaskStepList({ progress }: { progress: readonly unknown[] }) {
  const steps = taskSteps(progress);
  if (!steps.length) return null;
  return (
    <ol className="task-steps" aria-label={t("timeline.agents.steps")}>
      {steps.map((step) => (
        <li key={step.id} data-state={step.state}>
          <span className="task-step-mark" aria-hidden="true">
            {step.state === "running" ? (
              <Spinner size={14} />
            ) : step.state === "failed" ? (
              <CircleAlert size={14} strokeWidth={ICON_STROKE} />
            ) : (
              <Check size={14} strokeWidth={2.2} />
            )}
          </span>
          <span className="task-step-text">{step.text}</span>
        </li>
      ))}
    </ol>
  );
}

/** One child, read-only: people steer work by talking to the agent that started it. */
function SubagentRow({ child }: { child: SubagentView }) {
  return (
    <li className={`subagent subagent-${child.status}`} data-subagent={child.id}>
      <details>
        <summary>
          <span className="subagent-mark">
            <StatusMark status={child.status} />
          </span>
          <span className="subagent-heading">
            <span className="subagent-task">{child.task}</span>
            <span className="subagent-meta">
              <span className={`subagent-status status-${child.status}`}>{t(STATUS_LABEL[child.status])}</span>
              {child.status === "running" ? <span> · {taskLine(child)}</span> : null}
            </span>
          </span>
          <ChevronRight className="subagent-chevron" size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </summary>
        <div className="subagent-body">
          <p className="subagent-detail">
            {child.model}
            {child.tools.length ? ` · ${child.tools.join(", ")}` : ""}
          </p>
          {child.reason ? <p className="subagent-detail">{child.reason}</p> : null}
          <TaskStepList progress={child.progress} />
        </div>
      </details>
    </li>
  );
}

/**
 * `delegate` children of this session, rebuilt from `subagent.*` events like the rest of the
 * screen. A pill in the composer's tool row opens them in a sheet; background tasks are chips
 * in the transcript instead (Timeline).
 */
export function AgentsTray({ subagents }: { subagents: SubagentView[] }) {
  const [open, setOpen] = useState(false);
  const delegated = subagents.filter((child) => !child.background);
  if (!delegated.length) return null;
  const active = delegated.filter((child) => child.status === "running" || child.status === "waiting_human").length;
  const waiting = delegated.some((child) => child.status === "waiting_human");
  return (
    <>
      <button type="button" className={`ui-chip agents-pill${waiting ? " waiting-pill" : ""}`} aria-haspopup="dialog" onClick={() => setOpen(true)}>
        <Bot size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
        <span className="ui-chip-label">{t("timeline.agents.count", { count: delegated.length })}</span>
      </button>
      <Dialog
        open={open}
        onClose={() => setOpen(false)}
        size="wide"
        className="tray-sheet"
        title={t("timeline.agents.title")}
        description={active ? t("timeline.agents.working", { count: active }) : t("timeline.agents.finished", { count: delegated.length })}
      >
        <div className="tray-scroll thin-scroll">
          <ul className="subagent-list" aria-label={t("timeline.agents.list")}>
            {delegated.map((child) => (
              <SubagentRow key={child.id} child={child} />
            ))}
          </ul>
        </div>
      </Dialog>
    </>
  );
}
