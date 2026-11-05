import { ChevronRight, CircleAlert } from "lucide-react";
import { ICON_STROKE, Spinner } from "../ui";
import {
  describeTool,
  groupSentence,
  isPageStep,
  opensPage,
  technicalView,
  toolKind,
  workedLabel,
  type ToolFactsMap,
  type ToolItem,
  type ToolKindId,
} from "./toolCopy";
import "./toolActivity.css";

const PREVIEW_LIMIT = 4000;

function pretty(value: unknown): string {
  const text = typeof value === "string" ? value : JSON.stringify(value, null, 2);
  return text.length > PREVIEW_LIMIT ? `${text.slice(0, PREVIEW_LIMIT)}\n… (${text.length - PREVIEW_LIMIT} more characters)` : text;
}

function ToolRow({ item, facts, step = false }: { item: ToolItem; facts: ToolFactsMap; step?: boolean }) {
  const known = facts.get(item.id);
  const { title, detail, Icon } = describeTool(item, known);
  const state = !item.done ? "running" : item.isError ? "failed" : "done";
  const navigating = state === "running" && opensPage(item);
  return (
    <li className={`tool tool-${state}${step ? " tool-step" : ""}`} data-tool={item.id}>
      <details className="tool-details">
        <summary className="tool-line">
          <span className="tool-icon" aria-hidden="true">
            {state === "failed" ? (
              <CircleAlert size={14} strokeWidth={ICON_STROKE} />
            ) : known?.favicon && opensPage(item) ? (
              <img className="tool-favicon" src={known.favicon} alt="" width={14} height={14} />
            ) : (
              <Icon size={14} strokeWidth={ICON_STROKE} />
            )}
          </span>
          <span className="tool-title">{title}</span>
          {detail ? (
            <span className="tool-detail" title={detail}>
              {detail}
            </span>
          ) : null}
          {/* Success is the quiet default; only a running or failed call says so. The span stays to keep the chevron at the end. */}
          <span className="tool-state">{state === "running" ? <Spinner size={13} /> : state === "failed" ? "Failed" : null}</span>
          <ChevronRight className="tool-chevron" size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
          {navigating ? <span className="tool-progress" aria-hidden="true" /> : null}
        </summary>
        <div className="tool-card">
          <h4>Arguments</h4>
          <pre tabIndex={0}>{pretty(technicalView(item, item.args))}</pre>
          {item.updates.length ? (
            <>
              <h4>Progress</h4>
              {item.updates.map((update, index) => (
                <pre key={index} tabIndex={0}>
                  {pretty(technicalView(item, update))}
                </pre>
              ))}
            </>
          ) : null}
          {item.done ? (
            <>
              <h4>{item.isError ? "Error" : "Result"}</h4>
              <pre tabIndex={0}>{pretty(technicalView(item, item.result))}</pre>
            </>
          ) : null}
        </div>
      </details>
    </li>
  );
}

/** A page visit: the calls that open a page, with the reading and clicking done on it beneath. */
export interface Visit {
  head: ToolItem;
  steps: ToolItem[];
}

export function visits(tools: readonly ToolItem[]): Visit[] {
  const result: Visit[] = [];
  let current: Visit | null = null;
  for (const tool of tools) {
    if (opensPage(tool)) {
      current = { head: tool, steps: [] };
      result.push(current);
    } else if (current && isPageStep(tool)) {
      current.steps.push(tool);
    } else {
      current = null;
      result.push({ head: tool, steps: [] });
    }
  }
  return result;
}

/** Consecutive visits of one kind: an opened run reads as a few sentences, not a wall of calls. */
export interface ActivityGroup {
  kind: ToolKindId;
  visits: Visit[];
}

export function activityGroups(tools: readonly ToolItem[]): ActivityGroup[] {
  const groups: ActivityGroup[] = [];
  for (const visit of visits(tools)) {
    const kind = toolKind(visit.head.name);
    const last = groups.at(-1);
    if (last?.kind === kind) last.visits.push(visit);
    else groups.push({ kind, visits: [visit] });
  }
  return groups;
}

function ActionList({ tools, facts }: { tools: ToolItem[]; facts: ToolFactsMap }) {
  return (
    <div className="activity-groups">
      {activityGroups(tools).map((group) => {
        const calls = group.visits.flatMap(({ head, steps }) => [head, ...steps]);
        return (
          <section key={group.visits[0]!.head.id} className="activity-group">
            {/* A lone call is its own sentence; a heading would only repeat it. */}
            {calls.length > 1 ? <p className="activity-group-title">{groupSentence(calls, facts)}</p> : null}
            <ol className="activity-actions">
              {group.visits.map(({ head, steps }) => (
                <ToolGroup key={head.id} head={head} steps={steps} facts={facts} />
              ))}
            </ol>
          </section>
        );
      })}
    </div>
  );
}

function ToolGroup({ head, steps, facts }: Visit & { facts: ToolFactsMap }) {
  return (
    <>
      <ToolRow item={head} facts={facts} />
      {steps.length ? (
        <li className="tool-steps-item">
          <ol className="tool-steps">
            {steps.map((step) => (
              <ToolRow key={step.id} item={step} facts={facts} step />
            ))}
          </ol>
        </li>
      ) : null}
    </>
  );
}

/**
 * A run of tool calls folded to one quiet line ("Worked for 12s") that opens onto its groups.
 * The time comes from the daemon's event times only; without them the line says "Worked".
 */
export function Activity({ tools, facts, inline = false }: { tools: ToolItem[]; facts: ToolFactsMap; inline?: boolean }) {
  const failed = tools.filter((tool) => tool.isError).length;
  const running = tools.some((tool) => !tool.done);
  return (
    <details className={`activity${inline ? " activity-inline" : ""}`}>
      <summary className="activity-summary">
        {running || failed ? (
          <span className="activity-icon" aria-hidden="true">
            {running ? <Spinner size={13} /> : <CircleAlert size={15} strokeWidth={ICON_STROKE} />}
          </span>
        ) : null}
        <span className="activity-label">{running ? "Working" : workedLabel(tools)}</span>
        {failed ? <span className="activity-failed">{failed} failed</span> : null}
        <ChevronRight className="activity-chevron" size={15} strokeWidth={ICON_STROKE} aria-hidden="true" />
      </summary>
      <ActionList tools={tools} facts={facts} />
    </details>
  );
}

/**
 * The transient line under the latest message while the agent works ("Clicking on
 * nid.naver.com"): borderless muted text; with calls behind it, it opens to show them.
 */
export function StatusLine({ label, tools, facts }: { label: string; tools?: ToolItem[]; facts?: ToolFactsMap }) {
  const text = (
    <span className="status-line-text" key={label}>
      {label}
    </span>
  );
  if (!tools || !facts) return <p className="status-line">{text}</p>;
  return (
    <details className="activity status-line-group">
      <summary className="status-line">
        {text}
        <ChevronRight className="activity-chevron" size={15} strokeWidth={ICON_STROKE} aria-hidden="true" />
      </summary>
      <ActionList tools={tools} facts={facts} />
    </details>
  );
}
