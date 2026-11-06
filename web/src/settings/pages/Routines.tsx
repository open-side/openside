import { History, Pencil, Play, Plus, Trash2 } from "lucide-react";
import { useCallback, useEffect, useId, useMemo, useRef, useState, type FormEvent } from "react";
import { Link } from "react-router";
import type { Routine, RoutineDraft, RoutineSchedule, RunRecord, UpdateRoutineRequest } from "../../api/types";
import { useClient } from "../../context";
import { errorMessage, useResource } from "../../hooks/useResource";
import { t, type MessageKey } from "../../i18n";
import { Badge as StatusBadge, ErrorNote, Loading, ResourceView, RowMenu, useConfirm, useDialogState, type BadgeTone } from "../../pages/blocks";
import { describeSchedule, formatDateTime, formatRelative, weekdayName, RRULE_DAYS, WEEK_ORDER } from "../../pages/format";
import {
  Badge,
  Button,
  Chip,
  Dialog,
  DialogActions,
  ICON_STROKE,
  MenuItem,
  Segmented,
  Select,
  SettingsCard,
  SettingsRow,
  SettingsSection,
  Switch,
  useToast,
} from "../../ui";
import { defaultForm, formComplete, formFromSchedule, MAX_COUNT, scheduleFromForm, type Ends, type Repeat, type ScheduleForm } from "./routineSchedule";
import "./routines.css";

const RUN_STATUS: Record<RunRecord["status"], { label: MessageKey; tone: BadgeTone }> = {
  started: { label: "agents.routines.run.started", tone: "neutral" },
  completed: { label: "agents.routines.run.completed", tone: "success" },
  failed: { label: "agents.routines.run.failed", tone: "danger" },
  skipped_overlap: { label: "agents.routines.run.skippedOverlap", tone: "attention" },
  paused_target_missing: { label: "agents.routines.run.pausedTargetMissing", tone: "attention" },
  missed: { label: "agents.routines.run.missed", tone: "attention" },
};

const REPEATS: { value: Exclude<Repeat, "existing">; label: MessageKey }[] = [
  { value: "once", label: "agents.routines.repeat.once" },
  { value: "hourly", label: "agents.routines.repeat.hourly" },
  { value: "daily", label: "agents.routines.repeat.daily" },
  { value: "weekdays", label: "agents.routines.repeat.weekdays" },
  { value: "weekly", label: "agents.routines.repeat.weekly" },
  { value: "monthly", label: "agents.routines.repeat.monthly" },
  { value: "custom", label: "agents.routines.repeat.custom" },
];

const UNITS: Partial<Record<Repeat, MessageKey>> = {
  hourly: "agents.routines.unit.hours",
  daily: "agents.routines.unit.days",
  weekly: "agents.routines.unit.weeks",
  monthly: "agents.routines.unit.months",
};

// Starting a new chat is what a routine does by default; only other targets get a note.
const PROMPT_HINT: Record<Exclude<Routine["action"]["type"], "new_session">, MessageKey> = {
  heartbeat: "agents.routines.promptHint.heartbeat",
  agent: "agents.routines.promptHint.agent",
};

/** Every IANA zone the browser knows, with the chosen one kept even when the list lacks it (e.g. "UTC"). */
function zoneOptions(current: string) {
  const zones = Intl.supportedValuesOf("timeZone");
  return (zones.includes(current) ? zones : [current, ...zones]).map((zone) => ({ value: zone, label: zone }));
}

/** `routine` edits it; otherwise a new routine, prefilled from `draft` when a suggestion is used. */
type Editing = { routine?: Routine; draft?: RoutineDraft };

