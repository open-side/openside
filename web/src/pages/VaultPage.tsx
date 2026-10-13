import { KeyRound, LockKeyhole, LockKeyholeOpen, Trash2 } from "lucide-react";
import { useCallback, useState, type FormEvent } from "react";
import { Link, useLocation } from "react-router";
import type { VaultAuditQuery } from "../api/client";
import type { VaultBackend, VaultPolicy, VaultPolicyOverride, VaultStatus } from "../api/types";
import { useClient } from "../context";
import { errorMessage, useResource } from "../hooks/useResource";
import { Button, Dialog, DialogActions, ICON_STROKE, MenuItem, Select, useToast } from "../ui";
import {
  AddButton,
  CardActions,
  EmptyState,
  ErrorNote,
  ResourceCard,
  ResourceView,
  RowMenu,
  SectionList,
  useConfirm,
  useDialogState,
  type BadgeTone,
} from "./blocks";
import { CliHint } from "./CliHint";
import { manageLinkState } from "./manageRoutes";
import { ScopeNotice } from "./ScopeNotice";
import { formatDateTime } from "./format";

const POLICY_LABEL: Record<VaultPolicy, string> = {
  always: "Always",
  while_unlocked: "While unlocked",
  never: "Never",
};

const BACKEND_LABEL: Record<VaultBackend, string> = {
  passphrase: "Passphrase",
  secret_service: "Linux keyring (Secret Service)",
  native: "System keychain",
};

const REASON: Record<string, string> = {
  keyring_unreachable: "The computer's keyring service is not running.",
  keyring_locked: "The computer's keyring is locked. Unlock it on the computer, then unlock the vault here.",
  keyring_key_mismatch: "The key in the computer's keyring does not open this vault file.",
  passphrase_cmd_failed: "The command that supplies the vault passphrase failed when openside started. Unlock with the passphrase.",
  unsupported_platform: "This computer has no keyring openside can use. Choose the passphrase unlock method in Configuration.",
  format_mismatch: "The vault file was made with a different unlock method than the one chosen in Configuration.",
  unreadable: "The vault file cannot be read.",
  probe_timeout: "The computer's keyring did not answer in time.",
};

const STATE: Record<VaultStatus["state"], { label: string; tone: BadgeTone }> = {
  uninitialized: { label: "Not set up", tone: "neutral" },
  locked: { label: "Locked", tone: "neutral" },
  unlocked: { label: "Unlocked", tone: "success" },
  unavailable: { label: "Unavailable", tone: "attention" },
};

const SECRET_INPUT = { autoComplete: "off", autoCapitalize: "none", autoCorrect: "off", spellCheck: false } as const;

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
    <form className="vault-form" onSubmit={submit} aria-label={label}>
      <label className="field">
        <span>{confirm ? "New passphrase (8+ characters)" : "Passphrase"}</span>
        <input type="password" value={passphrase} onChange={(e) => setPassphrase(e.target.value)} {...SECRET_INPUT} required />
      </label>
      {confirm ? (
        <label className="field">
          <span>Repeat passphrase</span>
          <input type="password" value={again} onChange={(e) => setAgain(e.target.value)} {...SECRET_INPUT} required />
        </label>
      ) : null}
      {mismatch ? <ErrorNote>The passphrases do not match.</ErrorNote> : null}
      <CardActions>
        <Button type="submit" variant="primary" disabled={busy || !passphrase || (confirm && (passphrase.length < 8 || again !== passphrase))}>
          {label}
        </Button>
      </CardActions>
    </form>
  );
}

