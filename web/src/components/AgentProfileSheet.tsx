import { useEffect, useState, type FormEvent } from "react";
import { Link, useLocation } from "react-router";
import { ApiError } from "../api/client";
import type { Agent, AgentChannelBinding, ChannelAccount, ModelAccountRef, ModelCatalog, PolicyMode, ProviderPoolList, Routine, SkillSummary, UpdateAgentRequest } from "../api/types";
import { useClient } from "../context";
import { useTeam, CHIEF_ID } from "../hooks/useAgents";
import { accountsOf, refKey, sameRef } from "../models/accounts";
import { providerLabel } from "../models/providers";
import { ErrorNote } from "../pages/blocks";
import { describeSchedule } from "../pages/format";
import { manageLinkState } from "../pages/manageRoutes";
import { Button, Dialog, DialogActions, Segmented, Select, Switch, useToast } from "../ui";
import { ACCENT_COLORS } from "./avatarTone";
import { Avatar } from "./Avatar";
import { ModelPicker } from "./ModelPicker";
import { AgentConnections } from "./AgentConnections";
import "./team.css";

const CHANNEL_LABEL: Record<string, string> = { telegram: "Telegram", slack: "Slack", discord: "Discord" };

const MODES: { value: PolicyMode; label: string }[] = [
  { value: "read-only", label: "Read only" },
  { value: "guard", label: "Ask first" },
  { value: "full", label: "Full" },
];

/** What the profile form edits; `null` on an override means "use the default". */
export interface ProfileForm {
  name: string;
  initials: string;
  color: string;
  role: string;
  voice: string;
  responsibilities: string;
  boundaries: string;
  model: string;
  accountPool: ModelAccountRef[] | null;
  channel: AgentChannelBinding | null;
  mode: PolicyMode;
  skills: string[] | null;
}

export function formOf(agent: Agent): ProfileForm {
  return {
    name: agent.name,
    initials: agent.initials,
    color: agent.color,
    role: agent.persona.role,
    voice: agent.persona.voice,
    responsibilities: agent.persona.responsibilities.join("\n"),
    boundaries: agent.persona.boundaries.join("\n"),
    model: agent.model ?? "",
    accountPool: agent.account_pool ?? null,
    channel: agent.channel ?? null,
    mode: agent.mode,
    skills: agent.skills ?? null,
  };
}

const lines = (text: string) =>
  text
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);

const sameJson = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

/**
 * Only what changed goes to the daemon: persona and visibility edits need less authority than
 * skills, model, accounts, channel or a wider mode, so an untouched field must not be sent.
 */
export function profilePatch(agent: Agent, form: ProfileForm): UpdateAgentRequest {
  const patch: UpdateAgentRequest = {};
  if (form.name.trim() !== agent.name) patch.name = form.name.trim();
  if (form.initials.trim() !== agent.initials) patch.initials = form.initials.trim();
  if (form.color !== agent.color) patch.color = form.color;
  const persona = { role: form.role.trim(), voice: form.voice.trim(), responsibilities: lines(form.responsibilities), boundaries: lines(form.boundaries) };
  if (!sameJson(persona, agent.persona)) patch.persona = persona;
  const model = form.model.trim() || null;
  if (model !== (agent.model ?? null)) patch.model = model;
  if (!sameJson(form.accountPool, agent.account_pool ?? null)) patch.account_pool = form.accountPool;
  if (!sameJson(form.channel, agent.channel ?? null)) patch.channel = form.channel;
  if (form.mode !== agent.mode) patch.mode = form.mode;
  if (!sameJson(form.skills, agent.skills ?? null)) patch.skills = form.skills;
  return patch;
}

interface Options {
  catalog: ModelCatalog | null;
  pools: ProviderPoolList | null;
  skills: SkillSummary[] | null;
  routines: Routine[] | null;
  channels: ChannelAccount[] | null;
}

const NO_OPTIONS: Options = { catalog: null, pools: null, skills: null, routines: null, channels: null };

