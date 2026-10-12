import { CalendarClock } from "lucide-react";
import { useCallback, useState, type FormEvent } from "react";
import { Link } from "react-router";
import type { CreateRoutineRequest, Routine, RoutineDraft, RoutineSchedule } from "../api/types";
import { useClient } from "../context";
import { errorMessage, useResource } from "../hooks/useResource";
import { Button, Dialog, DialogActions, Segmented, useToast } from "../ui";
import {
  AddButton,
  CardActions,
  EmptyState,
  ErrorNote,
  Loading,
  ResourceCard,
  SectionList,
  useConfirm,
  useDialogState,
} from "./blocks";

function describeSchedule(schedule: RoutineSchedule): string {
  if (schedule.type === "cron") return `cron ${schedule.expr}${schedule.timezone ? ` (${schedule.timezone})` : " (UTC)"}`;
  return `every ${schedule.secs} s`;
}

function describeAction(routine: Routine): string {
  const action = routine.action;
  return action.type === "new_session" ? `New conversation: ${action.prompt}` : `Message ${action.session_id}: ${action.prompt}`;
}

function when(value?: string | null): string {
  return value ? new Date(value).toLocaleString() : "—";
}

function Runs({ routine }: { routine: Routine }) {
  const client = useClient();
  const load = useCallback(() => client.routineRuns(routine.id), [client, routine.id]);
  const runs = useResource(load);
  if (runs.error) return <ErrorNote>{runs.error}</ErrorNote>;
  if (!runs.data) return <Loading />;
  if (runs.data.runs.length === 0) return <p className="settings-text">No runs yet.</p>;
  return (
    <ol className="card-history" aria-label={`Runs of ${routine.name}`}>
      {runs.data.runs.map((run) => (
        <li key={run.id} data-status={run.status}>
          <span>{when(run.scheduled_at)}</span>
          <span className="badge badge--muted">{run.status}</span>
          {run.finished_at || run.detail || run.session_id ? (
            <p>
              {run.finished_at ? `Finished ${when(run.finished_at)}` : null}
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
      ))}
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
            <ResourceCard title={draft.name} detail={`${describeSchedule(draft.schedule)} · seen ${draft.occurrences} times`}>
              <p className="resource-card-body">“{draft.prompt}”</p>
              <CardActions>
                <Button size="compact" onClick={() => onUse(draft)}>
                  Use
                </Button>
              </CardActions>
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

  return (
    <div className="manage-body" data-section="routines">
      <SectionList label="Scheduled routines" actions={<AddButton onClick={() => form.show({})}>New routine</AddButton>}>
        {list.error ? <ErrorNote>{list.error}</ErrorNote> : null}
        {!list.data ? (
          list.error ? null : <Loading />
        ) : list.data.routines.length === 0 ? (
          <EmptyState icon={CalendarClock} title="No routines yet" detail="A routine starts a conversation with your prompt on a schedule." />
        ) : (
          <ul className="resource-list" aria-label="Routines">
            {list.data.routines.map((routine) => (
              <li key={routine.id}>
                <ResourceCard
                  data-routine={routine.id}
                  title={routine.name}
                  detail={describeSchedule(routine.schedule)}
                  badge={routine.enabled ? undefined : `Paused${routine.paused_reason ? `: ${routine.paused_reason}` : ""}`}
                  badgeTone="attention"
                >
                  <p className="resource-card-body">{describeAction(routine)}</p>
                  <p className="resource-card-detail">Next run: {when(routine.next_run_at)}</p>
                  <CardActions>
                    <Button
                      size="compact"
                      onClick={() =>
                        void act(async () => {
                          const run = await client.runRoutine(routine.id);
                          toast.notify(`${routine.name}: run ${run.status}`);
                        })
                      }
                    >
                      Run now
                    </Button>
                    <Button size="compact" onClick={() => void act(() => client.updateRoutine(routine.id, { enabled: !routine.enabled }))}>
                      {routine.enabled ? "Pause" : "Resume"}
                    </Button>
                    <Button
                      size="compact"
                      aria-expanded={open === routine.id}
                      onClick={() => setOpen(open === routine.id ? null : routine.id)}
                    >
                      Runs
                    </Button>
                    <Button
                      size="compact"
                      variant="danger"
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
                    </Button>
                  </CardActions>
                  {open === routine.id ? <Runs routine={routine} /> : null}
                </ResourceCard>
              </li>
            ))}
          </ul>
        )}
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
