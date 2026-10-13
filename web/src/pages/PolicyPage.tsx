import { Plus, X } from "lucide-react";
import { useCallback, useId, useState } from "react";
import type {
  NetworkPolicy,
  PolicyCapability,
  PolicyDecision,
  PolicyEffect,
  PolicyMode,
  PolicyProblem,
  PolicyRule,
  PolicySettings,
} from "../api/types";
import { useClient } from "../context";
import { errorMessage, useResource } from "../hooks/useResource";
import { Button, ICON_STROKE, IconButton, Select, useToast } from "../ui";
import { Badge, ErrorNote, ResourceView, SectionList } from "./blocks";
import { FailureNote, toFailure, type Failure } from "./Failure";
import { RestartNotice } from "./RestartNotice";

const MODES: { value: PolicyMode; label: string; detail: string }[] = [
  { value: "read-only", label: "Read only", detail: "Agents may look but not change anything." },
  { value: "guard", label: "Guard", detail: "Agents work inside folders you approved and ask elsewhere." },
  { value: "full", label: "Full access", detail: "Agents act freely; payments and messages still ask first." },
];

const EFFECTS: { value: PolicyEffect; label: string }[] = [
  { value: "allow", label: "Allow" },
  { value: "ask", label: "Ask me" },
  { value: "deny", label: "Deny" },
];

const CAPABILITIES: { value: PolicyCapability; label: string }[] = [
  { value: "fs_read", label: "Read files" },
  { value: "fs_write", label: "Change files" },
  { value: "exec", label: "Run commands" },
  { value: "browse", label: "Browse pages" },
  { value: "fetch", label: "Fetch from the network" },
  { value: "vault_use", label: "Use a saved login" },
  { value: "sensitive", label: "Payments and messages" },
  { value: "privacy_read", label: "Read imported private data" },
];

const REASONS: Record<string, string> = {
  deny_rule: "A deny rule matches.",
  origin_denied: "The site is on the denied list.",
  origin_not_allowed: "The site is not on the allowed list.",
  rule: "A rule matches.",
  mode: "The default mode decides it.",
  sensitive_floor: "Payments and messages always ask first.",
};

const EFFECT_TONE = { allow: "success", ask: "attention", deny: "danger" } as const;

const lines = (text: string) =>
  text
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);

/** Problems for one list, in the words of the entry they point at. */
function ProblemList({ problems, list, entries }: { problems: PolicyProblem[]; list: PolicyProblem["list"]; entries: string[] }) {
  const mine = problems.filter((problem) => problem.list === list);
  if (mine.length === 0) return null;
  return (
    <ul className="policy-problems" role="alert">
      {mine.map((problem) => (
        <li key={`${problem.index}-${problem.message}`}>
          {entries[problem.index] ? <code>{entries[problem.index]}</code> : `Entry ${problem.index + 1}`}: {problem.message}
        </li>
      ))}
    </ul>
  );
}

interface Draft {
  mode: PolicyMode;
  allow: string;
  deny: string;
  rules: PolicyRule[];
}

const toDraft = (policy: PolicySettings): Draft => ({
  mode: policy.default_mode,
  allow: policy.allow_origins.join("\n"),
  deny: policy.deny_origins.join("\n"),
  rules: policy.rules.map((rule) => ({ ...rule })),
});

const fromDraft = (draft: Draft): PolicySettings => ({
  default_mode: draft.mode,
  allow_origins: lines(draft.allow),
  deny_origins: lines(draft.deny),
  rules: draft.rules.filter((rule) => rule.pattern.trim()).map((rule) => ({ ...rule, pattern: rule.pattern.trim() })),
});

