import { ChevronDown, Plus, RefreshCw, X } from "lucide-react";
import { useCallback, useEffect, useId, useMemo, useRef, useState, type FormEvent } from "react";
import type { SyncStatus, UpdateVaultSettingsRequest, VaultAutoLock, VaultPolicy, VaultSettings } from "../../api/types";
import { useClient } from "../../context";
import { errorMessage, useResource } from "../../hooks/useResource";
import { t, tMaybe, type MessageKey } from "../../i18n";
import { ResourceView } from "../../pages/blocks";
import { ScopeNotice } from "../../pages/ScopeNotice";
import { Button, ICON_STROKE, IconButton, Select, SettingsCard, SettingsRow, SettingsSection, Switch, useToast } from "../../ui";
import { settingsPath } from "../nav";
import { syncDescription } from "./Account";
import { ClearDialog, ExportDialog, ImportDialog, RecoveryKeyDialog, type VaultContext } from "./passwordsData";
import { POLICY_LABEL, VaultsDialog, VaultStatusCard } from "./passwordsVault";
import "./personal.css";
import "./passwords.css";

const ICON = { size: 16, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;

const AUTO_LOCK: readonly VaultAutoLock[] = ["immediately", "one_minute", "five_minutes", "fifteen_minutes", "one_hour", "four_hours", "one_day", "one_week", "never"];

// The PATCH body names every field; null leaves a setting as it is.
const UNCHANGED: UpdateVaultSettingsRequest = { auto_lock: null, autofill: null, biometric_unlock: null, default_policy: null, disable_in_incognito: null };
type AutofillPatch = NonNullable<UpdateVaultSettingsRequest["autofill"]>;
const AUTOFILL_UNCHANGED: AutofillPatch = { allow_insecure: null, auto_submit: null, enabled: null, excluded_domains: null };

type Sheet = "vaults" | "import" | "connect" | "export" | "recovery" | "clear" | null;

/**
 * Remote devices can view these settings but not run the local-only actions. A relay is always
 * remote; otherwise the daemon says whether this request carries the computer's own authority.
 * An unknown answer counts as local and lets the daemon refuse.
 */
function useRemoteDevice(): boolean {
  const client = useClient();
  const [remote, setRemote] = useState(client.relay);
  useEffect(() => {
    if (client.relay) return;
    let alive = true;
    client.remoteStatus().then(
      (status) => alive && setRemote(!status.configure_allowed),
      () => undefined,
    );
    return () => {
      alive = false;
    };
  }, [client]);
  return remote;
}

/** The vault settings a person can change, one entry per control; autofill fields are flattened. */
interface Editable {
  auto_lock: VaultAutoLock;
  biometric_unlock: boolean;
  default_policy: VaultPolicy;
  enabled: boolean;
  auto_submit: boolean;
  allow_insecure: boolean;
  excluded_domains: string[];
}
type Change = Partial<Editable>;
/** Saves the change computed from the current settings; resolves to the refusal message, or null once saved. */
type SaveSettings = (next: (settings: VaultSettings) => Change, quiet?: boolean) => Promise<string | null>;
const AUTOFILL_FIELDS = ["enabled", "auto_submit", "allow_insecure", "excluded_domains"] as const;

function withChange(settings: VaultSettings, change: Change): VaultSettings {
  const next = { ...settings, autofill: { ...settings.autofill } };
  for (const [field, value] of Object.entries(change)) {
    if ((AUTOFILL_FIELDS as readonly string[]).includes(field)) Object.assign(next.autofill, { [field]: value });
    else Object.assign(next, { [field]: value });
  }
  return next;
}

function patchFor(change: Change): UpdateVaultSettingsRequest {
  const { enabled = null, auto_submit = null, allow_insecure = null, excluded_domains = null, ...top } = change;
  const autofill = enabled !== null || auto_submit !== null || allow_insecure !== null || excluded_domains !== null;
  return { ...UNCHANGED, ...top, autofill: autofill ? { ...AUTOFILL_UNCHANGED, enabled, auto_submit, allow_insecure, excluded_domains } : null };
}

/**
 * Vault settings with optimistic changes. Saves go out one at a time, and each changed field
 * remembers the generation of its latest save: an older save settling (or failing) never hides
 * a newer choice, and a refusal drops only that field's change, falling back to the daemon's
 * latest values for it rather than to a snapshot from before other changes.
 */
function useVaultSettings() {
  const client = useClient();
  const toast = useToast();
  const [server, setServer] = useState<VaultSettings | null>(null);
  const [pending, setPending] = useState<Change>({});
  // Read synchronously by `save`, so a change computed from the current state sees saves made in the same tick.
  const current = useRef<{ server: VaultSettings | null; pending: Change }>({ server: null, pending: {} });
  const generation = useRef(0);
  const fieldGeneration = useRef<Partial<Record<keyof Editable, number>>>({});
  const queue = useRef<Promise<unknown>>(Promise.resolve());

  const confirm = useCallback((next: VaultSettings) => {
    current.current.server = next;
    setServer(next);
  }, []);
  const reload = useCallback(() => client.vaultSettings().then(confirm, () => undefined), [client, confirm]);
  useEffect(() => {
    void reload();
  }, [reload]);

  // `quiet` leaves showing a refusal to the caller (the domain list shows it inline).
  const save = useCallback<SaveSettings>(
    (next, quiet = false) => {
      const base = current.current.server;
      if (!base) return Promise.resolve(null);
      const change = next(withChange(base, current.current.pending));
      const fields = Object.keys(change) as (keyof Editable)[];
      const id = ++generation.current;
      for (const field of fields) fieldGeneration.current[field] = id;
      const update = (map: (pending: Change) => Change) => {
        current.current.pending = map(current.current.pending);
        setPending(current.current.pending);
      };
      update((pending) => ({ ...pending, ...change }));
      const settle = () =>
        update((pending) => {
          const rest = { ...pending };
          for (const field of fields) if (fieldGeneration.current[field] === id) delete rest[field];
          return rest;
        });
      const result = queue.current.then(() =>
        client.updateVaultSettings(patchFor(change)).then(
          (saved) => {
            confirm(saved);
            settle();
            return null;
          },
          (cause: unknown) => {
            settle();
            const reason = errorMessage(cause);
            if (!quiet) toast.notify(t("passwords.saveFailed", { reason }), "error");
            return reason;
          },
        ),
      );
      queue.current = result;
      return result;
    },
    [client, toast, confirm],
  );
  const settings = useMemo(() => server && withChange(server, pending), [server, pending]);
  return { settings, reload, save };
}

function recoveryDescription(settings: VaultSettings | null): string {
  if (!settings) return t("settings.status.checking");
  if (!settings.recovery.configured) return t("passwords.recovery.none");
  return t(settings.recovery.current ? "passwords.recovery.current" : "passwords.recovery.stale");
}

function biometricsDescription(settings: VaultSettings | null, remote: boolean): string {
  const reason = settings?.biometrics.reason;
  // Reason codes from a newer daemon are shown as sent.
  if (settings && !settings.biometrics.available && reason) return tMaybe(`passwords.biometrics.reason.${reason}`) ?? reason;
  if (remote && settings && !settings.biometric_unlock) return t("passwords.biometrics.remote");
  return t("passwords.biometrics.description");
}

/** The excluded-domains list inside the Autofill card; refused domains are shown inline. */
function ExcludedDomains({ domains, save }: { domains: readonly string[]; save: SaveSettings }) {
  const id = useId();
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);

  async function put(next: (current: string[]) => string[]) {
    setError(null);
    const refusal = await save((settings) => ({ excluded_domains: next(settings.autofill.excluded_domains) }), true);
    setError(refusal);
    return refusal === null;
  }

  async function add(event: FormEvent) {
    event.preventDefault();
    const domain = draft.trim();
    if (!domain) return;
    if (await put((current) => [...current, domain])) setDraft("");
  }

  return (
    <div className="passwords-excluded" id={id}>
      {domains.length === 0 ? (
        <p className="passwords-hint">{t("passwords.excluded.empty")}</p>
      ) : (
        <ul className="passwords-domain-list" aria-label={t("passwords.excluded.title")}>
          {domains.map((domain) => (
            <li key={domain}>
              <span>{domain}</span>
              <IconButton
                label={t("passwords.excluded.remove", { domain })}
                size="small"
                onClick={() => void put((current) => current.filter((value) => value !== domain))}
              >
                <X {...ICON} />
              </IconButton>
            </li>
          ))}
        </ul>
      )}
      <form className="personal-inline-form" onSubmit={(event) => void add(event)}>
        <input
          className="passwords-domain-input"
          value={draft}
          placeholder="example.com"
          aria-label={t("passwords.excluded.label")}
          aria-invalid={error !== null || undefined}
          autoCapitalize="none"
          spellCheck={false}
          onChange={(event) => setDraft(event.target.value)}
        />
        <Button type="submit" disabled={!draft.trim()}>
          {t("common.add")}
        </Button>
      </form>
      {error ? (
        <p className="personal-error" role="alert">
          {error}
        </p>
      ) : null}
    </div>
  );
}

