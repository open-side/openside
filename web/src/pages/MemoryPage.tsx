import { Brain, Pencil, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState, type FormEvent } from "react";
import { ApiError } from "../api/client";
import type { Memory, MemoryKind, MemorySearchResponse, Retention } from "../api/types";
import { useClient } from "../context";
import { errorMessage, useResource } from "../hooks/useResource";
import { Button, Dialog, DialogActions, ICON_STROKE, MenuItem, Segmented, useToast } from "../ui";
import {
  AddButton,
  Badge,
  Disclosure,
  EmptyState,
  ErrorNote,
  Loading,
  ResourceCard,
  RowMenu,
  ScopeNotice,
  SectionList,
  useConfirm,
  useDialogState,
} from "./blocks";
import { formatDate, formatDateTime } from "./format";

const RETENTIONS: { value: Retention; label: string }[] = [
  { value: "permanent", label: "Keep forever" },
  { value: "days_90", label: "90 days after last edit" },
  { value: "days_30", label: "30 days after last edit" },
];

function tagsFrom(text: string): string[] {
  return text
    .split(",")
    .map((tag) => tag.trim())
    .filter(Boolean);
}

function History({ memory }: { memory: Memory }) {
  const client = useClient();
  const load = useCallback(() => client.memoryHistory(memory.id), [client, memory.id]);
  const history = useResource(load);
  if (history.error) return <ErrorNote>{history.error}</ErrorNote>;
  if (!history.data) return <Loading />;
  return (
    <ol className="card-history" aria-label={`History of ${memory.id}`}>
      {[...history.data.versions].reverse().map((version) => (
        <li key={version.version}>
          <span>
            <Badge>{version.op}</Badge> by {version.actor}
          </span>
          <span className="status-row-value">{formatDateTime(version.at)}</span>
          {version.content != null ? <p>{version.content}</p> : null}
        </li>
      ))}
    </ol>
  );
}

/** Create (no `memory`) or edit one memory: content, tags and retention. */
function MemoryForm({ memory, onSaved, onCancel }: { memory?: Memory; onSaved: () => void; onCancel: () => void }) {
  const client = useClient();
  const [content, setContent] = useState(memory?.content ?? "");
  const [tags, setTags] = useState(memory?.tags.join(", ") ?? "");
  const [retention, setRetention] = useState<Retention>(memory?.retention ?? "permanent");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function save(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const fields = { content, tags: tagsFrom(tags), retention };
      if (memory) await client.updateMemory(memory.id, fields);
      else await client.createMemory(fields);
      onSaved();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="dialog-form" onSubmit={save} aria-label={memory ? `Edit memory ${memory.id}` : "New memory"}>
      <label className="field">
        <span>Content</span>
        <textarea rows={4} value={content} onChange={(e) => setContent(e.target.value)} required />
      </label>
      <label className="field">
        <span>Tags (comma separated)</span>
        <input value={tags} onChange={(e) => setTags(e.target.value)} />
      </label>
      <label className="field">
        <span>Retention</span>
        <select value={retention} onChange={(e) => setRetention(e.target.value as Retention)}>
          {RETENTIONS.map((option) => (
            <option key={option.value} value={option.value}>
              {option.label}
            </option>
          ))}
        </select>
      </label>
      {error ? <ErrorNote>{error}</ErrorNote> : null}
      <DialogActions>
        <Button onClick={onCancel}>Cancel</Button>
        <Button type="submit" variant="primary" disabled={busy || !content.trim()}>
          {memory ? "Save" : "Remember"}
        </Button>
      </DialogActions>
    </form>
  );
}

