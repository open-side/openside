import { Bot, Ellipsis, MonitorSmartphone } from "lucide-react";
import { useCallback, useEffect, useId, useState, type FormEvent } from "react";
import type { Device, SyncStatus } from "../../api/types";
import { applyLook, DEFAULT_LOOK } from "../../appearance";
import { useClient } from "../../context";
import { errorMessage, useResource } from "../../hooks/useResource";
import { applyLocalePreference, t } from "../../i18n";
import { ResourceView, useConfirm } from "../../pages/blocks";
import { formatDate, formatRelative } from "../../pages/format";
import { SyncPage } from "../../pages/SyncPage";
import { applyTheme } from "../../theme";
import { Badge, Button, Dialog, DialogActions, ICON_STROKE, IconButton, MenuItem, PopoverMenu, SettingsCard, SettingsRow, SettingsSection, useToast } from "../../ui";
import { settingsPath } from "../nav";
import { Avatar } from "./personalShared";
import "./personal.css";

const ICON = { size: 16, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;
const NAME_LIMIT = 80;
/** The daemon refuses longer device names (400 invalid_value). */
const DEVICE_NAME_LIMIT = 100;

function ProfileSection() {
  const client = useClient();
  const toast = useToast();
  const id = useId();
  const [saved, setSaved] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [invalid, setInvalid] = useState(false);
  useEffect(() => {
    let cancelled = false;
    client.preferences().then(
      (preferences) => {
        if (cancelled) return;
        setSaved(preferences.name ?? "");
        setDraft(preferences.name ?? "");
      },
      () => undefined,
    );
    return () => {
      cancelled = true;
    };
  }, [client]);

  const save = async () => {
    const name = draft.trim();
    if (saved === null || name === saved) return;
    if ([...name].length > NAME_LIMIT) {
      setInvalid(true);
      return;
    }
    setInvalid(false);
    try {
      const next = await client.updatePreferences({ name });
      setSaved(next.name ?? "");
      setDraft(next.name ?? "");
      toast.notify(t(name ? "personal.account.name.saved" : "personal.account.name.cleared"));
    } catch (cause) {
      toast.notify(errorMessage(cause), "error");
    }
  };

  return (
    <SettingsSection label={t("personal.account.profile")}>
      <SettingsCard>
        <SettingsRow title={t("personal.account.avatar")} description={t("personal.account.avatar.description")} control={<Avatar name={saved || "?"} size={32} />} />
        <SettingsRow
          title={<label htmlFor={`${id}-name`}>{t("personal.account.name.title")}</label>}
          description={invalid ? t("personal.account.name.invalid") : t("personal.account.name.description")}
          control={(ids) => (
            <input
              id={`${id}-name`}
              className="personal-name-input"
              value={draft}
              maxLength={NAME_LIMIT * 2}
              disabled={saved === null}
              placeholder={t("personal.account.name.placeholder")}
              aria-describedby={ids.descriptionId}
              aria-invalid={invalid || undefined}
              onChange={(event) => setDraft(event.target.value)}
              onBlur={() => void save()}
              onKeyDown={(event) => {
                if (event.key === "Enter") void save();
                if (event.key === "Escape") setDraft(saved ?? "");
              }}
            />
          )}
        />
      </SettingsCard>
    </SettingsSection>
  );
}

/** The device this browser is paired as, by the daemon's own account; null until known or when it has none. */
function useMyDeviceId(): string | null {
  const client = useClient();
  const [id, setId] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    client.myDevice().then(
      (me) => alive && setId(me.device_id || null),
      () => undefined,
    );
    return () => {
      alive = false;
    };
  }, [client]);
  return id;
}

function RenameDialog({ device, onClose, onRenamed }: { device: Device | null; onClose: () => void; onRenamed: () => void }) {
  const client = useClient();
  const toast = useToast();
  const id = useId();
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (!device) return;
    setDraft(device.name);
    setError(null);
  }, [device]);

  const save = async (event: FormEvent) => {
    event.preventDefault();
    if (!device) return;
    const name = draft.trim();
    if (!name || [...name].length > DEVICE_NAME_LIMIT) {
      setError(t("personal.account.device.nameInvalid"));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await client.renameDevice(device.device_id, name);
      toast.notify(t("personal.account.device.renamed"));
      onRenamed();
      onClose();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={device !== null} onClose={onClose} title={t("personal.account.device.renameTitle", { name: device?.name || t("settings.devices.unnamed") })}>
      <form onSubmit={(event) => void save(event)}>
        <label className="personal-field" htmlFor={`${id}-name`}>
          {t("personal.account.device.nameLabel")}
        </label>
        <input
          id={`${id}-name`}
          className="personal-wide-input"
          value={draft}
          maxLength={DEVICE_NAME_LIMIT * 2}
          aria-invalid={error !== null || undefined}
          aria-describedby={error ? `${id}-error` : undefined}
          onChange={(event) => setDraft(event.target.value)}
          autoFocus
        />
        {error ? (
          <p id={`${id}-error`} className="personal-error" role="alert">
            {error}
          </p>
        ) : null}
        <DialogActions>
          <Button variant="ghost" onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button type="submit" variant="primary" disabled={busy}>
            {t("common.save")}
          </Button>
        </DialogActions>
      </form>
    </Dialog>
  );
}

