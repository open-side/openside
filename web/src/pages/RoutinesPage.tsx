import { CalendarClock, Play, Trash2 } from "lucide-react";
import { useCallback, useState, type FormEvent } from "react";
import { Link } from "react-router";
import type { CreateRoutineRequest, Routine, RoutineDraft, RoutineSchedule, RunRecord } from "../api/types";
import { useClient } from "../context";
import { errorMessage, useResource } from "../hooks/useResource";
import { Button, Dialog, DialogActions, ICON_STROKE, MenuItem, Segmented, Switch, useToast } from "../ui";
import {
  AddButton,
  Badge,
  Disclosure,
  EmptyState,
  ErrorNote,
  Loading,
  ResourceCard,
  ResourceView,
  RowMenu,
  SectionList,
  useConfirm,
  useDialogState,
  type BadgeTone,
} from "./blocks";
import { describeSchedule, formatDateTime, formatRelative } from "./format";

const RUN_STATUS: Record<RunRecord["status"], { label: string; tone: BadgeTone }> = {
  started: { label: "Running", tone: "neutral" },
  completed: { label: "Completed", tone: "success" },
  failed: { label: "Failed", tone: "danger" },
  skipped_overlap: { label: "Skipped: still running", tone: "attention" },
  paused_target_missing: { label: "Paused: conversation gone", tone: "attention" },
  missed: { label: "Missed", tone: "attention" },
};

function Runs({ routine }: { routine: Routine }) {
  const client = useClient();
  const load = useCallback(() => client.routineRuns(routine.id), [client, routine.id]);
  const runs = useResource(load);
  return (
    <ol className="card-history" aria-label={`Runs of ${routine.name}`}>
      {runs.error ? (
        <li>
          <ErrorNote>{runs.error}</ErrorNote>
        </li>
      ) : !runs.data ? (
        <li>
          <Loading />
        </li>
      ) : runs.data.runs.length === 0 ? (
        <li>
          <p className="settings-text">No runs yet.</p>
        </li>
      ) : (
        runs.data.runs.map((run) => (
          <li key={run.id} data-status={run.status}>
            <span>{formatDateTime(run.scheduled_at)}</span>
            <Badge tone={RUN_STATUS[run.status].tone}>{RUN_STATUS[run.status].label}</Badge>
            {run.finished_at || run.detail || run.session_id ? (
              <p>
                {run.finished_at ? `Finished ${formatDateTime(run.finished_at)}` : null}
                {run.detail ? `${run.finished_at ? " · " : ""}${run.detail}` : null}
                {run.session_id ? (
                  <>
                    {run.finished_at || run.detail ? " · " : null}
                    <Link to={`/sessions/${encodeURIComponent(run.session_id)}`}>Open conversation</Link>
                  </>
                ) : null}
              </p>
            ) : null}
          </li>
        ))
      )}
    </ol>
  );
}

