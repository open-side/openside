import { Check, Copy, Pin, RefreshCw, Trash2 } from "lucide-react";
import { useCallback, useId, useState, type FormEvent } from "react";
import type { PinnedTab, SyncReport, SyncSecrets, SyncStatus } from "../api/types";
import { useClient } from "../context";
import { errorMessage, useResource } from "../hooks/useResource";
import { Button, copyText, ICON_STROKE, IconButton, Segmented, Switch, useToast } from "../ui";
import { Badge, EmptyState, ErrorNote, ResourceCard, ResourceView, SectionList, StatusRow, type BadgeTone } from "./blocks";
import { FailureNote, toFailure, type Failure } from "./Failure";
import { formatDateTime } from "./format";

type Backend = "off" | "dir" | "webdav";

const tags = (text: string) =>
  text
    .split(",")
    .map((tag) => tag.trim())
    .filter(Boolean);

function stateOf(status: SyncStatus): { label: string; tone: BadgeTone } {
  if (!status.configured) return { label: "Off", tone: "neutral" };
  if (!status.set_up) return { label: "Not joined yet", tone: "attention" };
  if (!status.unlocked) return { label: "Locked", tone: "attention" };
  return { label: "On", tone: "success" };
}

/** Where synced data lives and what is included. The place cannot change once an account is joined. */
function Storage({ status, onChanged }: { status: SyncStatus; onChanged: (next: SyncStatus) => void }) {
  const client = useClient();
  const toast = useToast();
  const id = useId();
  const [backend, setBackend] = useState<Backend>(status.configured ? (status.backend === "dir" ? "dir" : "webdav") : "off");
  const [dir, setDir] = useState(status.backend === "dir" ? (status.location ?? "") : "");
  const [url, setUrl] = useState(status.backend === "webdav" ? (status.location ?? "") : "");
  const [user, setUser] = useState(status.webdav_username ?? "");
  const [settings, setSettings] = useState(status.settings);
  const [tagText, setTagText] = useState(status.memory_tags.join(", "));
  const [failure, setFailure] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);
  const locked = status.set_up;

  const save = useCallback(
    async (event?: FormEvent) => {
      event?.preventDefault();
      setBusy(true);
      setFailure(null);
      try {
        const next = await client.setSyncConfig({
          ...(backend === "dir" ? { dir: dir.trim() } : {}),
          ...(backend === "webdav" ? { webdav_url: url.trim(), ...(user.trim() ? { webdav_username: user.trim() } : {}) } : {}),
          settings,
          memory_tags: tags(tagText),
        });
        onChanged(next);
        toast.notify("Sync settings saved.");
      } catch (cause) {
        setFailure(toFailure(cause));
      } finally {
        setBusy(false);
      }
    },
    [backend, client, dir, onChanged, settings, tagText, toast, url, user],
  );

  return (
    <SectionList title="Where to sync" description="Synced data is encrypted on this computer before it leaves, so the place only ever holds scrambled files.">
      <form className="policy-form" aria-label="Sync storage" onSubmit={(event) => void save(event)}>
        {locked ? (
          <p className="settings-text">
            This device joined a sync account, so the place stays {status.backend === "dir" ? "the shared folder" : "the WebDAV address"} <code>{status.location}</code>. You can still change what is synced.
          </p>
        ) : (
          <>
            <Segmented
              label="Sync storage"
              value={backend}
              onChange={setBackend}
              options={[
                { value: "off", label: "Off" },
                { value: "dir", label: "Shared folder" },
                { value: "webdav", label: "WebDAV" },
              ]}
            />
            {backend === "dir" ? (
              <div className="field">
                <label htmlFor={`${id}-dir`}>
                  <span>Folder</span>
                </label>
                <input id={`${id}-dir`} value={dir} placeholder="~/Sync/openside" spellCheck={false} onChange={(event) => setDir(event.target.value)} />
                <p className="settings-text">A folder another tool already keeps in step between your computers, such as Syncthing or a cloud drive.</p>
              </div>
            ) : null}
            {backend === "webdav" ? (
              <>
                <div className="field">
                  <label htmlFor={`${id}-url`}>
                    <span>WebDAV address</span>
                  </label>
                  <input id={`${id}-url`} value={url} placeholder="https://dav.example.com/openside/" spellCheck={false} onChange={(event) => setUrl(event.target.value)} />
                </div>
                <div className="field">
                  <label htmlFor={`${id}-user`}>
                    <span>Username (optional)</span>
                  </label>
                  <input id={`${id}-user`} value={user} autoComplete="off" spellCheck={false} onChange={(event) => setUser(event.target.value)} />
                  <p className="settings-text">The password is typed when you unlock sync and is never saved to disk.</p>
                </div>
              </>
            ) : null}
          </>
        )}
        {locked || backend !== "off" ? (
          <>
            <div className="policy-switch">
              <div>
                <label htmlFor={`${id}-settings`}>Sync the permission policy</label>
                <p className="settings-text">Your allowed sites and rules follow you to every device.</p>
              </div>
              <Switch id={`${id}-settings`} checked={settings} onChange={setSettings} />
            </div>
            <div className="field">
              <label htmlFor={`${id}-tags`}>
                <span>Memory tags to sync</span>
              </label>
              <input id={`${id}-tags`} value={tagText} placeholder="shared, travel" spellCheck={false} onChange={(event) => setTagText(event.target.value)} />
              <p className="settings-text">Comma separated. Memories with any of these tags are synced; the rest stay on this computer.</p>
            </div>
          </>
        ) : null}
        <FailureNote failure={failure} what="change sync" onRetry={() => void save()} />
        <div>
          <Button type="submit" variant="primary" disabled={busy || (backend === "dir" && !dir.trim()) || (backend === "webdav" && !url.trim())}>
            {busy ? "Saving…" : "Save"}
          </Button>
        </div>
      </form>
    </SectionList>
  );
}