function DevicesSection() {
  const client = useClient();
  const toast = useToast();
  const load = useCallback(() => client.listDevices(), [client]);
  const devices = useResource(load);
  const [confirm, ask] = useConfirm();
  const [renaming, setRenaming] = useState<Device | null>(null);
  const self = useMyDeviceId();

  const revoke = async (device: Device) => {
    try {
      await client.revokeDevice(device.device_id);
      toast.notify(t("settings.devices.revoked"));
      await devices.reload();
    } catch (cause) {
      devices.setError(errorMessage(cause));
    }
  };

  return (
    <SettingsSection label={t("personal.account.devices")}>
      <ResourceView resource={devices} what={t("settings.devices.what")}>
        {(data) =>
          data.devices.length === 0 ? (
            <SettingsCard>
              <SettingsRow title={t("personal.account.devices.empty")} to={settingsPath("developers")} description={t("personal.account.devices.pair")} />
            </SettingsCard>
          ) : (
            <SettingsCard>
              <ul className="personal-device-list" data-section="devices" aria-label={t("settings.devices.listLabel")}>
                {data.devices.map((device) => {
                  const name = device.name || t("settings.devices.unnamed");
                  const kind = t(device.kind === "agent_client" ? "settings.devices.kind.agent" : "settings.devices.kind.person");
                  const mine = self !== null && device.device_id === self;
                  const label = t("personal.account.device.options", { name });
                  return (
                    <li key={device.device_id} data-device={device.device_id}>
                      <SettingsRow
                        leading={device.kind === "agent_client" ? <Bot {...ICON} /> : <MonitorSmartphone {...ICON} />}
                        title={name}
                        badge={mine ? <Badge>{t("settings.devices.thisDevice")}</Badge> : undefined}
                        description={device.created_at ? t("personal.account.device.meta", { kind, date: formatDate(device.created_at) }) : kind}
                        control={
                          <PopoverMenu
                            label={label}
                            trigger={
                              <IconButton label={label} size="small">
                                <Ellipsis {...ICON} />
                              </IconButton>
                            }
                          >
                            <MenuItem onClick={() => setRenaming(device)}>{t("personal.account.device.rename")}</MenuItem>
                            <MenuItem
                              danger
                              onClick={() =>
                                ask({
                                  title: t("settings.devices.revokeTitle", { name }),
                                  description: t(mine ? "settings.devices.revokeSelf" : "settings.devices.revokeOther"),
                                  confirmLabel: t("settings.devices.revoke"),
                                  onConfirm: () => void revoke(device),
                                })
                              }
                            >
                              {t("settings.devices.revoke")}
                            </MenuItem>
                          </PopoverMenu>
                        }
                      />
                    </li>
                  );
                })}
              </ul>
            </SettingsCard>
          )
        }
      </ResourceView>
      {confirm}
      <RenameDialog device={renaming} onClose={() => setRenaming(null)} onRenamed={() => void devices.reload()} />
    </SettingsSection>
  );
}

export function syncDescription(status: SyncStatus | null): string {
  if (!status) return t("settings.status.checking");
  if (!status.configured) return t("personal.account.sync.off");
  if (!status.set_up) return t("personal.account.sync.needsSetup");
  if (!status.unlocked) return t("personal.account.sync.locked");
  return status.last_sync ? t("personal.account.sync.last", { time: formatRelative(status.last_sync) }) : t("personal.account.sync.never");
}

