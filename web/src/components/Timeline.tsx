import {
  Ban,
  BookOpen,
  Brain,
  Check,
  ChevronRight,
  CircleAlert,
  CodeXml,
  FilePenLine,
  FileText,
  Globe,
  KeyRound,
  MessageCircleQuestion,
  Search,
  Terminal,
  Users,
  type LucideIcon,
} from "lucide-react";
import { Fragment, type ReactNode } from "react";
import type { Visual } from "../api/types";
import { pendingTickets, type SessionView, type SubagentView, type TimelineItem } from "../state/session";
import { ICON_STROKE, Spinner } from "../ui";
import { ArtifactImage } from "./ArtifactImage";
import { Markdown } from "./Markdown";
import { isApprovalTicket, isChoiceTicket, isLoginTicket } from "../otp/loginForm";
import { isOtpTicket } from "../otp/otpForm";
import { StatusMark, STATUS_LABEL, SubagentSteer } from "./SubagentPanel";
import { TicketCard } from "./TicketCard";
import { lastHost, outcomeNote, statusPhrase, TASK_REPORT_PREFIX, taskPhrase, taskSteps } from "./toolStatus";
import { VisualFrame } from "./VisualFrame";

/** Fenced ```html visual / ```svg visual blocks; shown as rendered frames instead of text. */
const VISUAL_BLOCK = /```[a-z]+ visual[^\n]*\n[\s\S]*?```\n?/g;

const PREVIEW_LIMIT = 4000;

type ToolItem = Extract<TimelineItem, { type: "tool" }>;

function pretty(value: unknown): string {
  const text = typeof value === "string" ? value : JSON.stringify(value, null, 2);
  return text.length > PREVIEW_LIMIT ? `${text.slice(0, PREVIEW_LIMIT)}\n… (${text.length - PREVIEW_LIMIT} more characters)` : text;
}

interface ToolKind {
  Icon: LucideIcon;
  /** Group summary once every call of this kind has finished. */
  done: string;
  running: string;
}

const KINDS: { test: RegExp; kind: ToolKind }[] = [
  { test: /browser|navigate|tab|web|fetch|url/, kind: { Icon: Globe, done: "Browsed the web", running: "Browsing the web" } },
  { test: /search|grep|find|glob/, kind: { Icon: Search, done: "Searched", running: "Searching" } },
  { test: /write|edit|patch|save|create_file/, kind: { Icon: FilePenLine, done: "Edited files", running: "Editing files" } },
  { test: /read|list|file|ls/, kind: { Icon: FileText, done: "Read files", running: "Reading files" } },
  { test: /shell|bash|exec|command|terminal|run/, kind: { Icon: Terminal, done: "Ran commands", running: "Running commands" } },
  { test: /delegate|subagent|agent/, kind: { Icon: Users, done: "Delegated work", running: "Delegating work" } },
  { test: /skill/, kind: { Icon: BookOpen, done: "Loaded a skill", running: "Loading a skill" } },
  { test: /memory|remember|recall/, kind: { Icon: Brain, done: "Used memory", running: "Using memory" } },
  { test: /vault|secret|credential|otp/, kind: { Icon: KeyRound, done: "Used the vault", running: "Using the vault" } },
  { test: /ask|human|approval|ticket|takeover/, kind: { Icon: MessageCircleQuestion, done: "Asked you", running: "Asking you" } },
];

const DEFAULT_KIND: ToolKind = { Icon: CodeXml, done: "Ran actions", running: "Running actions" };

function kindOf(name: string): ToolKind {
  const lower = name.toLowerCase();
  return KINDS.find((entry) => entry.test.test(lower))?.kind ?? DEFAULT_KIND;
}

/** `browser_navigate` → "Browser navigate". */
function humanize(name: string): string {
  const words = name.replace(/[_.-]+/g, " ").trim();
  return words ? words.charAt(0).toUpperCase() + words.slice(1) : name;
}