/** Everything that defines a person on the team, editable in one sheet. */
export function AgentProfileSheet({ agent, open, onClose, onRetire }: { agent: Agent; open: boolean; onClose: () => void; onRetire: () => void }) {
  const client = useClient();
  const team = useTeam();
  const toast = useToast();
  const location = useLocation();
  const [form, setForm] = useState<ProfileForm>(() => formOf(agent));
  const [options, setOptions] = useState<Options>(NO_OPTIONS);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const isChief = agent.kind === "chief";
  const retired = agent.status === "retired";

  // Reopening starts from what the daemon has now, not from an abandoned edit.
  useEffect(() => {
    if (!open) return;
    setForm(formOf(agent));
    setError(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- reset per opening, not per roster sync
  }, [open, agent.id, agent.persona_version]);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    // Each list is optional: a device without the scope still edits the persona.
    const settle = <K extends keyof Options>(key: K, promise: Promise<Options[K]>) =>
      promise.then((value) => !cancelled && setOptions((current) => ({ ...current, [key]: value }))).catch(() => undefined);
    void settle("catalog", client.modelCatalog());
    void settle("pools", client.listPools());
    void settle("skills", client.listSkills().then((list) => list.skills));
    void settle("routines", client.listRoutines().then((list) => list.routines));
    void settle("channels", client.channelAccounts().then((list) => list.accounts));
    return () => {
      cancelled = true;
    };
  }, [client, open]);

  const set = <K extends keyof ProfileForm>(key: K, value: ProfileForm[K]) => setForm((current) => ({ ...current, [key]: value }));
  const patch = profilePatch(agent, form);
  const dirty = Object.keys(patch).length > 0;

  async function save(event: FormEvent) {
    event.preventDefault();
    if (!dirty) return onClose();
    setBusy(true);
    setError(null);
    try {
      await client.updateAgent(agent.id, patch);
      team.refresh();
      toast.notify(`Saved. ${form.name.trim()} uses it from the next message.`, "success");
      onClose();
    } catch (cause) {
      setError(cause instanceof ApiError ? cause.message : "Could not save the profile");
    } finally {
      setBusy(false);
    }
  }

  const owned = options.routines?.filter((routine) => routine.action.type === "agent" && routine.action.agent_id === agent.id) ?? [];
  const accountGroups = (options.catalog?.providers ?? []).flatMap((provider) => {
    const pool = options.pools?.pools.find((candidate) => candidate.provider === provider.provider)?.accounts ?? [];
    const rows = accountsOf(provider, pool, [], []).filter((row) => row.poolIndex !== null);
    return rows.length ? [{ provider: provider.provider, rows }] : [];
  });
  const channelValue = form.channel ? `${form.channel.kind}:${form.channel.account_id}` : "";

  return (
    <Dialog open={open} onClose={onClose} title={isChief ? "Your assistant" : "Profile"} size="wide" className="profile-sheet">
      <form className="dialog-form profile-form" onSubmit={(event) => void save(event)} aria-label={`${agent.name}'s profile`}>
        <div className="profile-identity">
          <Avatar initials={form.initials || "?"} color={form.color} size={64} />
          <div className="profile-identity-fields">
            <label className="field">
              <span>Name</span>
              <input value={form.name} maxLength={24} required disabled={retired} onChange={(event) => set("name", event.target.value)} />
            </label>
            <label className="field profile-initials">
              <span>Initials</span>
              <input value={form.initials} maxLength={3} required disabled={retired} onChange={(event) => set("initials", event.target.value)} />
            </label>
          </div>
        </div>
        <fieldset className="profile-colors" disabled={retired}>
          <legend>Colour</legend>
          {ACCENT_COLORS.map((color) => (
            <label key={color} className="profile-color">
              <input type="radio" name="color" value={color} checked={form.color === color} onChange={() => set("color", color)} />
              <Avatar initials="" color={color} size={24} />
              <span className="sr-only">{color}</span>
            </label>
          ))}
        </fieldset>
        <label className="field">
          <span>Role</span>
          <input value={form.role} maxLength={60} required disabled={retired} placeholder="Watches prices and plans for you" onChange={(event) => set("role", event.target.value)} />
        </label>
        <label className="field">
          <span>Voice</span>
          <textarea rows={2} value={form.voice} maxLength={100} disabled={retired} placeholder="Brief and upbeat. Leads with the number that changed." onChange={(event) => set("voice", event.target.value)} />
        </label>
        <label className="field">
          <span>Responsibilities · one per line</span>
          <textarea rows={3} value={form.responsibilities} required={!isChief} disabled={retired} onChange={(event) => set("responsibilities", event.target.value)} />
        </label>
        <label className="field">
          <span>Boundaries · one per line</span>
          <textarea rows={2} value={form.boundaries} disabled={retired} placeholder="Ask before anything you can’t undo" onChange={(event) => set("boundaries", event.target.value)} />
        </label>

        <section className="profile-section" aria-labelledby="profile-model">
          <h3 id="profile-model">Model</h3>
          <div className="profile-row">
            <span className="profile-row-label">Model</span>
            <ModelPicker value={form.model} onChange={(model) => set("model", model)} />
          </div>
          <div className="profile-row">
            <span className="profile-row-label" id="profile-pool-label">
              Use every account
            </span>
            <Switch
              label="Use every account"
              checked={form.accountPool === null}
              disabled={retired}
              onChange={(all) => set("accountPool", all ? null : accountGroups.flatMap((group) => group.rows.map((row) => row.ref)))}
            />
          </div>
          {form.accountPool !== null ? (
            accountGroups.length === 0 ? (
              <p className="muted profile-note">No provider has an account in its rotation yet.</p>
            ) : (
              accountGroups.map((group) => (
                <fieldset key={group.provider} className="profile-accounts">
                  <legend>{providerLabel(group.provider)}</legend>
                  {group.rows.map((row) => {
                    const checked = form.accountPool?.some((ref) => sameRef(ref, row.ref)) ?? false;
                    return (
                      <label key={refKey(row.ref)} className="profile-check">
                        <input
                          type="checkbox"
                          checked={checked}
                          onChange={() =>
                            set("accountPool", checked ? (form.accountPool ?? []).filter((ref) => !sameRef(ref, row.ref)) : [...(form.accountPool ?? []), row.ref])
                          }
                        />
                        <span>{row.label}</span>
                      </label>
                    );
                  })}
                </fieldset>
              ))
            )
          ) : null}
        </section>

        <section className="profile-section" aria-labelledby="profile-reach">
          <h3 id="profile-reach">Permissions and reach</h3>
          <Segmented label="Permission ceiling" options={MODES} value={form.mode} onChange={(mode) => set("mode", mode)} />
          <p className="muted profile-note">Never more than {isChief ? "the default policy" : "your assistant"} allows.</p>
          <div className="profile-row">
            <label className="profile-row-label" htmlFor="profile-channel">
              Messaging channel
            </label>
            <Select
              id="profile-channel"
              variant="inline"
              value={channelValue}
              disabled={retired}
              onChange={(value) => {
                const account = options.channels?.find((candidate) => `${candidate.kind}:${candidate.id}` === value);
                set("channel", account ? { kind: account.kind, account_id: account.id } : null);
              }}
              options={[
                { value: "", label: "None" },
                ...(options.channels ?? []).map((account) => ({
                  value: `${account.kind}:${account.id}`,
                  label: account.label || account.id,
                  detail: CHANNEL_LABEL[account.kind] ?? account.kind,
                  disabled: !!account.agent_id && account.agent_id !== agent.id,
                })),
              ]}
            />
          </div>
        </section>

        <section className="profile-section" aria-labelledby="profile-skills">
          <h3 id="profile-skills">Skills</h3>
          <div className="profile-row">
            <span className="profile-row-label">Every enabled skill</span>
            <Switch
              label="Every enabled skill"
              checked={form.skills === null}
              disabled={retired}
              onChange={(all) => set("skills", all ? null : (options.skills ?? []).filter((skill) => skill.enabled).map((skill) => skill.name))}
            />
          </div>
          {form.skills !== null ? (
            <fieldset className="profile-accounts">
              <legend className="sr-only">Allowed skills</legend>
              {(options.skills ?? []).length === 0 ? <p className="muted profile-note">No skills yet.</p> : null}
              {(options.skills ?? []).map((skill) => {
                const checked = form.skills?.includes(skill.name) ?? false;
                return (
                  <label key={skill.name} className="profile-check">
                    <input
                      type="checkbox"
                      checked={checked}
                      onChange={() => set("skills", checked ? (form.skills ?? []).filter((name) => name !== skill.name) : [...(form.skills ?? []), skill.name])}
                    />
                    <span>{skill.name}</span>
                  </label>
                );
              })}
            </fieldset>
          ) : null}
        </section>

        {retired ? null : (
          <section className="profile-section" aria-labelledby="profile-connections">
            <h3 id="profile-connections">Connections</h3>
            <AgentConnections agentId={agent.id} agentName={agent.name} />
          </section>
        )}

        <section className="profile-section" aria-labelledby="profile-routines">
          <h3 id="profile-routines">Routines and memory</h3>
          {owned.length === 0 ? (
            <p className="muted profile-note">{agent.name} owns no routines.</p>
          ) : (
            <ul className="profile-routines">
              {owned.map((routine) => (
                <li key={routine.id}>
                  <span>{routine.name}</span>
                  <span className="muted">{describeSchedule(routine.schedule)}</span>
                </li>
              ))}
            </ul>
          )}
          <div className="profile-links">
            <Link to="/routines" state={manageLinkState(location)} className="action-link">
              Routines
            </Link>
            <Link to="/memory" state={manageLinkState(location)} className="action-link">
              Memory
            </Link>
          </div>
        </section>

        {error ? <ErrorNote>{error}</ErrorNote> : null}
        <DialogActions>
          {isChief || retired ? null : (
            <Button variant="danger" className="profile-retire" onClick={onRetire}>
              Retire {agent.name}…
            </Button>
          )}
          <Button onClick={onClose}>Cancel</Button>
          <Button type="submit" variant="primary" disabled={busy || retired || !form.name.trim() || !form.role.trim()}>
            Save
          </Button>
        </DialogActions>
      </form>
    </Dialog>
  );
}