function LivePolicy({ policy, onSaved }: { policy: PolicySettings; onSaved: (next: PolicySettings) => void }) {
  const client = useClient();
  const toast = useToast();
  const id = useId();
  const [draft, setDraft] = useState<Draft>(() => toDraft(policy));
  const [problems, setProblems] = useState<PolicyProblem[]>([]);
  const [failure, setFailure] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);
  const dirty = JSON.stringify(draft) !== JSON.stringify(toDraft(policy));
  const mode = MODES.find((item) => item.value === draft.mode);
  const patch = (next: Partial<Draft>) => setDraft((current) => ({ ...current, ...next }));

  const save = useCallback(async () => {
    setBusy(true);
    setFailure(null);
    const next = fromDraft(draft);
    try {
      const checked = await client.validatePolicy({ policy: next });
      setProblems(checked.problems);
      if (!checked.valid) return;
      const saved = await client.setPolicy(next);
      setDraft(toDraft(saved));
      onSaved(saved);
      toast.notify("Policy saved. It applies to the next action of every conversation.");
    } catch (cause) {
      setFailure(toFailure(cause));
    } finally {
      setBusy(false);
    }
  }, [client, draft, onSaved, toast]);

  const normalized = fromDraft(draft);

  return (
    <SectionList title="Agent permissions" description="Applies to the next action of every conversation, including ones already running.">
      <div className="policy-form">
        <div className="field">
          <label htmlFor={`${id}-mode`}>
            <span>Default mode</span>
          </label>
          <Select id={`${id}-mode`} value={draft.mode} options={MODES} onChange={(value) => patch({ mode: value as PolicyMode })} />
          <p className="settings-text">{mode?.detail} Used for conversations that do not pick a mode.</p>
        </div>
        <div className="field">
          <label htmlFor={`${id}-allow`}>
            <span>Only these sites</span>
          </label>
          <textarea
            id={`${id}-allow`}
            rows={3}
            value={draft.allow}
            placeholder="https://*.example.com"
            spellCheck={false}
            onChange={(event) => patch({ allow: event.target.value })}
          />
          <p className="settings-text">One per line. When this list has entries, agents may only visit these sites. It never grants anything by itself.</p>
          <ProblemList problems={problems} list="allow_origins" entries={normalized.allow_origins} />
        </div>
        <div className="field">
          <label htmlFor={`${id}-deny`}>
            <span>Never these sites</span>
          </label>
          <textarea
            id={`${id}-deny`}
            rows={3}
            value={draft.deny}
            placeholder="https://*.bank.example"
            spellCheck={false}
            onChange={(event) => patch({ deny: event.target.value })}
          />
          <p className="settings-text">One per line. Agents are always refused these, whatever else allows them.</p>
          <ProblemList problems={problems} list="deny_origins" entries={normalized.deny_origins} />
        </div>
        <div className="field">
          <span id={`${id}-rules`}>Rules</span>
          <p className="settings-text">
            A matching Deny always wins. Payments and messages can be tightened to Deny but never allowed. The pattern matches the file, folder or address the action touches:
            <code>*</code> within one folder, <code>**</code> across folders.
          </p>
          <ul className="policy-rules" aria-labelledby={`${id}-rules`}>
            {draft.rules.map((rule, index) => (
              <li key={index}>
                <div className="policy-rule">
                  <Select
                    aria-label={`Rule ${index + 1} outcome`}
                    value={rule.effect}
                    options={EFFECTS}
                    onChange={(value) => patch({ rules: draft.rules.map((item, at) => (at === index ? { ...item, effect: value as PolicyEffect } : item)) })}
                  />
                  <Select
                    aria-label={`Rule ${index + 1} action`}
                    value={rule.capability}
                    options={CAPABILITIES}
                    onChange={(value) =>
                      patch({ rules: draft.rules.map((item, at) => (at === index ? { ...item, capability: value as PolicyCapability } : item)) })
                    }
                  />
                  <input
                    aria-label={`Rule ${index + 1} pattern`}
                    value={rule.pattern}
                    placeholder="/home/me/projects/**"
                    spellCheck={false}
                    onChange={(event) => patch({ rules: draft.rules.map((item, at) => (at === index ? { ...item, pattern: event.target.value } : item)) })}
                  />
                  <IconButton label={`Remove rule ${index + 1}`} onClick={() => patch({ rules: draft.rules.filter((_, at) => at !== index) })}>
                    <X size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
                  </IconButton>
                </div>
                {problems
                  .filter((problem) => problem.list === "rules" && problem.index === index)
                  .map((problem) => (
                    <p key={problem.message} className="policy-row-problem" role="alert">
                      {problem.message}
                    </p>
                  ))}
              </li>
            ))}
          </ul>
          <div>
            <Button
              size="compact"
              onClick={() => patch({ rules: [...draft.rules, { effect: "ask", capability: "fs_write", pattern: "" }] })}
            >
              <Plus size={16} strokeWidth={ICON_STROKE} aria-hidden="true" /> Add rule
            </Button>
          </div>
        </div>
      </div>
      <FailureNote failure={failure} what="change the policy" onRetry={() => void save()} />
      {dirty ? (
        <div className="config-save">
          <span className="config-save-status" role="status">
            Unsaved changes
          </span>
          <Button
            variant="ghost"
            onClick={() => {
              setDraft(toDraft(policy));
              setProblems([]);
              setFailure(null);
            }}
          >
            Discard
          </Button>
          <Button variant="primary" disabled={busy} onClick={() => void save()}>
            Save policy
          </Button>
        </div>
      ) : null}
    </SectionList>
  );
}

