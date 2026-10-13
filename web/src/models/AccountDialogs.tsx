import { Copy } from "lucide-react";
import { useEffect, useRef, useState, type FormEvent } from "react";
import type { SubscriptionAccount } from "../api/types";
import { useClient } from "../context";
import { errorMessage } from "../hooks/useResource";
import { ErrorNote } from "../pages/blocks";
import { Button, copyText, Dialog, DialogActions, IconButton, ICON_STROKE, useToast } from "../ui";
import type { AccountRow } from "./accounts";
import { providerLabel, SUBSCRIPTION_LABELS } from "./providers";

/** How often the sign-in dialog re-reads the account while the person signs in from a terminal. */
export const SIGN_IN_POLL_MS = 2500;

const SECRET_INPUT = { autoComplete: "new-password", autoCapitalize: "none", autoCorrect: "off", spellCheck: false } as const;

function useSubmit() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function run(action: () => Promise<void>): Promise<boolean> {
    setBusy(true);
    setError(null);
    try {
      await action();
      return true;
    } catch (cause) {
      setError(errorMessage(cause));
      return false;
    } finally {
      setBusy(false);
    }
  }
  return { busy, error, run };
}

function LabelField({ value, onChange, autoFocus }: { value: string; onChange: (label: string) => void; autoFocus?: boolean }) {
  return (
    <label className="field">
      <span>Name</span>
      <input value={value} onChange={(event) => onChange(event.target.value)} placeholder="Work, Personal, Team…" maxLength={80} autoFocus={autoFocus} required />
    </label>
  );
}

/**
 * Adds an API key. The value lives only in this component's state while the dialog is open; it
 * is cleared on save and on cancel and is never echoed back (the daemon returns the last four
 * characters at most).
 */
export function AddKeyDialog({
  open,
  provider,
  first,
  onClose,
  onSave,
}: {
  open: boolean;
  provider: string;
  /** No account serves this provider yet. */
  first: boolean;
  onClose: () => void;
  onSave: (provider: string, label: string, key: string, joinRotation: boolean) => Promise<void>;
}) {
  const [label, setLabel] = useState("");
  const [value, setValue] = useState("");
  const [join, setJoin] = useState(true);
  const { busy, error, run } = useSubmit();
  const name = providerLabel(provider);

  function close() {
    setLabel("");
    setValue("");
    setJoin(true);
    onClose();
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (await run(() => onSave(provider, label.trim(), value.trim(), join))) close();
  }

  return (
    <Dialog
      open={open}
      onClose={close}
      title={`Add ${name} API key`}
      description="Stored encrypted in the vault on the computer running openside. It is not shown again after you save it."
    >
      <form className="dialog-form" onSubmit={submit} aria-label={`${name} API key`}>
        <LabelField value={label} onChange={setLabel} />
        <label className="field">
          <span>API key</span>
          <input type="password" value={value} onChange={(event) => setValue(event.target.value)} {...SECRET_INPUT} data-1p-ignore data-lpignore="true" required />
        </label>
        {first ? null : (
          <label className="check-row">
            <input type="checkbox" checked={join} onChange={(event) => setJoin(event.target.checked)} />
            <span>Use it in the rotation right away</span>
          </label>
        )}
        {error ? <ErrorNote>{error}</ErrorNote> : null}
        <DialogActions>
          <Button onClick={close}>Cancel</Button>
          <Button type="submit" variant="primary" disabled={busy || !label.trim() || !value.trim()}>
            {busy ? "Saving…" : "Save key"}
          </Button>
        </DialogActions>
      </form>
    </Dialog>
  );
}

/** Shows the exact command to sign in from a terminal and notices when the sign-in lands. */
function SignInBody({ account, onSignedIn, onDone }: { account: SubscriptionAccount; onSignedIn: (account: SubscriptionAccount) => void; onDone: () => void }) {
  const client = useClient();
  const toast = useToast();
  const [current, setCurrent] = useState(account);
  // Parents re-render freely; the poll must not restart each time.
  const signedIn = useRef(onSignedIn);
  useEffect(() => {
    signedIn.current = onSignedIn;
  });
  const name = SUBSCRIPTION_LABELS[account.source as keyof typeof SUBSCRIPTION_LABELS] ?? account.source;

  useEffect(() => {
    if (current.logged_in) return;
    const timer = window.setInterval(() => {
      client
        .subscriptionAccount(account.source, account.account_id)
        .then((next) => {
          setCurrent(next);
          if (next.logged_in) signedIn.current(next);
        })
        .catch(() => undefined);
    }, SIGN_IN_POLL_MS);
    return () => window.clearInterval(timer);
  }, [client, account.source, account.account_id, current.logged_in]);

  return (
    <div className="dialog-form">
      {current.logged_in ? (
        <p className="signin-done" role="status">
          Signed in. This account is ready to use.
        </p>
      ) : (
        <>
          <p className="settings-text">
            {current.installed
              ? `Sign in to ${name} for this account from a terminal on this computer. This page notices when you finish.`
              : `${name} is not installed on this computer. Install it, then sign in from a terminal.`}
          </p>
          <span className="login-command login-command--block">
            <code>{current.login_command}</code>
            <IconButton
              label={`Copy ${current.login_command}`}
              size="small"
              onClick={() => void copyText(current.login_command).then((copied) => toast.notify(copied ? "Command copied." : "Could not copy. Select the command and copy it.", copied ? undefined : "error"))}
            >
              <Copy size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
            </IconButton>
          </span>
          <p className="settings-text signin-wait" role="status">
            Waiting for the sign-in…
          </p>
        </>
      )}
      <DialogActions>
        <Button variant={current.logged_in ? "primary" : "secondary"} onClick={onDone}>
          {current.logged_in ? "Done" : "Close"}
        </Button>
      </DialogActions>
    </div>
  );
}

