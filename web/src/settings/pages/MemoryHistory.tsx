import { Hourglass, Pencil, Plus, Trash2, type LucideIcon } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { ApiError } from "../../api/client";
import type { MemoryChange } from "../../api/types";
import { useClient } from "../../context";
import { errorMessage } from "../../hooks/useResource";
import { t, type MessageKey } from "../../i18n";
import { ErrorNote, Loading } from "../../pages/blocks";
import { formatDateTime, formatRelative } from "../../pages/format";
import { ScopeNotice } from "../../pages/ScopeNotice";
import { Button, cx, ICON_STROKE } from "../../ui";
import "./memory.css";

const PAGE = 50;

const OPS: Record<MemoryChange["op"], { label: MessageKey; Icon: LucideIcon }> = {
  create: { label: "memory.history.op.create", Icon: Plus },
  update: { label: "memory.history.op.update", Icon: Pencil },
  delete: { label: "memory.history.op.delete", Icon: Trash2 },
  purge: { label: "memory.history.op.purge", Icon: Hourglass },
};

function preview(change: MemoryChange): string {
  if (change.content == null) return t("memory.history.previewRemoved");
  return change.content.replace(/\s+/g, " ").trim();
}

function Detail({ change }: { change: MemoryChange }) {
  const { label, Icon } = OPS[change.op];
  const fields: [MessageKey, string][] = [
    ["memory.history.field.when", formatDateTime(change.at)],
    ["memory.history.field.actor", change.actor],
    ["memory.history.field.memory", change.memory_id],
    ["memory.history.field.version", String(change.version)],
  ];
  if (change.kind) fields.push(["memory.history.field.kind", t(change.kind === "history" ? "memory.history.kind.history" : "memory.history.kind.note")]);
  if (change.content != null) fields.push(["memory.history.field.tags", change.tags?.length ? change.tags.join(", ") : t("memory.history.noTags")]);
  return (
    <section className="memory-detail-pane" aria-label={t("memory.history.detail")}>
      <header className="memory-pane-head memory-detail-head">
        <Icon size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
        <h2 className="memory-detail-title">{t(label)}</h2>
        <span className="memory-updated">
          <time dateTime={change.at}>{formatRelative(change.at)}</time>
        </span>
      </header>
      <div className="memory-detail-body">
        <div className="memory-doc">
          <dl className="memory-fields">
            {fields.map(([key, value]) => (
              <div key={key}>
                <dt>{t(key)}</dt>
                <dd>{value}</dd>
              </div>
            ))}
          </dl>
          <h3 className="memory-field-heading">{t("memory.history.field.content")}</h3>
          {change.content != null ? <p className="memory-content">{change.content}</p> : <p className="memory-note">{t("memory.history.removed")}</p>}
        </div>
      </div>
    </section>
  );
}

/**
 * Settings › Memory › History: every memory change newest first (`GET /v1/memory/history`),
 * older pages on request, and the chosen entry on the right.
 */
export function MemoryHistory() {
  const client = useClient();
  const [changes, setChanges] = useState<MemoryChange[] | null>(null);
  const [next, setNext] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [forbidden, setForbidden] = useState(false);
  const [busy, setBusy] = useState(false);
  const [selected, setSelected] = useState<number | null>(null);

  const fetchPage = useCallback(
    async (before?: number) => {
      setBusy(true);
      try {
        const page = await client.memoryChanges(before === undefined ? { limit: PAGE } : { limit: PAGE, before });
        setChanges((current) => (before === undefined ? page.changes : [...(current ?? []), ...page.changes]));
        setNext(page.next_before ?? null);
        setError(null);
        setForbidden(false);
      } catch (cause) {
        setError(errorMessage(cause));
        setForbidden(cause instanceof ApiError && cause.status === 403);
      } finally {
        setBusy(false);
      }
    },
    [client],
  );

  useEffect(() => {
    void fetchPage();
  }, [fetchPage]);

  if (forbidden) {
    return (
      <div className="memory-scope">
        <ScopeNotice what={t("memory.what")} onGranted={() => void fetchPage()} />
      </div>
    );
  }

  const chosen = changes?.find((change) => change.seq === selected);
  return (
    <div className="memory-panes" data-section="memory-history">
      <div className="memory-list-pane">
        <div className="memory-list-scroll">
          {error ? <ErrorNote>{error}</ErrorNote> : null}
          {changes === null ? (
            error ? null : <Loading />
          ) : changes.length === 0 ? (
            <p className="memory-list-empty">{t("memory.history.empty")}</p>
          ) : (
            <ul className="memory-list" aria-label={t("memory.history.list")}>
              {changes.map((change) => {
                const { label, Icon } = OPS[change.op];
                const current = change.seq === selected;
                return (
                  <li key={change.seq}>
                    <button
                      type="button"
                      className={cx("memory-list-row", "memory-change-row", current && "memory-list-row--selected")}
                      aria-current={current ? "true" : undefined}
                      onClick={() => setSelected(change.seq)}
                    >
                      <span className="memory-change-line">
                        <Icon size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
                        <span className="memory-change-op">{t(label)}</span>
                        <time className="memory-change-time" dateTime={change.at}>
                          {formatRelative(change.at)}
                        </time>
                      </span>
                      <span className={cx("memory-change-preview", change.content == null && "memory-change-preview--removed")}>{preview(change)}</span>
                      <span className="memory-change-actor">{change.actor}</span>
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
          {next !== null ? (
            <div className="memory-list-more">
              <Button size="small" disabled={busy} onClick={() => void fetchPage(next)}>
                {t("memory.history.loadOlder")}
              </Button>
            </div>
          ) : null}
        </div>
      </div>
      {chosen ? (
        <Detail key={chosen.seq} change={chosen} />
      ) : (
        <div className="memory-detail-pane memory-detail-empty">
          <p>{t("memory.history.select")}</p>
        </div>
      )}
    </div>
  );
}