function groupSummary(tools: ToolItem[]): string {
  const kinds = [...new Set(tools.map((tool) => kindOf(tool.name)))];
  const first = kinds[0] ?? DEFAULT_KIND;
  if (kinds.length > 1) return `Used ${tools.length} actions`;
  const host = first.Icon === Globe ? lastHost(tools) : null;
  return host ? `Browsed ${host}` : first.done;
}

/** The argument a person recognises a call by: the file, page, command or question it acts on. */
const TARGET_KEYS = ["path", "url", "command", "query", "pattern", "task", "question", "summary", "name"];

function targetOf(args: unknown): string | null {
  if (!args || typeof args !== "object") return null;
  const record = args as Record<string, unknown>;
  for (const key of TARGET_KEYS) {
    const value = record[key];
    if (typeof value === "string" && value.trim()) return value.trim().split("\n")[0]!;
  }
  return null;
}

function ToolAction({ item }: { item: ToolItem }) {
  const { Icon } = kindOf(item.name);
  const state = !item.done ? "running" : item.isError ? "failed" : "done";
  const target = targetOf(item.args);
  return (
    <li className={`tool tool-${state}`} data-tool={item.id}>
      <details className="tool-details">
        <summary className="tool-line">
          <span className="tool-icon" aria-hidden="true">
            {state === "running" ? <Spinner size={14} /> : state === "failed" ? <CircleAlert size={16} strokeWidth={ICON_STROKE} /> : <Icon size={16} strokeWidth={ICON_STROKE} />}
          </span>
          <span className="tool-name">{humanize(item.name)}</span>
          {target ? (
            <span className="tool-target" title={target}>
              {target}
            </span>
          ) : null}
          {state !== "done" ? <span className="tool-state">{state === "running" ? "Running" : "Failed"}</span> : null}
        </summary>
        <div className="tool-body">
          <h4>Arguments</h4>
          <pre>{pretty(item.args)}</pre>
          {item.updates.length ? (
            <>
              <h4>Progress</h4>
              {item.updates.map((update, index) => (
                <pre key={index}>{pretty(update)}</pre>
              ))}
            </>
          ) : null}
          {item.done ? (
            <>
              <h4>{item.isError ? "Error" : "Result"}</h4>
              <pre>{pretty(item.result)}</pre>
            </>
          ) : null}
        </div>
      </details>
    </li>
  );
}

const ERROR_TITLES: Record<string, string> = {
  interrupted: "The agent stopped unexpectedly",
  sandbox_unavailable: "Shell commands are running without a sandbox",
  browser_tab_limit: "Too many browser tabs are open",
};

function ActionList({ tools }: { tools: ToolItem[] }) {
  return (
    <ol className="activity-actions">
      {tools.map((tool) => (
        <ToolAction key={tool.id} item={tool} />
      ))}
    </ol>
  );
}

/** A finished run of tool calls: a quiet event chip ("✓ Browsed example.com") that opens the calls. */
function Activity({ tools }: { tools: ToolItem[] }) {
  const failed = tools.filter((tool) => tool.isError).length;
  return (
    <details className="activity">
      <summary className="event-chip">
        <span className="activity-icon" aria-hidden="true">
          {tools.some((tool) => !tool.done) ? (
            <Spinner size={13} />
          ) : failed ? (
            <CircleAlert size={15} strokeWidth={ICON_STROKE} />
          ) : (
            <Check size={15} strokeWidth={2.2} />
          )}
        </span>
        <span className="activity-label">{groupSummary(tools)}</span>
        {failed ? <span className="activity-failed">{failed} failed</span> : null}
        <ChevronRight className="activity-chevron" size={15} strokeWidth={ICON_STROKE} aria-hidden="true" />
      </summary>
      <ActionList tools={tools} />
    </details>
  );
}

/**
 * The transient line under the latest message while the agent works ("Clicking on
 * nid.naver.com"). With calls behind it, it opens to show them, like a finished chip.
 */