function Passphrase({ id, label = "Sync passphrase", hint }: { id: string; label?: string; hint?: string }) {
  return (
    <div className="field">
      <label htmlFor={id}>
        <span>{label}</span>
      </label>
      <input id={id} name={id} type="password" autoComplete="off" spellCheck={false} />
      {hint ? <p className="settings-text">{hint}</p> : null}
    </div>
  );
}

/** Secrets are read from the form and the fields emptied before anything is sent; they never enter React state. */
function takeSecrets(form: HTMLFormElement, status: SyncStatus): SyncSecrets {
  const data = new FormData(form);
  const secrets: SyncSecrets = {};
  const passphrase = String(data.get("passphrase") ?? "");
  const password = String(data.get("webdav_password") ?? "");
  if (passphrase) secrets.passphrase = passphrase;
  if (password && status.webdav_username) secrets.webdav_password = password;
  for (const input of form.querySelectorAll<HTMLInputElement>("input[type=password]")) input.value = "";
  return secrets;
}

const needsPassphrase = (status: SyncStatus) => status.passphrase_source === "entered";
const needsPassword = (status: SyncStatus) => Boolean(status.webdav_username) && status.password_source === "entered";

function Account({ status, onChanged }: { status: SyncStatus; onChanged: (next: SyncStatus) => void }) {
  const client = useClient();
  const toast = useToast();
  const [failure, setFailure] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);
  const [mode, setMode] = useState<"new" | "join">("new");
  const [code, setCode] = useState("");
  const [created, setCreated] = useState<string | null>(null);

  async function submit(event: FormEvent<HTMLFormElement>, call: (secrets: SyncSecrets) => Promise<SyncStatus | null>) {
    event.preventDefault();
    const form = event.currentTarget;
    const secrets = takeSecrets(form, status);
    setBusy(true);
    setFailure(null);
    try {
      const next = await call(secrets);
      if (next) onChanged(next);
    } catch (cause) {
      setFailure(toFailure(cause));
    } finally {
      setBusy(false);
    }
  }

  async function copy(text: string) {
    toast.notify((await copyText(text)) ? "Code copied" : "Could not copy; select the code instead", "success");
  }

  if (!status.configured) return null;

  if (!status.set_up) {
    return (
      <SectionList title="Sync account" description="Start an account on this device, or join the one another device already started.">
        <Segmented
          label="Account"
          value={mode}
          onChange={setMode}
          options={[
            { value: "new", label: "Start a new account" },
            { value: "join", label: "Join with a code" },
          ]}
        />
        {created ? (
          <div className="agent-link-result" role="status">
            <p className="onb-result-title">
              <Check size={16} strokeWidth={ICON_STROKE} aria-hidden="true" /> Account started
            </p>
            <div className="remote-copy-row">
              <input readOnly aria-label="Sync code" value={created} onFocus={(event) => event.currentTarget.select()} />
              <Button size="compact" aria-label="Copy the sync code" onClick={() => void copy(created)}>
                <Copy size={16} strokeWidth={ICON_STROKE} aria-hidden="true" /> Copy
              </Button>
            </div>
            <p className="settings-text">
              On your other device, choose Join with a code, paste this code and type the same passphrase. The code is safe to share; the passphrase is not, and nobody can recover it for you.
            </p>
          </div>
        ) : (
          <form
            className="policy-form"
            aria-label={mode === "new" ? "Start a sync account" : "Join a sync account"}
            onSubmit={(event) =>
              void submit(event, async (secrets) => {
                if (mode === "new") {
                  const result = await client.setupSync(secrets);
                  setCreated(result.pairing_code);
                  return result.status;
                }
                return client.pairSync({ pairing_code: code.trim(), ...secrets });
              })
            }
          >
            {mode === "join" ? (
              <div className="field">
                <label htmlFor="sync-code">
                  <span>Sync code</span>
                </label>
                <input id="sync-code" value={code} placeholder="osync1-…" spellCheck={false} onChange={(event) => setCode(event.target.value)} />
              </div>
            ) : null}
            {needsPassphrase(status) ? (
              <Passphrase
                id="passphrase"
                hint={mode === "new" ? "At least 8 characters. It encrypts everything that syncs and is never stored; you type it again after openside restarts." : "The same passphrase you chose on the first device."}
              />
            ) : (
              <p className="settings-text">The passphrase comes from the command in your configuration, so there is nothing to type.</p>
            )}
            {needsPassword(status) ? <Passphrase id="webdav_password" label="WebDAV password" /> : null}
            <FailureNote failure={failure} what="set up sync" onRetry={() => setFailure(null)} />
            <div>
              <Button type="submit" variant="primary" disabled={busy || (mode === "join" && !code.trim())}>
                {busy ? "Working…" : mode === "new" ? "Start account" : "Join account"}
              </Button>
            </div>
          </form>
        )}
      </SectionList>
    );
  }

  if (!status.unlocked) {
    return (
      <SectionList title="Unlock sync" description="Sync needs the passphrase after openside starts. It is kept in memory only.">
        <form className="policy-form" aria-label="Unlock sync" onSubmit={(event) => void submit(event, (secrets) => client.unlockSync(secrets))}>
          {needsPassphrase(status) ? <Passphrase id="passphrase" /> : <p className="settings-text">The passphrase comes from the command in your configuration.</p>}
          {needsPassword(status) ? <Passphrase id="webdav_password" label="WebDAV password" /> : null}
          <FailureNote failure={failure} what="unlock sync" onRetry={() => setFailure(null)} />
          <div>
            <Button type="submit" variant="primary" disabled={busy}>
              {busy ? "Unlocking…" : "Unlock"}
            </Button>
          </div>
        </form>
      </SectionList>
    );
  }

  return null;
}

