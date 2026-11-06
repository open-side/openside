import { KeyRound, LockKeyhole, Plus, Search, Trash2 } from "lucide-react";
import { useState, type FormEvent } from "react";
import type { VaultBackend, VaultItemList, VaultPolicy, VaultPolicyOverride, VaultSettings, VaultStatus } from "../../api/types";
import { useClient } from "../../context";
import { errorMessage, type Resource } from "../../hooks/useResource";
import { t, tMaybe, type MessageKey } from "../../i18n";
import { CliHint } from "../../pages/CliHint";
import { CardActions, ErrorNote, ResourceCard, ResourceView, RowMenu, useConfirm } from "../../pages/blocks";
import { FailureNote, toFailure, type Failure } from "../../pages/Failure";
import { LocalOnlyNotice } from "../../remote/LocalOnlyNotice";
import { Badge, Button, Dialog, DialogActions, EmptyState, ICON_STROKE, MenuItem, Select, SettingsCard, SettingsRow, useToast } from "../../ui";

export const POLICY_LABEL: Record<VaultPolicy, MessageKey> = {
  always: "connections.vault.policy.always",
  while_unlocked: "connections.vault.policy.while_unlocked",
  never: "connections.vault.policy.never",
};

const BACKEND_LABEL: Record<VaultBackend, MessageKey> = {
  passphrase: "connections.vault.backend.passphrase",
  secret_service: "connections.vault.backend.secret_service",
  native: "connections.vault.backend.native",
};

const STATE_LABEL: Record<VaultStatus["state"], MessageKey> = {
  uninitialized: "connections.vault.state.uninitialized",
  locked: "connections.vault.state.locked",
  unlocked: "connections.vault.state.unlocked",
  unavailable: "connections.vault.state.unavailable",
};

export const SECRET_INPUT = { autoComplete: "off", autoCapitalize: "none", autoCorrect: "off", spellCheck: false } as const;