function StatusLine({ label, tools }: { label: string; tools?: ToolItem[] }) {
  const text = (
    <span className="status-line-text" key={label}>
      {label}
    </span>
  );
  if (!tools) return <p className="status-line">{text}</p>;
  return (
    <details className="activity status-line-group">
      <summary className="status-line">
        {text}
        <ChevronRight className="activity-chevron" size={15} strokeWidth={ICON_STROKE} aria-hidden="true" />
      </summary>
      <ActionList tools={tools} />
    </details>
  );
}

/**
 * A background task, where it was started: its goal and state on a chip that opens to its
 * steps, its final answer, and steer/cancel while it runs.
 */
function TaskChip({ sessionId, task }: { sessionId: string; task: SubagentView }) {
  const live = task.status === "running" || task.status === "waiting_human";
  const steps = taskSteps(task.progress);
  const state = task.status === "running" ? taskPhrase(task.progress) : STATUS_LABEL[task.status];
  return (
    <details className={`activity task task-${task.status}`} data-task={task.id}>
      <summary className="event-chip task-chip">
        <span className="activity-icon" aria-hidden="true">
          <StatusMark status={task.status} />
        </span>
        <span className="task-goal" title={task.task}>
          {task.task}
        </span>
        <span className={task.status === "running" ? "task-state status-line-text" : "task-state"} key={state}>
          {state}
        </span>
        <ChevronRight className="activity-chevron" size={15} strokeWidth={ICON_STROKE} aria-hidden="true" />
      </summary>
      <div className="task-body">
        {task.output ? (
          <div className="task-output">
            <Markdown text={task.output} />
          </div>
        ) : null}
        {task.reason && task.status !== "completed" ? <p className="task-reason">{task.reason}</p> : null}
        {steps.length ? (
          <ol className="task-steps" aria-label="Steps">
            {steps.map((step, index) => (
              <li key={index}>{step}</li>
            ))}
          </ol>
        ) : null}
        {live ? <SubagentSteer sessionId={sessionId} child={task} placeholder="Tell this task something" /> : null}
      </div>
    </details>
  );
}

const REPORT_LABEL: Partial<Record<SubagentView["status"], string>> = {
  completed: "Task finished",
  failed: "Task failed",
  cancelled: "Task cancelled",
};

