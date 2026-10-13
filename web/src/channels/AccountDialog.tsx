import { useState, type FormEvent, type ReactNode } from "react";
import type { AgentSummary, ChannelAccount, ChannelAccountOptions, UpdateChannelAccountRequest } from "../api/types";
import { Avatar } from "../components/Avatar";
import { useClient } from "../context";
import { errorMessage } from "../hooks/useResource";
import { ErrorNote } from "../pages/blocks";
import { Button, Dialog, DialogActions, Select, Spinner, Switch, type SelectOption } from "../ui";
import { hasAllowList, ID_PATTERN, isLegacy, listText, parseList, serviceOf, slugify, testSummary, type ChannelKind, type ListKey } from "./services";
import { WebhookUrl } from "./WebhookUrl";

const SECRET_INPUT = { autoComplete: "new-password", autoCapitalize: "none", autoCorrect: "off", spellCheck: false } as const;
const CHIEF_ID = "chief";

interface Secrets {
  token: string;
  signing: string;
}

/** Credentials are read from the form and the inputs emptied before anything is sent; they never enter React state. */
function readSecrets(form: HTMLFormElement): Secrets {
  const data = new FormData(form);
  return { token: String(data.get("token") ?? "").trim(), signing: String(data.get("signing_secret") ?? "").trim() };
}

function clearSecrets(form: HTMLFormElement) {
  for (const input of form.querySelectorAll<HTMLInputElement>("input[type=password]")) input.value = "";
}

function teammateOptions(agents: readonly AgentSummary[] | null, current: string): SelectOption[] {
  const active = (agents ?? []).filter((agent) => agent.status === "active");
  const chief = active.find((agent) => agent.kind === "chief");
  const options: SelectOption[] = [
    {
      value: "",
      label: chief?.name ?? "Your assistant",
      detail: "Default · answers when nobody else is chosen",
      icon: chief ? <Avatar chief initials={chief.initials} color={chief.color} size={24} /> : undefined,
    },
    ...active
      .filter((agent) => agent.kind === "teammate")
      .sort((a, b) => a.name.localeCompare(b.name))
      .map((agent) => ({ value: agent.id, label: agent.name, detail: agent.persona.role, icon: <Avatar initials={agent.initials} color={agent.color} size={24} /> })),
  ];
  if (current && current !== CHIEF_ID && !options.some((option) => option.value === current)) {
    const retired = agents?.find((agent) => agent.id === current);
    options.push({ value: current, label: retired ? `${retired.name} (retired)` : current, disabled: true });
  }
  return options;
}

/** A labelled control with optional help; the help stays outside the label so the control's name is just the label. */
function Field({ id, label, help, children }: { id: string; label: string; help?: string; children: ReactNode }) {
  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      {children}
      {help ? (
        <span id={`${id}-help`} className="field-help">
          {help}
        </span>
      ) : null}
    </div>
  );
}

