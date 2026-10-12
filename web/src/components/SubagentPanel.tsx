import { Ban, Check, ChevronRight, CircleAlert } from "lucide-react";
import { useState, type FormEvent } from "react";
import { ApiError } from "../api/client";
import { useClient } from "../context";
import type { SubagentView } from "../state/session";
import { Button, ICON_STROKE, Spinner } from "../ui";

const STATUS_LABEL: Record<SubagentView["status"], string> = {
  running: "Running",
  waiting_human: "Needs you",
  completed: "Completed",
  failed: "Failed",
  cancelled: "Cancelled",
};

function describe(progress: unknown): string {
  if (progress && typeof progress === "object" && "kind" in progress) {
    const { kind, ...rest } = progress as { kind: unknown } & Record<string, unknown>;
    const detail = Object.keys(rest).length ? ` ${JSON.stringify(rest)}` : "";
    return `${String(kind)}${detail}`;
  }
  return JSON.stringify(progress);
}

function StatusMark({ status }: { status: SubagentView["status"] }) {
  switch (status) {
    case "running":
      return <Spinner size={16} />;
    case "waiting_human":
      return <span className="attention-ring" aria-hidden="true" />;
    case "completed":
      return <Check size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />;
    case "failed":
      return <CircleAlert size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />;
    case "cancelled":
      return <Ban size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />;
  }
}

function SubagentRow({ sessionId, child }: { sessionId: string; child: SubagentView }) {
  const client = useClient();
  const [steer, setSteer] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const live = child.status === "running" || child.status === "waiting_human";

  async function act(action: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (cause) {
      setError(cause instanceof ApiError ? cause.message : "Request failed");
    } finally {
      setBusy(false);
    }
  }

  function sendSteer(event: FormEvent) {
    event.preventDefault();
    const text = steer.trim();
    if (!text) return;
    void act(async () => {
      await client.steerSubagent(sessionId, child.id, text);
      setSteer("");
    });
  }

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
              <span className={`subagent-status status-${child.status}`}>{STATUS_LABEL[child.status]}</span>
              <span aria-hidden="true"> · </span>
              <span className="subagent-count" aria-label="Progress events">
                {child.progress.length}
              </span>
              <span aria-hidden="true"> events</span>
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
          {child.progress.length ? (
            <ol className="subagent-events" aria-label="Subagent events">
              {child.progress.map((progress, index) => (
                <li key={index}>
                  <code>{describe(progress)}</code>
                </li>
              ))}
            </ol>
          ) : null}
          {live ? (
            <form className="subagent-steer" onSubmit={sendSteer}>
              <label>
                <span className="sr-only">Steer {child.id}</span>
                <input value={steer} onChange={(e) => setSteer(e.target.value)} placeholder="Steer this subagent" disabled={busy} />
              </label>
              <Button type="submit" size="compact" variant="primary" disabled={busy || !steer.trim()}>
                Steer
              </Button>
              <Button size="compact" variant="danger" disabled={busy} onClick={() => act(() => client.cancelSubagent(sessionId, child.id))}>
                Cancel
              </Button>
            </form>
          ) : null}
          {error ? (
            <p role="alert" className="subagent-error">
              {error}
            </p>
          ) : null}
        </div>
      </details>
    </li>
  );
}

/** Children of this session, rebuilt from `subagent.*` events like the rest of the screen. */
export function SubagentPanel({ sessionId, subagents }: { sessionId: string; subagents: SubagentView[] }) {
  if (!subagents.length) return null;
  const active = subagents.filter((child) => child.status === "running" || child.status === "waiting_human").length;
  return (
    <section className="panel subagents" aria-label="Subagents">
      <h3>
        Agents <span className="subagents-count">{active ? `${active} active` : `${subagents.length} finished`}</span>
      </h3>
      <ul>
        {subagents.map((child) => (
          <SubagentRow key={child.id} sessionId={sessionId} child={child} />
        ))}
      </ul>
    </section>
  );
}