const ICON = { size: 16, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;

/** A keyring vault with biometric unlock on opens only after this computer's fingerprint check. */
export function needsBiometric(status: VaultStatus, settings: VaultSettings | null): boolean {
  return status.backend !== "passphrase" && status.backend != null && (status.reason === "biometric_required" || settings?.biometric_unlock === true);
}

function PassphraseForm({ label, confirm, onSubmit }: { label: string; confirm: boolean; onSubmit: (passphrase: string) => Promise<void> }) {
  const [passphrase, setPassphrase] = useState("");
  const [again, setAgain] = useState("");
  const [busy, setBusy] = useState(false);
  const mismatch = confirm && again.length > 0 && again !== passphrase;

  async function submit(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    try {
      await onSubmit(passphrase);
    } finally {
      setPassphrase("");
      setAgain("");
      setBusy(false);
    }
  }

  return (
    <form className="dialog-form" onSubmit={submit} aria-label={label}>
      <label className="field">
        <span>{t(confirm ? "connections.vault.newPassphrase" : "connections.vault.passphrase")}</span>
        <input type="password" value={passphrase} onChange={(e) => setPassphrase(e.target.value)} {...SECRET_INPUT} required />
      </label>
      {confirm ? (
        <label className="field">
          <span>{t("connections.vault.repeatPassphrase")}</span>
          <input type="password" value={again} onChange={(e) => setAgain(e.target.value)} {...SECRET_INPUT} required />
        </label>
      ) : null}
      {mismatch ? <ErrorNote>{t("connections.vault.mismatch")}</ErrorNote> : null}
      <CardActions>
        <Button type="submit" variant="primary" disabled={busy || !passphrase || (confirm && (passphrase.length < 8 || again !== passphrase))}>
          {label}
        </Button>
      </CardActions>
    </form>
  );
}

/** Opens a locked vault with its recovery key; a passphrase vault gets a new passphrase on the way. */
function RecoverForm({ passphrase, onDone }: { passphrase: boolean; onDone: () => void }) {
  const client = useClient();
  const toast = useToast();
  const [key, setKey] = useState("");
  const [next, setNext] = useState("");
  const [again, setAgain] = useState("");
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<Failure | null>(null);
  const ready = key.trim() && (!passphrase || (next.length >= 8 && next === again));

  async function submit(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setFailure(null);
    try {
      await client.recoverVault({ recovery_key: key, passphrase: passphrase ? next : null });
      toast.notify(t("passwords.recover.done"), "success");
      onDone();
    } catch (cause) {
      setFailure(toFailure(cause));
    } finally {
      // The key and passphrase leave page state either way; the person retypes them.
      setKey("");
      setNext("");
      setAgain("");
      setBusy(false);
    }
  }

  return (
    <form className="dialog-form" onSubmit={submit} aria-label={t("passwords.recover.title")}>
      <p className="settings-text">{t(passphrase ? "passwords.recover.passphraseNote" : "passwords.recover.keyringNote")}</p>
      <label className="field">
        <span>{t("passwords.recover.key")}</span>
        <input value={key} onChange={(e) => setKey(e.target.value)} {...SECRET_INPUT} required />
      </label>
      {passphrase ? (
        <>
          <label className="field">
            <span>{t("connections.vault.newPassphrase")}</span>
            <input type="password" value={next} onChange={(e) => setNext(e.target.value)} {...SECRET_INPUT} required />
          </label>
          <label className="field">
            <span>{t("connections.vault.repeatPassphrase")}</span>
            <input type="password" value={again} onChange={(e) => setAgain(e.target.value)} {...SECRET_INPUT} required />
          </label>
          {again && again !== next ? <ErrorNote>{t("connections.vault.mismatch")}</ErrorNote> : null}
        </>
      ) : null}
      <FailureNote failure={failure} what={t("passwords.recover.what")} onRetry={() => setFailure(null)} />
      <DialogActions>
        <Button type="submit" variant="primary" disabled={busy || !ready}>
          {t("passwords.recover.action")}
        </Button>
      </DialogActions>
    </form>
  );
}

type StatusDialog = "setup" | "unlock" | "recover" | null;

/**
 * The vault's state as a settings row: set up, unlock or lock it, or open it with the recovery
 * key. Passphrases only travel to the daemon and are cleared after each try. The recovery key is
 * the vault's master secret, so on another device the recover step explains instead of asking for it.
 */
export function VaultStatusCard({ status, settings, remote, onChanged }: { status: VaultStatus; settings: VaultSettings | null; remote: boolean; onChanged: () => void }) {
  const client = useClient();
  const [dialog, setDialog] = useState<StatusDialog>(null);
  const [failure, setFailure] = useState<Failure | null>(null);
  const passphrase = status.backend === "passphrase";
  const biometric = needsBiometric(status, settings);

  async function act(call: () => Promise<VaultStatus>) {
    try {
      await call();
      setFailure(null);
      setDialog(null);
      onChanged();
    } catch (cause) {
      setFailure(toFailure(cause));
    }
  }

  const backend = status.backend ? t(BACKEND_LABEL[status.backend]) : t("connections.vault.noBackend");
  // Unknown reason codes from a newer daemon are shown as sent.
  const reason = status.reason ? tMaybe(`connections.vault.reason.${status.reason}`, undefined, status.reason) : null;
  const description =
    reason ??
    (status.state === "uninitialized"
      ? t(passphrase ? "connections.vault.passphraseNote" : "connections.vault.keyringNote")
      : t("connections.vault.unlocksWith", { backend }));

  const control =
    status.state === "unlocked" ? (
      <Button onClick={() => void act(() => client.lockVault())}>{t("connections.vault.lock")}</Button>
    ) : status.state === "locked" ? (
      <span className="personal-actions">
        {settings?.recovery.configured ? <Button onClick={() => setDialog("recover")}>{t("passwords.recover.open")}</Button> : null}
        <Button variant="primary" onClick={() => (passphrase ? setDialog("unlock") : void act(() => client.unlockVault(undefined, biometric)))}>
          {t("connections.vault.unlock")}
        </Button>
      </span>
    ) : status.state === "uninitialized" && status.backend ? (
      <Button variant="primary" onClick={() => (passphrase ? setDialog("setup") : void act(() => client.setupVault(undefined, biometric)))}>
        {t("passwords.vault.setUp")}
      </Button>
    ) : null;

  return (
    <div className="passwords-vault" data-vault-state={status.state}>
      <SettingsCard>
        <SettingsRow
          leading={<LockKeyhole {...ICON} />}
          title={t("connections.vault.title")}
          badge={<Badge>{t(STATE_LABEL[status.state])}</Badge>}
          description={description}
          control={control ?? undefined}
        />
      </SettingsCard>
      {status.state === "unavailable" ? <CliHint command="openside doctor">{t("connections.vault.cliHint")}</CliHint> : null}
      {dialog === null ? <FailureNote failure={failure} what={t("passwords.vault.what")} onRetry={() => setFailure(null)} /> : null}
      <Dialog open={dialog === "setup" || dialog === "unlock"} onClose={() => setDialog(null)} title={t(dialog === "setup" ? "connections.vault.setUp" : "connections.vault.unlock")}>
        {dialog === "setup" ? <p className="settings-text">{t("connections.vault.passphraseNote")}</p> : null}
        <PassphraseForm
          key={dialog ?? "none"}
          label={t(dialog === "setup" ? "connections.vault.setUp" : "connections.vault.unlock")}
          confirm={dialog === "setup"}
          onSubmit={(value) => act(() => (dialog === "setup" ? client.setupVault(value) : client.unlockVault(value)))}
        />
        {failure ? <FailureNote failure={failure} what={t("passwords.vault.what")} onRetry={() => setFailure(null)} /> : null}
      </Dialog>
      <Dialog open={dialog === "recover"} onClose={() => setDialog(null)} title={t("passwords.recover.title")}>
        {dialog !== "recover" ? null : remote ? (
          <LocalOnlyNotice what={t("passwords.recover.what")} onChanged={onChanged} />
        ) : (
          <RecoverForm
            passphrase={passphrase}
            onDone={() => {
              setDialog(null);
              onChanged();
            }}
          />
        )}
      </Dialog>
    </div>
  );
}

type ItemKind = "login" | "authenticator";

function originsFrom(text: string): string[] {
  return text
    .split(/[\s,]+/)
    .map((origin) => origin.trim())
    .filter(Boolean);
}

/** `otp-<host>` for an authenticator whose id the person has not typed, as the daemon names them. */
function suggestedId(kind: ItemKind, origins: string): string {
  try {
    const host = new URL(originsFrom(origins)[0] ?? "").hostname.replace(/^\*\./, "");
    const slug = host.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
    return slug ? (kind === "authenticator" ? `otp-${slug}` : slug).slice(0, 64) : "";
  } catch {
    return "";
  }
}

/** Values are typed here and sent once to the daemon; the response carries only metadata. */
function ItemForm({ kind, onSaved, onCancel }: { kind: ItemKind; onSaved: (id: string) => void; onCancel: () => void }) {
  const client = useClient();
  const [id, setId] = useState("");
  const [origins, setOrigins] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [totp, setTotp] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const itemId = id.trim() || suggestedId(kind, origins);

  async function save(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const item = await client.createVaultItem({
        id: itemId,
        origins: originsFrom(origins),
        username: kind === "login" ? username : null,
        password: kind === "login" ? password : null,
        totp: kind === "authenticator" ? totp : null,
        policy: null,
      });
      onSaved(item.id);
    } catch (cause) {
      // A failed save may still have stored the item (e.g. its audit record failed), and a
      // secret must not linger in page state either way; the person retypes it.
      setPassword("");
      setTotp("");
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  const ready = itemId && originsFrom(origins).length > 0 && (kind === "login" ? password : totp.trim());
  return (
    <form className="dialog-form" onSubmit={save} aria-label={t(kind === "login" ? "connections.vault.form.newLogin" : "connections.vault.form.newAuthenticator")}>
      <label className="field">
        <span>{t("connections.vault.form.website")}</span>
        <input inputMode="url" placeholder="https://example.com" value={origins} onChange={(e) => setOrigins(e.target.value)} autoCapitalize="none" required />
      </label>
      <label className="field">
        <span>{t("connections.vault.form.itemId")}</span>
        <input value={id} placeholder={suggestedId(kind, origins) || "example"} onChange={(e) => setId(e.target.value)} autoCapitalize="none" spellCheck={false} />
      </label>
      {kind === "login" ? (
        <>
          <label className="field">
            <span>{t("connections.vault.form.username")}</span>
            <input value={username} onChange={(e) => setUsername(e.target.value)} {...SECRET_INPUT} />
          </label>
          <label className="field">
            <span>{t("connections.vault.form.password")}</span>
            <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} {...SECRET_INPUT} required />
          </label>
        </>
      ) : (
        <label className="field">
          <span>{t("connections.vault.form.setupKey")}</span>
          <input type="password" value={totp} onChange={(e) => setTotp(e.target.value)} {...SECRET_INPUT} required />
        </label>
      )}
      {error ? <ErrorNote>{error}</ErrorNote> : null}
      <DialogActions>
        <Button onClick={onCancel}>{t("common.cancel")}</Button>
        <Button type="submit" variant="primary" disabled={busy || !ready}>
          {t("common.add")}
        </Button>
      </DialogActions>
    </form>
  );
}

/**
 * Vaults ›: the one vault's state and its items (value-free metadata), searchable, with adding,
 * per-item use policy and deleting. The audit log stays in the CLI (`openside vault audit`), as
 * Aside's Passwords page has none.
 */
export function VaultsDialog({
  open,
  onClose,
  status,
  settings,
  remote,
  items,
  onChanged,
}: {
  open: boolean;
  onClose: () => void;
  status: VaultStatus;
  settings: VaultSettings | null;
  remote: boolean;
  items: Resource<VaultItemList>;
  onChanged: () => void;
}) {
  const client = useClient();
  const toast = useToast();
  const [query, setQuery] = useState("");
  const [form, setForm] = useState<ItemKind | null>(null);
  const [confirm, ask] = useConfirm();

  async function setPolicy(id: string, policy: VaultPolicyOverride) {
    try {
      await client.updateVaultItem(id, policy);
      await items.reload();
    } catch (cause) {
      items.setError(errorMessage(cause));
    }
  }

  // Keyring vaults are created by the first item, unless biometric unlock wants a person first.
  const canAdd =
    status.state === "unlocked" || (status.state === "uninitialized" && status.backend != null && status.backend !== "passphrase" && !needsBiometric(status, settings));
  const needle = query.trim().toLowerCase();

  return (
    <Dialog open={open} onClose={onClose} size="wide" title={t("passwords.vaults.title")}>
      <div className="passwords-vaults" data-section="vault">
        <VaultStatusCard status={status} settings={settings} remote={remote} onChanged={onChanged} />
        <div className="passwords-toolbar">
          <label className="passwords-search">
            <Search {...ICON} />
            <input type="search" value={query} placeholder={t("passwords.vaults.search")} aria-label={t("passwords.vaults.search")} onChange={(e) => setQuery(e.target.value)} />
          </label>
          {canAdd ? (
            <span className="personal-actions">
              <Button onClick={() => setForm("login")}>
                <Plus {...ICON} />
                {t("connections.vault.items.addLogin")}
              </Button>
              <Button onClick={() => setForm("authenticator")}>
                <Plus {...ICON} />
                {t("connections.vault.items.addAuthenticator")}
              </Button>
            </span>
          ) : null}
        </div>
        <ResourceView resource={items} what={t("connections.vault.what")}>
          {(data) => {
            if (!data.available) return <p className="settings-text">{t("connections.vault.items.notAvailable")}</p>;
            if (data.items.length === 0)
              return <EmptyState icon={<KeyRound {...ICON} />} title={t("connections.vault.items.emptyTitle")} description={t("connections.vault.items.emptyDetail")} />;
            const shownItems = needle ? data.items.filter((item) => [item.id, ...item.origins].some((text) => text.toLowerCase().includes(needle))) : data.items;
            if (shownItems.length === 0) return <p className="settings-text">{t("passwords.vaults.noMatch", { query: query.trim() })}</p>;
            return (
              <ul className="resource-list" aria-label={t("connections.vault.items.label")}>
                {shownItems.map((item) => {
                  const shown = item.policy ?? data.default_policy;
                  return (
                    <li key={item.id}>
                      <ResourceCard
                        data-item={item.id}
                        icon={KeyRound}
                        title={item.id}
                        detail={item.origins.join(", ")}
                        trailing={
                          <>
                            <Select
                              variant="inline"
                              className="inline-select"
                              aria-label={t("connections.vault.items.usePolicy", { id: item.id })}
                              title={t("connections.vault.items.policyTitle")}
                              value={item.policy ?? "default"}
                              onChange={(value) => void setPolicy(item.id, value as VaultPolicyOverride)}
                              options={[
                                { value: "default", label: t("connections.vault.items.defaultPolicy", { policy: t(POLICY_LABEL[data.default_policy]).toLowerCase() }) },
                                ...(Object.keys(POLICY_LABEL) as VaultPolicy[]).map((policy) => ({ value: policy, label: t(POLICY_LABEL[policy]) })),
                              ]}
                            />
                            <RowMenu name={item.id}>
                              <MenuItem
                                danger
                                icon={<Trash2 size={18} strokeWidth={ICON_STROKE} />}
                                onClick={() =>
                                  ask({
                                    title: t("connections.vault.items.deleteTitle", { id: item.id }),
                                    description: t("connections.vault.items.deleteDescription"),
                                    confirmLabel: t("common.delete"),
                                    onConfirm: () =>
                                      void client.deleteVaultItem(item.id).then(
                                        () => {
                                          toast.notify(t("connections.vault.items.deleted", { id: item.id }));
                                          void items.reload();
                                        },
                                        (cause: unknown) => items.setError(errorMessage(cause)),
                                      ),
                                  })
                                }
                              >
                                {t("common.delete")}
                              </MenuItem>
                            </RowMenu>
                          </>
                        }
                      >
                        {item.has_totp || item.passkey_rp_ids.length || item.effective_policy !== shown ? (
                          <div className="resource-card-meta">
                            {item.has_totp ? <span className="tag">{t("connections.vault.items.oneTimeCodes")}</span> : null}
                            {item.passkey_rp_ids.length ? <span className="tag">{t("connections.vault.items.passkeys", { ids: item.passkey_rp_ids.join(", ") })}</span> : null}
                            {item.effective_policy !== shown ? (
                              <span className="tag">{t("connections.vault.items.enforced", { policy: t(POLICY_LABEL[item.effective_policy]).toLowerCase() })}</span>
                            ) : null}
                          </div>
                        ) : null}
                      </ResourceCard>
                    </li>
                  );
                })}
              </ul>
            );
          }}
        </ResourceView>
        <p className="passwords-hint">{t("connections.vault.items.description")}</p>
      </div>
      <Dialog open={form !== null} onClose={() => setForm(null)} title={t(form === "authenticator" ? "connections.vault.form.newAuthenticator" : "connections.vault.form.newLogin")}>
        {form && (
          <ItemForm
            key={form}
            kind={form}
            onCancel={() => setForm(null)}
            onSaved={(id) => {
              setForm(null);
              toast.notify(t("connections.vault.items.added", { id }));
              onChanged();
            }}
          />
        )}
      </Dialog>
      {confirm}
    </Dialog>
  );
}