/**
 * Settings › Passwords: the built-in vault (Aside's five sections). Rows without an engine show
 * the nearest behaviour: there is one vault and no external manager, so connecting one points
 * at the CSV import, and exports are CSV only.
 */
export function PasswordsSettings() {
  const client = useClient();
  const toast = useToast();
  const remote = useRemoteDevice();
  const loadStatus = useCallback(() => client.vaultStatus(), [client]);
  const status = useResource(loadStatus);
  const loadItems = useCallback(() => client.vaultItems(), [client]);
  const items = useResource(loadItems);
  const { settings, reload: reloadSettings, save } = useVaultSettings();
  const [sync, setSync] = useState<SyncStatus | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [sheet, setSheet] = useState<Sheet>(null);
  const [domainsOpen, setDomainsOpen] = useState(false);
  const domainsId = useId();

  const refreshSync = useCallback(() => client.syncStatus().then(setSync, () => undefined), [client]);
  useEffect(() => {
    void refreshSync();
  }, [refreshSync]);

  const { reload: reloadStatus } = status;
  const { reload: reloadItems } = items;
  const changed = useCallback(() => {
    void reloadStatus();
    void reloadItems();
    void reloadSettings();
  }, [reloadStatus, reloadItems, reloadSettings]);

  if (status.forbidden) {
    return (
      <div className="personal-page" data-section="passwords">
        <ScopeNotice what={t("connections.vault.what")} onGranted={changed} />
      </div>
    );
  }

  const vault = status.data;
  const context: VaultContext | null = vault ? { status: vault, settings, remote } : null;
  const close = () => setSheet(null);
  const itemCount = items.data?.available ? items.data.items.length : null;
  const vaultsDescription = !vault
    ? t("settings.status.checking")
    : vault.state === "unlocked"
      ? itemCount === null
        ? t("settings.status.checking")
        : t("passwords.vaults.count", { count: itemCount })
      : vault.state === "locked"
        ? t("passwords.vaults.locked")
        : vault.state === "uninitialized"
          ? t("passwords.vaults.none")
          : t("connections.vault.state.unavailable");
  const autofill = settings?.autofill;
  const syncReady = sync !== null && sync.set_up && sync.unlocked;

  const syncNow = async () => {
    setSyncing(true);
    try {
      await client.runSync();
      toast.notify(t("personal.account.sync.done"));
      await refreshSync();
    } catch (cause) {
      toast.notify(errorMessage(cause), "error");
    } finally {
      setSyncing(false);
    }
  };

  return (
    <div className="personal-page" data-section="passwords">
      <SettingsSection label={t("passwords.managers")} info={t("passwords.managers.info")}>
        <SettingsCard>
          <SettingsRow
            title={t("passwords.default.title")}
            description={t("passwords.default.description")}
            control={(ids) => (
              <span className="passwords-fixed" title={t("passwords.default.only")}>
                <Switch aria-labelledby={ids.labelId} aria-describedby={ids.descriptionId} checked disabled onChange={() => undefined} />
              </span>
            )}
          />
        </SettingsCard>
        <SettingsCard>
          <button type="button" className="ui-row ui-row--single ui-row--action passwords-connect" onClick={() => setSheet("connect")}>
            <Plus size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
            <span className="ui-row-title">{t("passwords.connect")}</span>
          </button>
        </SettingsCard>
        {vault && vault.state !== "unlocked" ? (
          <VaultStatusCard status={vault} settings={settings} remote={remote} onChanged={changed} />
        ) : !vault && status.error ? (
          <ResourceView resource={status} what={t("connections.vault.what")}>
            {() => null}
          </ResourceView>
        ) : null}
      </SettingsSection>

      <SettingsSection label={t("passwords.general")}>
        <SettingsCard>
          <SettingsRow
            title={t("passwords.autoLock.title")}
            description={t("passwords.autoLock.description")}
            control={(ids) => (
              <Select
                variant="inline"
                aria-labelledby={ids.labelId}
                aria-describedby={ids.descriptionId}
                disabled={!settings}
                value={settings?.auto_lock ?? ""}
                placeholder="—"
                options={AUTO_LOCK.map((value) => ({ value, label: t(`passwords.autoLock.${value}` as MessageKey) }))}
                onChange={(value) => void save(() => ({ auto_lock: value as VaultAutoLock }))}
              />
            )}
          />
          <SettingsRow
            title={t("passwords.biometrics.title")}
            description={biometricsDescription(settings, remote)}
            control={(ids) => (
              <Switch
                aria-labelledby={ids.labelId}
                aria-describedby={ids.descriptionId}
                checked={settings?.biometric_unlock ?? false}
                disabled={!settings || (!settings.biometric_unlock && (!settings.biometrics.available || remote))}
                onChange={(on) => void save(() => ({ biometric_unlock: on }))}
              />
            )}
          />
          <SettingsRow
            title={t("passwords.recovery.title")}
            description={recoveryDescription(settings)}
            control={
              <Button disabled={!settings || vault?.state !== "unlocked"} onClick={() => setSheet("recovery")}>
                {t(settings?.recovery.configured ? "passwords.recovery.regenerate" : "passwords.recovery.create")}
              </Button>
            }
          />
        </SettingsCard>
        <SettingsCard>
          <SettingsRow
            title={t("passwords.policy.title")}
            description={t("passwords.policy.description")}
            control={(ids) => (
              <Select
                variant="inline"
                aria-labelledby={ids.labelId}
                aria-describedby={ids.descriptionId}
                disabled={!settings}
                value={settings?.default_policy ?? ""}
                placeholder="—"
                options={(Object.keys(POLICY_LABEL) as VaultPolicy[]).map((policy) => ({ value: policy, label: t(POLICY_LABEL[policy]) }))}
                onChange={(value) => void save(() => ({ default_policy: value as VaultPolicy }))}
              />
            )}
          />
        </SettingsCard>
      </SettingsSection>

      <SettingsSection label={t("passwords.data")}>
        <SettingsCard>
          <SettingsRow title={t("passwords.vaults.row")} description={vaultsDescription} onClick={() => setSheet("vaults")} />
          <SettingsRow title={t("passwords.import.title")} description={t("passwords.import.description")} onClick={() => setSheet("import")} />
          <SettingsRow title={t("passwords.export.title")} description={t("passwords.export.description")} onClick={() => setSheet("export")} />
        </SettingsCard>
      </SettingsSection>

      <SettingsSection label={t("passwords.autofill")}>
        <SettingsCard>
          <SettingsRow
            title={t("passwords.autofill.enabled.title")}
            description={t("passwords.autofill.enabled.description")}
            control={(ids) => (
              <Switch
                aria-labelledby={ids.labelId}
                aria-describedby={ids.descriptionId}
                checked={autofill?.enabled ?? false}
                disabled={!autofill}
                onChange={(on) => void save(() => ({ enabled: on }))}
              />
            )}
          />
          <SettingsRow
            title={t("passwords.autofill.autoSubmit.title")}
            description={t("passwords.autofill.autoSubmit.description")}
            control={(ids) => (
              <Switch
                aria-labelledby={ids.labelId}
                aria-describedby={ids.descriptionId}
                checked={autofill?.auto_submit ?? false}
                disabled={!autofill}
                onChange={(on) => void save(() => ({ auto_submit: on }))}
              />
            )}
          />
          <SettingsRow
            title={t("passwords.autofill.incognito.title")}
            description={t("passwords.autofill.incognito.description")}
            control={(ids) => (
              <span className="passwords-fixed">
                <Switch aria-labelledby={ids.labelId} aria-describedby={ids.descriptionId} checked disabled onChange={() => undefined} />
              </span>
            )}
          />
        </SettingsCard>
        <SettingsCard>
          <SettingsRow
            title={t("passwords.autofill.insecure.title")}
            description={t("passwords.autofill.insecure.description")}
            control={(ids) => (
              <Switch
                aria-labelledby={ids.labelId}
                aria-describedby={ids.descriptionId}
                checked={autofill?.allow_insecure ?? false}
                disabled={!autofill}
                onChange={(on) => void save(() => ({ allow_insecure: on }))}
              />
            )}
          />
          <button
            type="button"
            className="ui-row ui-row--action passwords-disclosure"
            aria-expanded={domainsOpen}
            aria-controls={domainsId}
            disabled={!autofill}
            onClick={() => setDomainsOpen(!domainsOpen)}
          >
            <span className="ui-row-copy">
              <span className="ui-row-title">{t("passwords.excluded.title")}</span>
              <span className="ui-row-description">{t("passwords.excluded.description")}</span>
            </span>
            <ChevronDown className="ui-row-glyph" strokeWidth={ICON_STROKE} aria-hidden="true" />
          </button>
          {domainsOpen && autofill ? (
            <div id={domainsId} className="passwords-excluded-panel">
              <ExcludedDomains domains={autofill.excluded_domains} save={save} />
            </div>
          ) : null}
        </SettingsCard>
      </SettingsSection>

      <SettingsSection label={t("passwords.danger")}>
        <SettingsCard>
          {syncReady ? (
            <SettingsRow
              title={t("passwords.sync.title")}
              description={syncDescription(sync)}
              control={
                <Button disabled={syncing} onClick={() => void syncNow()}>
                  <RefreshCw {...ICON} />
                  {t("personal.account.sync.now")}
                </Button>
              }
            />
          ) : (
            <SettingsRow title={t("passwords.sync.title")} description={syncDescription(sync)} to={settingsPath("account")} />
          )}
          <SettingsRow
            title={t("passwords.clear.title")}
            description={t("passwords.clear.description")}
            control={
              <Button variant="danger" disabled={!vault || vault.state === "unavailable"} onClick={() => setSheet("clear")}>
                {t("passwords.clear.action")}
              </Button>
            }
          />
        </SettingsCard>
      </SettingsSection>

      {vault && context ? (
        <>
          <VaultsDialog open={sheet === "vaults"} onClose={close} status={vault} settings={settings} remote={remote} items={items} onChanged={changed} />
          <ExportDialog open={sheet === "export"} onClose={close} vault={context} />
          <RecoveryKeyDialog open={sheet === "recovery"} onClose={close} vault={context} onCreated={() => void reloadSettings()} />
          <ClearDialog open={sheet === "clear"} onClose={close} vault={context} onCleared={changed} />
        </>
      ) : null}
      <ImportDialog open={sheet === "import" || sheet === "connect"} onClose={close} remote={remote} fromConnect={sheet === "connect"} onImported={changed} />
    </div>
  );
}