/** Setup, unlock and lock. Passphrases only travel to the daemon and are cleared after each try. */
function Status({ status, onChanged }: { status: VaultStatus; onChanged: () => void }) {
  const client = useClient();
  const location = useLocation();
  const [error, setError] = useState<string | null>(null);
  const passphrase = status.backend === "passphrase";

  async function act(call: () => Promise<VaultStatus>) {
    try {
      await call();
      setError(null);
      onChanged();
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }

  const backend = status.backend ? BACKEND_LABEL[status.backend] : "No unlock method available";
  const reason = status.reason ? (REASON[status.reason] ?? status.reason) : null;
  const trailing =
    status.state === "unlocked" ? (
      <Button size="compact" onClick={() => void act(() => client.lockVault())}>
        Lock
      </Button>
    ) : status.state === "locked" && !passphrase ? (
      <Button size="compact" variant="primary" onClick={() => void act(() => client.unlockVault())}>
        Unlock
      </Button>
    ) : status.state === "uninitialized" && !passphrase && status.backend ? (
      <Button size="compact" variant="primary" onClick={() => void act(() => client.setupVault())}>
        Set up vault
      </Button>
    ) : undefined;

  return (
    <ResourceCard
      data-state={status.state}
      icon={status.state === "unlocked" ? LockKeyholeOpen : LockKeyhole}
      title="Vault"
      detail={`Unlocks with ${backend}`}
      badge={STATE[status.state].label}
      badgeTone={STATE[status.state].tone}
      trailing={trailing}
    >
      {reason ? <p className="resource-card-body">{reason}</p> : null}
      {status.state === "uninitialized" && passphrase ? (
        <>
          <p className="resource-card-body">
            No keyring is available, so the vault key is derived from a passphrase. Nobody can recover a forgotten passphrase.
          </p>
          <PassphraseForm label="Set up vault" confirm onSubmit={(value) => act(() => client.setupVault(value))} />
        </>
      ) : null}
      {status.state === "uninitialized" && !passphrase && status.backend ? (
        <p className="resource-card-body">The vault key is kept in the computer's keyring. Adding the first item also sets it up.</p>
      ) : null}
      {status.state === "locked" && passphrase ? (
        <PassphraseForm label="Unlock" confirm={false} onSubmit={(value) => act(() => client.unlockVault(value))} />
      ) : null}
      {status.state === "unavailable" ? (
        <>
          <div className="resource-card-body">
            <Link className="ui-button ui-button--compact" to="/config" state={manageLinkState(location)}>
              Open Configuration
            </Link>
          </div>
          <CliHint command="openside doctor">To check this computer from a terminal:</CliHint>
        </>
      ) : null}
      {error ? <ErrorNote>{error}</ErrorNote> : null}
    </ResourceCard>
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
    <form className="dialog-form" onSubmit={save} aria-label={kind === "login" ? "New login" : "New authenticator"}>
      <label className="field">
        <span>Website (space-separate several)</span>
        <input
          inputMode="url"
          placeholder="https://example.com"
          value={origins}
          onChange={(e) => setOrigins(e.target.value)}
          autoCapitalize="none"
          required
        />
      </label>
      <label className="field">
        <span>Item id</span>
        <input
          value={id}
          placeholder={suggestedId(kind, origins) || "example"}
          onChange={(e) => setId(e.target.value)}
          autoCapitalize="none"
          spellCheck={false}
        />
      </label>
      {kind === "login" ? (
        <>
          <label className="field">
            <span>Username</span>
            <input value={username} onChange={(e) => setUsername(e.target.value)} {...SECRET_INPUT} />
          </label>
          <label className="field">
            <span>Password</span>
            <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} {...SECRET_INPUT} required />
          </label>
        </>
      ) : (
        <label className="field">
          <span>Setup key or otpauth:// link</span>
          <input type="password" value={totp} onChange={(e) => setTotp(e.target.value)} {...SECRET_INPUT} required />
        </label>
      )}
      {error ? <ErrorNote>{error}</ErrorNote> : null}
      <DialogActions>
        <Button onClick={onCancel}>Cancel</Button>
        <Button type="submit" variant="primary" disabled={busy || !ready}>
          Add
        </Button>
      </DialogActions>
    </form>
  );
}