function Network() {
  const client = useClient();
  const toast = useToast();
  const id = useId();
  const load = useCallback(() => client.networkPolicy(), [client]);
  const resource = useResource(load);
  const [saved, setSaved] = useState<NetworkPolicy | null>(null);
  const [text, setText] = useState<string | null>(null);
  const [problems, setProblems] = useState<PolicyProblem[]>([]);
  const [failure, setFailure] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);
  const current = saved ?? resource.data;
  const value = text ?? current?.allow.join("\n") ?? "";
  const dirty = text !== null && JSON.stringify(lines(text)) !== JSON.stringify(current?.allow ?? []);

  async function save() {
    setBusy(true);
    setFailure(null);
    const allow = lines(value);
    try {
      const checked = await client.validatePolicy({ network_allow: allow });
      setProblems(checked.problems);
      if (!checked.valid) return;
      setSaved(await client.setNetworkPolicy(allow));
      setText(null);
      toast.notify("Network list saved.");
    } catch (cause) {
      setFailure(toFailure(cause));
    } finally {
      setBusy(false);
    }
  }

  return (
    <SectionList
      title="Network"
      description="Where openside and its agents may connect, besides this computer and the model and channel addresses you configured. Downloads such as the memory model also need to be listed here."
    >
      {!current ? (
        resource.error ? <ErrorNote>{resource.error}</ErrorNote> : null
      ) : (
        <div className="policy-form">
          <div className="field">
            <label htmlFor={`${id}-allow`}>
              <span>Allowed addresses</span>
            </label>
            <textarea
              id={`${id}-allow`}
              rows={4}
              value={value}
              placeholder="huggingface.co"
              spellCheck={false}
              onChange={(event) => setText(event.target.value)}
            />
            <p className="settings-text">
              One per line: <code>host</code>, <code>*.domain</code>, <code>host:port</code> or <code>https://host</code>.
            </p>
            <ProblemList problems={problems} list="network_allow" entries={lines(value)} />
          </div>
        </div>
      )}
      <FailureNote failure={failure} what="change the network list" onRetry={() => void save()} />
      {dirty ? (
        <div className="config-save">
          <span className="config-save-status" role="status">
            Unsaved changes
          </span>
          <Button
            variant="ghost"
            onClick={() => {
              setText(null);
              setProblems([]);
              setFailure(null);
            }}
          >
            Discard
          </Button>
          <Button variant="primary" disabled={busy} onClick={() => void save()}>
            Save list
          </Button>
        </div>
      ) : null}
      {current?.restart_required ? (
        <RestartNotice
          onRestarted={() => {
            setSaved(null);
            void resource.reload();
          }}
        >
          The saved list applies after openside restarts.
        </RestartNotice>
      ) : null}
    </SectionList>
  );
}

function TryAction() {
  const client = useClient();
  const id = useId();
  const [target, setTarget] = useState("");
  const [capability, setCapability] = useState<PolicyCapability>("fs_write");
  const [mode, setMode] = useState<PolicyMode | "">("");
  const [decision, setDecision] = useState<PolicyDecision | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function run() {
    setError(null);
    try {
      setDecision(await client.testPolicy({ target: target.trim(), capability, ...(mode ? { mode } : {}) }));
    } catch (cause) {
      setDecision(null);
      setError(errorMessage(cause));
    }
  }

  return (
    <SectionList title="Try an action" description="See what the policy would do with an action, without running it.">
      <div className="policy-form">
        <div className="field">
          <label htmlFor={`${id}-target`}>
            <span>File, folder or address</span>
          </label>
          <input id={`${id}-target`} value={target} placeholder="/home/me/projects/notes.md" spellCheck={false} onChange={(event) => setTarget(event.target.value)} />
        </div>
        <div className="policy-try">
          <div className="field">
            <label htmlFor={`${id}-capability`}>
              <span>Action</span>
            </label>
            <Select id={`${id}-capability`} value={capability} options={CAPABILITIES} onChange={(value) => setCapability(value as PolicyCapability)} />
          </div>
          <div className="field">
            <label htmlFor={`${id}-mode`}>
              <span>Mode</span>
            </label>
            <Select
              id={`${id}-mode`}
              value={mode}
              options={[{ value: "", label: "Default mode" }, ...MODES]}
              onChange={(value) => setMode(value as PolicyMode | "")}
            />
          </div>
        </div>
        <div>
          <Button variant="secondary" disabled={!target.trim()} onClick={() => void run()}>
            Check
          </Button>
        </div>
        {decision ? (
          <p className="policy-decision" role="status">
            <Badge tone={EFFECT_TONE[decision.effect]}>{EFFECTS.find((effect) => effect.value === decision.effect)?.label}</Badge>
            {REASONS[decision.reason] ?? decision.reason}
          </p>
        ) : null}
        {error ? <ErrorNote>{error}</ErrorNote> : null}
      </div>
    </SectionList>
  );
}

/** What agents may do and where openside may connect, with the checks that stop a bad entry. */
export function PolicyPage() {
  const client = useClient();
  const load = useCallback(() => client.policy(), [client]);
  const policy = useResource(load);
  const [saved, setSaved] = useState<PolicySettings | null>(null);

  return (
    <div className="manage-body" data-section="policy">
      <ResourceView resource={policy} what="the policy">
        {(loaded) => <LivePolicy key={JSON.stringify(saved ?? loaded)} policy={saved ?? loaded} onSaved={setSaved} />}
      </ResourceView>
      <Network />
      <TryAction />
    </div>
  );
}