function AccountForm({
  kind,
  account,
  replaceCredentials,
  agents,
  onClose,
  onSaved,
  onTested,
  onRemote,
}: {
  kind: ChannelKind;
  account: ChannelAccount | null;
  replaceCredentials: boolean;
  agents: readonly AgentSummary[] | null;
  onClose: () => void;
  onSaved: (message: string) => Promise<void>;
  onTested: () => Promise<void>;
  onRemote: () => void;
}) {
  const client = useClient();
  const service = serviceOf(kind);
  const [label, setLabel] = useState(account?.label ?? "");
  const [id, setId] = useState(account?.id ?? "");
  const [idTouched, setIdTouched] = useState(false);
  const [agentId, setAgentId] = useState(account?.agent_id ?? "");
  const [enabled, setEnabled] = useState(account?.enabled ?? true);
  const [lists, setLists] = useState<Record<ListKey, string>>(() => ({
    allowed_chats: listText(account?.options.allowed_chats),
    allowed_users: listText(account?.options.allowed_users),
    allowed_workspaces: listText(account?.options.allowed_workspaces),
    allowed_channels: listText(account?.options.allowed_channels),
    allowed_guilds: listText(account?.options.allowed_guilds),
  }));
  const [publicKey, setPublicKey] = useState(account?.options.public_key ?? "");
  const [pollTimeout, setPollTimeout] = useState(account?.options.poll_timeout_secs ? String(account.options.poll_timeout_secs) : "");
  // Credentials are write-only: a new account needs them, an existing one only when the person replaces them.
  const [replacing, setReplacing] = useState(account === null || replaceCredentials || account.status === "expired");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [testing, setTesting] = useState(false);
  const [test, setTest] = useState<{ ok: boolean; text: string } | null>(null);

  function options(): ChannelAccountOptions {
    const base: ChannelAccountOptions = account?.options ?? {
      allowed_chats: [],
      allowed_users: [],
      allowed_workspaces: [],
      allowed_channels: [],
      allowed_guilds: [],
      api_base: null,
      listen: null,
      poll_timeout_secs: null,
      public_key: null,
    };
    const shown = new Set(service.lists.map((list) => list.key));
    const next = { ...base };
    for (const key of Object.keys(lists) as ListKey[]) next[key] = shown.has(key) ? parseList(lists[key]) : [];
    if (kind === "discord") next.public_key = publicKey.trim() || null;
    if (kind === "telegram") next.poll_timeout_secs = pollTimeout.trim() ? Number(pollTimeout) : null;
    return next;
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = event.currentTarget;
    const secrets = replacing ? readSecrets(form) : null;
    const name = label.trim();
    const next = options();
    const problem = (() => {
      if (!name) return "Give the account a name.";
      if (!account && !ID_PATTERN.test(id)) return "The id needs 1–40 lowercase letters, digits, - or _.";
      if (kind === "discord" && !next.public_key) return "Paste the application's public key from the Discord Developer Portal.";
      if (kind === "telegram" && pollTimeout.trim() && !(Number.isInteger(next.poll_timeout_secs) && (next.poll_timeout_secs ?? 0) > 0)) return "The poll timeout is a whole number of seconds.";
      if (enabled && !hasAllowList(next)) return "Add at least one allowed chat, channel or person before turning the account on, or save it turned off.";
      if (secrets && (!secrets.token || (service.signingSecret && !secrets.signing))) return service.signingSecret ? "Paste both the bot token and the signing secret." : "Paste the bot token.";
      return null;
    })();
    if (problem) {
      setError(problem);
      return;
    }
    const credentials = secrets ? { token: secrets.token, signing_secret: service.signingSecret ? secrets.signing : null } : null;
    clearSecrets(form);
    setBusy(true);
    setError(null);
    try {
      if (account) {
        const patch: Partial<UpdateChannelAccountRequest> = {};
        if (name !== account.label) patch.label = name;
        if (enabled !== account.enabled) patch.enabled = enabled;
        if (agentId !== (account.agent_id ?? "")) patch.agent_id = agentId;
        if (JSON.stringify(next) !== JSON.stringify(account.options)) patch.options = next;
        if (credentials) patch.credentials = credentials;
        if (Object.keys(patch).length) await client.updateChannelAccount(kind, account.id, patch);
        await onSaved(credentials ? `${name} saved with new credentials.` : `${name} saved.`);
      } else if (credentials) {
        await client.addChannelAccount(kind, { id, label: name, enabled, agent_id: agentId || null, options: next, credentials });
        await onSaved(`${name} added.`);
      }
      onClose();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function runTest() {
    if (!account) return;
    setTesting(true);
    setTest(null);
    try {
      const result = await client.testChannelAccount(kind, account.id);
      setTest({ ok: result.ok, text: testSummary(result) });
      await onTested();
    } catch (cause) {
      setTest({ ok: false, text: errorMessage(cause) });
    } finally {
      setTesting(false);
    }
  }

  const legacy = account !== null && isLegacy(account);

  return (
    <form className="dialog-form channel-form" onSubmit={(event) => void submit(event)} aria-label={`${service.name} account`}>
      <Field id="channel-name" label="Name">
        <input
          id="channel-name"
          value={label}
          onChange={(event) => {
            setLabel(event.target.value);
            if (!account && !idTouched) setId(slugify(event.target.value));
          }}
          placeholder="Work, Personal, Support…"
          maxLength={128}
          autoFocus
          required
        />
      </Field>
      {account ? null : (
        <Field id="channel-id" label="Account id" help="Lowercase letters, digits, - and _. It names this account in the address and cannot change later.">
          <input
            id="channel-id"
            value={id}
            onChange={(event) => {
              setId(event.target.value);
              setIdTouched(true);
            }}
            placeholder="work"
            maxLength={40}
            aria-describedby="channel-id-help"
            autoCapitalize="none"
            spellCheck={false}
            required
          />
        </Field>
      )}
      <Field id="channel-teammate" label="Answers as" help="New conversations on this account go to this teammate. Existing conversations keep theirs.">
        <Select id="channel-teammate" variant="field" aria-describedby="channel-teammate-help" value={agentId} onChange={setAgentId} options={teammateOptions(agents, agentId)} />
      </Field>

      <div className="channel-section" role="group" aria-labelledby="channel-who-title">
        <h3 id="channel-who-title">Who may talk to it</h3>
        <p className="field-help">Add at least one before turning the account on. Separate ids with commas.</p>
        {service.lists.map((list) => (
          <Field key={list.key} id={`channel-${list.key}`} label={list.label} help={list.hint}>
            <input
              id={`channel-${list.key}`}
              value={lists[list.key]}
              onChange={(event) => setLists({ ...lists, [list.key]: event.target.value })}
              placeholder={list.placeholder}
              aria-describedby={`channel-${list.key}-help`}
              autoCapitalize="none"
              spellCheck={false}
            />
          </Field>
        ))}
        {kind === "discord" ? (
          <Field id="channel-public-key" label="Application public key" help="From General Information in the Discord Developer Portal. openside uses it to check that callbacks really come from Discord.">
            <input id="channel-public-key" value={publicKey} onChange={(event) => setPublicKey(event.target.value)} placeholder="64 hexadecimal characters" aria-describedby="channel-public-key-help" autoCapitalize="none" spellCheck={false} />
          </Field>
        ) : null}
        {kind === "telegram" ? (
          <Field id="channel-poll" label="Poll timeout (seconds)" help="Optional. How long each wait for new messages lasts.">
            <input id="channel-poll" type="number" min={1} inputMode="numeric" value={pollTimeout} onChange={(event) => setPollTimeout(event.target.value)} placeholder="30" aria-describedby="channel-poll-help" />
          </Field>
        ) : null}
      </div>

      <div className="channel-section" role="group" aria-labelledby="channel-credentials-title">
        <h3 id="channel-credentials-title">Credentials</h3>
        {replacing ? (
          <>
            <p className="field-help">
              {legacy
                ? "This account still reads its credentials from config.toml. Paste them here to move it into the vault; its conversations are kept."
                : "Stored encrypted in the vault on the computer running openside, and never shown again."}
            </p>
            <Field id="channel-token" label="Bot token">
              <input id="channel-token" name="token" type="password" {...SECRET_INPUT} data-1p-ignore data-lpignore="true" />
            </Field>
            {service.signingSecret ? (
              <Field id="channel-signing" label="Signing secret" help="Basic Information → App Credentials in your Slack app. Both values replace the old ones together.">
                <input id="channel-signing" name="signing_secret" type="password" {...SECRET_INPUT} aria-describedby="channel-signing-help" data-1p-ignore data-lpignore="true" />
              </Field>
            ) : null}
            {account ? (
              <Button variant="ghost" size="compact" className="channel-keep" onClick={() => setReplacing(false)}>
                {legacy ? "Not now" : "Keep the current credentials"}
              </Button>
            ) : null}
          </>
        ) : (
          <div className="channel-credentials-kept">
            <p className="field-help">
              {legacy
                ? "This account still reads its credentials from config.toml. Moving them into the vault lets you manage and rotate them here; its conversations are kept."
                : "Stored encrypted in the vault. They are never shown, and a new set replaces the old one."}
            </p>
            <Button size="compact" onClick={() => setReplacing(true)}>
              {legacy ? "Move to vault" : "Replace credentials"}
            </Button>
          </div>
        )}
      </div>

      {service.callback ? (
        <div className="channel-section" role="group" aria-labelledby="channel-callback-title">
          <h3 id="channel-callback-title">Callback address</h3>
          {account?.webhook_url ? (
            <>
              <p className="field-help">{service.callback.steps}</p>
              <WebhookUrl account={account} onRemote={onRemote} />
            </>
          ) : (
            <p className="field-help">After you save, the address to paste into {service.name} appears here and on the account's row.</p>
          )}
        </div>
      ) : null}

      <div className="channel-enable">
        <label htmlFor="channel-enabled">
          <strong>Turn on</strong>
          <span className="field-help">{account ? "Off stops this account only." : "You can also save it off and turn it on later."}</span>
        </label>
        <Switch id="channel-enabled" label={`Turn on ${label.trim() || "this account"}`} checked={enabled} onChange={setEnabled} />
      </div>

      {account ? (
        <div className="channel-test-row">
          <Button size="compact" onClick={() => void runTest()} disabled={testing || !account.secret_present}>
            {testing ? <Spinner size={14} /> : null}
            Test connection
          </Button>
          <span className="field-help">Checks the saved credentials with {service.name}. It never sends a message{service.callback ? " and does not check the callback address" : ""}.</span>
          {test ? (
            <p role="status" className={test.ok ? "channel-result" : "channel-result channel-result--bad"}>
              {test.text}
            </p>
          ) : null}
        </div>
      ) : null}

      {error ? <ErrorNote>{error}</ErrorNote> : null}
      <DialogActions>
        <Button onClick={onClose}>Cancel</Button>
        <Button type="submit" variant="primary" disabled={busy}>
          {busy ? "Saving…" : account ? "Save" : "Add account"}
        </Button>
      </DialogActions>
    </form>
  );
}

/** Adds or edits one account. Remounting the form on every open keeps nothing from the last one. */
export function AccountDialog({
  open,
  kind,
  account,
  replaceCredentials,
  agents,
  onClose,
  onSaved,
  onTested,
  onRemote,
}: {
  open: boolean;
  kind: ChannelKind;
  account: ChannelAccount | null;
  /** Open with the credential fields showing: reconnecting an expired account or moving a legacy one. */
  replaceCredentials: boolean;
  agents: readonly AgentSummary[] | null;
  onClose: () => void;
  onSaved: (message: string) => Promise<void>;
  onTested: () => Promise<void>;
  onRemote: () => void;
}) {
  const service = serviceOf(kind);
  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="wide"
      title={account ? `Edit ${account.label}` : `Add ${service.name} account`}
      description={account ? `${service.name} · ${account.id}` : service.blurb}
    >
      <AccountForm kind={kind} account={account} replaceCredentials={replaceCredentials} agents={agents} onClose={onClose} onSaved={onSaved} onTested={onTested} onRemote={onRemote} />
    </Dialog>
  );
}