/** Local memory: search, create, edit, delete, per-item history and retention. */
export function MemoryPage() {
  const client = useClient();
  const toast = useToast();
  const [query, setQuery] = useState("");
  const [kind, setKind] = useState<MemoryKind | "all">("all");
  const [result, setResult] = useState<MemorySearchResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [forbidden, setForbidden] = useState(false);
  const [historyOf, setHistoryOf] = useState<string | null>(null);
  // `{}` opens a blank form; `{ memory }` edits that memory.
  const form = useDialogState<{ memory?: Memory }>();
  const [confirm, ask] = useConfirm();
  const loadHealth = useCallback(() => client.memoryHealth(), [client]);
  const health = useResource(loadHealth);

  const search = useCallback(async () => {
    try {
      setResult(await client.searchMemory({ q: query, kind: kind === "all" ? undefined : kind }));
      setError(null);
      setForbidden(false);
    } catch (cause) {
      setError(errorMessage(cause));
      setForbidden(cause instanceof ApiError && cause.status === 403);
    }
  }, [client, query, kind]);

  useEffect(() => {
    // Debounce typing; every keystroke would otherwise be a search request.
    const timer = setTimeout(() => void search(), 200);
    return () => clearTimeout(timer);
  }, [search]);

  async function refresh() {
    await search();
    await health.reload();
  }

  if (forbidden || health.forbidden) {
    return (
      <div className="manage-body" data-section="memory">
        <ScopeNotice what="memory" />
      </div>
    );
  }

  return (
    <div className="manage-body" data-section="memory">
      <SectionList
        label="Saved memories"
        summary={
          health.data ? (
            <span aria-label="Memory status">
              {health.data.count} {health.data.count === 1 ? "memory" : "memories"} ·{" "}
              {health.data.mode === "hybrid" ? "hybrid search (local embeddings)" : "keyword search"}
            </span>
          ) : undefined
        }
        description={health.data?.next_step || undefined}
        actions={<AddButton onClick={() => form.show({})}>New memory</AddButton>}
      >
        <div className="section-toolbar">
          <label className="search-field">
            <span className="sr-only">Search memory</span>
            <input type="search" value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search memory" />
          </label>
          <Segmented
            kind="tab"
            label="Kind"
            options={[
              { value: "all", label: "All" },
              { value: "note", label: "Notes" },
              { value: "history", label: "Visited pages" },
            ]}
            value={kind}
            onChange={setKind}
          />
        </div>
        {error ? <ErrorNote>{error}</ErrorNote> : null}
        {!result ? (
          error ? null : <Loading />
        ) : result.hits.length === 0 ? (
          query ? (
            <p className="settings-text">No memories match “{query}”.</p>
          ) : (
            <EmptyState icon={Brain} title="No memories yet" detail="Saved context the agent can recall in later conversations." />
          )
        ) : (
          <ul className="resource-list" aria-label="Memories">
            {result.hits.map(({ memory }) => (
              <li key={memory.id}>
                <ResourceCard
                  data-memory={memory.id}
                  className={memory.kind === "note" ? "memory-note" : undefined}
                  title={memory.kind === "history" ? memory.title || "Visited page" : memory.content}
                  detail={[
                    memory.kind === "history" && memory.url ? memory.url : null,
                    RETENTIONS.find((option) => option.value === memory.retention)?.label ?? memory.retention,
                    memory.expires_at ? `expires ${formatDate(memory.expires_at)}` : null,
                  ]
                    .filter(Boolean)
                    .join(" · ")}
                  badge={memory.kind === "history" ? "Visited page" : undefined}
                  trailing={
                    <RowMenu name={memory.kind === "history" ? memory.title || memory.url || "visited page" : memory.content.slice(0, 40)}>
                      <MenuItem icon={<Pencil size={18} strokeWidth={ICON_STROKE} />} onClick={() => form.show({ memory })}>
                        Edit
                      </MenuItem>
                      <MenuItem
                        danger
                        icon={<Trash2 size={18} strokeWidth={ICON_STROKE} />}
                        onClick={() =>
                          ask({
                            title: "Delete memory?",
                            description: "Its history keeps only who deleted it and when.",
                            confirmLabel: "Delete",
                            onConfirm: () =>
                              void client.deleteMemory(memory.id).then(refresh, (cause: unknown) => setError(errorMessage(cause))),
                          })
                        }
                      >
                        Delete
                      </MenuItem>
                    </RowMenu>
                  }
                >
                  {memory.kind === "history" ? <p className="resource-card-body">{memory.content}</p> : null}
                  {memory.tags.length ? (
                    <div className="resource-card-meta">
                      {memory.tags.map((tag) => (
                        <span key={tag} className="tag">
                          #{tag}
                        </span>
                      ))}
                    </div>
                  ) : null}
                  <Disclosure expanded={historyOf === memory.id} onToggle={() => setHistoryOf(historyOf === memory.id ? null : memory.id)}>
                    Change history
                  </Disclosure>
                  {historyOf === memory.id ? <History memory={memory} /> : null}
                </ResourceCard>
              </li>
            ))}
          </ul>
        )}
      </SectionList>

      <Dialog open={form.open} onClose={form.close} title={form.value?.memory ? "Edit memory" : "New memory"}>
        {form.value && (
          <MemoryForm
            memory={form.value.memory}
            onCancel={form.close}
            onSaved={() => {
              form.close();
              toast.notify(form.value?.memory ? "Memory saved." : "Memory added.");
              void refresh();
            }}
          />
        )}
      </Dialog>
      {confirm}
    </div>
  );
}
