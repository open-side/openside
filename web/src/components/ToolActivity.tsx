import { ChevronDown, CircleAlert } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { t } from "../i18n";
import { ICON_STROKE } from "../ui";
import { describeTool, opensPage, technicalView, type ToolFactsMap, type ToolItem } from "./toolCopy";
import "./toolActivity.css";

const PREVIEW_LIMIT = 4000;

function pretty(value: unknown): string {
  const text = typeof value === "string" ? value : JSON.stringify(value, null, 2);
  return text.length > PREVIEW_LIMIT ? `${text.slice(0, PREVIEW_LIMIT)}\n${t("timeline.activity.moreCharacters", { count: text.length - PREVIEW_LIMIT })}` : text;
}

/** One call of a turn: its icon and sentence, opening onto a monospace card of what it took and gave. */
function StepRow({ item, facts }: { item: ToolItem; facts: ToolFactsMap }) {
  const known = facts.get(item.id);
  const { title, detail, Icon } = describeTool(item, known);
  const state = !item.done ? "running" : item.isError ? "failed" : "done";
  return (
    <li className="step" data-state={state} data-tool={item.id}>
      <details className="step-details">
        <summary className="step-line">
          <span className="step-icon" aria-hidden="true">
            {state === "failed" ? (
              <CircleAlert size={14} strokeWidth={ICON_STROKE} />
            ) : known?.favicon && opensPage(item) ? (
              <img className="step-favicon" src={known.favicon} alt="" width={14} height={14} />
            ) : (
              <Icon size={14} strokeWidth={ICON_STROKE} />
            )}
          </span>
          <span className="step-title">{title}</span>
          {detail ? (
            <span className="step-detail" title={detail}>
              {detail}
            </span>
          ) : null}
          {state === "failed" ? <span className="step-failed">{t("timeline.activity.toolFailed")}</span> : null}
        </summary>
        <div className="step-card">
          <h4>{t("timeline.activity.arguments")}</h4>
          <pre tabIndex={0}>{pretty(technicalView(item, item.args))}</pre>
          {item.updates.length ? (
            <>
              <h4>{t("timeline.activity.progress")}</h4>
              {item.updates.map((update, index) => (
                <pre key={index} tabIndex={0}>
                  {pretty(technicalView(item, update))}
                </pre>
              ))}
            </>
          ) : null}
          {item.done ? (
            <>
              <h4>{t(item.isError ? "timeline.activity.error" : "timeline.activity.result")}</h4>
              <pre tabIndex={0}>{pretty(technicalView(item, item.result))}</pre>
            </>
          ) : null}
        </div>
      </details>
    </li>
  );
}

/** What a turn did before its reply, in order: calls, and the words the agent wrote between them. */
export type WorkEntry = { kind: "tool"; item: ToolItem } | { kind: "note"; id: string; node: ReactNode };

/**
 * A turn's work under one line: "Working for 4s", open while the agent works with "Thinking"
 * under the newest step, then "Worked for 12s", folded, above the reply.
 */
export function WorkGroup({ entries, facts, label, live }: { entries: readonly WorkEntry[]; facts: ToolFactsMap; label: string; live: boolean }) {
  const [open, setOpen] = useState(live);
  // A finished turn folds itself away; a reader's own toggle holds until the next change.
  useEffect(() => setOpen(live), [live]);
  const running = entries.some((entry) => entry.kind === "tool" && !entry.item.done);
  const empty = entries.length === 0;
  if (empty && !live) {
    return <p className="work-summary work-summary-static">{label}</p>;
  }
  return (
    <details className="work" data-live={live || undefined} open={open} onToggle={(event) => setOpen(event.currentTarget.open)}>
      <summary className="work-summary">
        <span>{label}</span>
        <ChevronDown className="work-chevron" size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
      </summary>
      <ol className="work-steps">
        {entries.map((entry) =>
          entry.kind === "tool" ? (
            <StepRow key={entry.item.id} item={entry.item} facts={facts} />
          ) : (
            <li key={entry.id} className="work-note">
              {entry.node}
            </li>
          ),
        )}
        {live && !running ? (
          <li className="work-thinking" role="status">
            <span className="shimmer-text">{t("timeline.status.thinking")}</span>
          </li>
        ) : null}
      </ol>
    </details>
  );
}
