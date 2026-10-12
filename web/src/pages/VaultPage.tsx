import { KeyRound } from "lucide-react";
import { useCallback, useState, type FormEvent } from "react";
import { ApiError, type VaultAuditQuery } from "../api/client";
import type { VaultBackend, VaultPolicyOverride, VaultStatus } from "../api/types";
import { useClient } from "../context";
import { errorMessage, useResource } from "../hooks/useResource";
import { Button, Dialog, DialogActions, useToast } from "../ui";
import {
  AddButton,
  CardActions,
  EmptyState,
  ErrorNote,
  Loading,
  ResourceCard,
  SectionList,
  useConfirm,
  useDialogState,
} from "./blocks";

const POLICY_LABEL: Record<VaultPolicyOverride, string> = {
  default: "Vault default",
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
  passphrase_cmd_failed: "vault.passphrase_cmd failed when the daemon started. Unlock with the passphrase.",
  unsupported_platform: "This computer has no keyring openside can use. Set vault.unlock = \"passphrase\".",
  format_mismatch: "The vault file was made with a different unlock method than vault.unlock.",
  unreadable: "The vault file cannot be read.",
  probe_timeout: "The computer's keyring did not answer in time.",
};

/** A 403 here means the paired device lacks the admin scope, which every vault route needs. */
function adminScope(cause: unknown): unknown {
  return cause instanceof ApiError && cause.status === 403 && cause.code === "forbidden"
    ? new ApiError(
        403,
        "admin_scope_required",
        "This device needs the admin scope to manage the vault. On the computer, pair it again with openside pair --scope admin.",
      )
    : cause;
}

function vaultError(cause: unknown): string {
  return errorMessage(adminScope(cause));
}

/** Rethrows with the admin-scope explanation so `useResource` shows it. */
function explained<T>(request: Promise<T>): Promise<T> {
  return request.catch((cause: unknown) => {
    throw adminScope(cause);
  });
}

