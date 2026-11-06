import { Check, Copy, Pin, RefreshCw, Trash2 } from "lucide-react";
import { useCallback, useId, useState, type FormEvent } from "react";
import type { PinnedTab, SyncReport, SyncSecrets, SyncStatus } from "../api/types";
import { useClient } from "../context";
import { errorMessage, useResource } from "../hooks/useResource";
import { t, type MessageKey } from "../i18n";
import { tNodes } from "../i18n/react";
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
  if (!status.configured) return { label: t("common.off"), tone: "neutral" };
  if (!status.set_up) return { label: t("connections.sync.state.notJoined"), tone: "attention" };
  if (!status.unlocked) return { label: t("connections.sync.state.locked"), tone: "attention" };
  return { label: t("common.on"), tone: "success" };
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
        toast.notify(t("connections.sync.saved"));
      } catch (cause) {
        setFailure(toFailure(cause));
      } finally {
        setBusy(false);
      }
    },
    [backend, client, dir, onChanged, settings, tagText, toast, url, user],
  );

  return (
    <SectionList title={t("connections.sync.where.title")} description={t("connections.sync.where.description")}>
      <form className="policy-form" aria-label={t("connections.sync.storage")} onSubmit={(event) => void save(event)}>
        {locked ? (
          <p className="settings-text">
            {tNodes(status.backend === "dir" ? "connections.sync.lockedDir" : "connections.sync.lockedWebdav", { location: <code>{status.location}</code> })}
          </p>
        ) : (
          <>
            <Segmented
              label={t("connections.sync.storage")}
              value={backend}
              onChange={setBackend}
              options={[
                { value: "off", label: t("common.off") },
                { value: "dir", label: t("connections.sync.backend.dir") },
                { value: "webdav", label: "WebDAV" },
              ]}
            />
            {backend === "dir" ? (
              <div className="field">
                <label htmlFor={`${id}-dir`}>
                  <span>{t("connections.sync.folder")}</span>
                </label>
                <input id={`${id}-dir`} value={dir} placeholder="~/Sync/openside" spellCheck={false} onChange={(event) => setDir(event.target.value)} />
                <p className="settings-text">{t("connections.sync.folderHint")}</p>
              </div>
            ) : null}
            {backend === "webdav" ? (
              <>
                <div className="field">
                  <label htmlFor={`${id}-url`}>
                    <span>{t("connections.sync.webdavAddress")}</span>
                  </label>
                  <input id={`${id}-url`} value={url} placeholder="https://dav.example.com/openside/" spellCheck={false} onChange={(event) => setUrl(event.target.value)} />
                </div>
                <div className="field">
                  <label htmlFor={`${id}-user`}>
                    <span>{t("connections.sync.username")}</span>
                  </label>
                  <input id={`${id}-user`} value={user} autoComplete="off" spellCheck={false} onChange={(event) => setUser(event.target.value)} />
                  <p className="settings-text">{t("connections.sync.passwordHint")}</p>
                </div>
              </>
            ) : null}
          </>
        )}
        {locked || backend !== "off" ? (
          <>
            <div className="policy-switch">
              <div>
                <label htmlFor={`${id}-settings`}>{t("connections.sync.policySwitch")}</label>
                <p className="settings-text">{t("connections.sync.policyHint")}</p>
              </div>
              <Switch id={`${id}-settings`} checked={settings} onChange={setSettings} />
            </div>
            <div className="field">
              <label htmlFor={`${id}-tags`}>
                <span>{t("connections.sync.tags")}</span>
              </label>
              <input id={`${id}-tags`} value={tagText} placeholder="shared, travel" spellCheck={false} onChange={(event) => setTagText(event.target.value)} />
              <p className="settings-text">{t("connections.sync.tagsHint")}</p>
            </div>
          </>
        ) : null}
        <FailureNote failure={failure} what={t("connections.sync.whatChange")} onRetry={() => void save()} />
        <div>
          <Button type="submit" variant="primary" disabled={busy || (backend === "dir" && !dir.trim()) || (backend === "webdav" && !url.trim())}>
            {busy ? t("common.saving") : t("common.save")}
          </Button>
        </div>
      </form>
    </SectionList>
  );
}