function RoutineDialog({ open, editing, onClose, onSaved }: { open: boolean; editing: Editing | null; onClose: () => void; onSaved: (name: string) => void }) {
  const client = useClient();
  const routine = editing?.routine;
  const original: RoutineSchedule | undefined = routine?.schedule ?? editing?.draft?.schedule;
  const [name, setName] = useState("");
  const [prompt, setPrompt] = useState("");
  const [form, setForm] = useState<ScheduleForm>(() => defaultForm());
  const [scheduleChanged, setScheduleChanged] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const id = useId();

  useEffect(() => {
    if (!open) return;
    setName(routine?.name ?? editing?.draft?.name ?? "");
    setPrompt(routine?.action.prompt ?? editing?.draft?.prompt ?? "");
    setForm(original ? formFromSchedule(original) : defaultForm());
    setScheduleChanged(false);
    setError(null);
  }, [open, routine, editing, original]);

  const zones = useMemo(() => zoneOptions(form.timezone), [form.timezone]);
  const change = (patch: Partial<ScheduleForm>) => {
    setForm((current) => ({ ...current, ...patch }));
    setScheduleChanged(true);
  };
  const repeatOptions = [
    ...REPEATS.map((repeat) => ({ value: repeat.value, label: t(repeat.label) })),
    ...(form.repeat === "existing" || original?.type === "cron" || original?.type === "interval" ? [{ value: "existing", label: t("agents.routines.repeat.existing") }] : []),
  ];
  const unit = UNITS[form.repeat];
  const timed = form.repeat !== "existing";
  const ends = timed && form.repeat !== "once" && form.repeat !== "custom";

  async function submit(event: FormEvent) {
    event.preventDefault();
    const schedule = scheduleFromForm(form, original);
    setBusy(true);
    setError(null);
    try {
      if (routine) {
        const patch: UpdateRoutineRequest = {};
        if (name.trim() !== routine.name) patch.name = name.trim();
        // The action keeps its kind and target; only the prompt is edited here.
        if (prompt !== routine.action.prompt) patch.action = { ...routine.action, prompt };
        if (scheduleChanged) patch.schedule = schedule;
        if (Object.keys(patch).length) {
          await client.updateRoutine(routine.id, patch);
          onSaved(name.trim());
        }
      } else {
        await client.createRoutine({ name: name.trim(), schedule, action: { type: "new_session", prompt } });
        onSaved(name.trim());
      }
      onClose();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={t(routine ? "agents.routines.editTitle" : "agents.routines.new")}
      description={routine ? undefined : t("agents.routines.dialogDescription")}
    >
      <form className="dialog-form routines-form" onSubmit={(event) => void submit(event)}>
        <label className="field">
          <span>{t("agents.field.name")}</span>
          <input value={name} autoComplete="off" onChange={(event) => setName(event.target.value)} />
        </label>
        <div className="field">
          <label htmlFor={`${id}-prompt`}>{t("agents.routines.field.prompt")}</label>
          <textarea
            id={`${id}-prompt`}
            rows={3}
            value={prompt}
            aria-describedby={routine && routine.action.type !== "new_session" ? `${id}-prompt-hint` : undefined}
            onChange={(event) => setPrompt(event.target.value)}
          />
          {routine && routine.action.type !== "new_session" ? (
            <small id={`${id}-prompt-hint`} className="routines-hint">
              {t(PROMPT_HINT[routine.action.type])}
            </small>
          ) : null}
        </div>
        <div className="field">
          <label htmlFor={`${id}-repeat`}>{t("agents.routines.field.repeat")}</label>
          <Select id={`${id}-repeat`} value={form.repeat} options={repeatOptions} onChange={(repeat) => change({ repeat: repeat as Repeat })} />
        </div>
        {form.repeat === "existing" && original ? (
          <p className="routines-hint">
            <span className="routines-existing">{describeSchedule(original)}</span> {t("agents.routines.existingHint")}
          </p>
        ) : null}
        {form.repeat === "custom" ? (
          <div className="field">
            <label htmlFor={`${id}-rule`}>{t("agents.routines.field.rule")}</label>
            <input
              id={`${id}-rule`}
              className="routines-mono"
              value={form.rule}
              spellCheck={false}
              autoCapitalize="none"
              placeholder="FREQ=WEEKLY;BYDAY=MO,TH;BYHOUR=9;BYMINUTE=0"
              aria-describedby={`${id}-rule-hint`}
              onChange={(event) => change({ rule: event.target.value })}
            />
            <small id={`${id}-rule-hint`} className="routines-hint">
              {t("agents.routines.ruleHint")}
            </small>
          </div>
        ) : null}
        {unit ? (
          <div className="field">
            <label htmlFor={`${id}-every`}>{t("agents.routines.field.every")}</label>
            <span className="routines-inline">
              <input id={`${id}-every`} type="number" min={1} value={form.interval} aria-describedby={`${id}-unit`} onChange={(event) => change({ interval: event.target.value })} />
              <span id={`${id}-unit`}>{t(unit, { count: Number(form.interval) || 1 })}</span>
            </span>
          </div>
        ) : null}
        {form.repeat === "weekly" ? (
          <div className="field" role="group" aria-labelledby={`${id}-days`}>
            <span id={`${id}-days`}>{t("agents.routines.field.days")}</span>
            <span className="routines-days">
              {WEEK_ORDER.map((day) => (
                <Chip
                  key={day}
                  aria-label={weekdayName(RRULE_DAYS.indexOf(day))}
                  pressed={form.days.includes(day)}
                  onClick={() => change({ days: form.days.includes(day) ? form.days.filter((chosen) => chosen !== day) : [...form.days, day] })}
                >
                  {weekdayName(RRULE_DAYS.indexOf(day), "short")}
                </Chip>
              ))}
            </span>
          </div>
        ) : null}
        {form.repeat === "monthly" ? (
          <label className="field">
            <span>{t("agents.routines.field.monthDay")}</span>
            <input type="number" min={1} max={31} value={form.monthDay} onChange={(event) => change({ monthDay: event.target.value })} />
          </label>
        ) : null}
        {timed ? (
          <div className="routines-inline routines-when">
            <label className="field">
              <span>{t(form.repeat === "once" ? "agents.routines.field.date" : "agents.routines.field.starts")}</span>
              <input type="date" value={form.date} onChange={(event) => change({ date: event.target.value })} />
            </label>
            <label className="field">
              <span>{t("agents.routines.field.time")}</span>
              <input type="time" value={form.time} onChange={(event) => change({ time: event.target.value })} />
            </label>
          </div>
        ) : null}
        {ends ? (
          <div className="field">
            <span aria-hidden="true">{t("agents.routines.field.ends")}</span>
            <Segmented<Ends>
              label={t("agents.routines.field.ends")}
              options={[
                { value: "never", label: t("agents.routines.ends.never") },
                { value: "on", label: t("agents.routines.ends.on") },
                { value: "after", label: t("agents.routines.ends.after") },
              ]}
              value={form.ends}
              onChange={(next) => change({ ends: next })}
            />
            {form.ends === "on" ? (
              <label className="field">
                <span>{t("agents.routines.field.until")}</span>
                <input type="date" value={form.until} min={form.date} onChange={(event) => change({ until: event.target.value })} />
              </label>
            ) : form.ends === "after" ? (
              <label className="field">
                <span>{t("agents.routines.field.count")}</span>
                <input type="number" min={1} max={MAX_COUNT} value={form.count} onChange={(event) => change({ count: event.target.value })} />
              </label>
            ) : null}
          </div>
        ) : null}
        {timed ? (
          <div className="field">
            <label htmlFor={`${id}-zone`}>{t("agents.routines.field.timezone")}</label>
            <Select id={`${id}-zone`} value={form.timezone} options={zones} onChange={(timezone) => change({ timezone })} />
          </div>
        ) : null}
        {error ? <ErrorNote>{error}</ErrorNote> : null}
        <DialogActions>
          <Button variant="ghost" onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button type="submit" variant="primary" disabled={busy || !name.trim() || !prompt.trim() || !formComplete(form)}>
            {busy ? t("common.saving") : t(routine ? "common.save" : "common.create")}
          </Button>
        </DialogActions>
      </form>
    </Dialog>
  );
}

function RunHistory({ routine }: { routine: Routine }) {
  const client = useClient();
  const load = useCallback(() => client.routineRuns(routine.id), [client, routine.id]);
  const runs = useResource(load);
  if (runs.error) return <ErrorNote>{runs.error}</ErrorNote>;
  if (!runs.data) return <Loading />;
  if (runs.data.runs.length === 0) return <p className="routines-hint">{t("agents.routines.noRuns")}</p>;
  return (
    <ol className="routines-runs" aria-label={t("agents.routines.runsLabel", { name: routine.name })}>
      {runs.data.runs.map((run) => (
        <li key={run.id} data-status={run.status}>
          <span className="routines-run-head">
            <span>{formatDateTime(run.scheduled_at)}</span>
            <StatusBadge tone={RUN_STATUS[run.status].tone}>{t(RUN_STATUS[run.status].label)}</StatusBadge>
          </span>
          {run.finished_at || run.detail || run.session_id ? (
            <p>
              {run.finished_at ? t("agents.routines.finished", { time: formatDateTime(run.finished_at) }) : null}
              {run.detail ? `${run.finished_at ? " · " : ""}${run.detail}` : null}
              {run.session_id ? (
                <>
                  {run.finished_at || run.detail ? " · " : null}
                  <Link to={`/sessions/${encodeURIComponent(run.session_id)}`}>{t("agents.routines.openConversation")}</Link>
                </>
              ) : null}
            </p>
          ) : null}
        </li>
      ))}
    </ol>
  );
}

/** Repeated requests the daemon noticed; shown only while there are some. */
function Suggestions({ onUse }: { onUse: (draft: RoutineDraft) => void }) {
  const client = useClient();
  const load = useCallback(() => client.routineSuggestions(), [client]);
  const suggestions = useResource(load);
  if (suggestions.error) return <ErrorNote>{suggestions.error}</ErrorNote>;
  if (!suggestions.data?.suggestions.length) return null;
  return (
    <SettingsSection label={t("agents.routines.suggestionsTitle")} info={t("agents.routines.suggestionsDescription")}>
      <SettingsCard className="routines-card">
        <ul aria-label={t("agents.routines.suggestionsLabel")}>
          {suggestions.data.suggestions.map((draft, index) => (
            <li key={`${draft.name}-${index}`}>
              <SettingsRow
                title={draft.name}
                description={t("agents.routines.suggestionDetail", { schedule: describeSchedule(draft.schedule), count: draft.occurrences })}
                control={<Button onClick={() => onUse(draft)}>{t("agents.routines.use")}</Button>}
              />
            </li>
          ))}
        </ul>
      </SettingsCard>
    </SettingsSection>
  );
}

/**
 * Settings › Routines (vm2/routines-0): "My routines", one card of routine rows ending in
 * "+ Create". Each row reads its schedule in words and its next run, Paused or Ended, with an
 * on/off switch and a menu for Run now, Edit, History and Delete. Suggestions follow when the
 * daemon found repeated requests.
 */
export function RoutinesSettings() {
  const client = useClient();
  const toast = useToast();
  const load = useCallback(() => client.listRoutines(), [client]);
  const list = useResource(load);
  const editor = useDialogState<Editing>();
  const history = useDialogState<Routine>();
  const [confirm, ask] = useConfirm();
  // Switch positions shown before the daemon answers; the latest change per routine wins.
  const [pending, setPending] = useState<Record<string, boolean>>({});
  const latest = useRef(new Map<string, number>());
  const queue = useRef<Promise<unknown>>(Promise.resolve());

  async function act(action: () => Promise<unknown>) {
    try {
      await action();
      await list.reload();
    } catch (cause) {
      list.setError(errorMessage(cause));
    }
  }

  function toggle(routine: Routine, enabled: boolean) {
    const ticket = (latest.current.get(routine.id) ?? 0) + 1;
    latest.current.set(routine.id, ticket);
    setPending((current) => ({ ...current, [routine.id]: enabled }));
    // One PATCH at a time, so the daemon sees the switch flips in the order they were made.
    queue.current = queue.current.then(async () => {
      await act(() => client.updateRoutine(routine.id, { enabled }));
      if (latest.current.get(routine.id) !== ticket) return;
      // Settled: show the daemon's value, which is the old one when the change was refused.
      setPending((current) => {
        const next = { ...current };
        delete next[routine.id];
        return next;
      });
    });
  }

  function row(routine: Routine) {
    const enabled = pending[routine.id] ?? routine.enabled;
    const settled = pending[routine.id] === undefined;
    const schedule = describeSchedule(routine.schedule);
    // An RRULE series that ran out (COUNT/UNTIL) stays enabled with no next run.
    const ended = settled && routine.enabled && !routine.next_run_at && routine.schedule.type === "rrule";
    const status = !enabled
      ? t(routine.paused_reason && settled ? "agents.routines.run.pausedTargetMissing" : "agents.routines.paused")
      : ended
        ? t("agents.routines.ended")
        : undefined;
    return (
      <li key={routine.id} data-routine={routine.id}>
        <SettingsRow
          title={routine.name}
          badge={status ? <Badge>{status}</Badge> : undefined}
          description={enabled && settled && routine.next_run_at ? t("agents.routines.detailNext", { schedule, when: formatRelative(routine.next_run_at) }) : schedule}
          control={
            <>
              <Switch label={t("agents.routines.enable", { name: routine.name })} checked={enabled} onChange={(next) => toggle(routine, next)} />
              <RowMenu name={routine.name}>
                <MenuItem
                  icon={<Play size={18} strokeWidth={ICON_STROKE} />}
                  onClick={() =>
                    void act(async () => {
                      const run = await client.runRoutine(routine.id);
                      const outcome = run.status === "started" ? t("agents.routines.runStarted") : t(RUN_STATUS[run.status].label);
                      toast.notify(t("agents.routines.runToast", { name: routine.name, status: outcome }));
                    })
                  }
                >
                  {t("agents.routines.runNow")}
                </MenuItem>
                <MenuItem icon={<Pencil size={18} strokeWidth={ICON_STROKE} />} onClick={() => editor.show({ routine })}>
                  {t("common.edit")}
                </MenuItem>
                <MenuItem icon={<History size={18} strokeWidth={ICON_STROKE} />} onClick={() => history.show(routine)}>
                  {t("agents.routines.history")}
                </MenuItem>
                <MenuItem
                  danger
                  icon={<Trash2 size={18} strokeWidth={ICON_STROKE} />}
                  onClick={() =>
                    ask({
                      title: t("agents.routines.deleteTitle"),
                      description: t("agents.routines.deleteDescription", { name: routine.name }),
                      confirmLabel: t("common.delete"),
                      onConfirm: () => void act(() => client.deleteRoutine(routine.id)),
                    })
                  }
                >
                  {t("common.delete")}
                </MenuItem>
              </RowMenu>
            </>
          }
        />
      </li>
    );
  }

  return (
    <div className="routines-page" data-section="routines">
      <SettingsSection label={t("agents.routines.section")}>
        <ResourceView resource={list} what={t("agents.routines.what")}>
          {(data) => (
            <SettingsCard className="routines-card">
              {data.routines.length ? <ul aria-label={t("agents.routines.listLabel")}>{data.routines.map(row)}</ul> : null}
              <button type="button" className="routines-create" onClick={() => editor.show({})}>
                <Plus strokeWidth={ICON_STROKE} aria-hidden="true" />
                {t("common.create")}
              </button>
            </SettingsCard>
          )}
        </ResourceView>
      </SettingsSection>
      <Suggestions onUse={(draft) => editor.show({ draft })} />
      <RoutineDialog
        open={editor.open}
        editing={editor.value}
        onClose={editor.close}
        onSaved={(name) => {
          toast.notify(t(editor.value?.routine ? "agents.routines.saved" : "agents.routines.scheduled", { name }));
          void list.reload();
        }}
      />
      <Dialog open={history.open} onClose={history.close} title={history.value ? t("agents.routines.runsLabel", { name: history.value.name }) : ""}>
        {history.value ? <RunHistory routine={history.value} /> : null}
      </Dialog>
      {confirm}
    </div>
  );
}