function Items({ status, onChanged }: { status: VaultStatus; onChanged: () => void }) {
  const client = useClient();
  const toast = useToast();
  const load = useCallback(() => client.vaultItems(), [client]);
  const items = useResource(load);
  const form = useDialogState<ItemKind>();
  const [confirm, ask] = useConfirm();

  async function setPolicy(id: string, policy: VaultPolicyOverride) {
    try {
      await client.updateVaultItem(id, policy);
      await items.reload();
    } catch (cause) {
      items.setError(errorMessage(cause));
    }
  }

  // Keyring vaults are created by the first item, so adding is offered before setup too.
  const canAdd = status.state === "unlocked" || (status.state === "uninitialized" && status.backend != null && status.backend !== "passphrase");

  return (
    <SectionList
      title="Items"
      label="Vault items"
      description="Only metadata is shown. Usernames, passwords, one-time codes and passkeys never leave the daemon."
      actions={
        canAdd ? (
          <>
            <AddButton onClick={() => form.show("login")}>Login</AddButton>
            <AddButton variant="secondary" onClick={() => form.show("authenticator")}>
              Authenticator
            </AddButton>
          </>
        ) : undefined
      }
    >
      <ResourceView resource={items} what="the vault">
        {(data) =>
          !data.available ? (
            <p className="settings-text">Items appear here once the vault is set up and unlocked.</p>
          ) : data.items.length === 0 ? (
            <EmptyState icon={KeyRound} title="The vault has no items" detail="Add a login or an authenticator for a site the agent signs in to." />
          ) : (
            <ul className="resource-list" aria-label="Vault items">
              {data.items.map((item) => {
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
                            aria-label={`Use policy for ${item.id}`}
                            title="When the agent may use this item"
                            value={item.policy ?? "default"}
                            onChange={(value) => void setPolicy(item.id, value as VaultPolicyOverride)}
                            options={[
                              { value: "default", label: `Default (${POLICY_LABEL[data.default_policy].toLowerCase()})` },
                              ...(Object.keys(POLICY_LABEL) as VaultPolicy[]).map((policy) => ({ value: policy, label: POLICY_LABEL[policy] })),
                            ]}
                          />
                          <RowMenu name={item.id}>
                            <MenuItem
                              danger
                              icon={<Trash2 size={18} strokeWidth={ICON_STROKE} />}
                              onClick={() =>
                                ask({
                                  title: `Delete ${item.id}?`,
                                  description: "Its login, one-time codes and passkeys are removed from the vault. The audit log keeps the deletion.",
                                  confirmLabel: "Delete",
                                  onConfirm: () =>
                                    void client.deleteVaultItem(item.id).then(
                                      () => {
                                        toast.notify(`Deleted ${item.id}.`);
                                        void items.reload();
                                      },
                                      (cause: unknown) => items.setError(errorMessage(cause)),
                                    ),
                                })
                              }
                            >
                              Delete
                            </MenuItem>
                          </RowMenu>
                        </>
                      }
                    >
                      {item.has_totp || item.passkey_rp_ids.length || item.effective_policy !== shown ? (
                        <div className="resource-card-meta">
                          {item.has_totp ? <span className="tag">one-time codes</span> : null}
                          {item.passkey_rp_ids.length ? <span className="tag">passkeys: {item.passkey_rp_ids.join(", ")}</span> : null}
                          {item.effective_policy !== shown ? <span className="tag">enforced: {POLICY_LABEL[item.effective_policy].toLowerCase()}</span> : null}
                        </div>
                      ) : null}
                    </ResourceCard>
                  </li>
                );
              })}
            </ul>
          )
        }
      </ResourceView>
      <Dialog open={form.open} onClose={form.close} title={form.value === "authenticator" ? "New authenticator" : "New login"}>
        {form.value && (
          <ItemForm
            key={form.value}
            kind={form.value}
            onCancel={form.close}
            onSaved={(id) => {
              form.close();
              toast.notify(`Added ${id}.`);
              onChanged();
            }}
          />
        )}
      </Dialog>
      {confirm}
    </SectionList>
  );
}