function Passphrase({ id, label = t("connections.sync.passphrase"), hint }: { id: string; label?: string; hint?: string }) {
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
    toast.notify((await copyText(text)) ? t("connections.sync.codeCopied") : t("connections.sync.copyFailed"), "success");
  }

  if (!status.configured) return null;

  if (!status.set_up) {
    return (
      <SectionList title={t("connections.sync.account.title")} description={t("connections.sync.account.description")}>
        <Segmented
          label={t("connections.sync.account.label")}
          value={mode}
          onChange={setMode}
          options={[
            { value: "new", label: t("connections.sync.account.new") },
            { value: "join", label: t("connections.sync.account.join") },
          ]}
        />
        {created ? (
          <div className="agent-link-result" role="status">
            <p className="onb-result-title">
              <Check size={16} strokeWidth={ICON_STROKE} aria-hidden="true" /> {t("connections.sync.account.started")}
            </p>
            <div className="remote-copy-row">
              <input readOnly aria-label={t("connections.sync.code")} value={created} onFocus={(event) => event.currentTarget.select()} />
              <Button aria-label={t("connections.sync.copyCode")} onClick={() => void copy(created)}>
                <Copy size={16} strokeWidth={ICON_STROKE} aria-hidden="true" /> {t("common.copy")}
              </Button>
            </div>
            <p className="settings-text">
              {t("connections.sync.codeHint")}
            </p>
          </div>
        ) : (
          <form
            className="policy-form"
            aria-label={t(mode === "new" ? "connections.sync.startForm" : "connections.sync.joinForm")}
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
                  <span>{t("connections.sync.code")}</span>
                </label>
                <input id="sync-code" value={code} placeholder="osync1-…" spellCheck={false} onChange={(event) => setCode(event.target.value)} />
              </div>
            ) : null}
            {needsPassphrase(status) ? (
              <Passphrase
                id="passphrase"
                hint={t(mode === "new" ? "connections.sync.newHint" : "connections.sync.joinHint")}
              />
            ) : (
              <p className="settings-text">{t("connections.sync.fromCommandSetup")}</p>
            )}
            {needsPassword(status) ? <Passphrase id="webdav_password" label={t("connections.sync.webdavPassword")} /> : null}
            <FailureNote failure={failure} what={t("connections.sync.whatSetup")} onRetry={() => setFailure(null)} />
            <div>
              <Button type="submit" variant="primary" disabled={busy || (mode === "join" && !code.trim())}>
                {busy ? t("connections.sync.working") : mode === "new" ? t("connections.sync.start") : t("connections.sync.join")}
              </Button>
            </div>
          </form>
        )}
      </SectionList>
    );
  }

  if (!status.unlocked) {
    return (
      <SectionList title={t("connections.sync.unlock.title")} description={t("connections.sync.unlock.description")}>
        <form className="policy-form" aria-label={t("connections.sync.unlock.title")} onSubmit={(event) => void submit(event, (secrets) => client.unlockSync(secrets))}>
          {needsPassphrase(status) ? <Passphrase id="passphrase" /> : <p className="settings-text">{t("connections.sync.unlock.fromCommand")}</p>}
          {needsPassword(status) ? <Passphrase id="webdav_password" label={t("connections.sync.webdavPassword")} /> : null}
          <FailureNote failure={failure} what={t("connections.sync.unlock.what")} onRetry={() => setFailure(null)} />
          <div>
            <Button type="submit" variant="primary" disabled={busy}>
              {busy ? t("connections.sync.unlock.busy") : t("connections.sync.unlock.submit")}
            </Button>
          </div>
        </form>
      </SectionList>
    );
  }

  return null;
}