export function Timeline({
  view,
  sessionId,
  visuals = {},
}: {
  view: SessionView;
  sessionId: string;
  visuals?: Record<number, Visual[]>;
}) {
  const rows: ReactNode[] = [];
  let tools: ToolItem[] = [];
  // The last flushed group of calls; the live status line takes its row when nothing follows it.
  const lastGroup = { row: -1, tools: [] as ToolItem[] };
  const flush = () => {
    const first = tools[0];
    if (first) {
      lastGroup.row = rows.length;
      lastGroup.tools = tools;
      rows.push(
        <li key={`t-${first.id}`} className="activity-row">
          <Activity tools={tools} />
        </li>,
      );
    }
    tools = [];
  };

  const tasks = new Map(view.subagents.filter((child) => child.background).map((child) => [child.parentToolCallId, child]));

  for (const item of view.items) {
    if (item.type === "tool") {
      const task = tasks.get(item.id);
      if (!task) {
        tools.push(item);
        continue;
      }
      flush();
      rows.push(
        <li key={`task-${task.id}`} className="activity-row task-row">
          <TaskChip sessionId={sessionId} task={task} />
        </li>,
      );
      continue;
    }
    flush();
    switch (item.type) {
      case "user": {
        if (item.id.startsWith(TASK_REPORT_PREFIX)) {
          // The daemon hands a finished task's result to the front agent as a message; it is not
          // the person's words, so it shows as a marker and the full answer stays on the chip.
          const task = view.subagents.find((child) => child.id === item.id.slice(TASK_REPORT_PREFIX.length));
          const status = task?.status ?? "completed";
          rows.push(
            <li key={`u-${item.id}`} className="task-report" data-status={status}>
              <span className="activity-icon" aria-hidden="true">
                {status === "failed" ? (
                  <CircleAlert size={15} strokeWidth={ICON_STROKE} />
                ) : status === "cancelled" ? (
                  <Ban size={15} strokeWidth={ICON_STROKE} />
                ) : (
                  <Check size={15} strokeWidth={2.2} />
                )}
              </span>
              <span>{REPORT_LABEL[status] ?? "Task update"}</span>
              {task ? (
                <span className="task-report-goal" title={task.task}>
                  {task.task}
                </span>
              ) : null}
            </li>,
          );
          break;
        }
        rows.push(
          <li key={`u-${item.id}`} className="bubble user">
            {item.attachments?.length ? (
              <ul className="attachments" aria-label="Attachments">
                {item.attachments.map((attachment) => (
                  <li key={attachment.path}>
                    {attachment.kind === "image" ? (
                      <ArtifactImage sessionId={sessionId} path={attachment.path} alt={attachment.name ?? attachment.path} />
                    ) : (
                      <span className="attachment-file">
                        <FileText size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
                        {attachment.name ?? attachment.path}
                      </span>
                    )}
                  </li>
                ))}
              </ul>
            ) : null}
            {item.text ? <p>{item.text}</p> : null}
            {item.outcome && item.outcome !== "completed" && item.outcome !== "deferred" ? (
              <span className="message-outcome">
                {outcomeNote(item.outcome, item.reason)}
              </span>
            ) : null}
          </li>,
        );
        break;
      }
      case "assistant": {
        const frames = item.finalSeq !== undefined ? (visuals[item.finalSeq] ?? []) : [];
        const text = frames.length ? item.text.replace(VISUAL_BLOCK, "").trim() : item.text;
        rows.push(
          <Fragment key={item.id}>
            <li className={`bubble assistant${item.final ? "" : " streaming"}`} data-final={item.final}>
              <Markdown text={text} />
            </li>
            {frames.length ? (
              <li className="visuals">
                {frames.map((visual) => (
                  <VisualFrame key={visual.id} visual={visual} />
                ))}
              </li>
            ) : null}
          </Fragment>,
        );
        break;
      }
      case "ticket": {
        const ticket = view.tickets[item.id];
        if (ticket)
          rows.push(
            <li key={`k-${item.id}`} className="ticket-row">
              {ticket.status === "pending" && (isOtpTicket(ticket) || isLoginTicket(ticket) || isChoiceTicket(ticket) || isApprovalTicket(ticket)) ? (
                <span className="action-chip" data-tone="warn">
                  <span className="action-chip-dot" aria-hidden="true" />
                  Action needed
                </span>
              ) : null}
              <TicketCard
                ticket={ticket}
                subagentTask={view.subagents.find((child) => child.id === ticket.origin.subagent_id)?.task}
              />
            </li>,
          );
        break;
      }
      case "error":
        rows.push(
          <li key={item.id} className="runtime-error" role="status">
            <CircleAlert size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
            <div>
              <p className="runtime-error-title">{ERROR_TITLES[item.code] ?? "Something went wrong"}</p>
              {item.message ? <p>{item.message}</p> : null}
              {item.code in ERROR_TITLES ? null : <p className="runtime-error-code">{item.code}</p>}
            </div>
          </li>,
        );
        break;
    }
  }
  flush();

  // A run with nothing new on screen (no streaming text, no card waiting for an answer) shows
  // what it is doing; a trailing group of calls becomes that line instead of a finished chip.
  const last = view.items.at(-1);
  const live =
    view.status === "running" && pendingTickets(view).length === 0 && (view.thinking || last?.type !== "assistant");
  if (live) {
    if (lastGroup.row >= 0 && lastGroup.row === rows.length - 1) {
      const host = lastHost(view.items);
      rows[lastGroup.row] = (
        <li key={`t-${lastGroup.tools[0]!.id}`} className="activity-row live-status" role="status">
          <StatusLine label={view.thinking ? "Thinking" : statusPhrase(lastGroup.tools, host)} tools={lastGroup.tools} />
        </li>
      );
    } else {
      rows.push(
        <li key="status" className="live-status" role="status">
          <StatusLine label="Thinking" />
        </li>,
      );
    }
  }

  return (
    <ol className="timeline" aria-live="polite">
      {rows}
    </ol>
  );
}