const STATE_LABEL: Record<VaultStatus["state"], string> = {
  uninitialized: "not set up",
  locked: "locked",
  unlocked: "unlocked",
  unavailable: "unavailable",
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
    <form className="dialog-form vault-passphrase" onSubmit={submit} aria-label={label}>
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
  const [error, setError] = useState<string | null>(null);
  const passphrase = status.backend === "passphrase";

  async function act(call: () => Promise<VaultStatus>) {
    try {
      await call();
      setError(null);
      onChanged();
    } catch (cause) {
      setError(vaultError(cause));
    }
  }

  const backend = status.backend ? BACKEND_LABEL[status.backend] : "None available";
  const reason = status.reason ? (REASON[status.reason] ?? status.reason) : null;

  return (
    <ResourceCard
      data-state={status.state}
      title="Vault"
      detail={`Unlocks with: ${backend}`}
      badge={STATE_LABEL[status.state]}
      badgeTone={status.state === "unlocked" ? "accent" : status.state === "unavailable" ? "attention" : "muted"}
    >
      {reason ? <p className="resource-card-detail">{reason}</p> : null}
      {status.state === "uninitialized" && passphrase ? (
        <>
          <p className="resource-card-detail">
            No keyring is available, so the vault key is derived from a passphrase. Nobody can recover a forgotten passphrase.
          </p>
          <PassphraseForm label="Set up vault" confirm onSubmit={(value) => act(() => client.setupVault(value))} />
        </>
      ) : null}
      {status.state === "uninitialized" && !passphrase && status.backend ? (
        <>
          <p className="resource-card-detail">The vault key is kept in the computer's keyring. Adding the first item also sets it up.</p>
          <CardActions>
            <Button variant="primary" onClick={() => void act(() => client.setupVault())}>
              Set up vault
            </Button>
          </CardActions>
        </>
      ) : null}
      {status.state === "locked" && passphrase ? (
        <PassphraseForm label="Unlock" confirm={false} onSubmit={(value) => act(() => client.unlockVault(value))} />
      ) : null}
      {status.state === "locked" && !passphrase ? (
        <CardActions>
          <Button variant="primary" onClick={() => void act(() => client.unlockVault())}>
            Unlock
          </Button>
        </CardActions>
      ) : null}
      {status.state === "unlocked" ? (
        <CardActions>
          <Button onClick={() => void act(() => client.lockVault())}>Lock</Button>
        </CardActions>
      ) : null}
      {status.state === "unavailable" ? (
        <p className="resource-card-detail">Run openside doctor on the computer for the next step.</p>
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
      setError(vaultError(cause));
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

function Items({ status, onChanged }: { status: VaultStatus | null; onChanged: () => void }) {
  const client = useClient();
  const toast = useToast();
  const load = useCallback(() => explained(client.vaultItems()), [client]);
  const items = useResource(load);
  const form = useDialogState<ItemKind>();
  const [confirm, ask] = useConfirm();

  async function setPolicy(id: string, policy: VaultPolicyOverride) {
    try {
      await client.updateVaultItem(id, policy);
      await items.reload();
    } catch (cause) {
      items.setError(vaultError(cause));
    }
  }

  // Keyring vaults are created by the first item, so adding is offered before setup too.
  const canAdd =
    status?.state === "unlocked" || (status?.state === "uninitialized" && status.backend != null && status.backend !== "passphrase");

  return (
    <SectionList
      title="Items"
      label="Vault items"
      description="Only metadata is shown. Usernames, passwords, one-time codes and passkeys never leave the daemon."
      actions={
        canAdd ? (
          <>
            <AddButton onClick={() => form.show("login")}>Login</AddButton>
            <AddButton onClick={() => form.show("authenticator")}>Authenticator</AddButton>
          </>
        ) : undefined
      }
    >
      {items.error ? <ErrorNote>{items.error}</ErrorNote> : null}
      {!items.data ? (
        items.error ? null : <Loading />
      ) : !items.data.available ? (
        <EmptyState icon={KeyRound} title="Vault locked" detail="No unlocked vault on this daemon." />
      ) : items.data.items.length === 0 ? (
        <EmptyState icon={KeyRound} title="The vault has no items" />
      ) : (
        <ul className="resource-list" aria-label="Vault items">
          {items.data.items.map((item) => (
            <li key={item.id}>
              <ResourceCard data-item={item.id} title={item.id} detail={item.origins.join(", ")}>
                {item.has_totp || item.passkey_rp_ids.length ? (
                  <div className="resource-card-meta">
                    {item.has_totp ? <span className="tag">one-time codes</span> : null}
                    {item.passkey_rp_ids.length ? <span className="tag">passkeys: {item.passkey_rp_ids.join(", ")}</span> : null}
                  </div>
                ) : null}
                <div className="select-row">
                  <span>
                    Use policy
                    <span className="resource-card-detail"> enforced: {item.effective_policy}</span>
                  </span>
                  <select
                    aria-label={`Use policy for ${item.id}`}
                    value={item.policy ?? "default"}
                    onChange={(e) => void setPolicy(item.id, e.target.value as VaultPolicyOverride)}
                  >
                    {(Object.keys(POLICY_LABEL) as VaultPolicyOverride[]).map((policy) => (
                      <option key={policy} value={policy}>
                        {policy === "default" ? `${POLICY_LABEL.default} (${items.data?.default_policy})` : POLICY_LABEL[policy]}
                      </option>
                    ))}
                  </select>
                </div>
                <CardActions>
                  <Button
                    size="compact"
                    aria-label={`Delete ${item.id}`}
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
                            (cause: unknown) => items.setError(vaultError(cause)),
                          ),
                      })
                    }
                  >
                    Delete
                  </Button>
                </CardActions>
              </ResourceCard>
            </li>
          ))}
        </ul>
      )}
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

function Audit() {
  const client = useClient();
  const [item, setItem] = useState("");
  const [session, setSession] = useState("");
  const [since, setSince] = useState("");
  const [query, setQuery] = useState<VaultAuditQuery>({});
  const load = useCallback(() => explained(client.vaultAudit(query)), [client, query]);
  const { data: audit, error } = useResource(load);

  function apply(event: FormEvent) {
    event.preventDefault();
    setQuery({
      item: item || undefined,
      session: session || undefined,
      since: since ? new Date(since).toISOString() : undefined,
    });
  }

  return (
    <SectionList title="Audit log" label="Vault audit log" description="Newest first.">
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
      {error ? <ErrorNote>{error}</ErrorNote> : null}
      {!audit ? (
        error ? null : <Loading />
      ) : audit.records.length === 0 ? (
        <p className="settings-text">No audit records match.</p>
      ) : (
        <ol className="resource-list audit-list" aria-label="Audit records">
          {[...audit.records].reverse().map((record, index) => (
            <li key={`${record.at}-${index}`} data-result={record.result} data-item={record.item}>
              <ResourceCard
                title={`${record.what} · ${record.item}`}
                detail={`${new Date(record.at).toLocaleString()} · ${record.who}${record.origin ? ` · ${record.origin}` : ""}`}
                badge={record.result}
                badgeTone="muted"
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
      )}
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
  const loadStatus = useCallback(() => explained(client.vaultStatus()), [client]);
  const status = useResource(loadStatus);
  const { reload } = status;
  const changed = useCallback(() => {
    void reload();
    setGeneration((value) => value + 1);
  }, [reload]);

  return (
    <div className="manage-body" data-section="vault">
      <SectionList title="Status" label="Vault status">
        {status.error ? <ErrorNote>{status.error}</ErrorNote> : null}
        {status.data ? <Status status={status.data} onChanged={changed} /> : status.error ? null : <Loading />}
      </SectionList>
      <Items key={`items-${generation}`} status={status.data} onChanged={changed} />
      <Audit key={`audit-${generation}`} />
    </div>
  );
}