function Report({ report }: { report: SyncReport }) {
  const rows: [string, number][] = [
    ["Uploaded", report.pushed],
    ["Downloaded", report.pulled],
    ["Applied here", report.applied],
    ["Replaced by a newer version", report.conflicts],
    ["Rejected", report.rejected],
    ["Waiting", report.deferred],
  ];
  return (
    <div className="sync-report" role="status" aria-label="Last sync">
      {rows.map(([label, value]) => (
        <StatusRow key={label} label={label}>
          {value}
        </StatusRow>
      ))}
      <StatusRow label="Vault items">{report.vault_available ? "Included" : "Left out (the vault is locked)"}</StatusRow>
      {report.rejected > 0 ? <p className="settings-text">Rejected records failed their integrity check and were not applied.</p> : null}
      {report.deferred > 0 ? <p className="settings-text">Waiting records arrive once the vault is unlocked or policy sync is on.</p> : null}
    </div>
  );
}

function Running({ status, onChanged }: { status: SyncStatus; onChanged: () => void }) {
  const client = useClient();
  const [report, setReport] = useState<SyncReport | null>(status.last_run ?? null);
  const [failure, setFailure] = useState<Failure | null>(null);
  const [busy, setBusy] = useState(false);

  async function run() {
    setBusy(true);
    setFailure(null);
    try {
      setReport(await client.runSync());
      onChanged();
    } catch (cause) {
      setFailure(toFailure(cause));
    } finally {
      setBusy(false);
    }
  }

  return (
    <SectionList
      title="This device"
      actions={
        <Button size="compact" variant="primary" disabled={busy || status.exhausted} onClick={() => void run()}>
          <RefreshCw size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
          {busy ? "Syncing…" : "Sync now"}
        </Button>
      }
    >
      {status.exhausted ? <ErrorNote>This account cannot take more changes. Local changes stay waiting until you start a new account.</ErrorNote> : null}
      <div className="settings-group">
        <StatusRow label="Last sync">{status.last_sync ? formatDateTime(status.last_sync) : "Never"}</StatusRow>
        <StatusRow label="Records">{status.records}</StatusRow>
        <StatusRow label="Waiting to upload">{status.pending}</StatusRow>
        {status.account_id ? <StatusRow label="Account"><code>{status.account_id}</code></StatusRow> : null}
        {status.device_id ? <StatusRow label="This device"><code>{status.device_id}</code></StatusRow> : null}
      </div>
      <FailureNote failure={failure} what="sync" onRetry={() => void run()} />
      {report ? <Report report={report} /> : null}
    </SectionList>
  );
}