const WHAT: Record<string, string> = {
  fill: "Fill",
  otp: "One-time code",
  otp_register: "Save authenticator",
  passkey_register: "Save passkey",
  passkey_login: "Passkey sign-in",
  get: "Read",
  policy_update: "Policy change",
  item_add: "Item added",
  item_delete: "Item deleted",
  setup: "Set up",
  unlock: "Unlock",
  lock: "Lock",
};

function Audit() {
  const client = useClient();
  const [item, setItem] = useState("");
  const [session, setSession] = useState("");
  const [since, setSince] = useState("");
  const [query, setQuery] = useState<VaultAuditQuery>({});
  const load = useCallback(() => client.vaultAudit(query), [client, query]);
  const audit = useResource(load);

  function apply(event: FormEvent) {
    event.preventDefault();
    setQuery({
      item: item || undefined,
      session: session || undefined,
      since: since ? new Date(since).toISOString() : undefined,
    });
  }

  return (
    <SectionList title="Audit log" label="Vault audit log" description="Every use and change of the vault, newest first.">
      <form className="audit-filters" onSubmit={apply} aria-label="Audit filters">
        <label className="field">
          <span>Item</span>
          <input value={item} onChange={(e) => setItem(e.target.value)} />
        </label>
        <label className="field">
          <span>Session</span>
          <input value={session} onChange={(e) => setSession(e.target.value)} />
        </label>
        <label className="field">
          <span>Since</span>
          <input type="datetime-local" value={since} onChange={(e) => setSince(e.target.value)} />
        </label>
        <Button type="submit">Filter</Button>
      </form>
      <ResourceView resource={audit} what="the vault">
        {(data) =>
          data.records.length === 0 ? (
            <p className="settings-text">No audit records match.</p>
          ) : (
            <ol className="resource-list audit-list" aria-label="Audit records">
              {[...data.records].reverse().map((record, index) => (
                <li key={`${record.at}-${index}`} data-result={record.result} data-item={record.item}>
                  <ResourceCard
                    title={`${WHAT[record.what] ?? record.what}${record.item ? ` · ${record.item}` : ""}`}
                    detail={`${formatDateTime(record.at)} · ${record.who}${record.origin && record.origin !== "-" ? ` · ${record.origin}` : ""}`}
                    badge={record.result}
                    badgeTone={["denied", "failed", "fill-failed"].includes(record.result) ? "danger" : "neutral"}
                  >
                    {record.detail || record.session_id ? (
                      <p className="resource-card-detail">
                        {record.detail ?? ""}
                        {record.detail && record.session_id ? " · " : ""}
                        {record.session_id ? `session ${record.session_id}${record.tool_call_id ? ` / ${record.tool_call_id}` : ""}` : ""}
                      </p>
                    ) : null}
                  </ResourceCard>
                </li>
              ))}
            </ol>
          )
        }
      </ResourceView>
    </SectionList>
  );
}

/**
 * Vault: setup, unlock and lock; adding logins and authenticators; value-free item metadata,
 * per-item use policy and the audit log (newest first). Every route needs the admin scope.
 */
export function VaultPage() {
  const client = useClient();
  // Items and audit remount after setup, unlock, lock or an added item so both refetch.
  const [generation, setGeneration] = useState(0);
  const loadStatus = useCallback(() => client.vaultStatus(), [client]);
  const status = useResource(loadStatus);
  const { reload } = status;
  const changed = useCallback(() => {
    void reload();
    setGeneration((value) => value + 1);
  }, [reload]);

  if (status.forbidden) {
    return (
      <div className="manage-body" data-section="vault">
        <ScopeNotice what="the vault" onGranted={changed} />
      </div>
    );
  }

  return (
    <div className="manage-body" data-section="vault">
      <SectionList title="Status" label="Vault status">
        <ResourceView resource={status} what="the vault">
          {(data) => <Status status={data} onChanged={changed} />}
        </ResourceView>
      </SectionList>
      {status.data ? <Items key={`items-${generation}`} status={status.data} onChanged={changed} /> : null}
      {status.data ? <Audit key={`audit-${generation}`} /> : null}
    </div>
  );
}