function Report({ report }: { report: SyncReport }) {
  const rows: [MessageKey, number][] = [
    ["connections.sync.report.uploaded", report.pushed],
    ["connections.sync.report.downloaded", report.pulled],
    ["connections.sync.report.applied", report.applied],
    ["connections.sync.report.replaced", report.conflicts],
    ["connections.sync.report.rejected", report.rejected],
    ["connections.sync.report.waiting", report.deferred],
  ];
  return (
    <div className="sync-report" role="status" aria-label={t("connections.sync.report.label")}>
      {rows.map(([label, value]) => (
        <StatusRow key={label} label={t(label)}>
          {value}
        </StatusRow>
      ))}
      <StatusRow label={t("connections.sync.report.vaultItems")}>{report.vault_available ? t("connections.sync.report.included") : t("connections.sync.report.leftOut")}</StatusRow>
      {report.rejected > 0 ? <p className="settings-text">{t("connections.sync.report.rejectedNote")}</p> : null}
      {report.deferred > 0 ? <p className="settings-text">{t("connections.sync.report.waitingNote")}</p> : null}
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
      title={t("connections.sync.running.title")}
      actions={
        <Button variant="primary" disabled={busy || status.exhausted} onClick={() => void run()}>
          <RefreshCw size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
          {busy ? t("connections.sync.running.syncing") : t("connections.sync.running.syncNow")}
        </Button>
      }
    >
      {status.exhausted ? <ErrorNote>{t("connections.sync.running.exhausted")}</ErrorNote> : null}
      <div className="settings-group">
        <StatusRow label={t("connections.sync.running.lastSync")}>{status.last_sync ? formatDateTime(status.last_sync) : t("connections.sync.running.never")}</StatusRow>
        <StatusRow label={t("connections.sync.running.records")}>{status.records}</StatusRow>
        <StatusRow label={t("connections.sync.running.pending")}>{status.pending}</StatusRow>
        {status.account_id ? <StatusRow label={t("connections.sync.running.account")}><code>{status.account_id}</code></StatusRow> : null}
        {status.device_id ? <StatusRow label={t("connections.sync.running.device")}><code>{status.device_id}</code></StatusRow> : null}
      </div>
      <FailureNote failure={failure} what={t("connections.sync.running.what")} onRetry={() => void run()} />
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
    <SectionList title={t("connections.sync.pinned.title")} description={t("connections.sync.pinned.description")}>
      <ResourceView resource={tabs} what={t("connections.sync.pinned.what")}>
        {({ tabs: list }) =>
          list.length === 0 ? (
            <EmptyState icon={Pin} title={t("connections.sync.pinned.emptyTitle")} detail={t("connections.sync.pinned.emptyDetail")} />
          ) : (
            <ul className="resource-list" aria-label={t("connections.sync.pinned.listLabel")}>
              {list.map((tab) => (
                <li key={tab.id}>
                  <ResourceCard
                    icon={Pin}
                    title={tab.title || tab.url}
                    detail={tab.title ? tab.url : undefined}
                    trailing={
                      <IconButton label={t("connections.sync.pinned.unpin", { name: tab.title || tab.url })} onClick={() => void remove(tab)}>
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
      <form className="agent-link-form" onSubmit={(event) => void add(event)} aria-label={t("connections.sync.pinned.form")}>
        <label className="field">
          <span>{t("connections.sync.pinned.address")}</span>
          <input value={url} placeholder="https://example.com" spellCheck={false} onChange={(event) => setUrl(event.target.value)} />
        </label>
        <Button type="submit" disabled={!url.trim()}>
          {t("connections.sync.pinned.pin")}
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
      <ResourceView resource={status ? { ...resource, data: status } : resource} what={t("connections.sync.what")}>
        {(current) => {
          const state = stateOf(current);
          return (
            <>
              <p className="settings-text">
                {t("connections.sync.intro")} <Badge tone={state.tone}>{state.label}</Badge>
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