function SyncSection() {
  const client = useClient();
  const toast = useToast();
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const refresh = useCallback(() => client.syncStatus().then(setStatus, () => undefined), [client]);
  useEffect(() => {
    void refresh();
  }, [refresh]);

  const syncNow = async () => {
    setBusy(true);
    try {
      await client.runSync();
      toast.notify(t("personal.account.sync.done"));
      await refresh();
    } catch (cause) {
      toast.notify(errorMessage(cause), "error");
    } finally {
      setBusy(false);
    }
  };
  const ready = status !== null && status.set_up && status.unlocked;

  return (
    <SettingsSection label={t("personal.account.sync")}>
      <SettingsCard>
        <SettingsRow
          title={t("personal.account.sync.title")}
          description={syncDescription(status)}
          control={
            <span className="personal-actions">
              {ready ? (
                <Button disabled={busy} onClick={() => void syncNow()}>
                  {t("personal.account.sync.now")}
                </Button>
              ) : null}
              <Button onClick={() => setOpen(true)}>{t("personal.configure")}</Button>
            </span>
          }
        />
      </SettingsCard>
      <Dialog
        open={open}
        onClose={() => {
          setOpen(false);
          void refresh();
        }}
        size="wide"
        title={t("personal.account.sync.title")}
      >
        <SyncPage />
      </Dialog>
    </SettingsSection>
  );
}

/**
 * Everything this browser keeps for openside, on disk and in the running page, so a reset leaves
 * nothing of the old setup behind.
 */
function resetThisBrowser() {
  for (const storage of [localStorage, sessionStorage]) {
    for (const key of Object.keys(storage)) if (key.startsWith("openside.")) storage.removeItem(key);
  }
  applyTheme("system");
  applyLook(DEFAULT_LOOK);
  applyLocalePreference();
  // The signal another tab's clear sends: the shortcut cache and sidebar choices re-read the emptied storage.
  window.dispatchEvent(new StorageEvent("storage", { key: null }));
}

/** `device` is the id the daemon knows this browser by, or null when it has none to disconnect. */
type Reset = { device: string | null; error: string | null; busy: boolean };

function DangerSection({ onSignOut }: { onSignOut: () => void }) {
  const client = useClient();
  const toast = useToast();
  const [reset, setReset] = useState<Reset | null>(null);
  const [checking, setChecking] = useState(false);

  const open = async () => {
    setChecking(true);
    const device = await client.myDevice().then(
      (me) => me.device_id || null,
      () => null,
    );
    setChecking(false);
    setReset({ device, error: null, busy: false });
  };
  const finish = (disconnected: boolean) => {
    resetThisBrowser();
    toast.notify(t(disconnected ? "personal.account.reset.done" : "personal.account.reset.clearedOnly"));
    setReset(null);
    onSignOut();
  };
  const confirm = async (current: Reset) => {
    if (!current.device) {
      finish(false);
      return;
    }
    setReset({ ...current, busy: true, error: null });
    try {
      await client.revokeDevice(current.device);
      finish(true);
    } catch (cause) {
      // Never sign out claiming a disconnect that did not happen; clearing only this browser is offered instead.
      setReset({ ...current, busy: false, error: errorMessage(cause) });
    }
  };

  return (
    <SettingsSection label={t("personal.account.danger")}>
      <SettingsCard>
        <SettingsRow
          title={t("personal.account.reset.title")}
          description={t("personal.account.reset.description")}
          control={
            <Button variant="danger" disabled={checking} onClick={() => void open()}>
              {t("personal.account.reset.action")}
            </Button>
          }
        />
      </SettingsCard>
      <Dialog
        open={reset !== null}
        onClose={() => setReset(null)}
        title={t("personal.account.reset.confirmTitle")}
        description={t(reset?.device === null ? "personal.account.reset.browserOnlyDescription" : "personal.account.reset.confirmDescription")}
      >
        {reset?.error ? (
          <p className="personal-error" role="alert">
            {t("personal.account.reset.revokeFailed", { reason: reset.error })}
          </p>
        ) : null}
        <DialogActions>
          <Button variant="ghost" onClick={() => setReset(null)}>
            {t("common.cancel")}
          </Button>
          {reset?.error ? (
            <Button variant="danger" onClick={() => finish(false)}>
              {t("personal.account.reset.clearOnly")}
            </Button>
          ) : null}
          <Button variant={reset?.error ? "secondary" : "danger"} disabled={reset?.busy} onClick={() => reset && void confirm(reset)}>
            {reset?.error ? t("common.retry") : reset?.device === null ? t("personal.account.reset.clearOnly") : t("personal.account.reset.action")}
          </Button>
        </DialogActions>
      </Dialog>
    </SettingsSection>
  );
}

/** Settings › Account: the person's name, paired devices, sync and resetting this browser. */
export function AccountSettings({ onSignOut }: { onSignOut: () => void }) {
  return (
    <div className="personal-page" data-section="account">
      <ProfileSection />
      <DevicesSection />
      <SyncSection />
      <DangerSection onSignOut={onSignOut} />
    </div>
  );
}