/** Retire hands a teammate's work to someone else, or pauses it; the thread moves to History. */
export function RetireDialog({ agent, open, onClose }: { agent: Agent; open: boolean; onClose: () => void }) {
  const client = useClient();
  const team = useTeam();
  const toast = useToast();
  const [successor, setSuccessor] = useState(CHIEF_ID);
  const [pause, setPause] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const others = (team.agents ?? []).filter((candidate) => candidate.status === "active" && candidate.id !== agent.id);

  async function retire() {
    setBusy(true);
    setError(null);
    try {
      await client.retireAgent(agent.id, { successor: pause ? null : successor, pause_routines: pause, revert_hire: false });
      team.refresh();
      toast.notify(`${agent.name} is retired. Their thread is in History.`, "success");
      onClose();
    } catch (cause) {
      setError(cause instanceof ApiError ? cause.message : `Could not retire ${agent.name}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={open} onClose={onClose} title={`Retire ${agent.name}?`} description={`${agent.name} stops working and the thread moves to History. You can restore them later.`}>
      <div className="dialog-form">
        <Segmented
          label="Their routines"
          options={[
            { value: "hand", label: "Hand over" },
            { value: "pause", label: "Pause" },
          ]}
          value={pause ? "pause" : "hand"}
          onChange={(value) => setPause(value === "pause")}
        />
        {pause ? null : (
          <label className="field">
            <span>Who takes over</span>
            <Select
              value={successor}
              onChange={setSuccessor}
              variant="field"
              aria-label="Who takes over"
              options={others.map((candidate) => ({ value: candidate.id, label: candidate.name, detail: candidate.persona.role }))}
            />
          </label>
        )}
        {error ? <ErrorNote>{error}</ErrorNote> : null}
        <DialogActions>
          <Button onClick={onClose}>Keep {agent.name}</Button>
          <Button variant="danger" disabled={busy} onClick={() => void retire()}>
            Retire
          </Button>
        </DialogActions>
      </div>
    </Dialog>
  );
}
