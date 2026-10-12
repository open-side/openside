import {
  BookOpen,
  Brain,
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
import type { SessionView, TimelineItem } from "../state/session";
import { ICON_STROKE, Spinner } from "../ui";
import { ArtifactImage } from "./ArtifactImage";
import { Markdown } from "./Markdown";
import { TicketCard } from "./TicketCard";
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

function groupSummary(tools: ToolItem[]): { Icon: LucideIcon; label: string } {
  const running = tools.some((tool) => !tool.done);
  const kinds = [...new Set(tools.map((tool) => kindOf(tool.name)))];
  const first = kinds[0] ?? DEFAULT_KIND;
  if (kinds.length === 1) return { Icon: first.Icon, label: running ? first.running : first.done };
  const count = `${tools.length} actions`;
  return { Icon: DEFAULT_KIND.Icon, label: running ? `Working · ${count}` : `Used ${count}` };
}

function ToolAction({ item }: { item: ToolItem }) {
  const { Icon } = kindOf(item.name);
  const state = !item.done ? "running" : item.isError ? "failed" : "done";
  return (
    <li className={`tool tool-${state}`} data-tool={item.id}>
      <div className="tool-line">
        <span className="tool-icon" aria-hidden="true">
          {state === "running" ? <Spinner size={14} /> : state === "failed" ? <CircleAlert size={16} strokeWidth={ICON_STROKE} /> : <Icon size={16} strokeWidth={ICON_STROKE} />}
        </span>
        <span className="tool-name">{humanize(item.name)}</span>
        <span className="tool-state">{state === "running" ? "Running" : state === "failed" ? "Failed" : "Completed"}</span>
      </div>
      <details className="tool-details">
        <summary>Technical details</summary>
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

function Activity({ tools }: { tools: ToolItem[] }) {
  const { Icon, label } = groupSummary(tools);
  const running = tools.some((tool) => !tool.done);
  const failed = tools.filter((tool) => tool.isError).length;
  return (
    <details className="activity" data-running={running}>
      <summary>
        <span className="activity-icon" aria-hidden="true">
          {running ? <Spinner size={14} /> : <Icon size={16} strokeWidth={ICON_STROKE} />}
        </span>
        <span className="activity-label">{label}</span>
        {failed ? <span className="activity-failed">{failed} failed</span> : null}
        <ChevronRight className="activity-chevron" size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
      </summary>
      <ol className="activity-actions">
        {tools.map((tool) => (
          <ToolAction key={tool.id} item={tool} />
        ))}
      </ol>
    </details>
  );
}

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
  const flush = () => {
    const first = tools[0];
    if (first) rows.push(
      <li key={`t-${first.id}`} className="activity-row">
        <Activity tools={tools} />
      </li>,
    );
    tools = [];
  };

  for (const item of view.items) {
    if (item.type === "tool") {
      tools.push(item);
      continue;
    }
    flush();
    switch (item.type) {
      case "user":
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
                {item.outcome}
                {item.reason ? `: ${item.reason}` : ""}
              </span>
            ) : null}
          </li>,
        );
        break;
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
            <span>
              <strong>{item.code}</strong> {item.message}
            </span>
          </li>,
        );
        break;
    }
  }
  flush();

  return (
    <ol className="timeline" aria-live="polite">
      {rows}
    </ol>
  );
}