function RoutineForm({ draft, onCreated, onCancel }: { draft?: RoutineDraft; onCreated: (name: string) => void; onCancel: () => void }) {
  const client = useClient();
  const [name, setName] = useState(draft?.name ?? "");
  const [kind, setKind] = useState<"cron" | "interval">(draft?.schedule.type ?? "cron");
  const [expr, setExpr] = useState(draft?.schedule.type === "cron" ? draft.schedule.expr : "0 9 * * *");
  const [timezone, setTimezone] = useState(draft?.schedule.type === "cron" ? (draft.schedule.timezone ?? "") : "");
  const [secs, setSecs] = useState(draft?.schedule.type === "interval" ? String(draft.schedule.secs) : "3600");
  const [prompt, setPrompt] = useState(draft?.prompt ?? "");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(event: FormEvent) {
    event.preventDefault();
    const schedule: RoutineSchedule =
      kind === "cron"
        ? { type: "cron", expr: expr.trim(), timezone: timezone.trim() || null }
        : { type: "interval", secs: Number(secs) };
    const request: CreateRoutineRequest = { name: name.trim(), schedule, action: { type: "new_session", prompt } };
    setBusy(true);
    setError(null);
    try {
      await client.createRoutine(request);
      onCreated(request.name);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="dialog-form" onSubmit={submit} aria-label={draft ? `Create routine from ${draft.name}` : "New routine"}>
      <label className="field">
        <span>Name</span>
        <input value={name} onChange={(e) => setName(e.target.value)} required />
      </label>
      <Segmented
        label="Schedule"
        options={[
          { value: "cron", label: "Cron expression" },
          { value: "interval", label: "Fixed interval" },
        ]}
        value={kind}
        onChange={setKind}
      />
      {kind === "cron" ? (
        <>
          <label className="field">
            <span>Cron (minute hour day month weekday)</span>
            <input value={expr} onChange={(e) => setExpr(e.target.value)} required />
          </label>
          <label className="field">
            <span>Time zone (IANA, empty = UTC)</span>
            <input value={timezone} onChange={(e) => setTimezone(e.target.value)} placeholder="Europe/Berlin" />
          </label>
        </>
      ) : (
        <label className="field">
          <span>Every (seconds)</span>
          <input type="number" min={1} value={secs} onChange={(e) => setSecs(e.target.value)} required />
        </label>
      )}
      <label className="field">
        <span>Prompt for a new conversation</span>
        <textarea rows={3} value={prompt} onChange={(e) => setPrompt(e.target.value)} required />
      </label>
      {error ? <ErrorNote>{error}</ErrorNote> : null}
      <DialogActions>
        <Button onClick={onCancel}>Cancel</Button>
        <Button type="submit" variant="primary" disabled={busy || !name.trim() || !prompt.trim()}>
          Create
        </Button>
      </DialogActions>
    </form>
  );
}

function Suggestions({ onUse }: { onUse: (draft: RoutineDraft) => void }) {
  const client = useClient();
  const load = useCallback(() => client.routineSuggestions(), [client]);
  const suggestions = useResource(load);
  if (suggestions.error) return <ErrorNote>{suggestions.error}</ErrorNote>;
  if (!suggestions.data || suggestions.data.suggestions.length === 0) return null;
  return (
    <SectionList
      title="Suggestions"
      label="Suggested routines"
      description="Repeated requests found in your conversations. Nothing is scheduled until you create it."
    >
      <ul className="resource-list">
        {suggestions.data.suggestions.map((draft, index) => (
          <li key={`${draft.name}-${index}`}>
            <ResourceCard
              title={draft.name}
              detail={`${describeSchedule(draft.schedule)} · seen ${draft.occurrences} times`}
              trailing={
                <Button size="compact" onClick={() => onUse(draft)}>
                  Use
                </Button>
              }
            >
              <p className="resource-card-body">“{draft.prompt}”</p>
            </ResourceCard>
          </li>
        ))}
      </ul>
    </SectionList>
  );
}

/** Scheduled routines: create, pause/resume, run now, run history, delete, suggestions. */
export function RoutinesPage() {
  const client = useClient();
  const toast = useToast();
  const load = useCallback(() => client.listRoutines(), [client]);
  const list = useResource(load);
  const [open, setOpen] = useState<string | null>(null);
  // `null` value = a blank routine; a draft prefills the form from a suggestion.
  const form = useDialogState<{ draft?: RoutineDraft }>();
  const [confirm, ask] = useConfirm();

  async function act(action: () => Promise<unknown>) {
    try {
      await action();
      await list.reload();
    } catch (cause) {
      list.setError(errorMessage(cause));
    }
  }

  const count = list.data?.routines.length ?? 0;
  return (
    <div className="manage-body" data-section="routines">
      <SectionList
        label="Scheduled routines"
        summary={list.data && count ? `${count} ${count === 1 ? "routine" : "routines"}` : undefined}
        actions={list.forbidden ? undefined : <AddButton onClick={() => form.show({})}>New routine</AddButton>}
      >
        <ResourceView resource={list} what="routines">
          {(data) =>
            data.routines.length === 0 ? (
              <EmptyState icon={CalendarClock} title="No routines yet" detail="A routine starts a conversation with your prompt on a schedule." />
            ) : (
              <ul className="resource-list" aria-label="Routines">
                {data.routines.map((routine) => (
                  <li key={routine.id}>
                    <ResourceCard
                      data-routine={routine.id}
                      title={routine.name}
                      detail={
                        routine.next_run_at
                          ? `${describeSchedule(routine.schedule)} · next ${formatRelative(routine.next_run_at)}`
                          : describeSchedule(routine.schedule)
                      }
                      badge={routine.enabled ? undefined : routine.paused_reason ? "Paused: conversation gone" : "Paused"}
                      badgeTone="attention"
                      trailing={
                        <>
                          <Switch
                            label={`Enable ${routine.name}`}
                            checked={routine.enabled}
                            onChange={(enabled) => void act(() => client.updateRoutine(routine.id, { enabled }))}
                          />
                          <RowMenu name={routine.name}>
                            <MenuItem
                              icon={<Play size={18} strokeWidth={ICON_STROKE} />}
                              onClick={() =>
                                void act(async () => {
                                  const run = await client.runRoutine(routine.id);
                                  toast.notify(`${routine.name}: ${run.status === "started" ? "run started" : RUN_STATUS[run.status].label}`);
                                })
                              }
                            >
                              Run now
                            </MenuItem>
                            <MenuItem
                              danger
                              icon={<Trash2 size={18} strokeWidth={ICON_STROKE} />}
                              onClick={() =>
                                ask({
                                  title: "Delete routine?",
                                  description: `${routine.name} stops running. Its past conversations stay.`,
                                  confirmLabel: "Delete",
                                  onConfirm: () => void act(() => client.deleteRoutine(routine.id)),
                                })
                              }
                            >
                              Delete
                            </MenuItem>
                          </RowMenu>
                        </>
                      }
                    >
                      <p className="resource-card-body">
                        {routine.action.type === "heartbeat" ? "In an existing conversation: " : null}“{routine.action.prompt}”
                      </p>
                      <Disclosure expanded={open === routine.id} onToggle={() => setOpen(open === routine.id ? null : routine.id)}>
                        Run history
                      </Disclosure>
                      {open === routine.id ? <Runs routine={routine} /> : null}
                    </ResourceCard>
                  </li>
                ))}
              </ul>
            )
          }
        </ResourceView>
      </SectionList>
      <Suggestions onUse={(draft) => form.show({ draft })} />

      <Dialog
        open={form.open}
        onClose={form.close}
        title={form.value?.draft ? "Create routine" : "New routine"}
        description="Starts a new conversation with this prompt on the schedule."
      >
        {form.value && (
          <RoutineForm
            draft={form.value.draft}
            onCancel={form.close}
            onCreated={(name) => {
              form.close();
              toast.notify(`${name} scheduled.`);
              void list.reload();
            }}
          />
        )}
      </Dialog>
      {confirm}
    </div>
  );
}