function PinnedTabs() {
  const client = useClient();
  const load = useCallback(() => client.pinnedTabs(), [client]);
  const tabs = useResource(load);
  const [url, setUrl] = useState("");
  const [error, setError] = useState<string | null>(null);

  async function add(event: FormEvent) {
    event.preventDefault();
    setError(null);
    try {
      await client.pinTab(url.trim());
      setUrl("");
      await tabs.reload();
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }

  async function remove(tab: PinnedTab) {
    try {
      await client.unpinTab(tab.id);
      await tabs.reload();
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }

  return (
    <SectionList title="Pinned tabs" description="Pages you keep close. They follow you to your other devices once sync is on.">
      <ResourceView resource={tabs} what="pinned tabs">
        {({ tabs: list }) =>
          list.length === 0 ? (
            <EmptyState icon={Pin} title="No pinned tabs" detail="Add an address below." />
          ) : (
            <ul className="resource-list" aria-label="Pinned tabs">
              {list.map((tab) => (
                <li key={tab.id}>
                  <ResourceCard
                    icon={Pin}
                    title={tab.title || tab.url}
                    detail={tab.title ? tab.url : undefined}
                    trailing={
                      <IconButton label={`Unpin ${tab.title || tab.url}`} onClick={() => void remove(tab)}>
                        <Trash2 size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
                      </IconButton>
                    }
                  />
                </li>
              ))}
            </ul>
          )
        }
      </ResourceView>
      <form className="agent-link-form" onSubmit={(event) => void add(event)} aria-label="Pin a tab">
        <label className="field">
          <span>Address</span>
          <input value={url} placeholder="https://example.com" spellCheck={false} onChange={(event) => setUrl(event.target.value)} />
        </label>
        <Button type="submit" disabled={!url.trim()}>
          Pin
        </Button>
      </form>
      {error ? <ErrorNote>{error}</ErrorNote> : null}
    </SectionList>
  );
}

/** Sync of settings, pinned tabs, vault items and tagged memories between this person's devices. */
export function SyncPage() {
  const client = useClient();
  const load = useCallback(() => client.syncStatus(), [client]);
  const resource = useResource(load);
  const [saved, setSaved] = useState<SyncStatus | null>(null);
  const status = saved ?? resource.data;

  const refresh = useCallback(() => {
    void client.syncStatus().then(setSaved, () => undefined);
  }, [client]);

  return (
    <div className="manage-body" data-section="sync">
      <ResourceView resource={status ? { ...resource, data: status } : resource} what="sync">
        {(current) => {
          const state = stateOf(current);
          return (
            <>
              <p className="settings-text">
                Keep settings, pinned tabs, vault items and chosen memories in step between your devices, end to end encrypted. <Badge tone={state.tone}>{state.label}</Badge>
              </p>
              <Storage key={`${current.set_up}-${current.location}`} status={current} onChanged={setSaved} />
              <Account status={current} onChanged={setSaved} />
              {current.set_up && current.unlocked ? <Running status={current} onChanged={refresh} /> : null}
            </>
          );
        }}
      </ResourceView>
      <PinnedTabs />
    </div>
  );
}