/** Sign-in for an existing account that is not signed in yet. */
export function SignInDialog({ open, account, onClose, onSignedIn }: { open: boolean; account: SubscriptionAccount; onClose: () => void; onSignedIn: (account: SubscriptionAccount) => void }) {
  return (
    <Dialog open={open} onClose={onClose} title={`Sign in: ${account.label}`}>
      <SignInBody key={account.account_id} account={account} onSignedIn={onSignedIn} onDone={onClose} />
    </Dialog>
  );
}

/** Creates a subscription account (its own managed sign-in home), then walks through signing in. */
export function AddSubscriptionDialog({
  open,
  source,
  first,
  onClose,
  onCreate,
  onSignedIn,
}: {
  open: boolean;
  source: string;
  first: boolean;
  onClose: () => void;
  onCreate: (source: string, label: string, joinRotation: boolean) => Promise<SubscriptionAccount>;
  onSignedIn: (account: SubscriptionAccount) => void;
}) {
  const [label, setLabel] = useState("");
  const [join, setJoin] = useState(true);
  const [created, setCreated] = useState<SubscriptionAccount | null>(null);
  const { busy, error, run } = useSubmit();
  const name = SUBSCRIPTION_LABELS[source as keyof typeof SUBSCRIPTION_LABELS] ?? source;

  function close() {
    setLabel("");
    setJoin(true);
    setCreated(null);
    onClose();
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    await run(async () => setCreated(await onCreate(source, label.trim(), join)));
  }

  return (
    <Dialog open={open} onClose={close} title={`Add ${name} account`} description={created ? undefined : "Each account keeps its own sign-in, separate from the one you use in the terminal."}>
      {created ? (
        <SignInBody account={created} onSignedIn={onSignedIn} onDone={close} />
      ) : (
        <form className="dialog-form" onSubmit={submit} aria-label={`${name} account`}>
          <LabelField value={label} onChange={setLabel} />
          {first ? null : (
            <label className="check-row">
              <input type="checkbox" checked={join} onChange={(event) => setJoin(event.target.checked)} />
              <span>Use it in the rotation right away</span>
            </label>
          )}
          {error ? <ErrorNote>{error}</ErrorNote> : null}
          <DialogActions>
            <Button onClick={close}>Cancel</Button>
            <Button type="submit" variant="primary" disabled={busy || !label.trim()}>
              {busy ? "Creating…" : "Continue"}
            </Button>
          </DialogActions>
        </form>
      )}
    </Dialog>
  );
}

export interface ManageChanges {
  label?: string;
  reserve_percent?: number;
  /** Replacement API key; only for key accounts. */
  key?: string;
}

/** Name, reserve and (for keys) replacement value of one account; removal is confirmed by the caller. */
export function ManageAccountDialog({
  open,
  row,
  onClose,
  onSave,
  onRemove,
}: {
  open: boolean;
  row: AccountRow;
  onClose: () => void;
  onSave: (row: AccountRow, changes: ManageChanges) => Promise<void>;
  onRemove: (row: AccountRow) => void;
}) {
  const currentReserve = row.key?.reserve_percent ?? row.subscription?.reserve_percent ?? 10;
  const [label, setLabel] = useState(row.label);
  const [reserve, setReserve] = useState(String(currentReserve));
  const [key, setKey] = useState("");
  const { busy, error, run } = useSubmit();
  const isKey = row.kind === "api-key";
  const external = row.subscription ? !row.subscription.managed : false;

  function close() {
    setKey("");
    onClose();
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    const changes: ManageChanges = {};
    if (label.trim() !== row.label) changes.label = label.trim();
    if (Number(reserve) !== currentReserve) changes.reserve_percent = Number(reserve);
    if (key.trim()) changes.key = key.trim();
    if (Object.keys(changes).length === 0 || (await run(() => onSave(row, changes)))) close();
  }

  const reserveValid = reserve !== "" && Number.isInteger(Number(reserve)) && Number(reserve) >= 0 && Number(reserve) <= 99;

  return (
    <Dialog open={open} onClose={close} title={`Manage ${row.label}`}>
      <form className="dialog-form" onSubmit={submit} aria-label="Manage account">
        {external ? <p className="settings-text">This is the sign-in your terminal already uses. openside only reads it and never changes it.</p> : null}
        <LabelField value={label} onChange={setLabel} />
        <label className="field">
          <span>Keep in reserve (%)</span>
          <input type="number" inputMode="numeric" min={0} max={99} step={1} value={reserve} onChange={(event) => setReserve(event.target.value)} aria-describedby="reserve-help" />
          <small id="reserve-help" className="field-help">
            openside moves to another account when less than this share of a usage window is left.
          </small>
        </label>
        {isKey ? (
          <label className="field">
            <span>Replace key (optional)</span>
            <input type="password" value={key} onChange={(event) => setKey(event.target.value)} {...SECRET_INPUT} data-1p-ignore data-lpignore="true" />
          </label>
        ) : null}
        {error ? <ErrorNote>{error}</ErrorNote> : null}
        <DialogActions>
          <Button
            variant="ghost"
            className="dialog-actions-start"
            onClick={() => {
              close();
              onRemove(row);
            }}
          >
            {external ? "Stop using" : "Remove"}
          </Button>
          <Button onClick={close}>Cancel</Button>
          <Button type="submit" variant="primary" disabled={busy || !reserveValid || !label.trim()}>
            {busy ? "Saving…" : "Save"}
          </Button>
        </DialogActions>
      </form>
    </Dialog>
  );
}

